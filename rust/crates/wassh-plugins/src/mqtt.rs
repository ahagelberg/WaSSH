//! The MQTT Analyser plugin: browse the topics of a broker on the SSH host.
//!
//! Port of `src/plugins/builtins/mqtt-analyser/`. The broker is configured per
//! host (Host settings → MQTT) and reached **through the session**, exactly like
//! the reference's `ctx.openTcpStream`: the pane asks the session for a loopback
//! forward and dials that, so no local broker is ever contacted. The pane itself
//! shows no connection fields — it connects as soon as it opens.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use egui::{Align, Align2, Color32, FontId, Layout, Rect, Sense, Stroke, Ui, Vec2};
use serde_json::{json, Value};
use wassh_core::{Palette, SessionStatus};
use wassh_mqtt::{MqttEvent, MqttOptions, MqttSession};
use wassh_ui::dock::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, MenuItem, MenuState, checkbox, select, text_area, text_input,
};

use crate::color::to_color32;
use crate::plugin::{HeaderButton, Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Host setting: broker address **as the remote host sees it**.
pub const HOST_SETTING: &str = "host";
/// Host setting: broker port.
pub const PORT_SETTING: &str = "port";
/// Host setting: MQTT user name.
pub const USERNAME_SETTING: &str = "username";
/// Host setting: MQTT password.
pub const PASSWORD_SETTING: &str = "password";
/// Plugin data: saved publish messages.
pub const SAVED_KEY: &str = "messages";
/// Plugin data: whether the saved-messages section is collapsed.
pub const COLLAPSED_KEY: &str = "messagesCollapsed";

/// Default broker address, as in the reference.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Default plain MQTT port.
pub const DEFAULT_PORT: u16 = 1883;
/// The single subscription the reference makes.
pub const SUBSCRIBE_TOPIC: &str = "#";
/// Messages kept per topic.
pub const HISTORY_LIMIT: usize = 100;
/// Topics kept in the tree, newest-touched last.
pub const MAX_TOPICS: usize = 500;
/// Key the broker's loopback forward is cached under.
pub const FORWARD_KEY: &str = "mqtt.broker";
/// How often a connected pane repaints.
pub const REPAINT_MS: u64 = 200;
/// Label of a saved message that has none.
pub const NEW_MESSAGE_LABEL: &str = "New message";

// --- layout metrics, all taken from the reference's `styles.css` -----------

/// `var(--gap)`.
const PANE_GAP: f32 = 8.0;
/// `--plugin-splitter-size`: the divider between Topics and Details.
const SPLITTER_WIDTH: f32 = 4.0;
/// `--mqtt-pane-min-width`.
const PANE_MIN_WIDTH: f32 = 100.0;
/// `--mqtt-history-max-height`.
const HISTORY_MAX_HEIGHT: f32 = 180.0;
/// `.mqtt-history-payload { max-height }`.
const PAYLOAD_MAX_HEIGHT: f32 = 120.0;
/// `.mqtt-pane-label`: 6 px + a 12 px line + 4 px.
const PANE_LABEL_HEIGHT: f32 = 22.0;
/// `.mqtt-tree-row`: 3 px + an 11 px mono line + 3 px. Rows sit flush — the
/// tree list has no margin of its own, which is what keeps topics tight.
const TOPIC_ROW_HEIGHT: f32 = 20.0;
/// `--mqtt-tree-indent`.
const TOPIC_INDENT: f32 = 14.0;
/// `.mqtt-tree-twist { width }`.
const TOPIC_TWIST_WIDTH: f32 = 14.0;
/// `.mqtt-tree-row { padding: 3px 6px }`, horizontal part.
const TOPIC_ROW_PAD: f32 = 6.0;
/// `.mqtt-tree-row { gap: 4px }`.
const TOPIC_ROW_GAP: f32 = 4.0;
/// `.mqtt-tree { padding: 0 4px 8px }`.
const TREE_PAD_X: f32 = 4.0;
/// `.mqtt-history { gap: 6px }`.
const HISTORY_GAP: f32 = 6.0;
/// `.mqtt-history-item` horizontal padding.
const HISTORY_ITEM_PAD: f32 = 8.0;
/// `.mqtt-msg-row { flex: 1 1 160px }`.
const SAVED_CARD_MIN: f32 = 160.0;
/// `.mqtt-msg-list { gap: 4px }`.
const SAVED_CARD_GAP: f32 = 4.0;
/// `.mqtt-msg-row`: 4 px padding + a 12 px and an 11 px line.
const SAVED_CARD_HEIGHT: f32 = 34.0;
/// `.mqtt-copy-btn { width/height: 18px }`.
const COPY_BUTTON: f32 = 18.0;
/// Height of a `.mqtt-field` caption (`font-size: 11px`).
const FIELD_LABEL_HEIGHT: f32 = 13.0;
/// Height of `.mqtt-field input` (4 px padding + a 12 px mono line + border).
const INPUT_HEIGHT: f32 = 24.0;
/// `.mqtt-field textarea { min-height: 48px }` plus padding and border.
const TEXT_AREA_HEIGHT: f32 = 58.0;
/// Height of a row of compact buttons.
const BUTTON_ROW_HEIGHT: f32 = 22.0;
/// `.mqtt-msg-head { min-height: 20px }`.
const SAVED_HEAD_HEIGHT: f32 = 20.0;
/// Height of the "Add message" button.
const ADD_BUTTON_HEIGHT: f32 = 26.0;
/// `MQTT_ANALYSER_BLINK_MS`: how long a topic stays highlighted.
const BLINK_MS: u64 = 500;
/// `COPY_FEEDBACK_MS`: how long a copy button confirms.
const COPY_FEEDBACK_MS: u64 = 1500;
/// Copy button glyphs, as in the reference.
const COPY_GLYPH: &str = "\u{29c9}";
const COPIED_GLYPH: &str = "\u{2713}";
/// `MQTT_SPLIT_DEFAULT_RATIO` / `_MIN_RATIO` / `_MAX_RATIO`.
const SPLIT_DEFAULT_RATIO: f32 = 0.42;
const SPLIT_MIN_RATIO: f32 = 0.18;
const SPLIT_MAX_RATIO: f32 = 0.75;
/// `.mqtt-msg-dialog { width: min(420px, 90vw) }`.
const MESSAGE_DIALOG_WIDTH: f32 = 420.0;
/// How many base64 characters a binary payload preview shows.
const BINARY_PREVIEW_CHARS: usize = 120;
/// Width the Delete button needs in the details header.
const DELETE_BUTTON_WIDTH: f32 = 54.0;

/// Connection state, mirroring the reference's status payload.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Status {
    /// Nothing asked yet.
    #[default]
    Idle,
    /// Asking for the forward or waiting for CONNACK.
    Connecting,
    /// CONNACK accepted and `#` subscribed.
    Connected,
    /// The broker went away.
    Disconnected(String),
    /// No session to reach a broker through.
    Unavailable(String),
    /// A failure with its kind.
    Failed(ErrorKind, String),
}

/// Why a connection attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Not an SSH session (local/telnet/serial).
    NotSsh,
    /// Nothing is listening on the configured host and port.
    NoBroker,
    /// The broker refused the credentials.
    AuthFailed,
    /// The host could not be reached from the session.
    Unreachable,
    /// Anything else.
    Other,
}

impl Status {
    /// The line the status bar shows, verbatim from the reference.
    pub fn label(&self) -> String {
        match self {
            Status::Idle => "Idle".to_owned(),
            Status::Connecting => "Connecting…".to_owned(),
            Status::Connected => "Connected".to_owned(),
            Status::Disconnected(reason) if reason.is_empty() => "Disconnected".to_owned(),
            Status::Disconnected(reason) => format!("Disconnected — {reason}"),
            Status::Unavailable(reason) if reason.is_empty() => "SSH only".to_owned(),
            Status::Unavailable(reason) => reason.clone(),
            Status::Failed(kind, reason) => {
                if !reason.is_empty() {
                    return reason.clone();
                }
                match kind {
                    ErrorKind::NotSsh => "SSH only".to_owned(),
                    ErrorKind::NoBroker => "No MQTT broker".to_owned(),
                    ErrorKind::AuthFailed => "Authentication failed".to_owned(),
                    ErrorKind::Unreachable => "Unreachable".to_owned(),
                    ErrorKind::Other => "Error".to_owned(),
                }
            }
        }
    }

    /// Whether the dot reads as healthy.
    pub fn health(&self) -> HeaderHealth {
        match self {
            Status::Connected => HeaderHealth::Ok,
            Status::Connecting | Status::Idle => HeaderHealth::Warn,
            Status::Disconnected(_) | Status::Unavailable(_) | Status::Failed(..) => {
                HeaderHealth::Bad
            }
        }
    }
}

/// Classify a connection error the way the reference does.
pub fn classify(error: &str) -> ErrorKind {
    let lower = error.to_lowercase();
    if lower.contains("connection refused") || lower.contains("econnrefused") {
        ErrorKind::NoBroker
    } else if lower.contains("not authorized")
        || lower.contains("bad user name")
        || lower.contains("bad username")
        || lower.contains("not authorised")
        || lower.contains("code 4")
        || lower.contains("code 5")
    {
        ErrorKind::AuthFailed
    } else if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("unreachable")
        || lower.contains("no route")
    {
        ErrorKind::Unreachable
    } else if lower.contains("needs an ssh session") {
        ErrorKind::NotSsh
    } else {
        ErrorKind::Other
    }
}

/// Broker settings as read from Host settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broker {
    /// Address the remote host dials.
    pub host: String,
    /// Its port.
    pub port: u16,
    /// Optional user name.
    pub username: String,
    /// Optional password.
    pub password: String,
}

impl Default for Broker {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            username: String::new(),
            password: String::new(),
        }
    }
}

/// Read the broker out of the merged plugin settings.
///
/// The password is not a setting: it is sealed in the credential vault, so the
/// pane passes the decrypted value here and the settings map never holds it.
pub fn broker_from_settings(
    settings: &serde_json::Map<String, Value>,
    password: Option<&str>,
) -> Broker {
    let text = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let port = match settings.get(PORT_SETTING) {
        Some(Value::Number(number)) => number.as_u64().unwrap_or(0),
        Some(Value::String(raw)) => raw.trim().parse().unwrap_or(0),
        _ => 0,
    };
    let port = if (1..=65_535).contains(&port) {
        port as u16
    } else {
        DEFAULT_PORT
    };
    let host = text(HOST_SETTING);
    Broker {
        host: if host.is_empty() {
            DEFAULT_HOST.to_owned()
        } else {
            host
        },
        port,
        username: text(USERNAME_SETTING),
        password: password.unwrap_or_default().trim().to_owned(),
    }
}

/// One received message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// When it arrived, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Topic it arrived on.
    pub topic: String,
    /// Payload as text (lossy for a binary payload).
    pub payload: String,
    /// Base64 of the raw payload, kept only for a binary message.
    pub base64: String,
    /// Delivery quality.
    pub qos: u8,
    /// Whether the broker marked it retained.
    pub retain: bool,
    /// Whether the payload had to be escaped because it is not text.
    pub binary: bool,
    /// What the details pane shows: [`format_payload`] of this message.
    ///
    /// Computed once, when the message arrives. Formatting is a JSON parse and
    /// a re-serialisation, which must not run again for every frame the history
    /// is on screen.
    pub display: String,
    /// Characters in [`Self::display`], cached so a card can be sized without
    /// walking its text.
    pub display_chars: usize,
    /// Hard line breaks in [`Self::display`].
    pub display_lines: usize,
    /// The start of [`Self::display`], cut to [`PREVIEW_CHARS`].
    ///
    /// A card shows a couple of hundred characters at most, so this is what the
    /// text layout gets: handing egui a megabyte of JSON to lay out nine rows of
    /// it would clone and hash that megabyte on every frame.
    pub preview: String,
}

impl Message {
    /// Record one payload as it came off the wire.
    pub fn new(topic: String, payload: &[u8], at_ms: u64, qos: u8, retain: bool) -> Self {
        let binary = !is_text(payload);
        let mut message = Self {
            at_ms,
            topic,
            payload: String::from_utf8_lossy(payload).into_owned(),
            base64: if binary {
                base64_encode(payload)
            } else {
                String::new()
            },
            qos,
            retain,
            binary,
            display: String::new(),
            display_chars: 0,
            display_lines: 0,
            preview: String::new(),
        };
        message.display = format_payload(&message);
        message.display_chars = message.display.chars().count();
        message.display_lines = message.display.lines().count().max(1);
        message.preview = cut_preview(&message.display);
        message
    }
}

/// Everything known about one topic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicEntry {
    /// How many messages arrived on it.
    pub received: u64,
    /// Its most recent messages, oldest first.
    pub history: VecDeque<Message>,
}

/// One row of the topic tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeLine {
    /// Full topic path of this node.
    pub path: String,
    /// The segment shown.
    pub label: String,
    /// Nesting depth.
    pub depth: u16,
    /// Whether it has children.
    pub expandable: bool,
    /// Whether it is expanded.
    pub expanded: bool,
    /// Messages seen at or under it.
    pub count: u64,
    /// Messages seen on this exact topic (what the row's badge shows).
    pub received: u64,
    /// Topics below it.
    pub sub_topics: u64,
    /// The row's trailing badge, formatted once per rebuild.
    pub meta: Option<String>,
}

/// Running totals for one tree node, gathered in a single pass.
#[derive(Default)]
struct NodeTotals {
    /// Messages heard at or under the node.
    under: u64,
    /// Messages heard at exactly the node.
    own: u64,
    /// Descendant nodes that heard something.
    sub_topics: u64,
    /// Whether any node hangs below this one.
    has_children: bool,
}

/// Parent path of `path`, if it has one.
fn parent_of(path: &str) -> Option<&str> {
    path.rfind('/').map(|slash| &path[..slash])
}

