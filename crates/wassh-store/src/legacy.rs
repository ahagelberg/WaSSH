use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wassh_core::{AppSettings, ConnectionParams, HostsDocument, TabSnapshot};

use crate::{StoreError, paths, read_json, write_atomic, write_json};

const MIGRATION_MARKER: &str = ".electron-migrated";
const LEGACY_HOSTS_FILE: &str = "sessions.json";
const LEGACY_VAULT_FILE: &str = "vault.json";
const SECRET_PROGRESS_FILE: &str = ".electron-secrets.json";
#[cfg(any(windows, test))]
const CHROMIUM_NONCE_LEN: usize = 12;
#[cfg(target_os = "linux")]
const CHROMIUM_KEY_LEN: usize = 16;
const PLUGIN_PREFIX: &str = "plugin-";
const DEFAULT_ENABLED_PLUGINS: &[&str] = &[
    "server-monitor",
    "scratchpad",
    "macro-pad",
    "mqtt-analyser",
    "sftp",
    "ai-agent",
    "connection-logger",
    "docker-manager",
];

fn decode_unencrypted_secret(encoded: &str) -> Option<String> {
    let bytes = STANDARD.decode(encoded).ok()?;
    if bytes.starts_with(b"v10") || bytes.starts_with(b"v11") {
        return None;
    }
    let value = String::from_utf8(bytes).ok()?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value)
}

#[cfg(target_os = "linux")]
fn linux_secret_key() -> Option<[u8; CHROMIUM_KEY_LEN]> {
    use secret_service::{EncryptionType, blocking::SecretService};
    let service = SecretService::connect(EncryptionType::Dh).ok()?;
    let mut passwords = Vec::new();
    for application in ["WaSSH", "wassh"] {
        let found = service
            .search_items(std::collections::HashMap::from([(
                "application",
                application,
            )]))
            .ok()?;
        if !found.locked.is_empty() {
            return None;
        }
        for item in found.unlocked {
            passwords.push(item.get_secret().ok()?);
        }
    }
    if passwords.len() != 1 {
        return None;
    }
    let mut key = [0; CHROMIUM_KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&passwords[0], b"saltysalt", 1, &mut key);
    Some(key)
}

#[cfg(target_os = "linux")]
fn decrypt_linux_secret(bytes: &[u8], key: &[u8; CHROMIUM_KEY_LEN]) -> Option<String> {
    use cbc::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
    let mut ciphertext = bytes.strip_prefix(b"v11")?.to_vec();
    let plaintext =
        cbc::Decryptor::<aes::Aes128>::new(key.into(), &[b' '; CHROMIUM_KEY_LEN].into())
            .decrypt_padded::<Pkcs7>(&mut ciphertext)
            .ok()?;
    String::from_utf8(plaintext.to_vec())
        .ok()
        .filter(|value| !value.is_empty())
}

#[cfg(windows)]
fn windows_secret_key(legacy: &Path) -> Option<Vec<u8>> {
    let state: Value = read_json(&legacy.join("Local State")).ok()??;
    let encoded = state.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let bytes = STANDARD.decode(encoded).ok()?;
    wassh_sys::unprotect_user_data(bytes.strip_prefix(b"DPAPI")?)
}

#[cfg(any(windows, test))]
fn decrypt_windows_secret(bytes: &[u8], key: &[u8]) -> Option<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    let ciphertext = bytes.strip_prefix(b"v10")?;
    let (nonce, ciphertext) = ciphertext.split_at_checked(CHROMIUM_NONCE_LEN)?;
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = aes_gcm::Nonce::try_from(nonce).ok()?;
    String::from_utf8(cipher.decrypt(&nonce, ciphertext).ok()?)
        .ok()
        .filter(|value| !value.is_empty())
}

