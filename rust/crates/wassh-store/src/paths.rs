//! Where configuration lives.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::StoreError;

/// Product folder inside the platform's application-data root.
///
/// Deliberately *not* `WaSSH`: the previous generation of the app uses that folder with a
/// different file schema, and both writing to it would corrupt each other.
pub const DATA_DIR_NAME: &str = "wassh-native";

pub const SETTINGS_FILE: &str = "settings.json";
pub const HOSTS_FILE: &str = "hosts.json";
pub const TABS_FILE: &str = "tabs.json";
pub const KNOWN_HOSTS_FILE: &str = "known_hosts.json";

/// Settings document path inside the data directory.
pub fn settings_file(data_dir: impl AsRef<std::path::Path>) -> PathBuf {
    data_dir.as_ref().join(SETTINGS_FILE)
}

pub fn hosts_file(data_dir: impl AsRef<std::path::Path>) -> PathBuf {
    data_dir.as_ref().join(HOSTS_FILE)
}

pub fn tabs_file(data_dir: impl AsRef<std::path::Path>) -> PathBuf {
    data_dir.as_ref().join(TABS_FILE)
}

pub fn known_hosts_file(data_dir: impl AsRef<std::path::Path>) -> PathBuf {
    data_dir.as_ref().join(KNOWN_HOSTS_FILE)
}

/// `plugin-<id>.json`, or `plugin-<id>.<scope>.json` when a scope is supplied.
///
/// The id is sanitised: anything outside `[A-Za-z0-9._-]` becomes `_`, so a plugin cannot
/// escape the data directory.
pub fn plugin_data_file(
    data_dir: impl AsRef<std::path::Path>,
    plugin_id: &str,
    scope: Option<&str>,
) -> PathBuf {
    let mut name = format!("plugin-{}", sanitize(plugin_id));
    if let Some(scope) = scope {
        name.push('.');
        name.push_str(&sanitize(scope));
    }
    name.push_str(".json");
    data_dir.as_ref().join(name)
}

fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "_".to_owned()
    } else {
        cleaned
    }
}

/// Platform application-data root, without the product folder.
///
/// - Linux/BSD: `$XDG_CONFIG_HOME`, else `$HOME/.config`
/// - Windows: `%APPDATA%`, else `%USERPROFILE%\AppData\Roaming`
/// - macOS: `$HOME/Library/Application Support`
pub fn platform_data_root_with(
    get: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = get("APPDATA").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(appdata));
        }
        let home = home_dir(&get)?;
        Some(home.join("AppData").join("Roaming"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = home_dir(&get)?;
        Some(home.join("Library").join("Application Support"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(xdg));
        }
        let home = home_dir(&get)?;
        Some(home.join(".config"))
    }
}

fn home_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    get("HOME")
        .or_else(|| get("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Application data directory for this process.
pub fn data_dir() -> Result<PathBuf, StoreError> {
    data_dir_with(|key| std::env::var_os(key))
}

/// Application data directory for an injectable environment (used by tests).
pub fn data_dir_with(get: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, StoreError> {
    match platform_data_root_with(get) {
        Some(root) => Ok(root.join(DATA_DIR_NAME)),
        None => Err(StoreError::NoDataDir(
            "neither XDG_CONFIG_HOME/HOME (unix), APPDATA/USERPROFILE (windows) \
             nor HOME (macos) is set",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn data_dir_uses_product_folder() {
        let dir = data_dir_with(env_of(&[("HOME", "/home/tester")])).unwrap();
        assert!(dir.ends_with(DATA_DIR_NAME));
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(dir, PathBuf::from("/home/tester/.config/wassh-native"));
    }

    #[test]
    fn missing_environment_is_an_error_not_a_panic() {
        let err = data_dir_with(env_of(&[])).unwrap_err();
        assert!(matches!(err, StoreError::NoDataDir(_)));
    }

    #[test]
    fn plugin_files_are_sanitised_and_scoped() {
        let dir = PathBuf::from("/data");
        assert_eq!(
            plugin_data_file(&dir, "sftp", None),
            PathBuf::from("/data/plugin-sftp.json")
        );
        assert_eq!(
            plugin_data_file(&dir, "ai-agent", Some("tab:abc")),
            PathBuf::from("/data/plugin-ai-agent.tab_abc.json")
        );
        assert_eq!(
            plugin_data_file(&dir, "../../etc/passwd", None),
            PathBuf::from("/data/plugin-.._.._etc_passwd.json")
        );
    }

    #[test]
    fn named_documents_live_in_the_data_dir() {
        let dir = PathBuf::from("/data");
        assert_eq!(settings_file(&dir), dir.join("settings.json"));
        assert_eq!(hosts_file(&dir), dir.join("hosts.json"));
        assert_eq!(tabs_file(&dir), dir.join("tabs.json"));
        assert_eq!(known_hosts_file(&dir), dir.join("known_hosts.json"));
    }
}