/// Flatten the topic tree into rows, honouring which nodes are open.
///
/// One pass over the topics creates every node and rolls the counts up its
/// parent chain, so this is linear in the number of topic segments — never a
/// scan of the whole map per row. The pane caches the result and rebuilds it
/// only when a topic arrives or a branch is toggled.
pub fn tree_lines(
    topics: &BTreeMap<String, TopicEntry>,
    expanded: &BTreeSet<String>,
) -> Vec<TreeLine> {
    let mut totals: BTreeMap<&str, NodeTotals> = BTreeMap::new();
    for topic in topics.keys() {
        totals.entry(topic.as_str()).or_default();
        let mut node = topic.as_str();
        while let Some(parent) = parent_of(node) {
            totals.entry(parent).or_default().has_children = true;
            node = parent;
        }
    }
    for (topic, entry) in topics {
        let heard = entry.received;
        let node = totals
            .get_mut(topic.as_str())
            .expect("every topic was registered above");
        node.own += heard;
        node.under += heard;
        if heard == 0 {
            continue;
        }
        let mut node = topic.as_str();
        while let Some(parent) = parent_of(node) {
            let parent = totals.get_mut(parent).expect("every parent was registered");
            parent.under += heard;
            parent.sub_topics += 1;
            node = parent_of(node).expect("still has a parent");
        }
    }

    // A row shows when every ancestor is open: walk up and stop at the first
    // closed one, without building the ancestor path again.
    let visible = |path: &str| {
        let mut node = path;
        while let Some(parent) = parent_of(node) {
            if !expanded.contains(parent) {
                return false;
            }
            node = parent;
        }
        true
    };

    let mut lines = Vec::with_capacity(totals.len());
    for (path, total) in &totals {
        if !visible(path) {
            continue;
        }
        let expandable = total.has_children;
        let open = expanded.contains(*path);
        lines.push(TreeLine {
            label: path.rsplit('/').next().unwrap_or(path).to_owned(),
            depth: path.matches('/').count() as u16,
            expandable,
            expanded: open,
            count: total.under,
            received: total.own,
            sub_topics: total.sub_topics,
            meta: row_meta(expandable && !open, total.sub_topics, total.under, total.own),
            path: (*path).to_owned(),
        });
    }
    lines
}

/// Whether `payload` is text a terminal would show.
pub fn is_text(payload: &[u8]) -> bool {
    payload
        .iter()
        .all(|byte| *byte == b'\n' || *byte == b'\r' || *byte == b'\t' || (0x20..0x7f).contains(byte) || *byte >= 0x80)
}

/// Keep a message list bounded.
pub fn push_history(entry: &mut TopicEntry, message: Message) {
    entry.received += 1;
    entry.history.push_back(message);
    while entry.history.len() > HISTORY_LIMIT {
        entry.history.pop_front();
    }
}

/// A saved publish message, as the reference stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedMessage {
    /// Stable id.
    pub id: String,
    /// Button label.
    pub label: String,
    /// Topic to publish to.
    pub topic: String,
    /// Payload text.
    pub payload: String,
    /// `text` or `json`.
    pub mode: SavedMode,
}

/// Payload format of a saved message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SavedMode {
    /// Sent as typed.
    #[default]
    Text,
    /// Validated and re-indented as JSON.
    Json,
}

impl SavedMode {
    /// How the mode chip reads.
    pub fn label(self) -> &'static str {
        match self {
            SavedMode::Text => "Text",
            SavedMode::Json => "JSON",
        }
    }

    /// Storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SavedMode::Text => "text",
            SavedMode::Json => "json",
        }
    }

    /// Parse the storage spelling.
    pub fn parse(raw: &str) -> Self {
        if raw == "json" {
            SavedMode::Json
        } else {
            SavedMode::Text
        }
    }
}

/// How the composer encodes what it publishes, the reference's `PublishMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublishMode {
    /// Send the text as typed.
    #[default]
    Text,
    /// Validate the JSON and send it compacted.
    Json,
    /// Send the bytes of a chosen file.
    RawFile,
}

impl PublishMode {
    /// Chip label.
    pub fn label(self) -> &'static str {
        match self {
            PublishMode::Text => "Text",
            PublishMode::Json => "JSON",
            PublishMode::RawFile => "Raw file",
        }
    }
}

impl From<SavedMode> for PublishMode {
    fn from(mode: SavedMode) -> Self {
        match mode {
            SavedMode::Text => PublishMode::Text,
            SavedMode::Json => PublishMode::Json,
        }
    }
}

/// The bytes a text publish sends, exactly like the reference's
/// `encodeTextPayload`: JSON is validated and sent compacted.
pub fn encode_payload(text: &str, mode: PublishMode) -> Result<Vec<u8>, String> {
    match mode {
        PublishMode::Text => Ok(text.as_bytes().to_vec()),
        PublishMode::Json => {
            let value: Value = serde_json::from_str(text)
                .map_err(|error| format!("Invalid JSON \u{2014} {error}"))?;
            let compact = serde_json::to_string(&value).map_err(|error| error.to_string())?;
            Ok(compact.into_bytes())
        }
        PublishMode::RawFile => Err("Choose a file".to_owned()),
    }
}

/// Read saved messages from plugin data.
pub fn saved_from_json(value: Option<&Value>) -> Vec<SavedMessage> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| SavedMessage {
            id: item["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("msg-{}", index + 1)),
            label: item["label"].as_str().unwrap_or_default().to_owned(),
            topic: item["topic"].as_str().unwrap_or_default().to_owned(),
            payload: item["payload"].as_str().unwrap_or_default().to_owned(),
            mode: SavedMode::parse(item["mode"].as_str().unwrap_or("text")),
        })
        .collect()
}

/// Write saved messages back to plugin data.
pub fn saved_to_json(messages: &[SavedMessage]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|message| {
                json!({
                    "id": message.id,
                    "label": message.label,
                    "topic": message.topic,
                    "payload": message.payload,
                    "mode": message.mode.as_str()
                })
            })
            .collect(),
    )
}

/// The payload actually sent for a mode: JSON is validated and re-indented.
pub fn payload_for_mode(payload: &str, mode: SavedMode) -> Result<String, String> {
    match mode {
        SavedMode::Text => Ok(payload.to_owned()),
        SavedMode::Json => {
            let value: Value = serde_json::from_str(payload)
                .map_err(|error| format!("Invalid JSON: {error}"))?;
            serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
        }
    }
}

/// Truncate a payload for a one-line preview.
pub fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|character| if character == '\n' || character == '\r' { ' ' } else { character })
        .collect();
    let trimmed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.chars().count() > 200 {
        let kept: String = trimmed.chars().take(200).collect();
        format!("{kept}…")
    } else {
        trimmed
    }
}

/// Standard base64, used for the binary preview the reference shows.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// What the details pane shows for one message: the reference's `formatPayload`.
///
/// A binary payload becomes a base64 preview, text is re-indented when it is
/// JSON and shown as received otherwise.
pub fn format_payload(message: &Message) -> String {
    if message.binary {
        let mut preview: String = message
            .base64
            .chars()
            .take(BINARY_PREVIEW_CHARS)
            .collect();
        if message.base64.chars().count() > BINARY_PREVIEW_CHARS {
            preview.push('\u{2026}');
        }
        return format!("(binary, base64) {preview}");
    }
    match serde_json::from_str::<Value>(&message.payload) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_else(|_| message.payload.clone()),
        Err(_) => message.payload.clone(),
    }
}

/// How much of a payload a history card renders.
///
/// The reference has no cap at all: it hands the whole payload to a `<pre>` and
/// lets CSS scroll it. Laying out an unbounded blob is what locked the UI, so a
/// card renders the first 4 KB and marks the rest with an ellipsis. The copy
/// button always copies the complete payload.
const PREVIEW_CHARS: usize = 4_096;

/// The beginning of `text`, with an ellipsis when it was cut.
pub fn cut_preview(text: &str) -> String {
    if text.len() <= PREVIEW_CHARS {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(PREVIEW_CHARS).collect();
    out.push('\u{2026}');
    out
}

/// Time of day in the local zone, like the reference's `toLocaleTimeString`.
pub fn clock_time(at_ms: u64) -> String {
    let seconds = at_ms / 1000;
    let offset = wassh_core::time::local_offset_seconds(seconds);
    let shifted = ((seconds as i64 + offset).max(0) as u64) % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        shifted / 3600,
        (shifted % 3600) / 60,
        shifted % 60
    )
}

/// The MQTT Analyser pane.
#[derive(Default)]
pub struct MqttAnalyser {
    /// Broker settings, read every frame from the merged settings.
    broker: Broker,
    /// The live client, once connected.
    session: Option<MqttSession>,
    /// Connection state.
    status: Status,
    /// Every topic heard, with its history.
    topics: BTreeMap<String, TopicEntry>,
    /// Which tree nodes are open.
    expanded: BTreeSet<String>,
    /// Selected topic path.
    selected: Option<String>,
    /// Publish topic.
    publish_topic: String,
    /// Publish payload.
    publish_payload: String,
    /// How the payload is encoded.
    publish_mode: PublishMode,
    /// Publish QoS.
    publish_qos: u64,
    /// Publish retain flag.
    publish_retain: bool,
    /// Last publish problem.
    publish_error: Option<String>,
    /// Name of the file chosen for a raw publish.
    file_name: Option<String>,
    /// Bytes of that file.
    file_bytes: Option<Vec<u8>>,
    /// The native file dialog, while it is open.
    picking: Option<PickedFile>,
    /// Saved messages.
    saved: Vec<SavedMessage>,
    /// Whether the saved section is collapsed.
    collapsed: bool,
    /// The saved message being edited, if any.
    editor: Option<SavedMessage>,
    /// Whether the editor is adding a message rather than editing one.
    editor_is_new: bool,
    /// Fraction of the width the topic list takes; dragged with the splitter.
    tree_ratio: f32,
    /// Flattened tree rows, rebuilt only when the tree changes.
    lines: Vec<TreeLine>,
    /// Whether [`Self::lines`] is stale.
    tree_dirty: bool,
    /// Whether the saved messages or their section state changed.
    data_dirty: bool,
    /// Topics that blinked when a message arrived: path to when it started.
    blink: BTreeMap<String, u64>,
    /// The copy button that confirmed recently, and when.
    copied: Option<(String, u64)>,
    /// The open context menu.
    menu: MenuState,
    /// What that menu belongs to.
    menu_target: Option<MenuTarget>,
    /// A topic the keyboard moved to, scrolled into view once.
    reveal: Option<String>,
    /// Whether the settings have been read back.
    loaded: bool,
    /// Whether the forward has been asked for already.
    forward_requested: bool,
    /// Status for the pane header.
    caption: (String, HeaderHealth),
}

/// What an open context menu belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuTarget {
    /// A topic row: Delete.
    Topic(String),
    /// A saved message card: Edit / Remove.
    Message(String),
}

impl MqttAnalyser {
    /// Whether the header should offer Reconnect.
    ///
    /// Only a connection that is down needs one: Connected and Connecting are
    /// live states, and "SSH only" cannot be retried from here.
    fn wants_reconnect(&self) -> bool {
        match &self.status {
            Status::Disconnected(_) => true,
            Status::Failed(kind, _) => *kind != ErrorKind::NotSsh,
            Status::Idle
            | Status::Connected
            | Status::Connecting
            | Status::Unavailable(_) => false,
        }
    }

    /// Fraction of the width the topic list takes, clamped like the reference.
    fn split_ratio(&self) -> f32 {
        if self.tree_ratio <= 0.0 {
            SPLIT_DEFAULT_RATIO
        } else {
            self.tree_ratio.clamp(SPLIT_MIN_RATIO, SPLIT_MAX_RATIO)
        }
    }

    /// Whether a broker connection is open.
    pub fn is_connected(&self) -> bool {
        self.session.is_some() && self.status == Status::Connected
    }

    /// The connection state.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Topics heard so far.
    pub fn topics(&self) -> &BTreeMap<String, TopicEntry> {
        &self.topics
    }