fn decode_secret(encoded: &str, key: &OnceCell<Option<Vec<u8>>>, legacy: &Path) -> Option<String> {
    let bytes = STANDARD.decode(encoded).ok()?;
    #[cfg(target_os = "linux")]
    if bytes.starts_with(b"v11") {
        let key = key
            .get_or_init(|| linux_secret_key().map(Vec::from))
            .as_ref()?;
        return decrypt_linux_secret(&bytes, key.as_slice().try_into().ok()?);
    }
    #[cfg(windows)]
    if bytes.starts_with(b"v10") {
        let key = key.get_or_init(|| windows_secret_key(legacy)).as_ref()?;
        return decrypt_windows_secret(&bytes, key);
    }
    let _ = (key, legacy);
    decode_unencrypted_secret(encoded)
}

pub(crate) fn migrate_host_secrets(
    target: &Path,
    legacy: &Path,
    hosts: &HostsDocument,
) -> Result<bool, StoreError> {
    let vault_path = legacy.join(LEGACY_VAULT_FILE);
    let Some(vault) = read_json::<BTreeMap<String, String>>(&vault_path)? else {
        return Ok(false);
    };
    let mut source = paths::hosts_file(legacy);
    if !source.exists() {
        source = legacy.join(LEGACY_HOSTS_FILE);
    }
    let Some(raw) = read_json::<Value>(&source)? else {
        return Ok(false);
    };
    let raw_hosts = raw
        .as_array()
        .or_else(|| raw.get("hosts").and_then(Value::as_array));
    let Some(raw_hosts) = raw_hosts else {
        return Ok(false);
    };
    let progress_path = target.join(SECRET_PROGRESS_FILE);
    let mut progress = read_json::<BTreeSet<String>>(&progress_path)?.unwrap_or_default();
    let mut store = crate::secrets::SecretsStore::open(target)?;
    let encryption_key = OnceCell::new();
    let mut incomplete = false;
    for (index, raw) in raw_hosts.iter().enumerate() {
        let Some(raw) = raw.as_object() else { continue };
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("legacy-host-{index}"));
        let Some(host) = hosts.hosts.iter().find(|host| host.id == id) else {
            continue;
        };
        let connection = connection_from_legacy(raw, false)
            .map_err(|error| StoreError::json(&source, error))?
            .normalized();
        if host.connection.kind != connection.kind
            || host.connection.host != connection.host
            || host.connection.port != connection.port
            || host.connection.username != connection.username
            || host.connection.private_key_path != connection.private_key_path
            || host.connection.auth != connection.auth
        {
            continue;
        }
        for (field, passphrase) in [("passwordVaultId", false), ("passphraseVaultId", true)] {
            let Some(vault_id) = raw.get(field).and_then(Value::as_str) else {
                continue;
            };
            let Some(encoded) = vault.get(vault_id) else {
                continue;
            };
            let key = format!("host:{id}:{field}");
            if progress.contains(&key) {
                continue;
            }
            let present = if passphrase {
                store.has_passphrase(&id)
            } else {
                store.has_password(&id)
            };
            if !present {
                let Some(secret) = decode_secret(encoded, &encryption_key, legacy) else {
                    incomplete = true;
                    continue;
                };
                if passphrase {
                    store.set_passphrase(&id, Some(&secret))?;
                } else {
                    store.set_password(&id, Some(&secret))?;
                }
            }
            progress.insert(key);
            write_json(&progress_path, &progress)?;
        }
    }
    Ok(incomplete)
}

