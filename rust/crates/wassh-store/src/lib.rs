//! Configuration persistence.
//!
//! Everything is plain JSON in the application data directory. Writes are atomic (temporary
//! file + rename) so a crash cannot truncate `hosts.json`.

#![forbid(unsafe_code)]

pub mod atomic;
pub mod backup;
pub mod config;
pub mod document;
pub mod paths;
pub mod secrets;
pub mod zip;

pub use atomic::{read_json, write_atomic, write_json};
pub use backup::{BackupSummary, default_archive_path, export as export_backup, import as import_backup};
pub use config::Config;
pub use secrets::SecretsStore;
pub use document::JsonDocument;
pub use paths::{
    data_dir, data_dir_with, known_hosts_file, plugin_data_file, settings_file, tabs_file,
};

use std::io;

/// Failures from the configuration store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot determine the application data directory: {0}")]
    NoDataDir(&'static str),
    #[error("i/o error on {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot generate random bytes: {detail}")]
    Random { detail: String },
    #[error("secret storage failed: {detail}")]
    Secret { detail: String },
}

impl StoreError {
    pub(crate) fn io(path: impl Into<std::path::PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(path: impl Into<std::path::PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_actionable() {
        let err = StoreError::NoDataDir("HOME is not set");
        assert!(err.to_string().contains("HOME is not set"));
    }
}