    /// Drop the client, keeping the messages.
    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            session.disconnect();
        }
        self.status = Status::Idle;
    }

    /// Ask for the broker forward and connect once it is up.
    ///
    /// Runs every frame: the forward is requested once, and the client is only
    /// built when the session hands back a local address.
    fn connect(&mut self, context: &mut PluginContext<'_>) {
        if context.status != SessionStatus::Connected || context.session.is_none() {
            self.status = Status::Unavailable("MQTT Analyser requires an SSH session".to_owned());
            return;
        }
        if self.session.is_some() {
            return;
        }
        let mut address = None;
        let mut failure = None;
        if let Some(access) = context.session.as_deref_mut() {
            if !self.forward_requested && access.forward_address(FORWARD_KEY).is_none() {
                access.open_forward(FORWARD_KEY, &self.broker.host, self.broker.port);
                self.forward_requested = true;
            }
            address = access.forward_address(FORWARD_KEY).map(str::to_owned);
            failure = access.forward_error(FORWARD_KEY).map(str::to_owned);
        }
        if let Some(error) = failure {
            // A failed forward is asked for again on the next Reconnect.
            self.forward_requested = false;
            self.status = Status::Failed(classify(&error), error);
            return;
        }
        let Some(address) = address else {
            // Still binding the listener (or waiting for the channel to open).
            self.status = Status::Connecting;
            return;
        };

        let mut options = MqttOptions::new(self.broker.host.clone(), self.broker.port);
        if !self.broker.username.is_empty() {
            options.username = Some(self.broker.username.clone());
        }
        if !self.broker.password.is_empty() {
            options.password = Some(self.broker.password.clone());
        }
        options.dial = Some(address);
        match MqttSession::connect(options) {
            Ok(session) => {
                session.subscribe(SUBSCRIBE_TOPIC);
                self.session = Some(session);
                self.status = Status::Connecting;
            }
            Err(error) => self.status = Status::Failed(classify(&error), error),
        }
    }

    /// Start over: drop the client, forget the forward and reconnect.
    ///
    /// The reference also clears the tree, because a fresh snapshot follows.
    fn reconnect(&mut self) {
        self.disconnect();
        self.forward_requested = false;
        self.status = Status::Connecting;
        self.topics.clear();
        self.selected = None;
        self.blink.clear();
        self.expanded.clear();
        self.lines.clear();
        self.tree_dirty = false;
    }

    /// Drop the blink and copy markers that have expired.
    fn tick(&mut self, ctx: &egui::Context) {
        let now = crate::connection_logger::now_ms();
        self.blink
            .retain(|_, started| now.saturating_sub(*started) < BLINK_MS);
        if self
            .copied
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) >= COPY_FEEDBACK_MS)
        {
            self.copied = None;
        }
        if !self.blink.is_empty() || self.copied.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.poll_pick();
        if self.picking.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Select a topic; the reference also fills the publish topic.
    fn select(&mut self, path: &str) {
        self.selected = Some(path.to_owned());
        self.publish_topic = if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        };
    }

    /// Open or close a branch.
    fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
        }
        self.tree_dirty = true;
    }

    /// Rebuild the flattened rows if the tree changed.
    fn refresh_lines(&mut self) {
        let stale = self.tree_dirty || (self.lines.is_empty() && !self.topics.is_empty());
        if !stale {
            return;
        }
        self.lines = tree_lines(&self.topics, &self.expanded);
        self.tree_dirty = false;
    }

    /// Remember that a copy button confirmed.
    fn mark_copied(&mut self, token: &str) {
        self.copied = Some((token.to_owned(), crate::connection_logger::now_ms()));
    }

    /// Whether `token`'s copy button is still showing its confirmation.
    fn copied_recently(&self, token: &str) -> bool {
        let now = crate::connection_logger::now_ms();
        self.copied
            .as_ref()
            .is_some_and(|(saved, at)| saved == token && now.saturating_sub(*at) < COPY_FEEDBACK_MS)
    }

    /// The reference's Delete: publish an empty retained message for the whole
    /// subtree, then forget those topics here.
    fn clear_topic(&mut self, path: &str) {
        if self.session.is_none() {
            return;
        }
        let prefix = format!("{path}/");
        let mut paths: Vec<String> = self
            .topics
            .keys()
            .filter(|topic| topic.as_str() == path || topic.starts_with(&prefix))
            .cloned()
            .collect();
        // Children before their parent, like `collectSubtreePaths`.
        paths.sort_by_key(|topic| std::cmp::Reverse(topic.matches('/').count()));
        for topic in &paths {
            self.publish_bytes(topic, Vec::new(), 0, true);
            self.topics.remove(topic);
        }
        if let Some(selected) = self.selected.clone()
            && (selected == path || selected.starts_with(&prefix))
        {
            self.selected = None;
        }
        // A node that no longer has topics under it is not a node any more.
        let live: Vec<String> = self.topics.keys().cloned().collect();
        self.expanded.retain(|node| {
            let prefix = format!("{node}/");
            live.iter().any(|topic| topic == node || topic.starts_with(&prefix))
        });
        self.tree_dirty = true;
    }

    /// Open the native file dialog for a raw publish.
    ///
    /// The dialog runs on its own thread and is polled once a frame, so the
    /// window keeps painting while the user browses.
    fn start_pick(&mut self) {
        if self.picking.is_some() {
            return;
        }
        if cfg!(test) {
            // Never pop a dialog from a test run.
            return;
        }
        let (sender, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(pick_payload_file());
        });
        self.picking = Some(events);
    }

    /// Collect the file the dialog returned.
    fn poll_pick(&mut self) {
        let Some(events) = &self.picking else {
            return;
        };
        match events.try_recv() {
            Ok(Some((name, bytes))) => {
                self.picking = None;
                self.file_name = Some(name);
                self.file_bytes = Some(bytes);
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picking = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Publish raw bytes (the composer and Delete both end up here).
    fn publish_bytes(&mut self, topic: &str, payload: Vec<u8>, qos: u64, retain: bool) {
        let Some(session) = &self.session else {
            self.publish_error = Some("Not connected".to_owned());
            return;
        };
        if topic.trim().is_empty() {
            self.publish_error = Some("Topic is required".to_owned());
            return;
        }
        session.publish_qos(topic.trim(), &payload, qos as u8, retain);
        self.publish_error = None;
    }

    /// Publish from the composer, in whichever mode it is in.
    fn publish_composed(&mut self) {
        self.publish_error = None;
        let topic = self.publish_topic.trim().to_owned();
        if topic.is_empty() {
            self.publish_error = Some("Topic is required".to_owned());
            return;
        }
        if self.session.is_none() {
            self.publish_error = Some("Not connected".to_owned());
            return;
        }
        let payload = match self.publish_mode {
            PublishMode::RawFile => match self.file_bytes.clone() {
                Some(bytes) => bytes,
                None => {
                    self.publish_error = Some("Choose a file".to_owned());
                    return;
                }
            },
            mode => match encode_payload(&self.publish_payload, mode) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.publish_error = Some(error);
                    return;
                }
            },
        };
        let (qos, retain) = (self.publish_qos, self.publish_retain);
        self.publish_bytes(&topic, payload, qos, retain);
    }

    /// Publish a saved message with the composer's QoS and retain settings.
    fn publish_saved(&mut self, id: &str) {
        self.publish_error = None;
        let Some(message) = self.saved.iter().find(|message| message.id == id).cloned() else {
            return;
        };
        let topic = message.topic.trim().to_owned();
        if topic.is_empty() {
            self.publish_error = Some("Saved message has no topic".to_owned());
            return;
        }
        if self.session.is_none() {
            self.publish_error = Some("Not connected".to_owned());
            return;
        }
        let bytes = match encode_payload(&message.payload, message.mode.into()) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.publish_error = Some(error);
                return;
            }
        };
        let (qos, retain) = (self.publish_qos, self.publish_retain);
        self.publish_bytes(&topic, bytes, qos, retain);
    }

    /// Open the editor on a saved message.
    fn edit_saved(&mut self, id: &str) {
        if let Some(message) = self.saved.iter().find(|message| message.id == id) {
            self.editor = Some(message.clone());
            self.editor_is_new = false;
        }
    }

    /// Forget a saved message.
    fn remove_saved(&mut self, id: &str) {
        self.saved.retain(|message| message.id != id);
        self.data_dirty = true;
    }

    /// Fold everything the client thread reported since the last frame.
    fn drain_events(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        for event in session.events() {
            match event {
                MqttEvent::Connected => self.status = Status::Connected,
                MqttEvent::Message {
                    topic,
                    payload,
                    qos,
                    retain,
                } => {
                    // An empty payload means the topic was cleared on the
                    // broker; the reference drops it from the tree.
                    if payload.is_empty() {
                        self.topics.remove(&topic);
                        self.blink.remove(&topic);
                        if self.selected.as_deref() == Some(topic.as_str()) {
                            self.selected = None;
                        }
                        self.tree_dirty = true;
                        continue;
                    }
                    let now = crate::connection_logger::now_ms();
                    let message = Message::new(topic.clone(), &payload, now, qos, retain);
                    let entry = self.topics.entry(topic.clone()).or_default();
                    push_history(entry, message);
                    self.tree_dirty = true;
                    // Blink the topic, and every ancestor that is collapsed, so
                    // a message in a closed branch is still noticed.
                    self.blink.insert(topic.clone(), now);
                    let segments: Vec<&str> = topic.split('/').collect();
                    let mut ancestor = String::new();
                    for segment in &segments[..segments.len().saturating_sub(1)] {
                        if !ancestor.is_empty() {
                            ancestor.push('/');
                        }
                        ancestor.push_str(segment);
                        if !self.expanded.contains(&ancestor) {
                            self.blink.insert(ancestor.clone(), now);
                        }
                    }
                    while self.topics.len() > MAX_TOPICS {
                        // Drop the least recently updated topic.
                        let Some(oldest) = self
                            .topics
                            .iter()
                            .min_by_key(|(_, entry)| {
                                entry.history.back().map_or(0, |message| message.at_ms)
                            })
                            .map(|(topic, _)| topic.clone())
                        else {
                            break;
                        };
                        self.topics.remove(&oldest);
                    }
                }
                MqttEvent::Error(error) => {
                    self.status = Status::Failed(classify(&error), error);
                    self.session = None;
                }
                MqttEvent::Closed => {
                    self.status = Status::Disconnected("Connection closed".to_owned());
                    self.session = None;
                }
            }
        }
    }

    /// Read the stored saved messages back.
    fn load(&mut self, settings: &serde_json::Map<String, Value>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        self.saved = saved_from_json(settings.get(SAVED_KEY));
        self.collapsed = settings
            .get(COLLAPSED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(true);
    }
}

impl Plugin for MqttAnalyser {
    fn manifest(&self) -> PluginManifest {
        PluginId::MqttAnalyser.manifest()
    }

    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        (!self.caption.0.is_empty()).then(|| self.caption.clone())
    }

    fn header_action(&self) -> Option<HeaderButton> {
        // A row of its own would cost the height the tree needs; the header
        // already shows the state, so the retry belongs next to it.
        self.wants_reconnect().then_some(HeaderButton {
            label: "Reconnect",
            enabled: true,
        })
    }

    fn header_action_clicked(&mut self) {
        self.reconnect();
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        self.load(context.settings);
        self.broker =
            broker_from_settings(context.settings, context.secret(PASSWORD_SETTING));

        // Connect as soon as the session is up, like the reference: no fields —
        // the broker comes from the host settings.
        if context.status == SessionStatus::Connected {
            // "Unavailable" only ever comes from the session not being up yet, so
            // it is retried once the tab connects. A failure or a closed broker
            // is the user's call to retry, from the header button.
            let retry = matches!(
                self.status,
                Status::Idle | Status::Connecting | Status::Unavailable(_)
            );
            if self.session.is_none() && retry {
                self.connect(context);
            }
        } else if self.session.is_none() {
            self.status = Status::Unavailable("MQTT Analyser requires an SSH session".to_owned());
        }
        self.drain_events();
        self.tick(ui.ctx());
        self.caption = (self.status.label(), self.status.health());

        // --- topics | splitter | details ---------------------------------
        //
        // `styles.css`: `.mqtt-split` is a row holding the tree pane (`flexGrow:
        // treeRatio`), a 4 px `.mqtt-splitter` and the detail pane (`flexGrow:
        // 1 - treeRatio`), each with a 100 px minimum.
        let width = ui.available_width();
        let height = ui.available_height();
        let ratio = self.split_ratio();
        let room = (width - SPLITTER_WIDTH).max(PANE_MIN_WIDTH * 2.0);
        let tree_width = (room * ratio).clamp(PANE_MIN_WIDTH, room - PANE_MIN_WIDTH);
        let detail_width = (room - tree_width).max(PANE_MIN_WIDTH);

        self.refresh_lines();
        self.handle_keys(ui, tree_width, height);

        ui.horizontal_top(|ui| {
            // `.mqtt-split` has no gap: the splitter divides the two panes.
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.allocate_ui_with_layout(
                Vec2::new(tree_width, height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(tree_width);
                    ui.set_height(height);
                    self.draw_topics(ui, &palette, height);
                },
            );

            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(SPLITTER_WIDTH, height),
                Sense::click_and_drag(),
            );
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            let colour = if response.hovered() || response.dragged() {
                palette.accent
            } else {
                palette.border
            };
            ui.painter().rect_filled(rect, 0.0, to_color32(colour));
            if response.dragged() {
                let total = width.max(1.0);
                self.tree_ratio =
                    (ratio + response.drag_delta().x / total).clamp(SPLIT_MIN_RATIO, SPLIT_MAX_RATIO);
            }

            ui.allocate_ui_with_layout(
                Vec2::new(detail_width, height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(detail_width);
                    ui.set_height(height);
                    self.draw_details(ui, &palette, detail_width, height);
                },
            );
        });

        self.draw_menus(ui, &palette);
        self.saved_dialog(ui, &palette);
        self.persist(context);
    }
}

impl MqttAnalyser {
    // --- topics --------------------------------------------------------

    /// The topic list: `.mqtt-pane-label` plus `.mqtt-tree`.
    ///
    /// Only the rows inside the viewport are built (`ScrollArea::show_rows`):
    /// an expanded tree can hold thousands of rows, and laying out every one of
    /// them each frame is what made the pane crawl.
    fn draw_topics(&mut self, ui: &mut Ui, palette: &Palette, height: f32) {
        // `.mqtt-tree-row` has no margin of its own, and `show_rows` reads the
        // spacing from this ui to compute where every row sits.
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        pane_label(ui, palette, "Topics");
        let list_height = (height - PANE_LABEL_HEIGHT).max(0.0);
        // The rows are lent out so the pane can be mutated while they draw.
        let lines = std::mem::take(&mut self.lines);
        if lines.is_empty() {
            empty_text(ui, palette, "No messages yet");
            self.lines = lines;
            return;
        }
        let now = crate::connection_logger::now_ms();
        let reveal = self.reveal.take();
        let mut clicked = None;
        let mut toggled = None;
        let mut secondary = None;

        egui::ScrollArea::vertical()
            .id_salt("mqtt-topics")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show_rows(ui, TOPIC_ROW_HEIGHT, lines.len(), |ui, range| {
                for index in range {
                    let Some(line) = lines.get(index) else {
                        continue;
                    };
                    let meta = line.meta.as_deref();
                    let blink = self.blink.get(&line.path).map_or(0.0, |started| {
                        let elapsed = now.saturating_sub(*started);
                        if elapsed >= BLINK_MS {
                            0.0
                        } else {
                            1.0 - elapsed as f32 / BLINK_MS as f32
                        }
                    });
                    let response = topic_row(
                        ui,
                        palette,
                        &TopicRow {
                            label: if line.label.is_empty() {
                                "/"
                            } else {
                                &line.label
                            },
                            meta,
                            depth: line.depth,
                            expandable: line.expandable,
                            expanded: line.expanded,
                            selected: self.selected.as_deref() == Some(line.path.as_str()),
                            blink,
                        },
                    );
                    if reveal.as_deref() == Some(line.path.as_str()) {
                        ui.scroll_to_rect(response.rect, Some(Align::Center));
                    }
                    // A click can only come from a row that was drawn, so the
                    // decision is applied after the list is handed back.
                    if response.toggled || (response.double_clicked && line.expandable) {
                        toggled = Some(line.path.clone());
                    } else if response.clicked {
                        clicked = Some(line.path.clone());
                    }
                    if response.secondary_clicked {
                        secondary = Some((line.path.clone(), response.rect.left_bottom()));
                    }
                }
            });
        self.lines = lines;

        if let Some(path) = toggled {
            self.toggle(&path);
        } else if let Some(path) = clicked {
            self.select(&path);
        }
        if let Some((path, at)) = secondary {
            // Right-click selects the row first, like the reference.
            self.select(&path);
            self.menu_target = Some(MenuTarget::Topic(path));
            let at = ui.ctx().pointer_latest_pos().unwrap_or(at);
            self.menu.open_at(
                at,
                vec![MenuItem::Action {
                    label: "Delete",
                    enabled: self.session.is_some(),
                    danger: true,
                }],
            );
        }
    }