pub(crate) fn migrate_ai_secrets(
    config: &mut crate::Config,
    legacy: &Path,
) -> Result<bool, StoreError> {
    let source = legacy.join("plugin-ai-agent.json");
    let Some(data) = read_json::<Value>(&source)? else {
        return Ok(false);
    };
    let Some(providers) = data.get("providers").and_then(Value::as_array) else {
        return Ok(false);
    };
    let progress_path = config.dir.join(SECRET_PROGRESS_FILE);
    let mut progress = read_json::<BTreeSet<String>>(&progress_path)?.unwrap_or_default();
    let provider_marker = "ai-providers".to_owned();
    if !progress.contains(&provider_marker) {
        let mut settings = config.settings.get().clone();
        let plugin = settings
            .plugin_settings
            .entry("ai-agent".to_owned())
            .or_default();
        if !plugin.contains_key("providers") {
            plugin.insert("providers".to_owned(), Value::Array(providers.clone()));
            config.settings.set(settings)?;
        }
        progress.insert(provider_marker);
        write_json(&progress_path, &progress)?;
    }
    let Some(current) = config
        .settings
        .get()
        .plugin_settings
        .get("ai-agent")
        .and_then(|plugin| plugin.get("providers"))
        .and_then(Value::as_array)
    else {
        return Ok(false);
    };
    let Some(vault) = read_json::<BTreeMap<String, String>>(&legacy.join(LEGACY_VAULT_FILE))?
    else {
        return Ok(false);
    };
    let mut store = crate::secrets::SecretsStore::open(&config.dir)?;
    let encryption_key = OnceCell::new();
    let mut incomplete = false;
    for provider in providers {
        let Some(id) = provider
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !current.iter().any(|other| {
            other.get("id") == provider.get("id")
                && other.get("baseUrl") == provider.get("baseUrl")
                && other.get("protocol") == provider.get("protocol")
        }) {
            continue;
        }
        let Some(encoded) = vault.get(&format!("ai-agent:{id}")) else {
            continue;
        };
        let key = format!("provider:{id}");
        if progress.contains(&key) {
            continue;
        }
        let field = format!("provider.{id}");
        if !store.has_plugin_secret(crate::secrets::APP_SCOPE, "ai-agent", &field) {
            let Some(secret) = decode_secret(encoded, &encryption_key, legacy) else {
                incomplete = true;
                continue;
            };
            store.set_plugin_secret(
                crate::secrets::APP_SCOPE,
                "ai-agent",
                &field,
                Some(&secret),
            )?;
        }
        progress.insert(key);
        write_json(&progress_path, &progress)?;
    }
    Ok(incomplete)
}

#[derive(Deserialize, Serialize)]
struct KnownHost {
    host: String,
    port: u16,
    fingerprint: String,
}

