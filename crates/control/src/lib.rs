//! Hardened native Unix-socket transport for browser control.

use lavish_browser_protocol::{
    Command, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope,
    ResponseStatus, decode_frame, encode_frame, validate_protocol_version, validate_session_url,
};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use thiserror::Error;

const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// A synchronous request boundary shared by native callers.
pub trait BrowserControl {
    type Error;

    fn request(&self, request: &RequestEnvelope) -> Result<ResponseEnvelope, Self::Error>;
}

#[derive(Debug, Error)]
pub enum ControlError {
    #[error("browser is not running at {0}")]
    Absent(PathBuf),
    #[error("browser control socket is already in use at {0}")]
    AlreadyRunning(PathBuf),
    #[error("unsafe browser control path {path}: {reason}")]
    UnsafePath { path: PathBuf, reason: String },
    #[error("browser control I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("browser control protocol failed: {0}")]
    Protocol(String),
    #[error("browser rejected request: {0}")]
    Rejected(String),
}

#[derive(Debug, Clone)]
pub struct SocketClient {
    path: PathBuf,
    timeout: Duration,
}

impl SocketClient {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            timeout: IO_TIMEOUT,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl BrowserControl for SocketClient {
    type Error = ControlError;

    fn request(&self, request: &RequestEnvelope) -> Result<ResponseEnvelope, Self::Error> {
        let mut stream = UnixStream::connect(&self.path).map_err(|error| {
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) {
                ControlError::Absent(self.path.clone())
            } else {
                ControlError::Io(error)
            }
        })?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.write_all(&encode_frame(request).map_err(protocol_error)?)?;
        let bytes = read_frame(&mut stream)?;
        let response: ResponseEnvelope = decode_frame(&bytes).map_err(protocol_error)?;
        validate_protocol_version(response.protocol_version).map_err(protocol_error)?;
        if response.request_id != request.request_id {
            return Err(ControlError::Protocol(format!(
                "response request ID {:?} did not match {:?}",
                response.request_id, request.request_id
            )));
        }
        if response.status == ResponseStatus::Error {
            return Err(ControlError::Rejected(response.message.unwrap_or_else(
                || "browser returned an unspecified error".to_owned(),
            )));
        }
        Ok(response)
    }
}

pub type CommandHandler = dyn Fn(RequestEnvelope) -> ResponseEnvelope + Send + Sync + 'static;

pub struct SocketServer {
    listener: UnixListener,
    path: PathBuf,
    socket_inode: u64,
    handler: Arc<CommandHandler>,
}

impl SocketServer {
    pub fn bind(
        path: impl Into<PathBuf>,
        handler: impl Fn(RequestEnvelope) -> ResponseEnvelope + Send + Sync + 'static,
    ) -> Result<Self, ControlError> {
        let path = path.into();
        prepare_parent(&path)?;
        recover_stale_socket(&path)?;
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let socket_inode = fs::metadata(&path)?.ino();
        Ok(Self {
            listener,
            path,
            socket_inode,
            handler: Arc::new(handler),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accept one client and process it on a detached worker thread.
    pub fn accept(&self) -> Result<thread::JoinHandle<()>, ControlError> {
        let (stream, _) = self.listener.accept()?;
        verify_peer_uid(&stream)?;
        let handler = Arc::clone(&self.handler);
        Ok(thread::spawn(move || handle_connection(stream, handler)))
    }

    /// Serve until the process exits or the listener encounters an error.
    pub fn run(&self) -> Result<(), ControlError> {
        loop {
            self.accept()?;
        }
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && metadata.ino() == self.socket_inode
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("LAVISH_BROWSER_SOCKET") {
        return PathBuf::from(path);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("lavish-browser").join("control.sock")
}

fn prepare_parent(path: &Path) -> Result<(), ControlError> {
    let parent = path.parent().ok_or_else(|| ControlError::UnsafePath {
        path: path.to_owned(),
        reason: "socket has no parent directory".to_owned(),
    })?;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.uid() != current_uid() {
        return Err(ControlError::UnsafePath {
            path: parent.to_owned(),
            reason: "directory is not owned by the current user".to_owned(),
        });
    }
    if metadata.permissions().mode() & 0o1000 != 0 {
        return Err(ControlError::UnsafePath {
            path: parent.to_owned(),
            reason: "refusing to place a control socket directly in a shared sticky directory"
                .to_owned(),
        });
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn recover_stale_socket(path: &Path) -> Result<(), ControlError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err(ControlError::UnsafePath {
            path: path.to_owned(),
            reason: "existing entry is not a socket owned by the current user".to_owned(),
        });
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(ControlError::AlreadyRunning(path.to_owned())),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn handle_connection(mut stream: UnixStream, handler: Arc<CommandHandler>) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let response = match read_frame(&mut stream)
        .and_then(|bytes| decode_frame::<RequestEnvelope>(&bytes).map_err(protocol_error))
    {
        Ok(request) => match validate_request(&request) {
            Ok(()) => {
                let request_id = request.request_id.clone();
                let mut response = handler(request);
                response.protocol_version = PROTOCOL_VERSION;
                response.request_id = request_id;
                response
            }
            Err((code, message)) => error_response(request.request_id, code, message),
        },
        Err(error) => error_response(String::new(), "malformed_request", error.to_string()),
    };
    let frame = encode_frame(&response).or_else(|error| {
        encode_frame(&error_response(
            response.request_id,
            "response_too_large",
            error.to_string(),
        ))
    });
    if let Ok(frame) = frame {
        let _ = stream.write_all(&frame);
    }
}

fn validate_request(request: &RequestEnvelope) -> Result<(), (&'static str, String)> {
    validate_protocol_version(request.protocol_version)
        .map_err(|error| ("unsupported_version", error.to_string()))?;
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err((
            "invalid_request_id",
            "request_id must contain between 1 and 128 bytes".to_owned(),
        ));
    }
    if let Command::OpenUrl { url, .. } = &request.command {
        validate_session_url(url).map_err(|error| ("invalid_session_url", error.to_string()))?;
    }
    Ok(())
}

fn error_response(request_id: String, code: &str, message: String) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        status: ResponseStatus::Error,
        error_code: Some(code.to_owned()),
        message: Some(message),
        state: None,
    }
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, ControlError> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(ControlError::Protocol(
                    "connection ended before a newline-terminated message".to_owned(),
                ));
            }
            Ok(_) => {
                bytes.push(byte[0]);
                if bytes.len() > MAX_MESSAGE_BYTES {
                    return Err(ControlError::Protocol(format!(
                        "message exceeds the {MAX_MESSAGE_BYTES}-byte limit"
                    )));
                }
                if byte[0] == b'\n' {
                    return Ok(bytes);
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn protocol_error(error: impl std::fmt::Display) -> ControlError {
    ControlError::Protocol(error.to_string())
}

#[cfg(target_os = "linux")]
fn verify_peer_uid(stream: &UnixStream) -> Result<(), ControlError> {
    use std::os::fd::AsRawFd;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: pointers refer to a correctly sized ucred and length for this live socket.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if credentials.uid != current_uid() {
        return Err(ControlError::UnsafePath {
            path: PathBuf::from("peer"),
            reason: format!("peer UID {} does not match current UID", credentials.uid),
        });
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn verify_peer_uid(_stream: &UnixStream) -> Result<(), ControlError> {
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}
