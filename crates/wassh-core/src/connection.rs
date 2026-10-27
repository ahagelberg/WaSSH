//! Connection parameters.
//!
//! These are the values a saved host stores and a session runs with: transport choice,
//! authentication, tunnels, remote-session (tmux/screen) handling, serial settings and
//! per-session style overrides. Secrets are never stored here.

use serde::{Deserialize, Serialize};

use crate::session::ReconnectMode;
use crate::theme::Rgba;

/// Transport used to reach the target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionType {
    #[default]
    Ssh,
    Telnet,
    Serial,
    Local,
}

impl ConnectionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Telnet => "telnet",
            Self::Serial => "serial",
            Self::Local => "local",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Telnet => "Telnet",
            Self::Serial => "Serial",
            Self::Local => "Local terminal",
        }
    }

    pub fn is_ssh(self) -> bool {
        matches!(self, Self::Ssh)
    }

    /// Port used when the user has not chosen one.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Ssh => 22,
            Self::Telnet => 23,
            Self::Serial | Self::Local => 0,
        }
    }
}

/// How to authenticate an SSH session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthMethod {
    #[default]
    Password,
    PrivateKey,
    /// Prompt for credentials, store nothing.
    None,
}

/// Serial parity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
    Mark,
    Space,
}

/// Serial stop bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopBits {
    #[default]
    #[serde(rename = "1")]
    One,
    #[serde(rename = "1.5")]
    OnePointFive,
    #[serde(rename = "2")]
    Two,
}

impl StopBits {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::One => "1",
            Self::OnePointFive => "1.5",
            Self::Two => "2",
        }
    }
}

/// Serial flow control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowControl {
    #[default]
    None,
    RtsCts,
    XonXoff,
}

impl FlowControl {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::RtsCts => "Hardware (RTS/CTS)",
            Self::XonXoff => "Software (XON/XOFF)",
        }
    }
}

/// Serial port configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SerialConfig {
    pub path: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    pub flow: FlowControl,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            baud: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow: FlowControl::None,
        }
    }
}

impl SerialConfig {
    pub const MIN_BAUD: u32 = 300;
    pub const MAX_BAUD: u32 = 3_000_000;

    /// Clamp values into the ranges the UI allows.
    pub fn normalized(mut self) -> Self {
        self.baud = self.baud.clamp(Self::MIN_BAUD, Self::MAX_BAUD);
        self.data_bits = self.data_bits.clamp(5, 8);
        self
    }
}

/// Which direction a tunnel forwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    /// `-L`: local listener, remote destination.
    #[default]
    Local,
    /// `-R`: remote listener, local destination.
    Remote,
    /// `-D`: SOCKS5 proxy on the local machine.
    Dynamic,
}

impl TunnelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Remote => "Remote",
            Self::Dynamic => "Dynamic (SOCKS)",
        }
    }
}

/// One port-forwarding rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tunnel {
    pub id: String,
    pub enabled: bool,
    pub kind: TunnelKind,
    pub listen_host: String,
    pub listen_port: u16,
    pub dest_host: String,
    pub dest_port: u16,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            kind: TunnelKind::Local,
            listen_host: "127.0.0.1".to_owned(),
            listen_port: 0,
            dest_host: String::new(),
            dest_port: 0,
        }
    }
}

impl Tunnel {
    /// Human summary used in cards and templates.
    pub fn summary(&self) -> String {
        match self.kind {
            TunnelKind::Dynamic => format!("SOCKS5 {}:{}", self.listen_host, self.listen_port),
            TunnelKind::Local => format!(
                "L {}:{} → {}:{}",
                self.listen_host, self.listen_port, self.dest_host, self.dest_port
            ),
            TunnelKind::Remote => format!(
                "R {}:{} → {}:{}",
                self.listen_host, self.listen_port, self.dest_host, self.dest_port
            ),
        }
    }
}

/// Terminal multiplexer used for "open in remote session".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Multiplexer {
    #[default]
    Tmux,
    Screen,
}

impl Multiplexer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Screen => "GNU Screen",
        }
    }
}

/// What to do when the multiplexer session already has clients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BusyPolicy {
    #[default]
    DoNotAttach,
    Share,
    ForceDetach,
}

/// Attach-to-existing-session settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteSession {
    pub enabled: bool,
    pub multiplexer: Multiplexer,
    pub name: String,
    pub busy: BusyPolicy,
}

