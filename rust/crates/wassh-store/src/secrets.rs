//! Passwords and key passphrases, encrypted at rest.
//!
//! Secrets never go into `hosts.json`. They live in `secrets.json`, one sealed blob per
//! host id, and every blob is encrypted with AES-256-GCM under a machine-local key file
//! (`secret.key`, created `0600` on Unix). Replacing the key file makes the stored blobs
//! unreadable, and a blob that fails to decrypt is treated as absent rather than an error.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::StoreError;

/// File holding the machine-local encryption key.
pub const KEY_FILE: &str = "secret.key";
/// File holding the sealed secrets.
pub const SECRETS_FILE: &str = "secrets.json";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Where the key file and the secrets file live.
pub fn key_file(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join(KEY_FILE)
}

/// Secrets file path.
pub fn secrets_file(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join(SECRETS_FILE)
}

/// Sealed secrets for one host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SealedEntry {
    /// Sealed password, base64 of `nonce || ciphertext`.
    password: Option<String>,
    /// Sealed key passphrase.
    passphrase: Option<String>,
}

/// Scope of a plugin secret that belongs to the app, not to one host.
pub const APP_SCOPE: &str = "app";

/// On-disk shape of `secrets.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SecretFile {
    version: u32,
    entries: BTreeMap<String, SealedEntry>,
    /// Plugin secrets, `scope -> plugin -> field -> sealed value`.
    ///
    /// A plugin cannot reach the vault itself, so the host seals the values of
    /// every field its settings declare as secret and hands the plain text to
    /// the pane at draw time.
    plugin_secrets: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
}

/// Encrypted secret storage.
pub struct SecretsStore {
    path: PathBuf,
    key: [u8; KEY_LEN],
    file: SecretFile,
}

