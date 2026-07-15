//! Versioned, GUI-free browser-control wire protocol.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;
use url::{Host, Url};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectKey {
    Zellij {
        session_name: String,
        stable_tab_id: u32,
    },
    Standalone {
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub key: ProjectKey,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_tab_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub command: Command,
}

impl RequestEnvelope {
    pub fn new(request_id: impl Into<String>, command: Command) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    OpenUrl {
        project: ProjectMetadata,
        source_file: String,
        url: String,
    },
    SelectProject {
        session_name: String,
        stable_tab_id: u32,
    },
    CloseProject {
        session_name: String,
        stable_tab_id: u32,
    },
    InspectState,
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Ok,
    Ignored,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub status: ResponseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<BrowserStateSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserStateSnapshot {
    pub projects: Vec<ProjectSnapshot>,
    pub selected_project: Option<ProjectKey>,
    pub presentation_count: u64,
    pub process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub key: ProjectKey,
    pub label: String,
    pub selected_source_file: Option<String>,
    pub documents: Vec<DocumentSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    pub source_file: String,
    pub url: String,
    pub title: Option<String>,
    pub lifecycle: DocumentLifecycleSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentLifecycleSnapshot {
    Dormant,
    Loading,
    Ready,
    Failed,
    Suspended,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported protocol version {actual}; expected {expected}")]
    UnsupportedVersion { actual: u32, expected: u32 },
    #[error("message exceeds the {MAX_MESSAGE_BYTES}-byte limit")]
    TooLarge,
    #[error("message is not terminated by one newline")]
    MissingNewline,
    #[error("message contains data after its terminating newline")]
    TrailingData,
    #[error("message contains invalid JSON: {0}")]
    InvalidJson(String),
}

pub fn validate_protocol_version(actual: u32) -> Result<(), ProtocolError> {
    if actual == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedVersion {
            actual,
            expected: PROTOCOL_VERSION,
        })
    }
}

pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|error| ProtocolError::InvalidJson(error.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    Ok(bytes)
}

pub fn decode_frame<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, ProtocolError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    let Some(payload) = bytes.strip_suffix(b"\n") else {
        return Err(ProtocolError::MissingNewline);
    };
    if payload.contains(&b'\n') {
        return Err(ProtocolError::TrailingData);
    }
    serde_json::from_slice(payload).map_err(|error| ProtocolError::InvalidJson(error.to_string()))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionUrlError {
    #[error("invalid URL: {0}")]
    Invalid(String),
    #[error("only HTTP(S) session URLs are accepted")]
    InvalidScheme,
    #[error("URL credentials are not allowed")]
    Credentials,
    #[error("URL host must be localhost or an exact loopback address")]
    NonLoopbackHost,
    #[error("URL path must contain a non-empty /session/ identifier")]
    InvalidSessionPath,
}

pub fn validate_session_url(value: &str) -> Result<Url, SessionUrlError> {
    let url = Url::parse(value).map_err(|error| SessionUrlError::Invalid(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SessionUrlError::InvalidScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SessionUrlError::Credentials);
    }
    let loopback = match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => IpAddr::V4(address) == IpAddr::V4(Ipv4Addr::LOCALHOST),
        Some(Host::Ipv6(address)) => IpAddr::V6(address) == IpAddr::V6(Ipv6Addr::LOCALHOST),
        None => false,
    };
    if !loopback {
        return Err(SessionUrlError::NonLoopbackHost);
    }
    let Some(identifier) = url.path().strip_prefix("/session/") else {
        return Err(SessionUrlError::InvalidSessionPath);
    };
    if identifier.is_empty() || identifier.starts_with('/') {
        return Err(SessionUrlError::InvalidSessionPath);
    }
    Ok(url)
}