pub(crate) fn migrate(target: &Path, legacy: &Path) -> Result<Option<bool>, StoreError> {
    if !legacy.is_dir() || target.join(MIGRATION_MARKER).exists() {
        return Ok(None);
    }
    let mut pending = Vec::<(PathBuf, Value)>::new();
    let mut found = false;
    let source = paths::settings_file(legacy);
    let destination = paths::settings_file(target);
    let legacy_settings = read_json::<Map<String, Value>>(&source)?;
    let reconnect_disabled = legacy_settings
        .as_ref()
        .and_then(|settings| settings.get("autoReconnectOnDrop"))
        .and_then(Value::as_bool)
        == Some(false);
    if let Some(raw) = &legacy_settings {
        found = true;
        if !destination.exists() {
            let settings =
                settings_from_legacy(raw).map_err(|error| StoreError::json(&source, error))?;
            pending.push((
                destination,
                serde_json::to_value(settings.normalized())
                    .map_err(|error| StoreError::json(&source, error))?,
            ));
        }
    }
    let mut source = paths::hosts_file(legacy);
    if !source.exists() {
        source = legacy.join(LEGACY_HOSTS_FILE);
    }
    if let Some(raw) = read_json::<Value>(&source)? {
        found = true;
        let imported = hosts_from_legacy(raw, reconnect_disabled)
            .map_err(|error| StoreError::json(&source, error))?;
        let destination = paths::hosts_file(target);
        let mut current = read_json::<HostsDocument>(&destination)?.unwrap_or_default();
        for group in imported.groups {
            if !current
                .groups
                .iter()
                .any(|existing| existing.id == group.id)
            {
                current.groups.push(group);
            }
        }
        for host in imported.hosts {
            if !current.hosts.iter().any(|existing| existing.id == host.id) {
                current.hosts.push(host);
            }
        }
        for id in imported.ungrouped_host_ids {
            if !current.ungrouped_host_ids.contains(&id) {
                current.ungrouped_host_ids.push(id);
            }
        }
        if !destination.exists() {
            current.ungrouped_collapsed = imported.ungrouped_collapsed;
        }
        pending.push((
            destination,
            serde_json::to_value(current.normalized())
                .map_err(|error| StoreError::json(&source, error))?,
        ));
    }
    let source = paths::tabs_file(legacy);
    let destination = paths::tabs_file(target);
    if let Some(raw) = read_json::<Value>(&source)? {
        found = true;
        if !destination.exists() {
            let tabs = tabs_from_legacy(raw, reconnect_disabled)
                .map_err(|error| StoreError::json(&source, error))?;
            pending.push((
                destination,
                serde_json::to_value(tabs).map_err(|error| StoreError::json(&source, error))?,
            ));
        }
    }
    let source = paths::known_hosts_file(legacy);
    if let Some(imported) = read_json::<Vec<KnownHost>>(&source)? {
        found = true;
        let destination = paths::known_hosts_file(target);
        let mut current = read_json::<Vec<KnownHost>>(&destination)?.unwrap_or_default();
        for host in imported {
            if !current
                .iter()
                .any(|existing| existing.host == host.host && existing.port == host.port)
            {
                current.push(host);
            }
        }
        pending.push((
            destination,
            serde_json::to_value(current).map_err(|error| StoreError::json(&source, error))?,
        ));
    }
    for entry in std::fs::read_dir(legacy).map_err(|error| StoreError::io(legacy, error))? {
        let entry = entry.map_err(|error| StoreError::io(legacy, error))?;
        if !entry
            .file_type()
            .map_err(|error| StoreError::io(entry.path(), error))?
            .is_file()
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(PLUGIN_PREFIX) || !name.ends_with(".json") {
            continue;
        }
        found = true;
        let destination = target.join(name);
        if destination.exists() {
            continue;
        }
        let source = entry.path();
        if let Some(mut data) = read_json::<Value>(&source)? {
            if (name == "plugin-scratchpad.json" || name.starts_with("plugin-scratchpad."))
                && let Some(data) = data.as_object_mut()
                && let Some(content) = data.remove("content")
            {
                data.entry("text".to_owned()).or_insert(content);
            }
            pending.push((destination, data));
        }
    }
    if !found && !legacy.join(LEGACY_VAULT_FILE).exists() {
        return Ok(None);
    }
    if legacy_settings.is_none() && !paths::settings_file(target).exists() {
        let settings = settings_from_legacy(&Map::new())
            .map_err(|error| StoreError::json(paths::settings_file(legacy), error))?;
        pending.push((
            paths::settings_file(target),
            serde_json::to_value(settings.normalized())
                .map_err(|error| StoreError::json(paths::settings_file(legacy), error))?,
        ));
    }
    for (path, data) in pending {
        write_json(&path, &data)?;
    }
    write_atomic(&target.join(MIGRATION_MARKER), b"")?;
    Ok(Some(legacy.join(LEGACY_VAULT_FILE).exists()))
}

fn mapped_fields(raw: &Map<String, Value>, fields: &[(&str, &str)]) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|(old, new)| {
            raw.get(*old)
                .map(|value| ((*new).to_owned(), value.clone()))
        })
        .collect()
}

fn style_from_legacy(raw: &Map<String, Value>) -> Map<String, Value> {
    let mut style = mapped_fields(
        raw,
        &[
            ("tabColor", "tabColor"),
            ("termBackground", "termBackground"),
            ("termForeground", "termForeground"),
            ("fontSizePx", "fontSize"),
            ("fontFamily", "fontFamily"),
            ("bellMode", "bell"),
            ("cursorStyle", "cursor"),
            ("cursorBlink", "cursorBlink"),
            ("scrollbackLines", "scrollback"),
        ],
    );
    if style.get("fontFamily").and_then(Value::as_str) == Some("") {
        style.remove("fontFamily");
    }
    style
}