    /// Arrow keys and Delete over the topic list, as in the reference.
    ///
    /// They are only taken while the pointer is over the list, so a terminal
    /// beside the pane keeps its own arrows and Delete.
    fn handle_keys(&mut self, ui: &Ui, tree_width: f32, height: f32) {
        let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(tree_width, height));
        if !ui.rect_contains_pointer(rect) || self.topics.is_empty() {
            return;
        }
        // The rows are lent out so the pane can be mutated afterwards.
        let visible = std::mem::take(&mut self.lines);
        let selected = self
            .selected
            .as_deref()
            .and_then(|path| visible.iter().position(|line| line.path == path));
        let action = tree_key_action(ui, &visible, selected);
        self.lines = visible;
        match action {
            Some(TreeKey::Select(path)) => {
                self.reveal = Some(path.clone());
                self.select(&path);
            }
            Some(TreeKey::Toggle(path)) => self.toggle(&path),
            Some(TreeKey::Delete(path)) => self.clear_topic(&path),
            None => {}
        }
    }

    // --- details -------------------------------------------------------

    /// The right-hand pane: `.mqtt-detail-meta`, `.mqtt-history` and, pinned to
    /// the bottom, `.mqtt-publish` (which also holds the saved messages).
    fn draw_details(&mut self, ui: &mut Ui, palette: &Palette, width: f32, height: f32) {
        pane_label(ui, palette, "Details");
        let publish_height = self.publish_height(width, height);
        let upper = (height - PANE_LABEL_HEIGHT - publish_height).max(0.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, upper),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                ui.set_height(upper);
                self.draw_detail_body(ui, palette, upper);
            },
        );
        ui.allocate_ui_with_layout(
            Vec2::new(width, publish_height),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                ui.set_height(publish_height);
                self.draw_publish(ui, palette, width, publish_height);
            },
        );
    }

    /// Topic header, facts and the message history of the selected topic.
    fn draw_detail_body(&mut self, ui: &mut Ui, palette: &Palette, height: f32) {
        let Some(path) = self.selected.clone() else {
            empty_text(ui, palette, "Select a topic");
            return;
        };
        // Lend the entry out instead of cloning up to a hundred messages on
        // every frame; it goes straight back below.
        let entry = self.topics.remove(&path).unwrap_or_default();
        let prefix = format!("{path}/");
        let sub_topics = self
            .topics
            .keys()
            .filter(|topic| topic.starts_with(&prefix))
            .count();
        let display = if path.is_empty() { "/" } else { path.as_str() };

        // --- `.mqtt-detail-meta` --------------------------------------
        let mut clear = false;
        let mut used = 0.0;
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: PANE_GAP as i8,
                right: PANE_GAP as i8,
                top: 0,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let start = ui.cursor().min.y;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = PANE_GAP;
                    let buttons = COPY_BUTTON + PANE_GAP + DELETE_BUTTON_WIDTH + PANE_GAP * 2.0;
                    let topic_room = (ui.available_width() - buttons).max(24.0);
                    topic_text(ui, palette, display, topic_room);
                    let token = format!("topic|{path}");
                    if copy_button(ui, palette, self.copied_recently(&token)) {
                        ui.ctx().copy_text(display.to_owned());
                        self.mark_copied(&token);
                    }
                    let delete = Button::new("Delete")
                        .compact()
                        .kind(ButtonKind::Danger)
                        .enabled(self.session.is_some())
                        .show(ui, palette);
                    if delete.clicked() {
                        clear = true;
                    }
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = PANE_GAP;
                    for text in [
                        format!("{} in history", entry.history.len()),
                        format!("{sub_topics} sub-topics"),
                        format!("{} msgs total", entry.received),
                    ] {
                        ui.label(muted_label(text, 10.0, palette));
                    }
                });
                used = ui.cursor().min.y - start;
            });

        // --- `.mqtt-history` ------------------------------------------
        //
        // `.mqtt-history { max-height: 180px }` means four or five cards are ever
        // on screen, so only those are built: with a hundred messages of large
        // payloads, laying out every card each frame is what made a topic with
        // data crawl.
        let history_height = (height - used - 6.0).clamp(0.0, HISTORY_MAX_HEIGHT);
        let mut copied_token: Option<String> = None;
        if entry.history.is_empty() {
            let hint = if sub_topics > 0 {
                "Branch node — expand and select a leaf topic with messages"
            } else {
                "No messages on this topic"
            };
            empty_text(ui, palette, hint);
        } else {
            egui::ScrollArea::vertical()
                .id_salt("mqtt-history")
                .max_height(history_height.min(f32::MAX))
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    // The payload sits inside `.mqtt-history`'s gap padding and
                    // the card's own padding.
                    let inner = ui.available_width()
                        - 2.0 * (PANE_GAP + HISTORY_ITEM_PAD);
                    let metrics = PayloadMetrics::new(ui, inner);
                    let mut y = 0.0f32;
                    for (index, message) in entry.history.iter().rev().enumerate() {
                        let card = metrics.card_height(message);
                        let visible = y + card >= viewport.min.y && y <= viewport.max.y;
                        if !visible {
                            // Reserve the space without building the card.
                            ui.allocate_space(Vec2::new(ui.available_width(), card + HISTORY_GAP));
                            y += card + HISTORY_GAP;
                            continue;
                        }
                        let token = format!("payload|{path}|{index}");
                        if history_item(
                            ui,
                            palette,
                            &token,
                            message,
                            self.copied_recently(&token),
                            metrics,
                        ) {
                            copied_token = Some(token);
                        }
                        ui.allocate_space(Vec2::new(ui.available_width(), HISTORY_GAP));
                        y += card + HISTORY_GAP;
                    }
                });
        }
        if let Some(token) = copied_token {
            self.mark_copied(&token);
        }

        // The entry was only borrowed for the draw; Delete then clears the whole
        // subtree, which needs it back in the map.
        self.topics.insert(path.clone(), entry);
        if clear {
            self.clear_topic(&path);
        }
    }

    /// `.mqtt-publish`: the composer and the saved messages, along the bottom.
    fn draw_publish(&mut self, ui: &mut Ui, palette: &Palette, width: f32, height: f32) {
        // `.mqtt-publish { border-top: 1px; padding: 0 8px 8px; overflow: auto }`
        let top = ui.cursor().min.y;
        ui.painter().hline(
            ui.cursor().x_range(),
            top,
            Stroke::new(1.0, to_color32(palette.border)),
        );
        egui::ScrollArea::vertical()
            .id_salt("mqtt-publish")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: PANE_GAP as i8,
                        right: PANE_GAP as i8,
                        top: 0,
                        bottom: PANE_GAP as i8,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        pane_label(ui, palette, "Publish");

                        field_caption(ui, palette, "Topic");
                        ui.add_space(2.0);
                        text_input(ui, palette, &mut self.publish_topic, "topic/path");
                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for mode in [PublishMode::Text, PublishMode::Json, PublishMode::RawFile] {
                                if mode_chip(ui, palette, mode, self.publish_mode == mode) {
                                    self.publish_mode = mode;
                                }
                            }
                        });
                        ui.add_space(6.0);

                        if self.publish_mode == PublishMode::RawFile {
                            field_caption(ui, palette, "File");
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                let choosing = self.picking.is_some();
                                if Button::new(if choosing { "Choosing…" } else { "Choose file…" })
                                    .compact()
                                    .enabled(!choosing)
                                    .show(ui, palette)
                                    .clicked()
                                {
                                    self.start_pick();
                                }
                                if let Some(name) = &self.file_name {
                                    ui.label(
                                        egui::RichText::new(name.as_str())
                                            .size(11.0)
                                            .color(to_color32(palette.text)),
                                    );
                                }
                            });
                        } else {
                            field_caption(ui, palette, "Payload");
                            ui.add_space(2.0);
                            text_area(ui, palette, &mut self.publish_payload, 4, "payload");
                        }
                        ui.add_space(6.0);

                        // `.mqtt-publish-options`
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = PANE_GAP;
                            ui.label(muted_label("QoS".to_owned(), 11.0, palette));
                            if let Some(qos) = select(
                                ui,
                                "mqtt-qos",
                                &self.publish_qos,
                                &[(0u64, "0"), (1u64, "1")],
                            ) {
                                self.publish_qos = qos;
                            }
                            checkbox(ui, palette, &mut self.publish_retain, "Retain");
                            let ready = self.session.is_some();
                            if Button::new("Publish")
                                .compact()
                                .enabled(ready)
                                .show(ui, palette)
                                .clicked()
                            {
                                self.publish_composed();
                            }
                        });
                        if let Some(error) = &self.publish_error {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(error.as_str())
                                    .size(11.0)
                                    .color(to_color32(palette.danger)),
                            );
                        }
                        ui.add_space(6.0);
                        self.draw_saved(ui, palette, width);
                    });
            });
    }

    /// `.mqtt-msg-section`: the collapsible Saved list and its Add button.
    fn draw_saved(&mut self, ui: &mut Ui, palette: &Palette, width: f32) {
        let top = ui.cursor().min.y;
        ui.painter().hline(
            ui.cursor().x_range(),
            top,
            Stroke::new(1.0, to_color32(palette.border)),
        );
        ui.add_space(6.0);

        // `.mqtt-msg-head`: caret, title, count — the whole row toggles.
        let (head, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), SAVED_HEAD_HEIGHT),
            Sense::click(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.collapsed = !self.collapsed;
            self.data_dirty = true;
        }
        let caret_rect = Rect::from_min_size(head.min, Vec2::splat(16.0));
        ui.painter().text(
            caret_rect.center(),
            Align2::CENTER_CENTER,
            if self.collapsed { "▸" } else { "▾" },
            FontId::proportional(10.0),
            to_color32(if response.hovered() {
                palette.text
            } else {
                palette.text_muted
            }),
        );
        let title = text_galley(ui, "SAVED", 11.0, palette.text_muted, 0.44);
        let title_pos = egui::pos2(
            caret_rect.right() + 4.0,
            head.center().y - title.size().y / 2.0,
        );
        ui.painter()
            .galley(title_pos, title.clone(), to_color32(palette.text_muted));
        let count = self.saved.len().to_string();
        ui.painter().text(
            egui::pos2(
                head.right(),
                head.center().y,
            ),
            Align2::RIGHT_CENTER,
            count,
            FontId::proportional(10.0),
            to_color32(palette.text_muted),
        );
        let _ = title_pos;

        if self.collapsed {
            return;
        }
        ui.add_space(4.0);
        if self.saved.is_empty() {
            empty_text(ui, palette, "No saved messages");
        } else {
            let columns = saved_columns(width - PANE_GAP * 2.0);
            let card_width = ((width - PANE_GAP * 2.0) - SAVED_CARD_GAP * (columns - 1) as f32)
                / columns as f32;
            let mut row_top = ui.cursor().min;
            let mut fire: Option<String> = None;
            let mut menu: Option<(String, egui::Pos2)> = None;
            for (index, message) in self.saved.iter().enumerate() {
                let column = index % columns;
                if column == 0 && index > 0 {
                    row_top.y += SAVED_CARD_HEIGHT + SAVED_CARD_GAP;
                }
                let left = row_top.x + column as f32 * (card_width + SAVED_CARD_GAP);
                let rect = Rect::from_min_size(
                    egui::pos2(left, row_top.y),
                    Vec2::new(card_width, SAVED_CARD_HEIGHT),
                );
                let response = saved_card(ui, palette, index, message, rect);
                if response.clicked {
                    fire = Some(message.id.clone());
                }
                if response.secondary_clicked {
                    menu = Some((
                        message.id.clone(),
                        ui.ctx()
                            .pointer_latest_pos()
                            .unwrap_or_else(|| rect.left_bottom()),
                    ));
                }
            }
            if let Some(id) = fire {
                self.publish_saved(&id);
            }
            if let Some((id, at)) = menu {
                self.menu_target = Some(MenuTarget::Message(id));
                self.menu
                    .open_at(at, vec![MenuItem::new("Edit"), MenuItem::danger("Remove")]);
            }
            // Reserve the rows that were painted.
            let rows = self.saved.len().div_ceil(columns);
            let painted = rows as f32 * SAVED_CARD_HEIGHT + (rows - 1) as f32 * SAVED_CARD_GAP;
            let _ = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), painted),
                Sense::hover(),
            );
        }
        ui.add_space(4.0);
        if Button::new("＋ Add message")
            .compact()
            .kind(ButtonKind::Primary)
            .show(ui, palette)
            .clicked()
        {
            self.editor = Some(SavedMessage {
                id: format!("msg-{}", crate::connection_logger::now_ms()),
                label: NEW_MESSAGE_LABEL.to_owned(),
                topic: self.publish_topic.trim().to_owned(),
                payload: String::new(),
                mode: SavedMode::Text,
            });
            self.editor_is_new = true;
        }
    }

    /// Keep the parts the reference stores in plugin data.
    ///
    /// Only the saved messages and the section's collapsed state: the reference
    /// keeps the tree's own selection and expansion in memory.
    fn persist(&mut self, context: &mut PluginContext<'_>) {
        if !self.data_dirty {
            return;
        }
        self.data_dirty = false;
        context.set_value(SAVED_KEY, saved_to_json(&self.saved));
        context.set_value(COLLAPSED_KEY, Value::Bool(self.collapsed));
    }

    /// Height the publish block asks for, capped like `.mqtt-publish
    /// { max-height: 45% }`.
    fn publish_height(&self, width: f32, available: f32) -> f32 {
        let mut height = PANE_LABEL_HEIGHT;
        height += FIELD_LABEL_HEIGHT + 2.0 + INPUT_HEIGHT + 6.0;
        height += BUTTON_ROW_HEIGHT + 6.0;
        height += match self.publish_mode {
            PublishMode::RawFile => FIELD_LABEL_HEIGHT + 2.0 + INPUT_HEIGHT + 6.0,
            _ => FIELD_LABEL_HEIGHT + 2.0 + TEXT_AREA_HEIGHT + 6.0,
        };
        height += BUTTON_ROW_HEIGHT;
        if self.publish_error.is_some() {
            height += 4.0 + 15.0;
        }
        height += 6.0 + 6.0 + SAVED_HEAD_HEIGHT;
        if !self.collapsed {
            height += 4.0;
            if self.saved.is_empty() {
                height += 17.0;
            } else {
                let columns = saved_columns(width - PANE_GAP * 2.0);
                let rows = self.saved.len().div_ceil(columns);
                height += rows as f32 * SAVED_CARD_HEIGHT
                    + (rows.saturating_sub(1)) as f32 * SAVED_CARD_GAP;
            }
            height += 4.0 + ADD_BUTTON_HEIGHT;
        }
        height += PANE_GAP;
        height
            .min((available * 0.45).max(PANE_LABEL_HEIGHT))
            .min(available)
    }

    // --- context menus ------------------------------------------------

    /// Draw whichever context menu is open and apply the choice.
    fn draw_menus(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(index) = self.menu.show(ui.ctx(), palette) else {
            if !self.menu.is_open() {
                self.menu_target = None;
            }
            return;
        };
        let Some(target) = self.menu_target.take() else {
            return;
        };
        match (target, index) {
            (MenuTarget::Topic(path), 0) => self.clear_topic(&path),
            (MenuTarget::Message(id), 0) => self.edit_saved(&id),
            (MenuTarget::Message(id), _) => self.remove_saved(&id),
            _ => {}
        }
    }

    // --- saved message editor ----------------------------------------

    /// The saved-message editor, matching the reference's `SavedMessageDialog`.
    fn saved_dialog(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(mut draft) = self.editor.clone() else {
            return;
        };
        let mut close = false;
        let mut save = false;
        let json_error = (draft.mode == SavedMode::Json)
            .then(|| payload_for_mode(&draft.payload, draft.mode).err())
            .flatten();
        let can_save = !draft.label.trim().is_empty()
            && !draft.topic.trim().is_empty()
            && json_error.is_none();
        egui::Area::new(egui::Id::unique("mqtt-saved-editor"))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(to_color32(palette.bg_panel))
                    .stroke(Stroke::new(1.0, to_color32(palette.border)))
                    .corner_radius(egui::CornerRadius::same(wassh_core::RADIUS as u8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 12],
                        blur: 32,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(102),
                    })
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        let width = MESSAGE_DIALOG_WIDTH.min(ui.available_width());
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        ui.label(
                            egui::RichText::new(if self.editor_is_new {
                                "Add saved message"
                            } else {
                                "Edit saved message"
                            })
                            .size(12.0)
                            .strong()
                            .color(to_color32(palette.text)),
                        );
                        ui.add_space(8.0);
                        field_caption(ui, palette, "Label");
                        ui.add_space(2.0);
                        text_input(ui, palette, &mut draft.label, "Name shown on the button");
                        ui.add_space(8.0);
                        field_caption(ui, palette, "Topic");
                        ui.add_space(2.0);
                        text_input(ui, palette, &mut draft.topic, "topic/path");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for mode in [SavedMode::Text, SavedMode::Json] {
                                if saved_mode_chip(ui, palette, mode, draft.mode == mode) {
                                    draft.mode = mode;
                                }
                            }
                        });
                        ui.add_space(8.0);
                        field_caption(ui, palette, "Payload");
                        ui.add_space(2.0);
                        text_area(ui, palette, &mut draft.payload, 6, "payload");
                        if let Some(error) = &json_error {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(error.as_str())
                                    .size(11.0)
                                    .color(to_color32(palette.danger)),
                            );
                        }
                        ui.add_space(8.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Button::new("Save")
                                .compact()
                                .kind(ButtonKind::Primary)
                                .enabled(can_save)
                                .show(ui, palette)
                                .clicked()
                            {
                                save = true;
                            }
                            if Button::new("Cancel").compact().show(ui, palette).clicked() {
                                close = true;
                            }
                        });
                    });
            });
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if save {
            draft.label = draft.label.trim().to_owned();
            draft.topic = draft.topic.trim().to_owned();
            match self
                .saved
                .iter_mut()
                .find(|message| message.id == draft.id)
            {
                Some(existing) => *existing = draft.clone(),
                None => self.saved.push(draft.clone()),
            }
            self.data_dirty = true;
            self.editor = None;
        } else if close {
            self.editor = None;
        } else {
            self.editor = Some(draft);
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers, each one a straight port of a rule in `styles.css`
// ---------------------------------------------------------------------------

/// What a key press on the topic list asks for.
enum TreeKey {
    /// Select and scroll to this row.
    Select(String),
    /// Open or close this branch.
    Toggle(String),
    /// Clear this topic (and everything under it).
    Delete(String),
}

/// The reference's key bindings over the visible rows.
///
/// Only reads input: the caller puts its cached rows back before acting, so a
/// key press never leaves the list borrowed.
fn tree_key_action(ui: &Ui, visible: &[TreeLine], selected: Option<usize>) -> Option<TreeKey> {
    let pick = |delta: isize| -> Option<TreeKey> {
        let next = match selected {
            Some(index) => index as isize + delta,
            None => 0,
        };
        (next >= 0 && (next as usize) < visible.len())
            .then(|| visible[next as usize].path.clone())
            .map(TreeKey::Select)
    };
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        return pick(1);
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        return pick(-1);
    }
    let index = selected?;
    let line = visible.get(index)?;
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)) {
        if line.expandable && !line.expanded {
            return Some(TreeKey::Toggle(line.path.clone()));
        }
        return visible
            .get(index + 1)
            .filter(|child| line.expandable && child.depth > line.depth)
            .map(|child| TreeKey::Select(child.path.clone()));
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
        if line.expandable && line.expanded {
            return Some(TreeKey::Toggle(line.path.clone()));
        }
        return visible[..index]
            .iter()
            .rev()
            .find(|candidate| candidate.depth < line.depth)
            .map(|candidate| TreeKey::Select(candidate.path.clone()));
    }
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
        return Some(TreeKey::Delete(line.path.clone()));
    }
    None
}