impl Default for RemoteSession {
    fn default() -> Self {
        Self {
            enabled: false,
            multiplexer: Multiplexer::Tmux,
            name: "WaSSH".to_owned(),
            busy: BusyPolicy::DoNotAttach,
        }
    }
}

/// How the bell character is presented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BellMode {
    #[default]
    System,
    InvertWindow,
    InvertLine,
}

impl BellMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Default system sound",
            Self::InvertWindow => "Blink whole terminal",
            Self::InvertLine => "Blink current line",
        }
    }
}

/// Cursor shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Bar,
}

impl CursorStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Block => "Block",
            Self::Underline => "Underline",
            Self::Bar => "Vertical line",
        }
    }
}

/// Per-host or per-tab style overrides. `None` means "use the application default".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStyle {
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub tab_color: Option<Rgba>,
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub term_background: Option<Rgba>,
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub term_foreground: Option<Rgba>,
    pub font_size: Option<u16>,
    pub font_family: Option<String>,
    pub bell: Option<BellMode>,
    pub cursor: Option<CursorStyle>,
    pub cursor_blink: Option<bool>,
    pub scrollback: Option<u32>,
}

impl SessionStyle {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Clamp overrides into valid ranges.
    pub fn normalized(mut self) -> Self {
        if let Some(size) = self.font_size {
            self.font_size = Some(size.clamp(
                crate::app::MIN_FONT_SIZE_PX as u16,
                crate::app::MAX_FONT_SIZE_PX as u16,
            ));
        }
        if let Some(lines) = self.scrollback {
            self.scrollback = Some(lines.clamp(
                crate::app::MIN_SCROLLBACK_LINES,
                crate::app::MAX_SCROLLBACK_LINES,
            ));
        }
        self
    }
}

/// Everything a session needs, minus secrets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionParams {
    #[serde(rename = "type")]
    pub kind: ConnectionType,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub private_key_path: Option<String>,
    pub jump_host_id: Option<String>,
    pub tunnels: Vec<Tunnel>,
    pub x11: bool,
    pub remote_session: RemoteSession,
    pub serial: SerialConfig,
    pub reconnect: ReconnectMode,
    pub style: SessionStyle,
}

impl Default for ConnectionParams {
    fn default() -> Self {
        Self {
            kind: ConnectionType::Ssh,
            host: String::new(),
            port: 0,
            username: String::new(),
            auth: AuthMethod::Password,
            private_key_path: None,
            jump_host_id: None,
            tunnels: Vec::new(),
            x11: false,
            remote_session: RemoteSession::default(),
            serial: SerialConfig::default(),
            reconnect: ReconnectMode::default(),
            style: SessionStyle::default(),
        }
    }
}

impl ConnectionParams {
    /// Fill in the protocol default port and clamp numeric fields.
    pub fn normalized(mut self) -> Self {
        if self.port == 0 {
            self.port = self.kind.default_port();
        }
        match self.kind {
            ConnectionType::Local => {
                self.host.clear();
                self.port = 0;
                self.username.clear();
                self.auth = AuthMethod::None;
            }
            ConnectionType::Serial => {
                self.port = 0;
                self.username.clear();
                self.auth = AuthMethod::None;
            }
            ConnectionType::Ssh | ConnectionType::Telnet => {}
        }
        if !self.kind.is_ssh() {
            self.tunnels.clear();
            self.x11 = false;
            self.remote_session.enabled = false;
            self.jump_host_id = None;
        }
        self.serial = self.serial.clone().normalized();
        self.style = self.style.clone().normalized();
        self.tunnels.retain(|tunnel| tunnel.listen_port != 0);
        self
    }

    /// `user@host:port` or a protocol-appropriate label.
    pub fn endpoint(&self) -> String {
        match self.kind {
            ConnectionType::Local => "local".to_owned(),
            ConnectionType::Serial => self.serial.path.clone(),
            ConnectionType::Ssh | ConnectionType::Telnet => {
                if self.username.is_empty() {
                    format!("{}:{}", self.host, self.port)
                } else {
                    format!("{}@{}:{}", self.username, self.host, self.port)
                }
            }
        }
    }