fn tunnel_from_legacy(value: &Value) -> Value {
    let mut tunnel = value.clone();
    if let Some(tunnel) = tunnel.as_object_mut()
        && let Some(kind) = tunnel.remove("type")
    {
        tunnel.insert("kind".to_owned(), kind);
    }
    tunnel
}

fn connection_from_legacy(
    raw: &Map<String, Value>,
    reconnect_disabled: bool,
) -> Result<ConnectionParams, serde_json::Error> {
    let mut connection = Map::new();
    for (old, new) in [
        ("connectionType", "type"),
        ("host", "host"),
        ("port", "port"),
        ("username", "username"),
        ("authMethod", "auth"),
        ("privateKeyPath", "privateKeyPath"),
        ("proxyHostId", "jumpHostId"),
        ("x11Forwarding", "x11"),
        ("reconnectMode", "reconnect"),
    ] {
        if let Some(value) = raw.get(old) {
            if matches!(old, "privateKeyPath" | "proxyHostId") && value.as_str() == Some("") {
                continue;
            }
            connection.insert(new.to_owned(), value.clone());
        }
    }
    let mut serial = mapped_fields(
        raw,
        &[
            ("serialBaudRate", "baud"),
            ("serialDataBits", "dataBits"),
            ("serialParity", "parity"),
            ("serialFlowControl", "flow"),
        ],
    );
    if raw.get("connectionType").and_then(Value::as_str) == Some("serial")
        && let Some(path) = raw.get("host")
    {
        serial.insert("path".to_owned(), path.clone());
    }
    if let Some(bits) = raw.get("serialStopBits").and_then(Value::as_number) {
        serial.insert("stopBits".to_owned(), Value::String(bits.to_string()));
    }
    if let Some(flow) = serial.get_mut("flow") {
        match flow.as_str() {
            Some("rtscts") => *flow = Value::String("rtsCts".to_owned()),
            Some("xonxoff") => *flow = Value::String("xonXoff".to_owned()),
            _ => {}
        }
    }
    connection.insert("serial".to_owned(), Value::Object(serial));
    connection.insert("style".to_owned(), Value::Object(style_from_legacy(raw)));
    let mut remote_session = mapped_fields(
        raw,
        &[
            ("openInScreen", "enabled"),
            ("remoteSessionKind", "multiplexer"),
            ("screenSessionName", "name"),
            ("screenBusyHandling", "busy"),
        ],
    );
    remote_session
        .entry("multiplexer".to_owned())
        .or_insert(Value::String("screen".to_owned()));
    connection.insert("remoteSession".to_owned(), Value::Object(remote_session));
    connection
        .entry("auth".to_owned())
        .or_insert(Value::String("none".to_owned()));
    if reconnect_disabled {
        connection
            .entry("reconnect".to_owned())
            .or_insert(Value::String("none".to_owned()));
    }
    if let Some(tunnels) = raw.get("tunnels").and_then(Value::as_array) {
        connection.insert(
            "tunnels".to_owned(),
            Value::Array(tunnels.iter().map(tunnel_from_legacy).collect()),
        );
    }
    serde_json::from_value(Value::Object(connection))
}

