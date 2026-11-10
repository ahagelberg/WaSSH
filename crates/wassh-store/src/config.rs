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
    pub notices: Vec<String>,
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
            notices: Vec::new(),
            settings,
            hosts,
            tabs,
        })
    }

    /// Open the configuration in the platform data directory.
    pub fn open_data_dir() -> Result<Self, StoreError> {
        let dir = paths::data_dir()?;
        let root = dir.parent().ok_or(StoreError::NoDataDir("data directory has no parent"))?;
        let mut legacy = root.join("WaSSH");
        if !legacy.is_dir() {
            legacy = root.join("wassh");
        }
        Self::open_with_legacy(&dir, &legacy)
    }

    fn open_with_legacy(dir: &Path, legacy: &Path) -> Result<Self, StoreError> {
        let migration = crate::legacy::migrate(dir, legacy);
        let mut config = Self::open(dir)?;
        match migration {
            Ok(Some(_)) => config.notices.push("Imported Electron configuration.".to_owned()),
            Ok(None) => {}
            Err(error) => config.problems.push(format!("Electron configuration migration failed: {error}")),
        }
        let host_secrets = crate::legacy::migrate_host_secrets(dir, legacy, config.hosts.get());
        let ai_secrets = crate::legacy::migrate_ai_secrets(&mut config, legacy);
        let mut incomplete = false;
        for result in [host_secrets, ai_secrets] {
            match result {
                Ok(missing) => incomplete |= missing,
                Err(error) => config.problems.push(format!("Electron credential migration failed: {error}")),
            }
        }
        if incomplete {
            config.notices.push("Some Electron credentials could not be imported and may need to be entered again.".to_owned());
        }
        Ok(config)
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

    /// Every stored document of one plugin, as the scope each belongs to.
    ///
    /// Scopes are read back from the directory, so documents left behind by
    /// sessions that are no longer open are found too. `None` is the
    /// plugin-wide file. The scope is the sanitised one the file name carries,
    /// which is what [`Self::plugin_data`] expects when reading it back.
    pub fn plugin_data_scopes(
        &self,
        plugin_id: &str,
    ) -> Result<Vec<Option<String>>, StoreError> {
        let prefix = paths::plugin_data_prefix(plugin_id);
        let mut scopes = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|error| StoreError::io(&self.dir, error))? {
            let entry = entry.map_err(|error| StoreError::io(&self.dir, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            let Some(rest) = rest.strip_suffix(".json") else {
                continue;
            };
            match rest.strip_prefix('.') {
                Some(scope) => scopes.push(Some(scope.to_owned())),
                None if rest.is_empty() => scopes.push(None),
                None => {}
            }
        }
        Ok(scopes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use wassh_core::{ConnectionParams, ConnectionType, HostProfile, LOCAL_HOST_ID};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("wassh-config-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn imports_plaintext_host_secrets_without_overwriting_or_reimporting() {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts": [
            {"id":"h", "host":"example.org", "username":"alice", "passwordVaultId":"pw", "passphraseVaultId":"key"}
        ]})).unwrap();
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({
            "pw": STANDARD.encode("password"), "key": STANDARD.encode("phrase")
        })).unwrap();
        crate::legacy::migrate(&native, &legacy).unwrap();
        let config = Config::open(&native).unwrap();
        let mut store = crate::secrets::SecretsStore::open(&native).unwrap();
        store.set_passphrase("h", Some("existing")).unwrap();
        assert!(!crate::legacy::migrate_host_secrets(&native, &legacy, config.hosts.get()).unwrap());
        let mut store = crate::secrets::SecretsStore::open(&native).unwrap();
        assert_eq!(store.password("h").as_deref(), Some("password"));
        assert_eq!(store.passphrase("h").as_deref(), Some("existing"));
        store.set_password("h", None).unwrap();
        crate::legacy::migrate_host_secrets(&native, &legacy, config.hosts.get()).unwrap();
        assert!(!crate::secrets::SecretsStore::open(&native).unwrap().has_password("h"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retries_unavailable_legacy_credentials() {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts": [
            {"id":"h", "host":"example.org", "username":"alice", "passwordVaultId":"pw"}
        ]})).unwrap();
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({"pw": "invalid-base64"})).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert!(config.notices.iter().any(|notice| notice.contains("credentials could not be imported")));
        assert!(!crate::secrets::SecretsStore::open(&native).unwrap().has_password("h"));
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({"pw": STANDARD.encode("password")})).unwrap();
        Config::open_with_legacy(&native, &legacy).unwrap();
        assert_eq!(crate::secrets::SecretsStore::open(&native).unwrap().password("h").as_deref(), Some("password"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn does_not_import_credentials_for_changed_host() {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts": [
            {"id":"h", "host":"example.org", "username":"alice", "passwordVaultId":"pw"}
        ]})).unwrap();
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({"pw": STANDARD.encode("password")})).unwrap();
        crate::legacy::migrate(&native, &legacy).unwrap();
        let config = Config::open(&native).unwrap();
        let mut hosts = config.hosts.get().clone();
        hosts.hosts.iter_mut().find(|host| host.id == "h").unwrap().connection.auth = wassh_core::AuthMethod::PrivateKey;
        crate::legacy::migrate_host_secrets(&native, &legacy, &hosts).unwrap();
        assert!(!crate::secrets::SecretsStore::open(&native).unwrap().has_password("h"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imports_legacy_ai_provider_and_key_once() {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        crate::write_json(&legacy.join("plugin-ai-agent.json"), &serde_json::json!({"providers": [
            {"id":"custom", "name":"Custom", "protocol":"openai", "baseUrl":"https://example.org", "models":[]}
        ]})).unwrap();
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({"ai-agent:custom": STANDARD.encode("api-key")})).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert_eq!(config.settings.get().plugin_settings["ai-agent"]["providers"][0]["id"], "custom");
        let mut store = crate::secrets::SecretsStore::open(&native).unwrap();
        assert_eq!(store.plugin_secret("app", "ai-agent", "provider.custom").as_deref(), Some("api-key"));
        store.set_plugin_secret("app", "ai-agent", "provider.custom", None).unwrap();
        Config::open_with_legacy(&native, &legacy).unwrap();
        assert!(!crate::secrets::SecretsStore::open(&native).unwrap().has_plugin_secret("app", "ai-agent", "provider.custom"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_electron_configuration_once() {
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        crate::write_json(&paths::settings_file(&legacy), &serde_json::json!({
            "theme":"dark", "termType":"vt100", "sessionStyleDefaults":{"fontSizePx":19}
        })).unwrap();
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({
            "groups":[{"id":"g", "name":"Servers", "collapsed":false, "hostIds":["h"]}],
            "hosts":[{"id":"h", "name":"Host", "host":"example.org", "port":2222, "username":"alice", "authMethod":"password"}]
        })).unwrap();
        crate::write_json(&paths::tabs_file(&legacy), &serde_json::json!([{
            "id":"t", "connection":{"hostId":"h", "host":"example.org", "port":2222}, "active":true
        }])).unwrap();
        crate::write_json(&paths::known_hosts_file(&legacy), &serde_json::json!([{
            "host":"example.org", "port":2222, "keyType":"ssh-ed25519", "fingerprint":"SHA256:test"
        }])).unwrap();
        crate::write_json(&legacy.join("plugin-scratchpad.h.json"), &serde_json::json!({"content":"notes"})).unwrap();
        crate::write_json(&legacy.join("vault.json"), &serde_json::json!({"password":"encrypted"})).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert!(config.problems.is_empty());
        assert!(config.notices[0].contains("Imported Electron configuration"));
        assert_eq!(config.settings.get().term_type, "vt100");
        assert_eq!(config.settings.get().session_style_defaults.font_size, 19);
        assert_eq!(config.hosts.get().host("h").unwrap().connection.host, "example.org");
        assert_eq!(config.hosts.get().host("h").unwrap().group_id.as_deref(), Some("g"));
        assert_eq!(config.tabs.get()[0].host_id.as_deref(), Some("h"));
        assert_eq!(config.plugin_data("scratchpad", Some("h")).unwrap().unwrap()["text"], "notes");
        let known: serde_json::Value = crate::read_json(&paths::known_hosts_file(&native)).unwrap().unwrap();
        assert_eq!(known[0]["fingerprint"], "SHA256:test");
        assert!(legacy.join("vault.json").exists());
        assert!(!native.join("vault.json").exists());
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts":[{"id":"later", "host":"later"}]})).unwrap();
        let reopened = Config::open_with_legacy(&native, &legacy).unwrap();
        assert!(reopened.hosts.get().host("later").is_none());
        assert!(reopened.notices.is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn migration_preserves_existing_native_configuration() {
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        let mut existing = Config::open(&native).unwrap();
        existing.settings.update(|settings| settings.term_type = "native".to_owned()).unwrap();
        existing.save_host(HostProfile {
            id:"same".into(), name:"Native".into(),
            connection: ConnectionParams { host:"native.example".into(), ..Default::default() },
            ..Default::default()
        }).unwrap();
        existing.tabs.set(Vec::new()).unwrap();
        existing.save_plugin_data("scratchpad", Some("same"), &serde_json::json!({"text":"native notes"})).unwrap();
        crate::write_json(&paths::known_hosts_file(&native), &serde_json::json!([
            {"host":"host", "port":22, "fingerprint":"SHA256:native"}
        ])).unwrap();
        crate::write_json(&paths::settings_file(&legacy), &serde_json::json!({"termType":"legacy"})).unwrap();
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts":[
            {"id":"same", "host":"old.example"}, {"id":"new", "host":"new.example"}
        ]})).unwrap();
        crate::write_json(&paths::tabs_file(&legacy), &serde_json::json!([{"id":"old-tab"}])).unwrap();
        crate::write_json(&legacy.join("plugin-scratchpad.same.json"), &serde_json::json!({"content":"old notes"})).unwrap();
        crate::write_json(&paths::known_hosts_file(&legacy), &serde_json::json!([
            {"host":"host", "port":22, "fingerprint":"SHA256:old"},
            {"host":"new", "port":22, "fingerprint":"SHA256:new"}
        ])).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert_eq!(config.settings.get().term_type, "native");
        assert_eq!(config.hosts.get().host("same").unwrap().connection.host, "native.example");
        assert_eq!(config.hosts.get().host("new").unwrap().connection.host, "new.example");
        assert!(config.tabs.get().is_empty());
        assert_eq!(config.plugin_data("scratchpad", Some("same")).unwrap().unwrap()["text"], "native notes");
        let known: serde_json::Value = crate::read_json(&paths::known_hosts_file(&native)).unwrap().unwrap();
        assert_eq!(known[0]["fingerprint"], "SHA256:native");
        assert_eq!(known[1]["fingerprint"], "SHA256:new");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn migration_reports_invalid_data_and_can_retry() {
        let root = scratch();
        let legacy = root.join("WaSSH");
        let native = root.join("wassh-native");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(paths::hosts_file(&legacy), "{broken").unwrap();
        crate::write_json(&paths::settings_file(&legacy), &serde_json::json!({"termType":"legacy"})).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert_eq!(config.problems.len(), 1);
        assert!(!paths::settings_file(&native).exists());
        assert!(!native.join(".electron-migrated").exists());
        crate::write_json(&paths::hosts_file(&legacy), &serde_json::json!({"hosts":[{"id":"h", "host":"host"}]})).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert!(config.problems.is_empty());
        assert_eq!(config.settings.get().term_type, "legacy");
        assert!(config.hosts.get().host("h").is_some());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn migrates_old_sessions_file_and_reconnect_default() {
        let root = scratch();
        let legacy = root.join("wassh");
        let native = root.join("wassh-native");
        crate::write_json(&paths::settings_file(&legacy), &serde_json::json!({"autoReconnectOnDrop":false})).unwrap();
        crate::write_json(&legacy.join("sessions.json"), &serde_json::json!([
            {"id":"h", "host":"example.org", "connectionType":"telnet"}
        ])).unwrap();
        crate::write_json(&paths::tabs_file(&legacy), &serde_json::json!([
            {"id":"t", "connection":{"hostId":"h", "host":"example.org", "connectionType":"telnet"}}
        ])).unwrap();
        let config = Config::open_with_legacy(&native, &legacy).unwrap();
        assert_eq!(config.hosts.get().host("h").unwrap().connection.reconnect, wassh_core::session::ReconnectMode::None);
        assert_eq!(config.tabs.get()[0].connection.reconnect, wassh_core::session::ReconnectMode::None);
        assert!(config.settings.get().enabled_plugins.contains(&"scratchpad".to_owned()));
        fs::remove_dir_all(&root).ok();
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
            hosts.host("h1").expect("saved host").plugin_settings["docker-manager"]
                ["socketPath"],
            serde_json::json!("/run/docker.sock")
        );
        // The file uses the camelCase key the Electron app writes.
        let text = fs::read_to_string(dir.join("hosts.json")).unwrap();
        assert!(text.contains("\"pluginSettings\""), "got {text}");
    }

    #[test]
    fn plugin_documents_are_listed_with_their_scope() {
        let dir = scratch();
        let config = Config::open(&dir).unwrap();
        for scope in [None, Some("host-1"), Some("tab:a1")] {
            config
                .save_plugin_data("ai-agent", scope, &serde_json::json!({}))
                .unwrap();
        }
        // Another plugin whose id starts the same must not be listed.
        config
            .save_plugin_data("ai-agent-extra", Some("host-1"), &serde_json::json!({}))
            .unwrap();

        let mut scopes = config.plugin_data_scopes("ai-agent").unwrap();
        scopes.sort();
        assert_eq!(
            scopes,
            [
                None,
                Some("host-1".to_owned()),
                // The scope comes back sanitised, which is what the file name holds.
                Some("tab_a1".to_owned()),
            ]
        );
        fs::remove_dir_all(&dir).ok();
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
        // A fresh document holds only the built-in local terminal host.
        let hosts = config.hosts.get();
        assert_eq!(hosts.hosts.len(), 1);
        assert!(hosts.host(LOCAL_HOST_ID).is_some());
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
        assert_eq!(
            reloaded.hosts.get().ungrouped_host_ids,
            vec![LOCAL_HOST_ID.to_owned(), "h1".to_owned()]
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_local_terminal_host_survives_a_delete() {
        let dir = scratch();
        let mut config = Config::open(&dir).unwrap();
        config.delete_host(LOCAL_HOST_ID).unwrap();
        let reloaded = Config::open(&dir).unwrap();
        assert!(reloaded.hosts.get().host(LOCAL_HOST_ID).is_some());
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