/// One row of the topic tree, as `.mqtt-tree-row` draws it.
struct TopicRow<'a> {
    label: &'a str,
    meta: Option<&'a str>,
    depth: u16,
    expandable: bool,
    expanded: bool,
    selected: bool,
    /// How much of the new-message highlight is left, 0.0 to 1.0.
    blink: f32,
}

/// What the user did to a row this frame.
#[derive(Clone, Copy, Debug)]
struct TopicRowResponse {
    rect: Rect,
    clicked: bool,
    toggled: bool,
    double_clicked: bool,
    secondary_clicked: bool,
}

/// `.mqtt-tree-meta`: the count a collapsed branch shows, else the leaf's own.
fn row_meta(collapsed_branch: bool, sub_topics: u64, under: u64, own: u64) -> Option<String> {
    if collapsed_branch {
        return Some(format!(
            "{sub_topics} topic{}, {under} msg{}",
            if sub_topics == 1 { "" } else { "s" },
            if under == 1 { "" } else { "s" }
        ));
    }
    (own > 0).then(|| own.to_string())
}

/// `.mqtt-pane-label`: uppercase, 10 px, letter-spaced, padded 6/8/4.
fn pane_label(ui: &mut Ui, palette: &Palette, text: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, PANE_LABEL_HEIGHT), Sense::hover());
    let galley = text_galley(ui, &text.to_uppercase(), 10.0, palette.text_muted, 0.6);
    ui.painter().galley(
        egui::pos2(rect.left() + PANE_GAP, rect.top() + 6.0),
        galley,
        to_color32(palette.text_muted),
    );
}

/// A single-line galley with letter spacing (CSS `letter-spacing`).
fn text_galley(
    ui: &Ui,
    text: &str,
    size: f32,
    colour: wassh_core::Rgba,
    spacing: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        FontId::proportional(size),
        to_color32(colour),
    );
    for section in &mut job.sections {
        section.format.extra_letter_spacing = spacing;
    }
    ui.painter().layout_job(job)
}

/// `.mqtt-empty`: muted 12 px text inset by one gap.
fn empty_text(ui: &mut Ui, palette: &Palette, text: &str) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: PANE_GAP as i8,
            right: PANE_GAP as i8,
            top: PANE_GAP as i8,
            bottom: PANE_GAP as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(text)
                    .size(12.0)
                    .color(to_color32(palette.text_muted)),
            );
        });
}

/// A muted label at `size`.
fn muted_label(text: String, size: f32, palette: &Palette) -> egui::RichText {
    egui::RichText::new(text)
        .size(size)
        .color(to_color32(palette.text_muted))
}

/// `.mqtt-field > span`: an 11 px muted caption.
fn field_caption(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(muted_label(text.to_owned(), 11.0, palette));
}

/// `.mqtt-copy-btn`: an 18 px flat button that confirms briefly.
fn copy_button(ui: &mut Ui, palette: &Palette, copied: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(COPY_BUTTON), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let colour = if copied {
        to_color32(palette.accent)
    } else if response.hovered() {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if copied { COPIED_GLYPH } else { COPY_GLYPH },
        FontId::proportional(11.0),
        colour,
    );
    response.clicked()
}

/// `.mqtt-detail-topic`: mono 12 px, broken anywhere so a long path wraps.
fn topic_text(ui: &mut Ui, palette: &Palette, text: &str, width: f32) {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::monospace(12.0),
        to_color32(palette.text),
        width,
    );
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, galley.size().y), Sense::hover());
    ui.painter()
        .galley(rect.min, galley, to_color32(palette.text));
}

/// One painted topic row; returns what the pointer did to it.
fn topic_row(ui: &mut Ui, palette: &Palette, row: &TopicRow<'_>) -> TopicRowResponse {
    let (full, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), TOPIC_ROW_HEIGHT), Sense::click());
    // `.mqtt-tree { padding: 0 4px }` insets the row's own background.
    let rect = Rect::from_min_size(
        egui::pos2(full.left() + TREE_PAD_X, full.top()),
        Vec2::new((full.width() - TREE_PAD_X * 2.0).max(0.0), full.height()),
    );
    let radius = egui::CornerRadius::same(wassh_core::RADIUS as u8);
    if row.blink > 0.0 {
        ui.painter().rect_filled(
            rect,
            radius,
            to_color32(palette.accent).gamma_multiply(0.35 * row.blink),
        );
    }
    if row.selected {
        // `color-mix(in srgb, accent 18%, bg-input)` plus an inset ring.
        ui.painter().rect_filled(
            rect,
            radius,
            to_color32(wassh_core::colors::tint(
                palette.accent,
                palette.bg_input,
                0.18,
            )),
        );
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, to_color32(palette.text).gamma_multiply(0.78)),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            radius,
            to_color32(palette.bg_input).gamma_multiply(0.8),
        );
    }

    let twist_left = rect.left() + TOPIC_ROW_PAD + row.depth as f32 * TOPIC_INDENT;
    let mut toggled = false;
    if row.expandable {
        let twist = Rect::from_min_size(
            egui::pos2(twist_left, rect.top()),
            Vec2::new(TOPIC_TWIST_WIDTH, rect.height()),
        );
        let hit = ui.interact(twist, response.id.with("twist"), Sense::click());
        if hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().text(
            twist.center(),
            Align2::CENTER_CENTER,
            if row.expanded { "▾" } else { "▸" },
            FontId::proportional(10.0),
            to_color32(if hit.hovered() {
                palette.text
            } else {
                palette.text_muted
            }),
        );
        toggled = hit.clicked();
    }

    let name_left = twist_left + TOPIC_TWIST_WIDTH + TOPIC_ROW_GAP;
    let meta_width = row.meta.map_or(0.0, |text| {
        ui.painter()
            .layout_no_wrap(
                text.to_owned(),
                FontId::proportional(10.0),
                to_color32(palette.text_muted),
            )
            .size()
            .x
            + TOPIC_ROW_GAP
    });
    let name_right = rect.right() - TOPIC_ROW_PAD - meta_width;
    if name_right > name_left {
        let mut job = egui::text::LayoutJob::simple_singleline(
            row.label.to_owned(),
            FontId::monospace(11.0),
            to_color32(palette.text),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(name_right - name_left);
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(
            egui::pos2(name_left, rect.center().y - galley.size().y / 2.0),
            galley,
            to_color32(palette.text),
        );
    }
    if let Some(meta) = row.meta {
        ui.painter().text(
            egui::pos2(rect.right() - TOPIC_ROW_PAD, rect.center().y),
            Align2::RIGHT_CENTER,
            meta,
            FontId::proportional(10.0),
            to_color32(palette.text_muted),
        );
    }

    TopicRowResponse {
        rect,
        clicked: response.clicked() && !toggled,
        toggled,
        double_clicked: response.double_clicked(),
        secondary_clicked: response.secondary_clicked(),
    }
}