fn settings_from_legacy(raw: &Map<String, Value>) -> Result<AppSettings, serde_json::Error> {
    let mut settings = mapped_fields(
        raw,
        &[
            ("theme", "theme"),
            ("termType", "termType"),
            ("reconnectOnStartup", "reconnectOnStartup"),
            ("sidebarCollapsed", "sidebarCollapsed"),
            ("windowBounds", "windowBounds"),
            ("enabledPlugins", "enabledPlugins"),
            ("pluginSettings", "pluginSettings"),
        ],
    );
    settings
        .entry("theme".to_owned())
        .or_insert(Value::String("dark".to_owned()));
    settings
        .entry("enabledPlugins".to_owned())
        .or_insert(serde_json::to_value(DEFAULT_ENABLED_PLUGINS)?);
    if let Some(plugin) = settings
        .get_mut("pluginSettings")
        .and_then(Value::as_object_mut)
        .and_then(|plugins| plugins.get_mut("macro-pad"))
        .and_then(Value::as_object_mut)
    {
        let groups = plugin
            .get("groups")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !plugin.contains_key("macros")
            && let Some(Value::Array(buttons)) = plugin.remove("buttons")
        {
            let macros = buttons
                .into_iter()
                .map(|mut button| {
                    if let Some(button) = button.as_object_mut()
                        && let Some(group_id) = button
                            .remove("groupId")
                            .and_then(|id| id.as_str().map(str::to_owned))
                        && let Some(name) = groups
                            .iter()
                            .find(|group| {
                                group.get("id").and_then(Value::as_str) == Some(group_id.as_str())
                            })
                            .and_then(|group| group.get("name"))
                    {
                        button.insert("group".to_owned(), name.clone());
                    }
                    button
                })
                .collect();
            plugin.insert("macros".to_owned(), Value::Array(macros));
        }
    }
    if let Some(style) = raw.get("sessionStyleDefaults").and_then(Value::as_object) {
        let mut style = style_from_legacy(style);
        style.retain(|_, value| !value.is_null());
        settings.insert("sessionStyleDefaults".to_owned(), Value::Object(style));
    } else {
        let mut style = style_from_legacy(raw);
        style.retain(|_, value| !value.is_null());
        settings.insert("sessionStyleDefaults".to_owned(), Value::Object(style));
    }
    if let Some(templates) = raw.get("tunnelTemplates").and_then(Value::as_array) {
        let templates = templates
            .iter()
            .cloned()
            .map(|mut template| {
                if let Some(tunnel) = template.get_mut("tunnel") {
                    *tunnel = tunnel_from_legacy(tunnel);
                }
                template
            })
            .collect();
        settings.insert("tunnelTemplates".to_owned(), Value::Array(templates));
    }
    serde_json::from_value(Value::Object(settings))
}

