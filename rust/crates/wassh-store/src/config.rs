//! The whole on-disk configuration.
//!
//! `Config` opens every document once so the UI can read and write without touching the
//! filesystem itself.

use std::path::{Path, PathBuf};

use wassh_core::{AppSettings, HostsDocument, TabSnapshot};

use crate::document::JsonDocument;
use crate::paths;
use crate::StoreError;

/// Settings, hosts and tabs, all backed by atomic JSON files.
#[derive(Debug)]
pub struct Config {
    pub dir: PathBuf,
    pub settings: JsonDocument<AppSettings>,
    pub hosts: JsonDocument<HostsDocument>,
    pub tabs: JsonDocument<Vec<TabSnapshot>>,
    /// Documents that could not be read and were reset, as `file: reason`.
    pub problems: Vec<String>,
}

impl Config {
    /// Open (or initialise) the configuration in `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let dir = dir.as_ref().to_path_buf();
        let mut problems = Vec::new();
        let (settings, problem) = JsonDocument::open_lenient(
            paths::settings_file(&dir),
            AppSettings::default(),
            AppSettings::normalized,
        );
        problems.extend(problem);
        let (hosts, problem) = JsonDocument::open_lenient(
            paths::hosts_file(&dir),
            HostsDocument::default(),
            HostsDocument::normalized,
        );
        problems.extend(problem);
        let (tabs, problem) = JsonDocument::open_lenient(
            paths::tabs_file(&dir),
            Vec::new(),
            wassh_core::tabs::normalize_tabs,
        );
        problems.extend(problem);
        Ok(Self {
            dir,
            problems,
            settings,
            hosts,
            tabs,
        })
    }

    /// Open the configuration in the platform data directory.
    pub fn open_data_dir() -> Result<Self, StoreError> {
        Self::open(paths::data_dir()?)
    }

    /// Persist every document that has not been written yet.
    pub fn flush(&mut self) -> Result<(), StoreError> {
        if self.settings.is_dirty() {
            self.settings.save()?;
        }
        if self.hosts.is_dirty() {
            self.hosts.save()?;
        }
        if self.tabs.is_dirty() {
            self.tabs.save()?;
        }
        Ok(())
    }

    /// Store a host, normalising the document first.
    pub fn save_host(&mut self, host: wassh_core::HostProfile) -> Result<(), StoreError> {
        let mut document = self.hosts.get().clone();
        document.upsert(host);
        self.hosts.set(document.normalized())
    }

    /// Delete a host and clear references to it.
    pub fn delete_host(&mut self, id: &str) -> Result<(), StoreError> {
        let mut document = self.hosts.get().clone();
        document.remove(id);
        self.hosts.set(document.normalized())
    }

    /// Read a plugin's stored data.
    ///
    /// `scope` is the host id (or `tab:<id>` for unsaved sessions); `None` reads the
    /// plugin-wide file. Missing files read as `None`.
    pub fn plugin_data(
        &self,
        plugin_id: &str,
        scope: Option<&str>,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let path = paths::plugin_data_file(&self.dir, plugin_id, scope);
        crate::read_json(&path)
    }

    /// Write a plugin's stored data atomically.
    pub fn save_plugin_data(
        &self,
        plugin_id: &str,
        scope: Option<&str>,
        data: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let path = paths::plugin_data_file(&self.dir, plugin_id, scope);
        crate::write_json(&path, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use wassh_core::{ConnectionParams, ConnectionType, HostProfile};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wassh-config-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plugin_settings_survive_a_restart() {
        let dir = scratch();
        {
            let mut config = Config::open(&dir).unwrap();
            config
                .settings
                .update(|settings| {
                    settings.enabled_plugins = vec!["docker-manager".to_owned()];
                    settings.plugin_settings.insert(
                        "docker-manager".to_owned(),
                        [("confirmDestructive".to_owned(), serde_json::json!(false))]
                            .into_iter()
                            .collect(),
                    );
                })
                .unwrap();
            let mut host = HostProfile {
                id: "h1".to_owned(),
                name: "box".to_owned(),
                ..HostProfile::default()
            };
            host.plugin_settings.insert(
                "docker-manager".to_owned(),
                [("socketPath".to_owned(), serde_json::json!("/run/docker.sock"))]
                    .into_iter()
                    .collect(),
            );
            config.save_host(host).unwrap();
        }

        let config = Config::open(&dir).unwrap();
        let settings = config.settings.get();
        assert_eq!(
            settings.plugin_settings["docker-manager"]["confirmDestructive"],
            serde_json::json!(false)
        );
        let hosts = config.hosts.get();
        assert_eq!(
            hosts.hosts[0].plugin_settings["docker-manager"]["socketPath"],
            serde_json::json!("/run/docker.sock")
        );
        // The file uses the camelCase key the Electron app writes.
        let text = fs::read_to_string(dir.join("hosts.json")).unwrap();
        assert!(text.contains("\"pluginSettings\""), "got {text}");
    }

    #[test]
    fn opens_with_defaults_then_persists() {
        let dir = scratch();
        {
            let mut config = Config::open(&dir).unwrap();
            config.settings.update(|settings| {
                settings.theme = wassh_core::ThemeMode::Light;
                settings.enabled_plugins = vec!["sftp".to_owned()];
            })
            .unwrap();
        }

        let config = Config::open(&dir).unwrap();
        assert_eq!(config.settings.get().theme, wassh_core::ThemeMode::Light);
        assert!(config.settings.get().plugin_enabled("sftp"));
        assert!(config.hosts.get().hosts.is_empty());
        assert!(config.tabs.get().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_document_is_parked_and_defaults_load() {
        let dir = scratch();
        fs::write(paths::settings_file(&dir), "{ not json").unwrap();
        fs::write(
            paths::hosts_file(&dir),
            r#"{"groups":[],"hosts":[{"id":"h1","name":"Kept","connection":{"host":"h"}}]}"#,
        )
        .unwrap();

        let config = Config::open(&dir).unwrap();
        assert_eq!(config.problems.len(), 1, "{:?}", config.problems);
        assert!(config.problems[0].contains("settings.json"));
        assert_eq!(config.settings.get(), &AppSettings::default().normalized());
        // The other documents are unaffected.
        assert!(config.hosts.get().host("h1").is_some());
        assert!(dir.join("settings.json.corrupt").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plugin_data_round_trips_per_scope() {
        let dir = scratch();
        let config = Config::open(&dir).unwrap();
        assert_eq!(config.plugin_data("scratchpad", None).unwrap(), None);

        let value = serde_json::json!({ "text": "hello" });
        config
            .save_plugin_data("scratchpad", Some("host-1"), &value)
            .unwrap();
        assert_eq!(
            config.plugin_data("scratchpad", Some("host-1")).unwrap(),
            Some(value.clone())
        );
        assert_eq!(config.plugin_data("scratchpad", None).unwrap(), None);
        assert_eq!(
            config.plugin_data("scratchpad", Some("host-2")).unwrap(),
            None
        );
        assert!(dir.join("plugin-scratchpad.host-1.json").exists());

        // A scope with path characters cannot escape the data directory.
        config
            .save_plugin_data("scratchpad", Some("../evil"), &value)
            .unwrap();
        assert!(
            !dir.parent().unwrap().join("evil.json").exists(),
            "scopes must be sanitised"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_a_host_normalises_it() {
        let dir = scratch();
        let mut config = Config::open(&dir).unwrap();
        config
            .save_host(HostProfile {
                id: "h1".to_owned(),
                name: "Example".to_owned(),
                connection: ConnectionParams {
                    kind: ConnectionType::Ssh,
                    host: "example.com".to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();

        let reloaded = Config::open(&dir).unwrap();
        let host = reloaded.hosts.get().host("h1").expect("host saved");
        assert_eq!(host.connection.port, 22);
        assert_eq!(reloaded.hosts.get().ungrouped_host_ids, vec!["h1".to_owned()]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deleting_a_host_clears_jump_references() {
        let dir = scratch();
        let mut config = Config::open(&dir).unwrap();
        config
            .save_host(HostProfile {
                id: "jump".to_owned(),
                ..Default::default()
            })
            .unwrap();
        config
            .save_host(HostProfile {
                id: "target".to_owned(),
                connection: ConnectionParams {
                    jump_host_id: Some("jump".to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();

        config.delete_host("jump").unwrap();
        let host = config.hosts.get().host("target").expect("target remains");
        assert_eq!(host.connection.jump_host_id, None);
        assert!(config.hosts.get().host("jump").is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flush_writes_documents_created_from_defaults() {
        let dir = scratch();
        let mut config = Config::open(&dir).unwrap();
        assert!(!paths::settings_file(&dir).exists());
        config.flush().unwrap();
        assert!(paths::settings_file(&dir).exists());
        assert!(paths::hosts_file(&dir).exists());
        assert!(paths::tabs_file(&dir).exists());
        fs::remove_dir_all(&dir).ok();
    }
}