/// Height of a card's header line (`10 px` text plus its margin).
const HISTORY_HEAD_HEIGHT: f32 = 15.0;
/// Vertical padding inside `.mqtt-history-item`.
const HISTORY_ITEM_PAD_Y: f32 = 6.0;

/// How wide one payload row is, and how tall the payload window is.
#[derive(Clone, Copy, Debug)]
struct PayloadMetrics {
    /// Width available for the payload text.
    width: f32,
    /// Height of one monospace row.
    line_height: f32,
    /// How many characters fit on one row.
    chars_per_row: f32,
}

impl PayloadMetrics {
    /// Measure the monospace face once per frame.
    fn new(ui: &Ui, width: f32) -> Self {
        let font = FontId::monospace(11.0);
        let glyph = ui
            .painter()
            .layout_no_wrap("0".to_owned(), font, Color32::WHITE)
            .size();
        Self {
            width: width.max(1.0),
            line_height: glyph.y.max(1.0),
            chars_per_row: (width / glyph.x.max(1.0)).max(1.0),
        }
    }

    /// Rows the message's preview needs once wrapped.
    fn rows(&self, message: &Message) -> usize {
        let wrapped = (message.display_chars as f32 / self.chars_per_row).ceil() as usize;
        wrapped.max(message.display_lines).max(1)
    }

    /// Height of the payload window: the whole text, up to the reference's
    /// `.mqtt-history-payload { max-height: 120px }`, which then scrolls.
    fn payload_height(&self, message: &Message) -> f32 {
        (self.rows(message) as f32 * self.line_height).min(PAYLOAD_MAX_HEIGHT)
    }

    /// Whether the payload is taller than its window and needs its own scroll.
    fn scrolls(&self, message: &Message) -> bool {
        self.rows(message) as f32 * self.line_height > PAYLOAD_MAX_HEIGHT + 0.5
    }

    /// Total height of one card, exactly what [`history_item`] paints.
    fn card_height(&self, message: &Message) -> f32 {
        HISTORY_ITEM_PAD_Y * 2.0
            + HISTORY_HEAD_HEIGHT
            + 4.0
            + self.payload_height(message)
            + 2.0
    }
}

/// One `.mqtt-history-item`; returns whether its copy button was pressed.
///
/// The card is painted by hand so its height is known up front: the history can
/// hold a hundred messages of any size, and laying all of them out every frame is
/// what made selecting a topic with data crawl. The payload is shown whole
/// (bounded by [`PREVIEW_CHARS`]) in a `max-height: 120px` window that scrolls,
/// exactly like `.mqtt-history-payload`.
fn history_item(
    ui: &mut Ui,
    palette: &Palette,
    token: &str,
    message: &Message,
    copied: bool,
    metrics: PayloadMetrics,
) -> bool {
    let payload = &message.display;
    let (full, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), metrics.card_height(message)),
        Sense::hover(),
    );
    // `.mqtt-history { padding: 0 var(--gap) }` insets the card itself.
    let rect = Rect::from_min_max(
        egui::pos2(full.left() + PANE_GAP, full.top()),
        egui::pos2(full.right() - PANE_GAP, full.bottom()),
    );
    let radius = egui::CornerRadius::same(wassh_core::RADIUS as u8);
    ui.painter().rect(
        rect,
        radius,
        to_color32(palette.bg_input),
        Stroke::new(1.0, to_color32(palette.border).gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );

    let inner = Rect::from_min_max(
        egui::pos2(rect.left() + HISTORY_ITEM_PAD, rect.top() + HISTORY_ITEM_PAD_Y),
        egui::pos2(rect.right() - HISTORY_ITEM_PAD, rect.bottom() - HISTORY_ITEM_PAD_Y),
    );

    // `.mqtt-history-head`: time, QoS, retain, binary, then the copy button.
    let mut x = inner.left();
    let mut head = |text: String, colour: wassh_core::Rgba| {
        let galley = text_galley(ui, &text, 10.0, colour, 0.0);
        ui.painter().galley(
            egui::pos2(x, inner.top() + (HISTORY_HEAD_HEIGHT - galley.size().y) / 2.0),
            galley.clone(),
            to_color32(colour),
        );
        x += galley.size().x + PANE_GAP;
    };
    head(clock_time(message.at_ms), palette.text_muted);
    head(format!("QoS {}", message.qos), palette.text_muted);
    if message.retain {
        head("retain".to_owned(), palette.accent);
    }
    if message.binary {
        head("binary".to_owned(), palette.text_muted);
    }

    let copy_rect = Rect::from_min_size(
        egui::pos2(inner.right() - COPY_BUTTON, inner.top() + (HISTORY_HEAD_HEIGHT - COPY_BUTTON) / 2.0),
        Vec2::splat(COPY_BUTTON),
    );
    let mut copy = false;
    if copy_button_in(ui, palette, copy_rect, token, copied) {
        ui.ctx().copy_text(payload.clone());
        copy = true;
    }

    // Payload: `white-space: pre-wrap; word-break: break-word`, in a window that
    // scrolls once it is taller than `max-height`.
    let mut job = egui::text::LayoutJob::simple(
        message.preview.clone(),
        FontId::monospace(11.0),
        to_color32(if message.binary {
            palette.text_muted
        } else {
            palette.text
        }),
        metrics.width,
    );
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    let payload_top = inner.top() + HISTORY_HEAD_HEIGHT + 4.0;
    let payload_rect = Rect::from_min_max(
        egui::pos2(inner.left(), payload_top),
        egui::pos2(inner.right(), rect.bottom() - HISTORY_ITEM_PAD_Y),
    );
    if metrics.scrolls(message) {
        // Only the handful of cards on screen ever get here.
        let mut scrolled = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(payload_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        scrolled.set_clip_rect(payload_rect.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical()
            .id_salt(("mqtt-payload", token))
            .max_height(payload_rect.height())
            .auto_shrink([false, false])
            .show(&mut scrolled, |ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(message.preview.as_str())
                        .font(FontId::monospace(11.0))
                        .color(to_color32(if message.binary {
                            palette.text_muted
                        } else {
                            palette.text
                        })),
                )
                .wrap());
            });
    } else {
        ui.painter()
            .with_clip_rect(payload_rect)
            .galley(payload_rect.min, galley, to_color32(palette.text));
    }
    copy
}

/// A copy button the caller positions itself.
fn copy_button_in(
    ui: &mut Ui,
    palette: &Palette,
    rect: Rect,
    token: &str,
    copied: bool,
) -> bool {
    let response = ui.interact(rect, ui.scope_id().with(("mqtt-copy", token)), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let colour = if copied {
        to_color32(palette.accent)
    } else if response.hovered() {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if copied { COPIED_GLYPH } else { COPY_GLYPH },
        FontId::proportional(11.0),
        colour,
    );
    response.clicked()
}

/// What the pointer did to a saved-message card.
#[derive(Clone, Copy, Debug, Default)]
struct CardResponse {
    clicked: bool,
    secondary_clicked: bool,
}

/// One `.mqtt-msg-row` card.
fn saved_card(
    ui: &mut Ui,
    palette: &Palette,
    index: usize,
    message: &SavedMessage,
    rect: Rect,
) -> CardResponse {
    let response = ui.interact(rect, ui.scope_id().with(("mqtt-saved", index)), Sense::click());
    let hovered = response.hovered();
    let radius = egui::CornerRadius::same(wassh_core::RADIUS as u8);
    let fill = if hovered {
        to_color32(wassh_core::colors::tint(
            palette.accent,
            palette.bg_panel_2,
            0.08,
        ))
    } else {
        to_color32(palette.bg_panel_2)
    };
    let border = if hovered {
        palette.accent
    } else {
        palette.border
    };
    ui.painter().rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0, to_color32(border)),
        egui::StrokeKind::Inside,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let inner = rect.shrink2(Vec2::new(7.0, 4.0));
    let label = if message.label.is_empty() {
        "Untitled"
    } else {
        message.label.as_str()
    };
    let mut job = egui::text::LayoutJob::simple_singleline(
        label.to_owned(),
        FontId::proportional(12.0),
        to_color32(palette.text),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
    let label_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        inner.min,
        label_galley.clone(),
        to_color32(palette.text),
    );
    let mut job = egui::text::LayoutJob::simple_singleline(
        message.topic.clone(),
        FontId::monospace(11.0),
        to_color32(palette.text_muted),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
    let topic_galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(inner.left(), inner.min.y + label_galley.size().y + 1.0),
        topic_galley,
        to_color32(palette.text_muted),
    );
    CardResponse {
        clicked: response.clicked(),
        secondary_clicked: response.secondary_clicked(),
    }
}

/// `.mqtt-publish-modes button`.
fn mode_chip(ui: &mut Ui, palette: &Palette, mode: PublishMode, active: bool) -> bool {
    chip_button(ui, palette, mode.label(), active)
}

/// The same chip for a saved message's two modes.
fn saved_mode_chip(ui: &mut Ui, palette: &Palette, mode: SavedMode, active: bool) -> bool {
    chip_button(ui, palette, mode.label(), active)
}