fn hosts_from_legacy(
    raw: Value,
    reconnect_disabled: bool,
) -> Result<HostsDocument, serde_json::Error> {
    let mut document = if let Value::Array(hosts) = raw {
        Map::from_iter([("hosts".to_owned(), Value::Array(hosts))])
    } else {
        serde_json::from_value::<Map<String, Value>>(raw)?
    };
    let host_values = document.remove("hosts").unwrap_or(Value::Array(Vec::new()));
    let hosts: Vec<Map<String, Value>> = serde_json::from_value(host_values)?;
    let groups = document
        .get("groups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut converted = Vec::with_capacity(hosts.len());
    for (index, raw) in hosts.iter().enumerate() {
        let mut host: wassh_core::HostProfile = serde_json::from_value(Value::Object(raw.clone()))?;
        if host.id.is_empty() {
            host.id = format!("legacy-host-{index}");
        }
        host.connection = connection_from_legacy(raw, reconnect_disabled)?;
        host.group_id = groups.iter().find_map(|group| {
            let ids = group.get("hostIds")?.as_array()?;
            if ids.iter().any(|id| id.as_str() == Some(host.id.as_str())) {
                group.get("id")?.as_str().map(str::to_owned)
            } else {
                None
            }
        });
        converted.push(host);
    }
    converted.sort_by_key(|host| {
        groups
            .iter()
            .enumerate()
            .find_map(|(group_index, group)| {
                let ids = group.get("hostIds")?.as_array()?;
                ids.iter()
                    .position(|id| id.as_str() == Some(host.id.as_str()))
                    .map(|host_index| (group_index, host_index))
            })
            .unwrap_or((groups.len(), usize::MAX))
    });
    document.insert("hosts".to_owned(), serde_json::to_value(converted)?);
    Ok(serde_json::from_value::<HostsDocument>(Value::Object(document))?.normalized())
}

fn tabs_from_legacy(
    raw: Value,
    reconnect_disabled: bool,
) -> Result<Vec<TabSnapshot>, serde_json::Error> {
    let tabs: Vec<Map<String, Value>> = serde_json::from_value(raw)?;
    let mut converted = Vec::with_capacity(tabs.len());
    for mut tab in tabs {
        let raw_connection = tab
            .remove("connection")
            .unwrap_or(Value::Object(Map::new()));
        let connection: Map<String, Value> = serde_json::from_value(raw_connection)?;
        let mut snapshot: TabSnapshot = serde_json::from_value(Value::Object(tab))?;
        snapshot.host_id = connection
            .get("hostId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        snapshot.connection = connection_from_legacy(&connection, reconnect_disabled)?;
        converted.push(snapshot);
    }
    Ok(wassh_core::tabs::normalize_tabs(converted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_windows_v10_secret_and_rejects_tampering() {
        use aes_gcm::aead::{Aead, KeyInit};
        let key = [7; 32];
        let nonce = [8; CHROMIUM_NONCE_LEN];
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&key).unwrap();
        let mut bytes = b"v10".to_vec();
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(
            &cipher
                .encrypt(&nonce.into(), b"password".as_slice())
                .unwrap(),
        );
        assert_eq!(
            decrypt_windows_secret(&bytes, &key).as_deref(),
            Some("password")
        );
        *bytes.last_mut().unwrap() ^= 1;
        assert_eq!(decrypt_windows_secret(&bytes, &key), None);
        assert_eq!(decrypt_windows_secret(b"v10short", &key), None);
    }
    use wassh_core::connection::{AuthMethod, ConnectionType};

    #[test]
    fn decodes_only_recognizable_unencrypted_secrets() {
        assert_eq!(
            decode_unencrypted_secret(&STANDARD.encode("correct horse")),
            Some("correct horse".to_owned())
        );
        assert_eq!(
            decode_unencrypted_secret(&STANDARD.encode(b"v10ciphertext")),
            None
        );
        assert_eq!(
            decode_unencrypted_secret(&STANDARD.encode([1, 0, 0, 0, 255])),
            None
        );
        assert_eq!(decode_unencrypted_secret("not base64"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn decrypts_linux_v11_secret() {
        use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
        let key = [42; CHROMIUM_KEY_LEN];
        let plaintext = b"test password";
        let mut buffer = [0; CHROMIUM_KEY_LEN];
        buffer[..plaintext.len()].copy_from_slice(plaintext);
        let encrypted =
            cbc::Encryptor::<aes::Aes128>::new(&key.into(), &[b' '; CHROMIUM_KEY_LEN].into())
                .encrypt_padded::<Pkcs7>(&mut buffer, plaintext.len())
                .unwrap();
        let mut data = b"v11".to_vec();
        data.extend_from_slice(encrypted);
        assert_eq!(
            decrypt_linux_secret(&data, &key).as_deref(),
            Some("test password")
        );
        assert!(decrypt_linux_secret(&data[..data.len() - 1], &key).is_none());
    }

    #[test]
    fn converts_flat_legacy_connection() {
        let legacy = serde_json::json!({
            "connectionType": "ssh",
            "host": "example.org",
            "port": 2222,
            "username": "alice",
            "authMethod": "privateKey",
            "privateKeyPath": "/home/alice/.ssh/id_ed25519",
            "proxyHostId": "jump"
        });
        let connection = connection_from_legacy(legacy.as_object().unwrap(), false).unwrap();
        assert_eq!(connection.kind, ConnectionType::Ssh);
        assert_eq!(connection.host, "example.org");
        assert_eq!(connection.port, 2222);
        assert_eq!(connection.username, "alice");
        assert_eq!(connection.auth, AuthMethod::PrivateKey);
        assert_eq!(
            connection.private_key_path.as_deref(),
            Some("/home/alice/.ssh/id_ed25519")
        );
        assert_eq!(connection.jump_host_id.as_deref(), Some("jump"));
    }

    #[test]
    fn converts_legacy_connection_options_and_settings() {
        let legacy = serde_json::json!({
            "connectionType": "ssh", "host": "example.org", "port": 22,
            "serialBaudRate": 9600, "serialDataBits": 7, "serialStopBits": 1.5,
            "serialFlowControl": "rtscts", "fontSizePx": 18,
            "bellMode": "invertLine", "x11Forwarding": true,
            "openInScreen": true, "remoteSessionKind": "screen",
            "screenSessionName": "work", "screenBusyHandling": "share",
            "tunnels": [{"id":"t", "type":"remote", "listenPort":2200}],
            "theme": "dark", "sessionStyleDefaults": {"fontSizePx": 16, "cursorStyle": "bar"},
            "pluginSettings": {"macro-pad": {
                "buttons": [{"id":"m", "label":"Deploy", "text":"deploy", "groupId":"g"}],
                "groups": [{"id":"g", "name":"Operations"}]
            }},
            "tunnelTemplates": [{"id":"template", "name":"Forward", "tunnel":{"type":"dynamic", "listenPort":1080}}]
        });
        let raw = legacy.as_object().unwrap();
        let connection = connection_from_legacy(raw, false).unwrap();
        assert_eq!(connection.serial.baud, 9600);
        assert_eq!(connection.serial.stop_bits.as_str(), "1.5");
        assert_eq!(
            connection.serial.flow,
            wassh_core::connection::FlowControl::RtsCts
        );
        assert_eq!(connection.style.font_size, Some(18));
        assert!(connection.x11);
        assert!(connection.remote_session.enabled);
        assert_eq!(
            connection.remote_session.multiplexer,
            wassh_core::connection::Multiplexer::Screen
        );
        assert_eq!(
            connection.tunnels[0].kind,
            wassh_core::connection::TunnelKind::Remote
        );
        let settings = settings_from_legacy(raw).unwrap();
        assert_eq!(settings.session_style_defaults.font_size, 16);
        assert_eq!(
            settings.session_style_defaults.cursor,
            wassh_core::connection::CursorStyle::Bar
        );
        assert_eq!(
            settings.tunnel_templates[0].tunnel.kind,
            wassh_core::connection::TunnelKind::Dynamic
        );
        assert_eq!(
            settings.plugin_settings["macro-pad"]["macros"][0]["group"],
            "Operations"
        );
    }

    #[test]
    fn converts_host_groups_and_tabs() {
        let hosts = hosts_from_legacy(
            serde_json::json!({
                "groups": [{"id":"g", "name":"Servers", "collapsed":true, "hostIds":["b", "a"]}],
                "ungroupedHostIds": [],
                "hosts": [
                    {"id":"a", "name":"First", "host":"a.example", "port":22},
                    {"id":"b", "name":"Second", "host":"b.example", "port":22}
                ]
            }),
            false,
        )
        .unwrap();
        let grouped: Vec<_> = hosts
            .hosts_in_group("g")
            .iter()
            .map(|host| host.id.as_str())
            .collect();
        assert_eq!(grouped, ["b", "a"]);
        let tabs = tabs_from_legacy(serde_json::json!([{
            "id":"tab", "active":true, "connection":{"hostId":"a", "host":"a.example", "port":22},
            "activePluginIds":["scratchpad"], "pluginLayout":{"left":null}
        }]), false).unwrap();
        assert_eq!(tabs[0].host_id.as_deref(), Some("a"));
        assert_eq!(tabs[0].connection.host, "a.example");
        assert_eq!(tabs[0].active_plugin_ids, ["scratchpad"]);
        assert!(tabs[0].plugin_layout.is_some());
    }
}
