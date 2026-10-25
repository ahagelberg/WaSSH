//! Atomic JSON documents.
//!
//! A crash mid-write must never leave `hosts.json` truncated, so every write goes through
//! a temporary file in the same directory followed by a rename.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::StoreError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` atomically.
///
/// On Unix the rename replaces the target in one step. On Windows the target is removed
/// first, because `std::fs::rename` does not replace existing files there.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
    }

    let temp = temp_path(path);
    {
        let mut file = fs::File::create(&temp).map_err(|e| StoreError::io(&temp, e))?;
        file.write_all(bytes)
            .map_err(|e| StoreError::io(&temp, e))?;
        file.sync_all().map_err(|e| StoreError::io(&temp, e))?;
    }

    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        #[cfg(target_os = "windows")]
        {
            if path.exists() {
                fs::remove_file(path).map_err(|e| StoreError::io(path, e))?;
                return fs::rename(&temp, path).map_err(|e| StoreError::io(path, e));
            }
        }
        return Err(StoreError::io(path, error));
    }
    Ok(())
}

/// Serialise `value` as pretty JSON (with a trailing newline) and write it atomically.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|e| StoreError::json(path, e))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Read and parse a JSON document. A missing file yields `Ok(None)`.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    match fs::read(path) {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes).map_err(|e| StoreError::json(path, e))?;
            Ok(Some(value))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StoreError::io(path, error)),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let unique = format!(".{}.{}.{}.tmp", name, std::process::id(), counter);
    match path.parent() {
        Some(parent) => parent.join(unique),
        None => PathBuf::from(unique),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Document {
        name: String,
        count: u32,
    }

    fn scratch_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wassh-store-test-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn writes_and_reads_round_trip() {
        let dir = scratch_dir();
        let path = dir.join("settings.json");
        let doc = Document {
            name: "Luna".into(),
            count: 3,
        };
        write_json(&path, &doc).unwrap();
        let loaded: Option<Document> = read_json(&path).unwrap();
        assert_eq!(loaded, Some(doc));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"name\": \"Luna\""));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overwrite_replaces_contents_in_place() {
        let dir = scratch_dir();
        let path = dir.join("hosts.json");
        write_json(
            &path,
            &Document {
                name: "first".into(),
                count: 1,
            },
        )
        .unwrap();
        write_json(
            &path,
            &Document {
                name: "second".into(),
                count: 2,
            },
        )
        .unwrap();
        let loaded: Document = read_json(&path).unwrap().unwrap();
        assert_eq!(loaded.name, "second");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temp files left");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = scratch_dir();
        let loaded: Option<Document> = read_json(&dir.join("absent.json")).unwrap();
        assert_eq!(loaded, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_json_reports_the_path() {
        let dir = scratch_dir();
        let path = dir.join("broken.json");
        fs::write(&path, b"{ not json").unwrap();
        let error = read_json::<Document>(&path).unwrap_err();
        match error {
            StoreError::Json { path: reported, .. } => assert_eq!(reported, path),
            other => panic!("unexpected error: {other}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = scratch_dir();
        let path = dir.join("nested").join("deeper").join("plugin-x.json");
        write_atomic(&path, b"{}").unwrap();
        assert!(path.exists());
        fs::remove_dir_all(&dir).ok();
    }
}
