//! A JSON document that is always written atomically.
//!
//! Opening creates the value from defaults when the file is missing, and every change is
//! written immediately, so an interrupted session cannot lose the last edit.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{StoreError, read_json, write_json};

/// A typed document backed by one file.
#[derive(Debug)]
pub struct JsonDocument<T> {
    path: PathBuf,
    value: T,
    dirty: bool,
}

impl<T> JsonDocument<T>
where
    T: Serialize + DeserializeOwned,
{
    /// Load `path`, falling back to `default` and applying `normalize` in both cases.
    pub fn open(
        path: impl Into<PathBuf>,
        default: T,
        normalize: impl FnOnce(T) -> T,
    ) -> Result<Self, StoreError> {
        let path = path.into();
        let loaded = read_json::<T>(&path)?;
        let present = loaded.is_some();
        let value = normalize(loaded.unwrap_or(default));
        Ok(Self {
            path,
            value,
            dirty: !present,
        })
    }

    /// Load `path` like [`Self::open`], but a corrupt file is moved aside instead of
    /// failing: the returned document falls back to `default` and the error text explains
    /// what happened.
    ///
    /// One unreadable document must never take the rest of the configuration with it.
    pub fn open_lenient(
        path: impl Into<PathBuf>,
        default: T,
        normalize: impl FnOnce(T) -> T,
    ) -> (Self, Option<String>) {
        let path = path.into();
        match read_json::<T>(&path) {
            Ok(Some(value)) => (
                Self {
                    path,
                    value: normalize(value),
                    dirty: false,
                },
                None,
            ),
            Ok(None) => (
                Self {
                    path,
                    value: normalize(default),
                    dirty: true,
                },
                None,
            ),
            Err(error) => {
                let backup = corrupt_path(&path);
                let _ = std::fs::rename(&path, &backup);
                (
                    Self {
                        path,
                        value: normalize(default),
                        dirty: true,
                    },
                    Some(format!("{error} (kept as {})", backup.display())),
                )
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    /// Replace the value and persist it.
    pub fn set(&mut self, value: T) -> Result<(), StoreError> {
        self.value = value;
        self.save()
    }

    /// Mutate the value and persist it.
    pub fn update(&mut self, change: impl FnOnce(&mut T)) -> Result<(), StoreError> {
        change(&mut self.value);
        self.save()
    }

    /// Write the current value. Call after mutating a value obtained via `get_mut`.
    pub fn save(&mut self) -> Result<(), StoreError> {
        write_json(&self.path, &self.value)?;
        self.dirty = false;
        Ok(())
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.dirty = true;
        &mut self.value
    }

    /// True when the file has not been written yet (first run or uncommitted mutation).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// `settings.json` -> `settings.json.corrupt`.
fn corrupt_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".corrupt");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde::Deserialize;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Doc {
        #[serde(default)]
        value: u32,
        #[serde(default)]
        name: String,
    }

    impl Default for Doc {
        fn default() -> Self {
            Self {
                value: 7,
                name: "default".to_owned(),
            }
        }
    }

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wassh-doc-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_uses_defaults_and_is_marked_dirty() {
        let dir = scratch();
        let document = JsonDocument::open(dir.join("settings.json"), Doc::default(), |d| d).unwrap();
        assert_eq!(document.get().value, 7);
        assert!(document.is_dirty());
        assert!(!document.path().exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_persists_and_reloads() {
        let dir = scratch();
        let path = dir.join("hosts.json");
        {
            let mut document = JsonDocument::open(&path, Doc::default(), |d| d).unwrap();
            document
                .set(Doc {
                    value: 42,
                    name: "saved".to_owned(),
                })
                .unwrap();
            assert!(!document.is_dirty());
        }
        let reloaded = JsonDocument::open(&path, Doc::default(), |d| d).unwrap();
        assert_eq!(reloaded.get().value, 42);
        assert_eq!(reloaded.get().name, "saved");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_mutates_and_saves() {
        let dir = scratch();
        let mut document = JsonDocument::open(dir.join("doc.json"), Doc::default(), |d| d).unwrap();
        document
            .update(|doc| {
                doc.value += 1;
                doc.name = "updated".to_owned();
            })
            .unwrap();
        assert_eq!(document.get().value, 8);
        let text = fs::read_to_string(document.path()).unwrap();
        assert!(text.contains("updated"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalization_runs_on_load_and_default() {
        let dir = scratch();
        let path = dir.join("doc.json");
        fs::write(&path, br#"{"value":1}"#).unwrap();
        let document = JsonDocument::open(&path, Doc::default(), |mut doc| {
            doc.value *= 10;
            doc
        })
        .unwrap();
        assert_eq!(document.get().value, 10);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_json_is_reported() {
        let dir = scratch();
        let path = dir.join("broken.json");
        fs::write(&path, b"{ nope").unwrap();
        let error = JsonDocument::<Doc>::open(&path, Doc::default(), |d| d).unwrap_err();
        assert!(matches!(error, StoreError::Json { .. }));
        fs::remove_dir_all(&dir).ok();
    }
}