    /// Sidebar subtitle.
    pub fn subtitle(&self) -> String {
        match self.kind {
            ConnectionType::Local => "Local terminal".to_owned(),
            ConnectionType::Serial => format!(
                "{} · {} {}{}{}",
                self.serial.path,
                self.serial.baud,
                self.serial.data_bits,
                match self.serial.parity {
                    Parity::None => "N",
                    Parity::Even => "E",
                    Parity::Odd => "O",
                    Parity::Mark => "M",
                    Parity::Space => "S",
                },
                self.serial.stop_bits.as_str()
            ),
            ConnectionType::Telnet => format!("telnet {}:{}", self.host, self.port),
            ConnectionType::Ssh => self.endpoint(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_hosts_reconnect_when_focused() {
        let params = ConnectionParams::default();
        assert_eq!(params.reconnect, ReconnectMode::OnFocus);
    }

    #[test]
    fn default_ports_follow_the_protocol() {
        assert_eq!(ConnectionType::Ssh.default_port(), 22);
        assert_eq!(ConnectionType::Telnet.default_port(), 23);
        assert_eq!(ConnectionType::Serial.default_port(), 0);
        assert_eq!(ConnectionType::Local.default_port(), 0);
    }

    #[test]
    fn normalization_fills_the_port_and_clears_irrelevant_fields() {
        let params = ConnectionParams {
            host: "example.com".to_owned(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(params.port, 22);

        let local = ConnectionParams {
            kind: ConnectionType::Local,
            host: "ignored".to_owned(),
            port: 1234,
            username: "someone".to_owned(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(local.port, 0);
        assert!(local.host.is_empty());
        assert_eq!(local.auth, AuthMethod::None);

        let telnet = ConnectionParams {
            kind: ConnectionType::Telnet,
            host: "h".to_owned(),
            tunnels: vec![Tunnel::default()],
            x11: true,
            ..Default::default()
        }
        .normalized();
        assert_eq!(telnet.port, 23);
        assert!(telnet.tunnels.is_empty());
        assert!(!telnet.x11);
    }

    #[test]
    fn numeric_fields_are_clamped() {
        let params = ConnectionParams {
            serial: SerialConfig {
                baud: 10,
                data_bits: 9,
                ..SerialConfig::default()
            },
            style: SessionStyle {
                font_size: Some(200),
                scrollback: Some(5),
                ..SessionStyle::default()
            },
            ..Default::default()
        }
        .normalized();
        assert_eq!(params.serial.baud, SerialConfig::MIN_BAUD);
        assert_eq!(params.serial.data_bits, 8);
        assert_eq!(params.style.font_size, Some(48));
        assert_eq!(params.style.scrollback, Some(100));
    }

    #[test]
    fn subtitles_match_the_electron_wording() {
        let ssh = ConnectionParams {
            host: "example.com".to_owned(),
            port: 22,
            username: "alice".to_owned(),
            ..Default::default()
        };
        assert_eq!(ssh.subtitle(), "alice@example.com:22");

        let serial = ConnectionParams {
            kind: ConnectionType::Serial,
            serial: SerialConfig {
                path: "/dev/ttyUSB0".to_owned(),
                baud: 115_200,
                ..SerialConfig::default()
            },
            ..Default::default()
        };
        assert_eq!(serial.subtitle(), "/dev/ttyUSB0 · 115200 8N1");

        assert_eq!(
            ConnectionParams {
                kind: ConnectionType::Local,
                ..Default::default()
            }
            .subtitle(),
            "Local terminal"
        );
    }

    #[test]
    fn tunnel_summaries_are_readable() {
        let local = Tunnel {
            kind: TunnelKind::Local,
            listen_port: 8080,
            dest_host: "db".to_owned(),
            dest_port: 5432,
            ..Default::default()
        };
        assert_eq!(local.summary(), "L 127.0.0.1:8080 → db:5432");
        let socks = Tunnel {
            kind: TunnelKind::Dynamic,
            listen_port: 1080,
            ..Default::default()
        };
        assert_eq!(socks.summary(), "SOCKS5 127.0.0.1:1080");
    }

    #[test]
    fn json_uses_camel_case_and_keeps_type() {
        let params = ConnectionParams {
            host: "h".to_owned(),
            x11: true,
            tunnels: vec![Tunnel {
                id: "t1".to_owned(),
                enabled: true,
                kind: TunnelKind::Remote,
                listen_host: "0.0.0.0".to_owned(),
                listen_port: 9000,
                dest_host: "localhost".to_owned(),
                dest_port: 3000,
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"type\":\"ssh\""));
        assert!(json.contains("\"privateKeyPath\":null"));
        assert!(json.contains("\"listenPort\":9000"));
        let back: ConnectionParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, params);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let params: ConnectionParams = serde_json::from_str("{\"type\":\"telnet\"}").unwrap();
        assert_eq!(params.kind, ConnectionType::Telnet);
        assert_eq!(params.serial.baud, 115_200);
        assert!(!params.x11);
    }
}
