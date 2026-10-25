//! Built-in plugins.
//!
//! The eight plugins compile into the binary behind a Rust trait; there is no dynamic
//! loading. Manifests carry the ids, names and toolbar labels used by settings and stored
//! plugin data.

#![forbid(unsafe_code)]

pub mod ai_agent;
pub mod color;
pub mod command;
pub mod connection_logger;
pub mod docker;
pub mod files;
pub mod macro_pad;
pub mod monitor;
pub mod mqtt;
pub mod plugin;
pub mod settings;
pub mod time;

pub use command::{CommandAccess, CommandOutput};
pub use connection_logger::ConnectionLogger;
pub use docker::DockerManager;
pub use files::{Files, SftpAccess, SftpEntry, SftpListing};
pub use macro_pad::MacroPad;
pub use monitor::{MonitorAccess, ServerMonitor, Snapshot};
pub use mqtt::MqttAnalyser;
pub use command::TcpAccess;
pub use plugin::{HeaderButton, Plugin, PluginContext, SessionAccess, TerminalAccess, create};
pub use settings::{FieldKind, SettingsField};

use serde::{Deserialize, Serialize};
use wassh_core::DockPlacement;

/// Stable plugin identifier (also the `plugin-<id>.json` file stem).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginId {
    AiAgent,
    Sftp,
    ServerMonitor,
    DockerManager,
    MqttAnalyser,
    MacroPad,
    ConnectionLogger,
    Scratchpad,
}

impl PluginId {
    pub const ALL: [Self; 8] = [
        Self::AiAgent,
        Self::Sftp,
        Self::ServerMonitor,
        Self::DockerManager,
        Self::MqttAnalyser,
        Self::MacroPad,
        Self::ConnectionLogger,
        Self::Scratchpad,
    ];

    /// The order the original app registers its plugins in
    /// (`BUILTIN_PLUGIN_DEFINITIONS`), which is the order of the plugin list in
    /// Options, the plugin sections, the toolbar and the Plugins menu.
    pub const UI_ORDER: [Self; 8] = [
        Self::ServerMonitor,
        Self::Scratchpad,
        Self::MacroPad,
        Self::MqttAnalyser,
        Self::Sftp,
        Self::AiAgent,
        Self::ConnectionLogger,
        Self::DockerManager,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AiAgent => "ai-agent",
            Self::Sftp => "sftp",
            Self::ServerMonitor => "server-monitor",
            Self::DockerManager => "docker-manager",
            Self::MqttAnalyser => "mqtt-analyser",
            Self::MacroPad => "macro-pad",
            Self::ConnectionLogger => "connection-logger",
            Self::Scratchpad => "scratchpad",
        }
    }

    /// Parse a stored plugin id, accepting the kebab-case form and the dotted
    /// variant older settings used.
    pub fn parse(text: &str) -> Option<Self> {
        let normalised: String = text
            .chars()
            .map(|c| match c {
                '_' | '.' | ' ' => '-',
                other => other.to_ascii_lowercase(),
            })
            .collect();
        let trimmed = normalised.strip_prefix("wassh-").unwrap_or(&normalised);
        Self::ALL
            .into_iter()
            .find(|id| id.as_str() == trimmed || id.name_key() == trimmed)
    }

    fn name_key(self) -> &'static str {
        match self {
            Self::AiAgent => "aiagent",
            Self::Sftp => "sftp",
            Self::ServerMonitor => "servermonitor",
            Self::DockerManager => "dockermanager",
            Self::MqttAnalyser => "mqttanalyser",
            Self::MacroPad => "macropad",
            Self::ConnectionLogger => "connectionlogger",
            Self::Scratchpad => "scratchpad",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::AiAgent => {
                "Chat with an LLM that can run commands on the remote host. Commands outside \
                 the safe/allow lists ask for approval."
            }
            Self::Sftp => "Browse and manage files for SSH and Local sessions.",
            Self::ServerMonitor => {
                "System stats with history: per-core CPU, memory, disk, network, and processes \
                 for local and SSH sessions."
            }
            Self::DockerManager => {
                "Manage containers, images, volumes, and networks on the session host."
            }
            Self::MqttAnalyser => "Browse topics and messages on an MQTT broker on the SSH host.",
            Self::MacroPad => {
                "Configurable buttons and hotkeys that inject text into the terminal."
            }
            Self::ConnectionLogger => {
                "Logs connection and disconnection events to analyse network stability."
            }
            Self::Scratchpad => "Notes shared across sessions of the same host.",
        }
    }

    pub fn manifest(self) -> PluginManifest {
        let (name, toolbar, title) = match self {
            Self::AiAgent => ("AI agent", "AI Agent", "AI agent"),
            Self::Sftp => ("Files", "Files", "Files"),
            Self::ServerMonitor => ("Server monitor", "Monitor", "Server"),
            Self::DockerManager => ("Docker", "Docker", "Docker"),
            Self::MqttAnalyser => ("MQTT Analyser", "MQTT", "MQTT"),
            Self::MacroPad => ("Macro pad", "Macros", "Macros"),
            Self::ConnectionLogger => ("Connection logger", "Conn Log", "Connection Log"),
            Self::Scratchpad => ("Scratchpad", "Scratch", "Scratchpad"),
        };
        PluginManifest {
            id: self,
            name,
            description: self.description(),
            toolbar_label: toolbar,
            view_title: title,
            placement: match self {
                Self::ConnectionLogger => DockPlacement::SplitBottom,
                _ => DockPlacement::SplitRight,
            },
            activation: match self {
                Self::Sftp => Activation::OnSshConnect,
                Self::ConnectionLogger => Activation::OnSession,
                _ => Activation::Manual,
            },
        }
    }
}