/// `.mqtt-publish-modes button { font-size: 11px; padding: 2px 8px }`, with
/// `button.active { border-color: accent; background: accent 18% over
/// bg-panel-2 }`.
fn chip_button(ui: &mut Ui, palette: &Palette, label: &str, active: bool) -> bool {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(11.0),
        to_color32(palette.text),
    );
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 16.0, 20.0),
        Sense::click(),
    );
    let border = if active {
        palette.accent
    } else if response.hovered() {
        palette.text_muted
    } else {
        palette.border
    };
    let fill = if active {
        wassh_core::colors::tint(palette.accent, palette.bg_panel_2, 0.18)
    } else if response.hovered() {
        palette.bg_panel_2
    } else {
        palette.bg_panel
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(wassh_core::RADIUS as u8),
        to_color32(fill),
        Stroke::new(1.0, to_color32(border)),
        egui::StrokeKind::Inside,
    );
    let colour = if active {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        colour,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// `.mqtt-msg-row { flex: 1 1 160px }` — how many cards fit in `width`.
fn saved_columns(width: f32) -> usize {
    let usable = width.max(SAVED_CARD_MIN);
    (((usable + SAVED_CARD_GAP) / (SAVED_CARD_MIN + SAVED_CARD_GAP)).floor() as usize).max(1)
}

/// A file chosen for a raw publish: its name and its bytes.
type ChosenFile = Option<(String, Vec<u8>)>;

/// A native file dialog running on its own thread.
type PickedFile = std::sync::mpsc::Receiver<ChosenFile>;

/// Pick a file for a raw publish; `None` when the dialog was cancelled.
fn pick_payload_file() -> ChosenFile {
    let path = rfd::FileDialog::new().pick_file()?;
    let bytes = std::fs::read(&path).ok()?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some((name, bytes))
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    use super::*;

    fn entry(received: u64) -> TopicEntry {
        TopicEntry {
            received,
            history: VecDeque::new(),
        }
    }

    fn topics(pairs: &[(&str, u64)]) -> BTreeMap<String, TopicEntry> {
        pairs
            .iter()
            .map(|(topic, count)| ((*topic).to_owned(), entry(*count)))
            .collect()
    }

    #[test]
    fn the_broker_comes_from_the_host_settings_with_the_reference_defaults() {
        let mut settings = serde_json::Map::new();
        assert_eq!(broker_from_settings(&settings, None), Broker::default());

        settings.insert(HOST_SETTING.to_owned(), json!("broker.local"));
        settings.insert(PORT_SETTING.to_owned(), json!(1884));
        settings.insert(USERNAME_SETTING.to_owned(), json!(" user "));
        // A password left in the settings map is ignored: only the vault value
        // counts, so an old plain-text copy cannot be used.
        settings.insert(PASSWORD_SETTING.to_owned(), json!("plain-text"));
        let broker = broker_from_settings(&settings, Some("secret"));
        assert_eq!(broker.host, "broker.local");
        assert_eq!(broker.port, 1884);
        assert_eq!(broker.username, "user");
        assert_eq!(broker.password, "secret");

        // A nonsense port falls back rather than connecting somewhere random.
        settings.insert(PORT_SETTING.to_owned(), json!("nope"));
        assert_eq!(broker_from_settings(&settings, None).port, DEFAULT_PORT);
        settings.insert(PORT_SETTING.to_owned(), json!(0));
        assert_eq!(broker_from_settings(&settings, None).port, DEFAULT_PORT);
        // As does a blank host.
        settings.insert(HOST_SETTING.to_owned(), json!("   "));
        assert_eq!(broker_from_settings(&settings, None).host, DEFAULT_HOST);
    }

    #[test]
    fn topics_become_a_tree_with_counts_rolled_up() {
        let heard = topics(&[("sensors/kitchen/temp", 3), ("sensors/kitchen/hum", 2), ("home/light", 1)]);
        let lines = tree_lines(&heard, &BTreeSet::new());
        let labels: Vec<&str> = lines.iter().map(|line| line.label.as_str()).collect();
        // Only the roots are open at first.
        assert_eq!(labels, ["home", "sensors"]);
        assert!(lines[0].expandable);
        assert_eq!(lines[1].count, 5, "a parent counts everything under it");
        assert_eq!(lines[0].count, 1);

        // Opening `sensors` shows its children, not their children.
        let mut expanded = BTreeSet::new();
        expanded.insert("sensors".to_owned());
        let lines = tree_lines(&heard, &expanded);
        let labels: Vec<&str> = lines.iter().map(|line| line.label.as_str()).collect();
        assert_eq!(labels, ["home", "sensors", "kitchen"]);
        assert_eq!(lines[2].depth, 1);
        assert_eq!(lines[2].count, 5);

        // A leaf is not expandable.
        expanded.insert("sensors/kitchen".to_owned());
        let lines = tree_lines(&heard, &expanded);
        let leaves: Vec<&TreeLine> = lines.iter().filter(|line| line.depth == 2).collect();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().all(|line| !line.expandable));
        // Rows keep the map's order, so the leaves read alphabetically.
        assert_eq!(leaves[0].path, "sensors/kitchen/hum");
        assert_eq!(leaves[1].path, "sensors/kitchen/temp");
        assert_eq!(leaves[1].count, 3);

        // `subTopicCount` counts what heard something below a node, so the bare
        // `kitchen` segment does not count itself.
        let root = lines.iter().find(|line| line.path == "sensors").expect("root");
        assert_eq!(root.sub_topics, 2, "two leaves below sensors");
        let kitchen = lines
            .iter()
            .find(|line| line.path == "sensors/kitchen")
            .expect("kitchen");
        assert_eq!(kitchen.sub_topics, 2);
        // A branch that only heard its leaves shows no badge of its own.
        assert_eq!(kitchen.received, 0);
    }

    #[test]
    fn history_and_counts_are_bounded() {
        let mut entry = TopicEntry::default();
        for index in 0..(HISTORY_LIMIT + 10) {
            push_history(
                &mut entry,
                Message::new("a/b".to_owned(), index.to_string().as_bytes(), index as u64, 0, false),
            );
        }
        assert_eq!(entry.received as usize, HISTORY_LIMIT + 10);
        assert_eq!(entry.history.len(), HISTORY_LIMIT);
        assert_eq!(entry.history.back().map(|m| m.payload.as_str()), Some("109"));
    }

    #[test]
    fn status_labels_match_the_reference() {
        assert_eq!(Status::Idle.label(), "Idle");
        assert_eq!(Status::Connecting.label(), "Connecting…");
        assert_eq!(Status::Connected.label(), "Connected");
        assert_eq!(Status::Disconnected("Connection closed".to_owned()).label(), "Disconnected — Connection closed");
        assert_eq!(Status::Unavailable(String::new()).label(), "SSH only");
        assert_eq!(Status::Failed(ErrorKind::NoBroker, String::new()).label(), "No MQTT broker");
        assert_eq!(Status::Failed(ErrorKind::AuthFailed, String::new()).label(), "Authentication failed");
        assert_eq!(Status::Failed(ErrorKind::NotSsh, String::new()).label(), "SSH only");
        assert_eq!(Status::Failed(ErrorKind::Other, "boom".to_owned()).label(), "boom");
        assert_eq!(Status::Connected.health(), HeaderHealth::Ok);
        assert_eq!(Status::Connecting.health(), HeaderHealth::Warn);
        assert_eq!(Status::Failed(ErrorKind::Other, String::new()).health(), HeaderHealth::Bad);
    }

    #[test]
    fn connection_errors_are_classified_like_the_reference() {
        assert_eq!(
            classify("connecting to 127.0.0.1:1883: Connection refused (os error 111)"),
            ErrorKind::NoBroker
        );
        assert_eq!(classify("broker refused the connection: bad user name"), ErrorKind::AuthFailed);
        assert_eq!(classify("connecting: timed out"), ErrorKind::Unreachable);
        assert_eq!(classify("A broker tunnel needs an SSH session."), ErrorKind::NotSsh);
        assert_eq!(classify("something else"), ErrorKind::Other);
    }

    #[test]
    fn saved_messages_round_trip() {
        let saved = vec![SavedMessage {
            id: "msg-1".to_owned(),
            label: "Restart".to_owned(),
            topic: "cmd/restart".to_owned(),
            payload: "{}".to_owned(),
            mode: SavedMode::Json,
        }];
        let json = saved_to_json(&saved);
        assert_eq!(saved_from_json(Some(&json)), saved);
        assert!(saved_from_json(None).is_empty());
        // A stored list without an id still gets one.
        let loose = json!([{ "label": "x", "topic": "t", "payload": "p" }]);
        let parsed = saved_from_json(Some(&loose));
        assert_eq!(parsed[0].id, "msg-1");
        assert_eq!(parsed[0].mode, SavedMode::Text);
    }

    #[test]
    fn json_payloads_are_validated_and_reindented() {
        assert_eq!(
            payload_for_mode(r#"{"a":1}"#, SavedMode::Json).unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert!(payload_for_mode("{oops", SavedMode::Json).is_err());
        // Text mode sends exactly what was typed.
        assert_eq!(payload_for_mode("{oops", SavedMode::Text).unwrap(), "{oops");
    }

    #[test]
    fn binary_payloads_are_previewed_as_base64() {
        assert!(!is_text(&[0x00, 0x01]));
        assert!(is_text(b"plain text\n"));
        assert_eq!(base64_encode(&[b'a', 0x00, b'\n']), "YQAK");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        // The details pane shows the reference's "(binary, base64) …" preview.
        let message = Message::new("a".to_owned(), &[0x00, 0x01, 0xff], 0, 0, false);
        assert!(message.binary);
        assert_eq!(format_payload(&message), "(binary, base64) AAH/");
        // The formatted text is cached with the message, not rebuilt per frame.
        assert_eq!(message.display, "(binary, base64) AAH/");
        assert_eq!(one_line("a\nb   c"), "a b c");
        let long = "x".repeat(300);
        assert_eq!(one_line(&long).chars().count(), 201);
    }

    #[test]
    fn json_payloads_are_reindented_in_the_history() {
        let message = Message::new("a".to_owned(), br#"{"a":1}"#, 0, 0, false);
        assert_eq!(format_payload(&message), "{\n  \"a\": 1\n}");
        // The formatted text is cached with the message.
        assert_eq!(message.display, "{\n  \"a\": 1\n}");
        // Plain text that is not JSON is shown as received.
        let message = Message::new("a".to_owned(), b"21.5", 0, 0, false);
        assert_eq!(format_payload(&message), "21.5");
    }

    #[test]
    fn the_composer_compacts_json_like_the_reference() {
        assert_eq!(encode_payload(r#"{ "a": 1 }"#, PublishMode::Json).unwrap(), b"{\"a\":1}");
        assert!(encode_payload("{oops", PublishMode::Json).is_err());
        assert_eq!(encode_payload("{oops", PublishMode::Text).unwrap(), b"{oops");
        assert!(encode_payload("", PublishMode::RawFile).is_err());
    }

    /// A session that hands back a fixed forward address.
    #[derive(Default)]
    struct FakeBroker {
        address: Option<String>,
        error: Option<String>,
        asked: Vec<(String, u16)>,
    }

    impl crate::files::SftpAccess for FakeBroker {
        fn request_listing(&mut self, _path: &str) {}
        fn refresh(&mut self, _path: &str) {}
        fn listing(&self) -> Option<&crate::files::SftpListing> {
            None
        }
        fn is_loading(&self) -> bool {
            false
        }
        fn error(&self) -> Option<&str> {
            None
        }
        fn upload(&mut self, _local: &str, _remote: &str) {}
        fn download(&mut self, _remote: &str, _local: &str) {}
        fn is_transferring(&self) -> bool {
            false
        }
        fn transfer_status(&self) -> Option<&str> {
            None
        }
        fn report_problem(&mut self, _message: &str) {}
        fn resolve(&mut self, path: &str) -> Option<String> {
            Some(path.to_owned())
        }
        fn mkdir(&mut self, _path: &str) {}
        fn rename(&mut self, _from: &str, _to: &str) {}
        fn remove(&mut self, _path: &str, _is_dir: bool) {}
        fn chmod(&mut self, _path: &str, _mode: u32) {}
        fn read(&mut self, _path: &str, _max_bytes: u64) -> Option<crate::files::FileContent> {
            None
        }
        fn upload_many(&mut self, _items: Vec<(String, String)>) {}
        fn download_many(&mut self, _items: Vec<(String, String)>) {}
        fn copy_many(&mut self, _items: Vec<(String, String)>, _move_items: bool) {}
        fn progress(&self) -> Option<(u64, Option<u64>)> {
            None
        }
        fn download_tree(&mut self, _items: Vec<String>, _dir: &str) {}
        fn download_zip(&mut self, _items: Vec<String>, _archive: &str) {}
        fn transfers(&self) -> Vec<crate::files::TransferRow> {
            Vec::new()
        }
        fn retry(&mut self) {}
    }

    impl crate::monitor::MonitorAccess for FakeBroker {
        fn request_sample(&mut self) {}
        fn sample(&self) -> Option<&crate::monitor::Snapshot> {
            None
        }
        fn is_sampling(&self) -> bool {
            false
        }
        fn last_error(&self) -> Option<&str> {
            None
        }
    }

    impl crate::command::CommandAccess for FakeBroker {
        fn exec(&mut self, _key: &str, _command: &str) {}
        fn output(&self, _key: &str) -> Option<&crate::command::CommandOutput> {
            None
        }
        fn failure(&self, _key: &str) -> Option<&str> {
            None
        }
        fn is_running(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::command::TcpAccess for FakeBroker {
        fn open_forward(&mut self, _key: &str, host: &str, port: u16) {
            self.asked.push((host.to_owned(), port));
        }
        fn forward_address(&self, _key: &str) -> Option<&str> {
            self.address.as_deref()
        }
        fn forward_error(&self, _key: &str) -> Option<&str> {
            self.error.as_deref()
        }
        fn forward_pending(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::plugin::TerminalAccess for FakeBroker {
        fn screen_text(&self) -> String {
            String::new()
        }
        fn type_text(&mut self, _text: &str) {}
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// Draw two frames and return the painted text.
    fn draw(
        plugin: &mut MqttAnalyser,
        access: &mut FakeBroker,
        settings: &mut serde_json::Map<String, Value>,
        status: SessionStatus,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let palette = wassh_core::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(560.0, 760.0),
            )),
            ..Default::default()
        };
        let mut painted = Vec::new();
        for _ in 0..2 {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status,
                settings,
                scope: "host-1".to_owned(),
                session: Some(access),
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            let shapes = std::mem::take(&mut output.shapes);
            painted.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    painted.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        painted
    }

    /// One frame of the pane inside `ctx`, returning every painted text with
    /// its rect plus the rects filled in the border colour (the splitter).
    fn frame_shapes(
        ctx: &egui::Context,
        plugin: &mut MqttAnalyser,
        access: &mut FakeBroker,
        settings: &mut serde_json::Map<String, Value>,
        status: SessionStatus,
        screen: egui::Rect,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>) {
        let palette = wassh_core::Palette::dark();
        let border = to_color32(palette.border);
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let mut texts = Vec::new();
        let mut rects = Vec::new();
        {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status,
                settings,
                scope: "host-1".to_owned(),
                session: Some(access),
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input, |ui| plugin.show(ui, &mut context));
            for clipped in std::mem::take(&mut output.shapes) {
                match clipped.shape {
                    egui::Shape::Text(text) => texts.push((
                        text.galley.text().to_owned(),
                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    egui::Shape::Rect(rect) if rect.fill == border => rects.push(rect.rect),
                    _ => {}
                }
            }
            output.drop_without_applying_deltas();
        }
        (texts, rects)
    }

    /// Draw two frames at `size` and return (text, rect) pairs.
    fn draw_texts(
        plugin: &mut MqttAnalyser,
        size: egui::Vec2,
    ) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut texts = Vec::new();
        for _ in 0..2 {
            (texts, _) = frame_shapes(
                &ctx,
                plugin,
                &mut access,
                &mut settings,
                SessionStatus::Disconnected,
                screen,
                Vec::new(),
            );
        }
        texts
    }

    #[test]
    fn topics_are_listed_tightly_like_the_reference() {
        let mut plugin = MqttAnalyser {
            topics: topics(&[("a/one", 1), ("a/two", 1), ("a/three", 1)]),
            ..MqttAnalyser::default()
        };
        plugin.expanded.insert("a".to_owned());
        let texts = draw_texts(&mut plugin, egui::Vec2::new(900.0, 700.0));
        let row = |label: &str| {
            texts
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{label} not painted: {texts:?}"))
        };
        let root = row("a");
        // Painted top to bottom; the tree sorts siblings by name.
        let mut rows: Vec<egui::Rect> = ["one", "three", "two"]
            .iter()
            .map(|label| row(label))
            .collect();
        rows.push(root);
        rows.sort_by(|a, b| a.top().total_cmp(&b.top()));
        // `.mqtt-tree-row` is 3 px + a line + 3 px with no margin between rows.
        for pair in rows.windows(2) {
            let gap = pair[1].top() - pair[0].top();
            assert!(
                (gap - TOPIC_ROW_HEIGHT).abs() < 0.6,
                "rows are {gap} apart, expected {TOPIC_ROW_HEIGHT}"
            );
        }
        // Children are indented by `--mqtt-tree-indent`.
        assert!((row("one").left() - root.left() - TOPIC_INDENT).abs() < 0.6);
    }


    #[test]
    fn a_payload_is_rendered_whole_up_to_four_kilobytes() {
        // The formatted text is kept whole (the copy button copies all of it),
        // but a card is only ever painted from a short preview.
        let blob = format!("{{\"values\":[{}]}}", "1,".repeat(20_000));
        let message = Message::new("a/b".to_owned(), blob.as_bytes(), 0, 0, false);
        assert!(message.display_chars > PREVIEW_CHARS);
        // 4 KB of payload, plus the ellipsis that marks what was cut.
        assert_eq!(message.preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(message.preview.ends_with('\u{2026}'));
        // Everything under the cap is shown in full, nothing cropped.
        let under = "x".repeat(PREVIEW_CHARS);
        let message = Message::new("a/b".to_owned(), under.as_bytes(), 0, 0, false);
        assert_eq!(message.preview, under);
        assert_eq!(message.preview.chars().count(), PREVIEW_CHARS);
        // Short payloads are not touched.
        let short = Message::new("a/b".to_owned(), b"21.5", 0, 0, false);
        assert_eq!(short.preview, "21.5");
        // A card's height is bounded by the reference's payload window.
        let ctx = egui::Context::default();
        let mut metrics = None;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(400.0, 700.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            metrics = Some(PayloadMetrics::new(ui, 320.0));
        });
        output.drop_without_applying_deltas();
        let metrics = metrics.expect("metrics");
        let tall = Message::new("a/b".to_owned(), blob.as_bytes(), 0, 0, false);
        let height = metrics.card_height(&tall);
        assert!(
            height < PAYLOAD_MAX_HEIGHT + 40.0,
            "a huge payload made a {height} px card"
        );
    }

    #[test]
    fn the_history_paints_its_cards() {
        let mut plugin = MqttAnalyser::default();
        let topic = "plant/line".to_owned();
        let entry = plugin.topics.entry(topic.clone()).or_default();
        push_history(entry, Message::new(topic.clone(), b"{\"a\":1}", 0, 0, true));
        plugin.selected = Some(topic);
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let painted = draw(&mut plugin, &mut access, &mut settings, SessionStatus::Disconnected);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"QoS 0"), "head missing: {texts:?}");
        assert!(texts.contains(&"retain"), "retain badge missing: {texts:?}");
        // The payload is shown re-indented, char by char per row.
        assert!(
            texts.iter().any(|text| text.contains("\"a\"")),
            "payload missing: {texts:?}"
        );
        assert!(texts.contains(&"1 in history"));
    }

    /// Frame time of the details pane alone, for a topic holding `count`
    /// messages of roughly `bytes` bytes each.
    fn details_frame_ms(count: u64, bytes: usize) -> f64 {
        let mut plugin = MqttAnalyser::default();
        let topic = "plant/line/sensor".to_owned();
        let entry = plugin.topics.entry(topic.clone()).or_default();
        let mut payload = String::from("{\"values\":[");
        while payload.len() < bytes {
            payload.push_str("{\"v\":21.5,\"t\":1},");
        }
        payload.push_str("{}]}");
        for at in 0..count {
            push_history(
                entry,
                Message::new(topic.clone(), payload.as_bytes(), at, 0, at % 2 == 0),
            );
        }
        plugin.selected = Some(topic);
        plugin.expanded.insert("plant".to_owned());
        plugin.expanded.insert("plant/line".to_owned());

        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let palette = wassh_core::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(420.0, 700.0),
            )),
            ..Default::default()
        };
        let mut paint = |plugin: &mut MqttAnalyser| {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status: SessionStatus::Disconnected,
                settings: &mut settings,
                scope: "host-1".to_owned(),
                session: Some(&mut access),
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            output.drop_without_applying_deltas();
        };
        for _ in 0..3 {
            paint(&mut plugin);
        }
        const FRAMES: u32 = 20;
        let start = std::time::Instant::now();
        for _ in 0..FRAMES {
            paint(&mut plugin);
        }
        start.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
    }

    #[test]
    fn the_details_pane_scales_with_the_history() {
        let mut worst: f64 = 0.0;
        for (count, bytes) in [
            (10u64, 200usize),
            (100, 200),
            (100, 4_000),
            (100, 40_000),
            (100, 100_000),
        ] {
            let ms = details_frame_ms(count, bytes);
            println!("{count:>3} messages x {bytes:>6} bytes: {ms:>8.2} ms");
            worst = worst.max(ms);
        }
        assert!(worst < 8.0, "the details pane takes {worst:.2} ms per frame");
    }

    /// Frame time with a busy tree — the pane must stay interactive when a lot
    /// of topics are open.
    #[test]
    fn a_busy_tree_stays_fast() {
        let mut plugin = MqttAnalyser::default();
        // 300 topics over 30 roots, with 40 JSON messages on the selected one.
        for index in 0..300u64 {
            let topic = format!("plant{}/line{}/sensor{}", index % 30, index % 7, index);
            let entry = plugin.topics.entry(topic.clone()).or_default();
            for at in 0..6u64 {
                push_history(
                    entry,
                    Message::new(
                        topic.clone(),
                        format!("{{\"value\":{at},\"unit\":\"celsius\"}}").as_bytes(),
                        at,
                        0,
                        false,
                    ),
                );
            }
            let mut expanded = topic.clone();
            while let Some(cut) = expanded.rfind('/') {
                expanded.truncate(cut);
                plugin.expanded.insert(expanded.clone());
            }
        }
        let selected = "plant0/line0/sensor0".to_owned();
        let entry = plugin.topics.entry(selected.clone()).or_default();
        for at in 0..40u64 {
            push_history(
                entry,
                Message::new(
                    selected.clone(),
                    format!("{{\"value\":{at},\"unit\":\"celsius\"}}").as_bytes(),
                    at,
                    0,
                    at % 3 == 0,
                ),
            );
        }
        plugin.selected = Some(selected);
        plugin.collapsed = false;
        for index in 0..3 {
            plugin.saved.push(SavedMessage {
                id: format!("msg-{index}"),
                label: format!("Message {index}"),
                topic: "plant0/line0/command".to_owned(),
                payload: "on".to_owned(),
                mode: SavedMode::Text,
            });
        }

        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0));
        let palette = wassh_core::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut paint = |plugin: &mut MqttAnalyser| {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status: SessionStatus::Disconnected,
                settings: &mut settings,
                scope: "host-1".to_owned(),
                session: Some(&mut access),
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            output.drop_without_applying_deltas();
        };
        // Warm the font atlas and the scroll-area state.
        for _ in 0..3 {
            paint(&mut plugin);
        }
        let start = std::time::Instant::now();
        const FRAMES: u32 = 20;
        for _ in 0..FRAMES {
            paint(&mut plugin);
        }
        let per_frame = start.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        println!("busy MQTT frame: {per_frame:.2} ms");

        assert!(
            per_frame < 16.0,
            "a busy tree takes {per_frame:.2} ms per frame"
        );
    }

    #[test]
    fn a_4px_divider_splits_topics_and_details() {
        let mut plugin = MqttAnalyser {
            topics: topics(&[("a/one", 1)]),
            ..MqttAnalyser::default()
        };
        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0));
        let mut texts = Vec::new();
        let mut rects = Vec::new();
        for _ in 0..2 {
            (texts, rects) = frame_shapes(
                &ctx,
                &mut plugin,
                &mut access,
                &mut settings,
                SessionStatus::Disconnected,
                screen,
                Vec::new(),
            );
        }
        let splitter = rects
            .iter()
            .find(|rect| (rect.width() - SPLITTER_WIDTH).abs() < 0.1 && rect.height() > 200.0)
            .unwrap_or_else(|| panic!("no splitter in {rects:?}"));
        let at = |label: &str| {
            texts
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{label} not painted"))
        };
        assert!(at("TOPICS").left() < splitter.left());
        assert!(at("DETAILS").left() > splitter.right());
        // 42 % of the width, `MQTT_SPLIT_DEFAULT_RATIO`.
        let expected = (900.0 - SPLITTER_WIDTH) * SPLIT_DEFAULT_RATIO;
        assert!(
            (splitter.left() - expected).abs() < 2.0,
            "divider at {}, expected near {expected}",
            splitter.left()
        );
    }

    #[test]
    fn dragging_the_divider_resizes_both_panes() {
        let mut plugin = MqttAnalyser {
            topics: topics(&[("a/one", 1)]),
            ..MqttAnalyser::default()
        };
        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 700.0));
        let start = (900.0 - SPLITTER_WIDTH) * SPLIT_DEFAULT_RATIO + SPLITTER_WIDTH / 2.0;
        let at = |x: f32| egui::pos2(x, 300.0);
        let move_to = |x: f32| egui::Event::PointerMoved(at(x));
        let button = |pressed: bool, x: f32| egui::Event::PointerButton {
            pos: at(x),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let frames: Vec<Vec<egui::Event>> = vec![
            vec![move_to(start)],
            vec![button(true, start)],
            vec![move_to(start + 90.0)],
            vec![button(false, start + 90.0)],
        ];
        for events in frames {
            let _ = frame_shapes(
                &ctx,
                &mut plugin,
                &mut access,
                &mut settings,
                SessionStatus::Disconnected,
                screen,
                events,
            );
        }
        assert!(
            plugin.tree_ratio > SPLIT_DEFAULT_RATIO + 0.05,
            "the divider did not move: {}",
            plugin.tree_ratio
        );
        // Never outside the reference's limits.
        assert!(plugin.tree_ratio <= SPLIT_MAX_RATIO);
    }

    #[test]
    fn the_header_offers_reconnect_only_while_the_broker_is_down() {
        let reconnect = Some(HeaderButton {
            label: "Reconnect",
            enabled: true,
        });
        let mut plugin = MqttAnalyser::default();
        // Idle: nothing to retry yet, so no button.
        assert_eq!(plugin.header_action(), None);
        plugin.status = Status::Connecting;
        assert_eq!(plugin.header_action(), None);
        plugin.status = Status::Connected;
        assert_eq!(plugin.header_action(), None);
        plugin.status = Status::Disconnected("Connection closed".to_owned());
        assert_eq!(plugin.header_action(), reconnect);
        plugin.status = Status::Failed(ErrorKind::NoBroker, String::new());
        assert_eq!(plugin.header_action(), reconnect);
        // There is nothing to retry without an SSH session.
        plugin.status = Status::Unavailable("not ssh".to_owned());
        assert_eq!(plugin.header_action(), None);
        plugin.status = Status::Failed(ErrorKind::NotSsh, String::new());
        assert_eq!(plugin.header_action(), None);

        // Clicking it starts a fresh connection.
        plugin.status = Status::Failed(ErrorKind::NoBroker, String::new());
        plugin.header_action_clicked();
        assert_eq!(plugin.status, Status::Connecting);
    }

    #[test]
    fn the_pane_asks_the_session_for_the_broker_and_shows_no_connection_fields() {
        let mut plugin = MqttAnalyser::default();
        let mut settings = serde_json::Map::new();
        settings.insert(HOST_SETTING.to_owned(), json!("broker.internal"));
        settings.insert(PORT_SETTING.to_owned(), json!(1884));
        let mut access = FakeBroker {
            address: None,
            error: None,
            asked: Vec::new(),
        };
        let painted = draw(&mut plugin, &mut access, &mut settings, SessionStatus::Connected);
        // The forward is asked for the *configured* broker, through the session.
        assert_eq!(access.asked, [("broker.internal".to_owned(), 1884)]);
        assert!(matches!(plugin.status(), Status::Connecting));
        // The connection state lives in the pane header, not in a status bar.
        assert_eq!(plugin.header_status().map(|(text, _)| text), Some("Connecting…".to_owned()));
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"TOPICS"), "tree label missing: {texts:?}");
        assert!(texts.contains(&"DETAILS"), "detail label missing: {texts:?}");
        assert!(texts.contains(&"PUBLISH"), "publish label missing: {texts:?}");
        // No broker host/port/topic fields in the panel.
        assert!(!texts.contains(&"broker host"), "configuration leaked: {texts:?}");
        assert!(!texts.contains(&"port"), "configuration leaked: {texts:?}");
    }

    #[test]
    fn a_refused_connection_reads_as_no_broker() {
        let mut plugin = MqttAnalyser::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker {
            address: None,
            error: Some("connecting to 127.0.0.1:1883: Connection refused (os error 111)".to_owned()),
            asked: Vec::new(),
        };
        draw(&mut plugin, &mut access, &mut settings, SessionStatus::Connected);
        match plugin.status() {
            Status::Failed(kind, reason) => {
                assert_eq!(*kind, ErrorKind::NoBroker);
                assert!(reason.contains("Connection refused"));
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert_eq!(
            plugin.header_status(),
            Some((plugin.status().label(), HeaderHealth::Bad))
        );
    }

    #[test]
    fn the_broker_is_reached_once_the_session_is_connected() {
        let mut plugin = MqttAnalyser::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        // While the tab is still connecting there is nothing to tunnel through.
        draw(&mut plugin, &mut access, &mut settings, SessionStatus::Connecting);
        assert!(access.asked.is_empty(), "the forward was asked for too early");
        assert!(matches!(plugin.status(), Status::Unavailable(_)));

        // The moment the session is up, the pane connects on its own.
        draw(&mut plugin, &mut access, &mut settings, SessionStatus::Connected);
        assert_eq!(access.asked, [("127.0.0.1".to_owned(), DEFAULT_PORT)]);
        assert_eq!(plugin.status(), &Status::Connecting);
    }

    #[test]
    fn a_local_session_says_it_needs_ssh() {
        let mut plugin = MqttAnalyser::default();
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let painted = draw(&mut plugin, &mut access, &mut settings, SessionStatus::Disconnected);
        assert_eq!(
            plugin.status(),
            &Status::Unavailable("MQTT Analyser requires an SSH session".to_owned())
        );
        // The reason is the pane's header caption now.
        assert_eq!(
            plugin.header_status(),
            Some((
                "MQTT Analyser requires an SSH session".to_owned(),
                HeaderHealth::Bad
            ))
        );
        // There is nothing to reconnect to, so no header button is offered.
        assert_eq!(plugin.header_action(), None);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"DETAILS"), "pane missing: {texts:?}");
    }

    #[test]
    fn received_messages_build_the_topic_tree() {
        let mut plugin = MqttAnalyser::default();
        for (topic, count) in [("home/kitchen/temp", 2u64), ("home/light", 1)] {
            let entry = plugin.topics.entry(topic.to_owned()).or_default();
            for index in 0..count {
                push_history(
                    entry,
                    Message::new(topic.to_owned(), b"21.5", index, 0, false),
                );
            }
        }
        let mut settings = serde_json::Map::new();
        let mut access = FakeBroker::default();
        let painted = draw(&mut plugin, &mut access, &mut settings, SessionStatus::Disconnected);
        let texts: Vec<&str> = painted.iter().map(String::as_str).collect();
        assert!(texts.contains(&"home"), "root topic missing: {texts:?}");
        // A collapsed branch shows its rolled-up counts: `kitchen` and `light`
        // heard something, and the three messages sit under them.
        assert!(
            texts.contains(&"2 topics, 3 msgs"),
            "rolled-up count missing: {texts:?}"
        );
    }
}