impl SecretsStore {
    /// Open (creating when missing) the store in `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|source| StoreError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let key = load_or_create_key(&key_file(dir))?;
        let path = secrets_file(dir);
        let mut file: SecretFile = crate::read_json(&path)?.unwrap_or_default();
        if file.version < 2 {
            // Version 2 added the plugin-secret namespace; older files are fine
            // as they are and are rewritten on the next save.
            file.version = 2;
        }
        Ok(Self { path, key, file })
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.key).expect("32-byte key")
    }

    fn seal(&self, plain: &str) -> Result<String, StoreError> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|error| StoreError::Random {
            detail: error.to_string(),
        })?;
        let nonce = Nonce::try_from(&nonce[..]).map_err(|_| StoreError::Secret {
            detail: "bad nonce length".to_owned(),
        })?;
        let sealed = self
            .cipher()
            .encrypt(&nonce, plain.as_bytes())
            .map_err(|_| StoreError::Secret {
                detail: "encryption failed".to_owned(),
            })?;
        let mut blob = Vec::with_capacity(NONCE_LEN + sealed.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&sealed);
        Ok(STANDARD.encode(blob))
    }

    fn unseal(&self, sealed: &str) -> Option<String> {
        let blob = STANDARD.decode(sealed).ok()?;
        let (nonce, ciphertext) = blob.split_at_checked(NONCE_LEN)?;
        let nonce = Nonce::try_from(nonce).ok()?;
        let plain = self.cipher().decrypt(&nonce, ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    fn entry(&self, host_id: &str) -> Option<&SealedEntry> {
        self.file.entries.get(host_id)
    }

    /// Stored password for `host_id`, if one is saved and decryptable.
    pub fn password(&self, host_id: &str) -> Option<String> {
        self.entry(host_id)?.password.as_deref().and_then(|s| self.unseal(s))
    }

    /// Stored key passphrase for `host_id`.
    pub fn passphrase(&self, host_id: &str) -> Option<String> {
        self.entry(host_id)?
            .passphrase
            .as_deref()
            .and_then(|s| self.unseal(s))
    }

    /// Whether a password is saved for `host_id`.
    pub fn has_password(&self, host_id: &str) -> bool {
        self.entry(host_id)
            .is_some_and(|entry| entry.password.is_some())
    }

    /// Whether a passphrase is saved for `host_id`.
    pub fn has_passphrase(&self, host_id: &str) -> bool {
        self.entry(host_id)
            .is_some_and(|entry| entry.passphrase.is_some())
    }

    /// Save (`Some`) or delete (`None`) the password for `host_id`.
    pub fn set_password(
        &mut self,
        host_id: &str,
        password: Option<&str>,
    ) -> Result<(), StoreError> {
        let sealed = match password {
            Some(password) if !password.is_empty() => Some(self.seal(password)?),
            _ => None,
        };
        self.edit(host_id, |entry| entry.password = sealed)
    }

    /// Save (`Some`) or delete (`None`) the key passphrase for `host_id`.
    pub fn set_passphrase(
        &mut self,
        host_id: &str,
        passphrase: Option<&str>,
    ) -> Result<(), StoreError> {
        let sealed = match passphrase {
            Some(passphrase) if !passphrase.is_empty() => Some(self.seal(passphrase)?),
            _ => None,
        };
        self.edit(host_id, |entry| entry.passphrase = sealed)
    }

    /// Stored plugin secret, if one is saved and decryptable.
    pub fn plugin_secret(&self, scope: &str, plugin: &str, field: &str) -> Option<String> {
        self.file
            .plugin_secrets
            .get(scope)?
            .get(plugin)?
            .get(field)
            .and_then(|sealed| self.unseal(sealed))
    }

    /// Whether a plugin secret is saved.
    pub fn has_plugin_secret(&self, scope: &str, plugin: &str, field: &str) -> bool {
        self.file
            .plugin_secrets
            .get(scope)
            .and_then(|plugins| plugins.get(plugin))
            .is_some_and(|fields| fields.contains_key(field))
    }

    /// Save (`Some`) or delete (`None`) one plugin secret.
    pub fn set_plugin_secret(
        &mut self,
        scope: &str,
        plugin: &str,
        field: &str,
        value: Option<&str>,
    ) -> Result<(), StoreError> {
        let sealed = match value {
            Some(value) if !value.is_empty() => Some(self.seal(value)?),
            _ => None,
        };
        let fields = self
            .file
            .plugin_secrets
            .entry(scope.to_owned())
            .or_default()
            .entry(plugin.to_owned())
            .or_default();
        match sealed {
            Some(sealed) => {
                fields.insert(field.to_owned(), sealed);
            }
            None => {
                fields.remove(field);
            }
        }
        // Keep the file tidy: drop empty plugins and scopes.
        if fields.is_empty()
            && let Some(plugins) = self.file.plugin_secrets.get_mut(scope)
        {
            plugins.remove(plugin);
        }
        if self
            .file
            .plugin_secrets
            .get(scope)
            .is_some_and(BTreeMap::is_empty)
        {
            self.file.plugin_secrets.remove(scope);
        }
        crate::write_json(&self.path, &self.file)
    }

    /// Forget everything stored for `scope`: its host secrets and its plugins'.
    ///
    /// Called when a host is deleted, so a re-used id cannot inherit them.
    pub fn forget_scope(&mut self, scope: &str) -> Result<(), StoreError> {
        self.file.entries.remove(scope);
        self.file.plugin_secrets.remove(scope);
        crate::write_json(&self.path, &self.file)
    }

    fn edit(
        &mut self,
        host_id: &str,
        change: impl FnOnce(&mut SealedEntry),
    ) -> Result<(), StoreError> {
        let entry = self.file.entries.entry(host_id.to_owned()).or_default();
        change(entry);
        if entry.password.is_none() && entry.passphrase.is_none() {
            self.file.entries.remove(host_id);
        }
        crate::write_json(&self.path, &self.file)
    }
}

/// Read the key file, creating it with random bytes and `0600` when missing.
fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], StoreError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == KEY_LEN => {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
        Ok(_) => {
            // A truncated or foreign key file cannot decrypt anything: replace it.
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(StoreError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let mut key = [0u8; KEY_LEN];
    getrandom::fill(&mut key).map_err(|error| StoreError::Random {
        detail: error.to_string(),
    })?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(&key).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wassh-secrets-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn passwords_round_trip_and_survive_a_reopen() {
        let dir = scratch();
        {
            let mut store = SecretsStore::open(&dir).unwrap();
            assert_eq!(store.password("host-1"), None);
            store.set_password("host-1", Some("hunter2")).unwrap();
            assert!(store.has_password("host-1"));
            assert_eq!(store.password("host-1").as_deref(), Some("hunter2"));
        }
        let store = SecretsStore::open(&dir).unwrap();
        assert_eq!(store.password("host-1").as_deref(), Some("hunter2"));
        assert_eq!(store.password("host-2"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_password_is_not_written_in_the_clear() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        store.set_password("host-1", Some("hunter2")).unwrap();
        let raw = std::fs::read_to_string(secrets_file(&dir)).unwrap();
        assert!(!raw.contains("hunter2"), "secrets.json was {raw}");
        assert!(raw.contains("host-1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clearing_removes_the_entry() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        store.set_password("host-1", Some("hunter2")).unwrap();
        store.set_password("host-1", None).unwrap();
        assert!(!store.has_password("host-1"));
        let raw = std::fs::read_to_string(secrets_file(&dir)).unwrap();
        assert!(!raw.contains("host-1"), "secrets.json was {raw}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_lost_key_makes_stored_secrets_unreadable() {
        let dir = scratch();
        {
            let mut store = SecretsStore::open(&dir).unwrap();
            store.set_password("host-1", Some("hunter2")).unwrap();
        }
        std::fs::remove_file(key_file(&dir)).unwrap();
        let store = SecretsStore::open(&dir).unwrap();
        assert_eq!(store.password("host-1"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn passphrases_are_separate_from_passwords() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        store.set_passphrase("host-1", Some("key-pass")).unwrap();
        assert!(store.has_passphrase("host-1"));
        assert!(!store.has_password("host-1"));
        assert_eq!(store.passphrase("host-1").as_deref(), Some("key-pass"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_password_clears_the_entry() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        store.set_password("host-1", Some("hunter2")).unwrap();
        store.set_password("host-1", Some("")).unwrap();
        assert!(!store.has_password("host-1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_plugin_secret_round_trips_and_stays_sealed() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        assert!(!store.has_plugin_secret("h1", "mqtt-analyser", "password"));
        assert_eq!(store.plugin_secret("h1", "mqtt-analyser", "password"), None);

        store
            .set_plugin_secret("h1", "mqtt-analyser", "password", Some("broker-secret"))
            .unwrap();
        assert!(store.has_plugin_secret("h1", "mqtt-analyser", "password"));
        assert_eq!(
            store.plugin_secret("h1", "mqtt-analyser", "password").as_deref(),
            Some("broker-secret")
        );

        // A reopen decrypts it with the same key file, and the file has no text.
        let mut store = SecretsStore::open(&dir).unwrap();
        assert_eq!(
            store.plugin_secret("h1", "mqtt-analyser", "password").as_deref(),
            Some("broker-secret")
        );
        let raw = std::fs::read_to_string(secrets_file(&dir)).unwrap();
        assert!(!raw.contains("broker-secret"), "{raw}");
        assert!(raw.contains("pluginSecrets"), "{raw}");

        // Clearing drops the entry and the now-empty scopes.
        store
            .set_plugin_secret("h1", "mqtt-analyser", "password", None)
            .unwrap();
        assert!(!store.has_plugin_secret("h1", "mqtt-analyser", "password"));
        let raw = std::fs::read_to_string(secrets_file(&dir)).unwrap();
        assert!(!raw.contains("mqtt-analyser"), "{raw}");

        // App-scope secrets share the same storage.
        store
            .set_plugin_secret(APP_SCOPE, "ai-agent", "webSearchApiKey", Some("key-1"))
            .unwrap();
        assert_eq!(
            store.plugin_secret(APP_SCOPE, "ai-agent", "webSearchApiKey").as_deref(),
            Some("key-1")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn forgetting_a_scope_drops_host_and_plugin_secrets() {
        let dir = scratch();
        let mut store = SecretsStore::open(&dir).unwrap();
        store.set_password("h1", Some("pw")).unwrap();
        store
            .set_plugin_secret("h1", "mqtt-analyser", "password", Some("mqtt"))
            .unwrap();
        store
            .set_plugin_secret("h2", "mqtt-analyser", "password", Some("other"))
            .unwrap();

        store.forget_scope("h1").unwrap();
        assert_eq!(store.password("h1"), None);
        assert!(!store.has_plugin_secret("h1", "mqtt-analyser", "password"));
        // Another host is untouched.
        assert_eq!(
            store.plugin_secret("h2", "mqtt-analyser", "password").as_deref(),
            Some("other")
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
