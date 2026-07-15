use crate::{
    BrowserModel, BrowserStateStore, Document, DocumentKey, DocumentLifecycle, Project, ProjectKey,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

pub const STATE_SCHEMA_VERSION: u32 = 1;
const APPLICATION_DIRECTORY: &str = "lavish-browser";
const STATE_FILE: &str = "state.json";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum PersistenceError {
    Io(io::Error),
    Serialize(serde_json::Error),
    MissingStateDirectory,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "browser metadata I/O failed: {error}"),
            Self::Serialize(error) => {
                write!(formatter, "browser metadata serialization failed: {error}")
            }
            Self::MissingStateDirectory => write!(
                formatter,
                "cannot determine an XDG state directory (XDG_STATE_HOME and HOME are unset)"
            ),
        }
    }
}

impl std::error::Error for PersistenceError {}

impl From<io::Error> for PersistenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadIssue {
    Corrupt(String),
    UnknownSchemaVersion(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub model: BrowserModel,
    pub issue: Option<LoadIssue>,
}

#[derive(Debug, Clone)]
pub struct MetadataStore {
    state_file: PathBuf,
}

impl MetadataStore {
    pub fn new(state_directory: impl Into<PathBuf>) -> Self {
        Self {
            state_file: state_directory.into().join(STATE_FILE),
        }
    }

    pub fn from_xdg_environment() -> Result<Self, PersistenceError> {
        Self::from_environment(|name| std::env::var_os(name).map(PathBuf::from))
    }

    pub fn from_environment(
        get: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, PersistenceError> {
        let root = get("XDG_STATE_HOME")
            .or_else(|| get("HOME").map(|home| home.join(".local/state")))
            .ok_or(PersistenceError::MissingStateDirectory)?;
        Ok(Self::new(root.join(APPLICATION_DIRECTORY)))
    }

    pub fn state_file(&self) -> &Path {
        &self.state_file
    }

    pub fn load_with_report(&self) -> Result<LoadReport, PersistenceError> {
        let bytes = match fs::read(&self.state_file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadReport {
                    model: BrowserModel::default(),
                    issue: None,
                });
            }
            Err(error) => return Err(error.into()),
        };

        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                return Ok(empty_report(LoadIssue::Corrupt(error.to_string())));
            }
        };
        let Some(version) = value.get("schema_version").and_then(|value| value.as_u64()) else {
            return Ok(empty_report(LoadIssue::Corrupt(
                "missing integer schema_version".into(),
            )));
        };
        if version != u64::from(STATE_SCHEMA_VERSION) {
            return Ok(empty_report(LoadIssue::UnknownSchemaVersion(version)));
        }

        let state: PersistedState = match serde_json::from_value(value) {
            Ok(state) => state,
            Err(error) => return Ok(empty_report(LoadIssue::Corrupt(error.to_string()))),
        };
        Ok(LoadReport {
            model: state.into_model(),
            issue: None,
        })
    }

    fn write_atomically(&self, bytes: &[u8]) -> Result<(), PersistenceError> {
        let directory = self
            .state_file
            .parent()
            .expect("state file always has a parent directory");
        fs::create_dir_all(directory)?;
        set_directory_permissions(directory)?;

        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = directory.join(format!(
            ".{STATE_FILE}.tmp-{}-{sequence}",
            std::process::id()
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_file_creation_permissions(&mut options);
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.state_file)?;
            set_file_permissions(&self.state_file)?;
            sync_directory(directory)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl BrowserStateStore for MetadataStore {
    type Error = PersistenceError;

    fn save(&self, model: &BrowserModel) -> Result<(), Self::Error> {
        let bytes = serde_json::to_vec_pretty(&PersistedState::from_model(model))?;
        self.write_atomically(&bytes)
    }

    fn load(&self) -> Result<BrowserModel, Self::Error> {
        Ok(self.load_with_report()?.model)
    }
}

pub struct CoalescingStateStore<S> {
    inner: S,
    last_saved: Mutex<Option<BrowserModel>>,
}

impl<S> CoalescingStateStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            last_saved: Mutex::new(None),
        }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: BrowserStateStore> BrowserStateStore for CoalescingStateStore<S> {
    type Error = S::Error;

    fn save(&self, model: &BrowserModel) -> Result<(), Self::Error> {
        let mut last_saved = self.last_saved.lock().expect("state store lock poisoned");
        if last_saved.as_ref() == Some(model) {
            return Ok(());
        }
        self.inner.save(model)?;
        *last_saved = Some(model.clone());
        Ok(())
    }

    fn load(&self) -> Result<BrowserModel, Self::Error> {
        let model = self.inner.load()?;
        *self.last_saved.lock().expect("state store lock poisoned") = Some(model.clone());
        Ok(model)
    }
}

fn empty_report(issue: LoadIssue) -> LoadReport {
    LoadReport {
        model: BrowserModel::default(),
        issue: Some(issue),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedState {
    schema_version: u32,
    projects: Vec<PersistedProject>,
    selected_project: Option<ProjectKey>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedProject {
    key: ProjectKey,
    label: String,
    raw_tab_name: Option<String>,
    documents: Vec<PersistedDocument>,
    selected_document: Option<String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedDocument {
    canonical_source_file: String,
    lavish_url: String,
    title: Option<String>,
    last_activated_at: u64,
}

impl PersistedState {
    fn from_model(model: &BrowserModel) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            projects: model
                .projects
                .iter()
                .map(|project| PersistedProject {
                    key: project.key.clone(),
                    label: project.label.clone(),
                    raw_tab_name: project.raw_tab_name.clone(),
                    documents: project
                        .documents
                        .iter()
                        .map(|document| PersistedDocument {
                            canonical_source_file: document.key.canonical_source_file.clone(),
                            lavish_url: document.lavish_url.clone(),
                            title: document.title.clone(),
                            last_activated_at: document.last_activated_at,
                        })
                        .collect(),
                    selected_document: project.selected_document.clone(),
                    created_at: project.created_at,
                    updated_at: project.updated_at,
                })
                .collect(),
            selected_project: model.selected_project.clone(),
        }
    }

    fn into_model(self) -> BrowserModel {
        let mut projects = Vec::with_capacity(self.projects.len());
        for persisted in self.projects {
            if projects
                .iter()
                .any(|project: &Project| project.key == persisted.key)
            {
                continue;
            }
            let mut documents = Vec::with_capacity(persisted.documents.len());
            for document in persisted.documents {
                if documents.iter().any(|existing: &Document| {
                    existing.key.canonical_source_file == document.canonical_source_file
                }) {
                    continue;
                }
                documents.push(Document {
                    key: DocumentKey {
                        project: persisted.key.clone(),
                        canonical_source_file: document.canonical_source_file,
                    },
                    lavish_url: document.lavish_url,
                    title: document.title,
                    lifecycle: DocumentLifecycle::Dormant,
                    last_activated_at: document.last_activated_at,
                    load_error: None,
                });
            }
            let selected_document = persisted.selected_document.filter(|selected| {
                documents
                    .iter()
                    .any(|document| document.key.canonical_source_file == *selected)
            });
            projects.push(Project {
                key: persisted.key,
                label: persisted.label,
                raw_tab_name: persisted.raw_tab_name,
                documents,
                selected_document,
                created_at: persisted.created_at,
                updated_at: persisted.updated_at,
            });
        }
        let selected_project = self
            .selected_project
            .filter(|selected| projects.iter().any(|project| project.key == *selected));
        BrowserModel {
            projects,
            selected_project,
        }
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

#[cfg(unix)]
fn set_file_creation_permissions(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_file_creation_permissions(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
