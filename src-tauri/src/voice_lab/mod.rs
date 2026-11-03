pub mod audio;
pub mod commands;
pub mod mixer;
pub mod models;
pub mod runner;
pub mod store;
pub mod style;
pub mod train_runner;
pub mod trainer;
pub mod worker;

#[derive(Debug, thiserror::Error)]
pub enum VlError {
    #[error("{0}")]
    Code(&'static str),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl VlError {
    pub fn code(&self) -> String {
        match self {
            Self::Code(code) => (*code).to_string(),
            Self::Other(_) => "IO_FAILED".to_string(),
        }
    }
}

impl From<std::io::Error> for VlError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}

impl From<rusqlite::Error> for VlError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Other(error.into())
    }
}

impl From<serde_json::Error> for VlError {
    fn from(error: serde_json::Error) -> Self {
        Self::Other(error.into())
    }
}

pub fn err(code: &'static str) -> VlError {
    VlError::Code(code)
}

use runner::Runner;
use std::{path::PathBuf, sync::Arc};
use store::Store;
use tauri::Manager;

pub struct VoiceLab {
    pub root: PathBuf,
    pub store: Arc<std::sync::Mutex<Store>>,
    pub runner: Arc<Runner>,
    pub train: Arc<train_runner::TrainRunner>,
    pub train_install: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub torch_cache: std::sync::Mutex<Option<Option<String>>>,
    pub installing: tokio::sync::Mutex<()>,
}

pub struct VlState(pub Option<Arc<VoiceLab>>);

pub fn init(app: &tauri::App) {
    let result = (|| -> Result<VoiceLab, VlError> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(anyhow::Error::from)?
            .join("voice-lab");
        let store = Store::open(&root)?;
        if models::installed(&root) {
            let _ = models::register_builtins(&store, &root);
        }
        let train = Arc::new(train_runner::TrainRunner::new(
            trainer::venv_python(&root),
            trainer::trainer_dir(&root).join("adapter/train_job.py"),
            vec![],
            std::time::Duration::from_secs(30),
        ));
        Ok(VoiceLab {
            train,
            train_install: tokio::sync::Mutex::new(None),
            torch_cache: std::sync::Mutex::new(None),
            root,
            store: Arc::new(std::sync::Mutex::new(store)),
            runner: Arc::new(Runner::new(std::env::current_exe()?, vec![])),
            installing: tokio::sync::Mutex::new(()),
        })
    })();
    if let Err(e) = &result {
        eprintln!("Voice Lab unavailable: {e:?}");
    }
    app.manage(VlState(result.ok().map(Arc::new)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        assert_eq!(err("STYLE_BAD_SCHEMA").code(), "STYLE_BAD_SCHEMA");
        assert_eq!(
            VlError::from(anyhow::anyhow!("internal detail")).code(),
            "IO_FAILED"
        );
        assert_eq!(
            VlError::from(std::io::Error::other("internal detail")).code(),
            "IO_FAILED"
        );
        assert_eq!(
            VlError::from(rusqlite::Error::InvalidQuery).code(),
            "IO_FAILED"
        );
        let json_error = serde_json::from_str::<serde_json::Value>("!").unwrap_err();
        assert_eq!(VlError::from(json_error).code(), "IO_FAILED");
    }
}
