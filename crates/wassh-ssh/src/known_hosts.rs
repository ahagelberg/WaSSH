//! Known-host storage.
//!
//! `known_hosts.json` is a JSON array of `{ host, port, fingerprint }` entries. Unknown keys
//! are remembered after the user accepts them; a differing key for a known endpoint is
//! treated as a mismatch.

use std::path::PathBuf;
use std::sync::Mutex;

use wassh_store::{StoreError, read_json, write_json};

use crate::hostkey::HostKeyFingerprint;

/// In-memory view of the known-host file, written back atomically on every change.
pub struct KnownHosts {
    path: Option<PathBuf>,
    entries: Mutex<Vec<HostKeyFingerprint>>,
}

impl KnownHosts {
    /// Load the document at `path`; a missing or invalid file starts empty.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let entries = read_json::<Vec<HostKeyFingerprint>>(&path)
            .ok()
            .flatten()
            .unwrap_or_default();
        Self {
            path: Some(path),
            entries: Mutex::new(entries),
        }
    }

    /// Storage that never touches disk, for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Recorded fingerprint for an endpoint, if any.
    pub fn get(&self, host: &str, port: u16) -> Option<String> {
        self.entries
            .lock()
            .expect("known-hosts lock")
            .iter()
            .find(|entry| entry.matches(host, port))
            .map(|entry| entry.fingerprint.clone())
    }

    /// Add or replace the entry for its endpoint and persist the file.
    pub fn remember(&self, entry: HostKeyFingerprint) -> Result<(), StoreError> {
        {
            let mut entries = self.entries.lock().expect("known-hosts lock");
            entries.retain(|existing| !existing.matches(&entry.host, entry.port));
            entries.push(entry);
        }
        self.persist()
    }

    /// Remove an endpoint (used when a key legitimately changes).
    pub fn forget(&self, host: &str, port: u16) -> Result<(), StoreError> {
        {
            let mut entries = self.entries.lock().expect("known-hosts lock");
            entries.retain(|existing| !existing.matches(host, port));
        }
        self.persist()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("known-hosts lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn persist(&self) -> Result<(), StoreError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = self.entries.lock().expect("known-hosts lock").clone();
        write_json(path, &snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wassh-known-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn remembers_and_retrieves_entries() {
        let hosts = KnownHosts::in_memory();
        assert_eq!(hosts.get("example.com", 22), None);
        hosts
            .remember(HostKeyFingerprint::new("example.com", 22, "SHA256:a"))
            .unwrap();
        assert_eq!(hosts.get("example.com", 22).as_deref(), Some("SHA256:a"));
        assert_eq!(hosts.get("example.com", 2222), None);
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn replacing_an_endpoint_keeps_one_entry() {
        let hosts = KnownHosts::in_memory();
        hosts
            .remember(HostKeyFingerprint::new("h", 22, "SHA256:old"))
            .unwrap();
        hosts
            .remember(HostKeyFingerprint::new("h", 22, "SHA256:new"))
            .unwrap();
        assert_eq!(hosts.get("h", 22).as_deref(), Some("SHA256:new"));
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn persists_across_instances_and_forgets() {
        let dir = scratch();
        let path = dir.join("known_hosts.json");
        {
            let hosts = KnownHosts::load(&path);
            hosts
                .remember(HostKeyFingerprint::new("h", 2200, "SHA256:x"))
                .unwrap();
        }
        let reloaded = KnownHosts::load(&path);
        assert_eq!(reloaded.get("h", 2200).as_deref(), Some("SHA256:x"));
        reloaded.forget("h", 2200).unwrap();
        assert!(KnownHosts::load(&path).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_empty_not_an_error() {
        let dir = scratch();
        let hosts = KnownHosts::load(dir.join("absent.json"));
        assert!(hosts.is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