/// When the host may start a plugin without the user opening its panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    Manual,
    /// Started headlessly for every SSH session (SFTP drag-and-drop uploads).
    OnSshConnect,
    /// Started for every session (connection logger).
    OnSession,
}

/// Data the host needs to show and place a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: &'static str,
    /// One-line summary shown under the name in Options.
    pub description: &'static str,
    pub toolbar_label: &'static str,
    pub view_title: &'static str,
    pub placement: DockPlacement,
    pub activation: Activation,
}

/// Toolbar ordering: every plugin, in the registry order of the original app.
pub fn toolbar_plugins() -> impl Iterator<Item = PluginManifest> {
    PluginId::UI_ORDER.into_iter().map(PluginId::manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_plugin_has_a_unique_id_and_name() {
        let manifests: Vec<_> = PluginId::ALL.into_iter().map(PluginId::manifest).collect();
        assert_eq!(manifests.len(), 8);
        let mut ids: Vec<_> = manifests.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8);
        let mut names: Vec<_> = manifests.iter().map(|m| m.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn ids_match_their_stored_strings() {
        assert_eq!(PluginId::AiAgent.as_str(), "ai-agent");
        assert_eq!(PluginId::MqttAnalyser.as_str(), "mqtt-analyser");
        assert_eq!(PluginId::ConnectionLogger.as_str(), "connection-logger");
    }

    #[test]
    fn placements_and_activation_match_the_registry() {
        assert_eq!(
            PluginId::ConnectionLogger.manifest().placement,
            DockPlacement::SplitBottom
        );
        assert_eq!(PluginId::Sftp.manifest().activation, Activation::OnSshConnect);
        assert_eq!(
            PluginId::ConnectionLogger.manifest().activation,
            Activation::OnSession
        );
        assert_eq!(PluginId::Scratchpad.manifest().activation, Activation::Manual);
    }

    #[test]
    fn toolbar_labels_are_stable() {
        let labels: Vec<_> = toolbar_plugins().map(|m| m.toolbar_label).collect();
        assert!(labels.contains(&"Conn Log"));
        assert!(labels.contains(&"Scratch"));
    }
}
