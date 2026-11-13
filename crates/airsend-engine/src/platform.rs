use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{ConnectionState, DiagnosticState, DiscoveryState, StreamingState};

type EventHandler = dyn Fn(&str, Value) + Send + Sync;

#[derive(Clone)]
pub struct AppHandle(Arc<Host>);

struct Host {
    directory: PathBuf,
    settings: Settings,
    discovery: DiscoveryState,
    connection: ConnectionState,
    streaming: StreamingState,
    diagnostics: DiagnosticState,
    event: Box<EventHandler>,
}

impl AppHandle {
    pub fn new(
        directory: PathBuf,
        event: impl Fn(&str, Value) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let settings = Settings::open(directory.join("settings.json"))?;
        let journal = directory.join("local-mute-recovery.json");
        if journal.exists() {
            match std::thread::spawn(move || audio_capture::recover_local_mute(&journal)).join() {
                Ok(Ok(())) => {}
                result => tracing::warn!(?result, "local mute recovery remains pending"),
            }
        }
        Ok(Self(Arc::new(Host {
            directory,
            settings,
            event: Box::new(event),
            discovery: DiscoveryState::default(),
            connection: ConnectionState::default(),
            streaming: StreamingState::default(),
            diagnostics: DiagnosticState::default(),
        })))
    }

    pub fn discovery(&self) -> &DiscoveryState {
        &self.0.discovery
    }
    pub fn connection(&self) -> &ConnectionState {
        &self.0.connection
    }
    pub fn streaming(&self) -> &StreamingState {
        &self.0.streaming
    }
    pub fn diagnostics(&self) -> &DiagnosticState {
        &self.0.diagnostics
    }
    pub(crate) fn data_dir(&self) -> &Path {
        &self.0.directory
    }
    pub(crate) fn store(&self) -> &Settings {
        &self.0.settings
    }
    pub(crate) fn emit(&self, name: &str, data: impl Serialize) -> Result<(), serde_json::Error> {
        (self.0.event)(name, serde_json::to_value(data)?);
        Ok(())
    }
}

pub(crate) struct Settings {
    path: PathBuf,
    values: Mutex<Map<String, Value>>,
}

impl Settings {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let values = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            values: Mutex::new(values),
        })
    }

    pub(crate) fn get(&self, key: impl AsRef<str>) -> Option<Value> {
        self.values.lock().unwrap().get(key.as_ref()).cloned()
    }

    pub(crate) fn set(&self, key: impl Into<String>, value: Value) -> Result<(), String> {
        self.update(|values| {
            values.insert(key.into(), value);
        })
    }

    pub(crate) fn delete(&self, key: &str) -> Result<(), String> {
        self.update(|values| {
            values.remove(key);
        })
    }

    fn update(&self, change: impl FnOnce(&mut Map<String, Value>)) -> Result<(), String> {
        let mut current = self.values.lock().map_err(|error| error.to_string())?;
        let mut next = current.clone();
        change(&mut next);
        let parent = self.path.parent().ok_or("missing settings directory")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut temporary, &next).map_err(|error| error.to_string())?;
        temporary
            .persist(&self.path)
            .map_err(|error| error.to_string())?;
        *current = next;
        Ok(())
    }
}
