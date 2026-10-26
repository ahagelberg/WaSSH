//! Schema-driven plugin settings.
//!
//! Mirrors `contributes.settingsSchema` and `contributes.hostSettingsSchema` in
//! the Electron manifests: every plugin declares which fields Options shows
//! app-wide and which ones Host settings shows per host, so both dialogs can be
//! drawn from one description instead of hand-written rows.

use serde_json::{Map, Value};

use crate::PluginId;

/// How a settings field is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Boolean,
    Number,
    String,
    TextArea,
    Directory,
    Select,
    StringList,
    /// A list of records; every record is edited with the fields in `item_schema`.
    ItemList,
    /// A boolean master toggle whose `children` are disabled while it is off.
    Group,
    Permission,
}

/// Type of a field's default, so schemas stay `const`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldDefault {
    Bool(bool),
    Number(i64),
    Text(&'static str),
    TextList(&'static [&'static str]),
}

impl FieldDefault {
    pub fn to_json(self) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::from(value),
            Self::Text(value) => Value::from(value),
            Self::TextList(items) => {
                Value::Array(items.iter().map(|item| Value::from(*item)).collect())
            }
        }
    }
}

/// One option in a `Select` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub value: &'static str,
    pub label: &'static str,
}

/// One row (or group) in a plugin settings section.
#[derive(Clone, Copy, Debug)]
pub struct SettingsField {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub default: FieldDefault,
    /// Muted second line under the label, like `.settings-row-label span`.
    pub description: &'static str,
    /// Mask the text field.
    pub secret: bool,
    pub options: &'static [SelectOption],
    /// Child rows of a `Group` field.
    pub children: &'static [SettingsField],
    /// Fields of one record in an `ItemList` field.
    pub item_schema: &'static [SettingsField],
    /// Singular noun of one record ("button", "message"), used by the add button.
    pub item_label: &'static str,
}

impl SettingsField {
    pub fn default_value(&self) -> Value {
        self.default.to_json()
    }
}

/// A boolean row.
pub const fn boolean(
    key: &'static str,
    label: &'static str,
    default: bool,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        key,
        label,
        kind: FieldKind::Boolean,
        default: FieldDefault::Bool(default),
        description,
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    }
}

/// A numeric row (rendered as a drag value).
pub const fn number(
    key: &'static str,
    label: &'static str,
    default: i64,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        key,
        label,
        kind: FieldKind::Number,
        default: FieldDefault::Number(default),
        description,
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    }
}

/// A single-line text row.
pub const fn text(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        key,
        label,
        kind: FieldKind::String,
        default: FieldDefault::Text(default),
        description,
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    }
}

/// A single-line text row whose value is masked.
pub const fn secret_text(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        secret: true,
        ..text(key, label, default, description)
    }
}

/// A multi-line text row.
pub const fn text_area(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        kind: FieldKind::TextArea,
        ..text(key, label, default, description)
    }
}

/// A path row.
pub const fn directory(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    description: &'static str,
) -> SettingsField {
    SettingsField {
        kind: FieldKind::Directory,
        ..text(key, label, default, description)
    }
}

/// A list of strings, edited one per line.
pub const fn string_list(
    key: &'static str,
    label: &'static str,
    default: &'static [&'static str],
    description: &'static str,
) -> SettingsField {
    SettingsField {
        key,
        label,
        kind: FieldKind::StringList,
        default: FieldDefault::TextList(default),
        description,
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    }
}

/// A list of records, each edited with the fields in `item_schema`.
pub const fn item_list(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    item_label: &'static str,
    item_schema: &'static [SettingsField],
) -> SettingsField {
    SettingsField {
        key,
        label,
        kind: FieldKind::ItemList,
        default: FieldDefault::TextList(&[]),
        description,
        secret: false,
        options: &[],
        children: &[],
        item_schema,
        item_label,
    }
}

/// A boolean master toggle with child rows.
pub const fn group(
    key: &'static str,
    label: &'static str,
    default: bool,
    description: &'static str,
    children: &'static [SettingsField],
) -> SettingsField {
    SettingsField {
        kind: FieldKind::Group,
        children,
        ..boolean(key, label, default, description)
    }
}

// --- the schemas, copied from the Electron manifests ----------------------

/// Command patterns the AI agent may run without asking, copied from the
/// original `AI_AGENT_SAFE_RULES`.
const AI_AGENT_SAFE_RULES: &[&str] = &[
    "pwd",
    "whoami",
    "id*",
    "hostname*",
    "uname*",
    "date",
    "uptime",
    "ls*",
    "lscpu*",
    "free*",
    "df*",
    "ps*",
    "git status*",
    "git log*",
    "git diff*",
    "git branch*",
    "git remote*",
    "git show*",
    "git rev-parse*",
];

const AI_AGENT_SEARCH_PROVIDERS: &[SelectOption] = &[
    SelectOption {
        value: "bing",
        label: "Bing (Default)",
    },
    SelectOption {
        value: "brave",
        label: "Brave Search",
    },
    SelectOption {
        value: "google",
        label: "Google Search",
    },
    SelectOption {
        value: "duckduckgo",
        label: "DuckDuckGo",
    },
    SelectOption {
        value: "custom",
        label: "Custom Endpoint",
    },
];

const AI_AGENT_KNOWLEDGE_CHILDREN: &[SettingsField] = &[
    directory(
        "ragFolder",
        "Folder",
        "",
        "Local directory containing documents to index and search.",
    ),
    SettingsField {
        key: "ragProviderId",
        label: "Embedding provider id",
        kind: FieldKind::String,
        default: FieldDefault::Text(""),
        description: "Optional. Id of one of the providers above (for example \u{201c}openai\u{201d}), to enable semantic knowledge-base search.",
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    },
    SettingsField {
        key: "ragEmbeddingModel",
        label: "Embedding model",
        kind: FieldKind::String,
        default: FieldDefault::Text(""),
        description: "Optional. Embedding model that provider offers; without it, text matching is used.",
        secret: false,
        options: &[],
        children: &[],
        item_schema: &[],
        item_label: "",
    },
];

const AI_AGENT_SEARCH_CHILDREN: &[SettingsField] = &[
    SettingsField {
        key: "webSearchProvider",
        label: "Web search provider",
        kind: FieldKind::Select,
        default: FieldDefault::Text("bing"),
        description: "Search engine used for web searches.",
        secret: false,
        options: AI_AGENT_SEARCH_PROVIDERS,
        children: &[],
        item_schema: &[],
        item_label: "",
    },
    secret_text(
        "webSearchApiKey",
        "Web search API key",
        "",
        "Optional API key for the search engine provider.",
    ),
];

const AI_AGENT_APP: &[SettingsField] = &[
    text_area(
        "globalPrompt",
        "Global prompt text",
        "",
        "Text included in every AI agent prompt.",
    ),
    group(
        "globalKnowledgeGroup",
        "Global knowledge",
        true,
        "Knowledge base folder and embedding model settings for vector search.",
        AI_AGENT_KNOWLEDGE_CHILDREN,
    ),
    string_list(
        "defaultAllowRules",
        "Default allow rules",
        AI_AGENT_SAFE_RULES,
        "App-wide command patterns that run without asking (applies to every host). Host allow rules are checked first.",
    ),
    string_list(
        "defaultDenyRules",
        "Default deny rules",
        &[],
        "App-wide command patterns that are always blocked. Deny always wins over allow.",
    ),
    group(
        "webSearchGroup",
        "Web search",
        true,
        "Search engine and API key used by the web search tool.",
        AI_AGENT_SEARCH_CHILDREN,
    ),
];

const AI_AGENT_HOST: &[SettingsField] = &[
    text_area(
        "hostPrompt",
        "Host prompt text",
        "",
        "Text included in AI agent prompts for this host or session.",
    ),
    directory(
        "hostRagFolder",
        "Host knowledge folder",
        "",
        "Additional local folder of documents indexed just for this host, combined with the global knowledge folder.",
    ),
    string_list(
        "allowRules",
        "Allow rules",
        &[],
        "Command patterns for this host that run without asking. One per line; glob by default, prefix with \"regex:\" for a regular expression.",
    ),
    string_list(
        "denyRules",
        "Deny rules",
        &[],
        "Command patterns for this host that are always blocked. Deny always wins over allow.",
    ),
];

const CONNECTION_LOGGER_APP: &[SettingsField] = &[
    boolean(
        "defaultEnabled",
        "Enable connection logging by default",
        true,
        "Record connection and disconnection events for new hosts and sessions.",
    ),
    number(
        "retentionMaxEntries",
        "Default log retention (entries)",
        500,
        "Maximum number of event log entries kept in history per host.",
    ),
];

const CONNECTION_LOGGER_HOST: &[SettingsField] = &[
    boolean(
        "enabled",
        "Enable connection logging",
        true,
        "Record all connection and disconnection events for this host.",
    ),
    number(
        "maxEntries",
        "Max log entries",
        500,
        "Maximum number of connection events to store for this host.",
    ),
];

const DOCKER_APP: &[SettingsField] = &[boolean(
    "confirmDestructive",
    "Confirm destructive actions",
    true,
    "Ask before removing a container, killing it, deleting an image, volume, or network, or pruning.",
)];

const DOCKER_HOST: &[SettingsField] = &[
    text(
        "socketPath",
        "Daemon socket path",
        "/var/run/docker.sock",
        "Path to the Docker daemon socket on this host. Rootless Docker and Docker Desktop use different paths.",
    ),
    boolean(
        "showStopped",
        "Show stopped containers",
        false,
        "Include exited and created containers in the list.",
    ),
    number(
        "refreshIntervalMs",
        "List refresh interval (ms)",
        5000,
        "How often the container list refreshes while the panel is visible.",
    ),
    number(
        "statsIntervalMs",
        "Stats interval (ms)",
        2000,
        "How often live container stats are sampled while the stats tab is open.",
    ),
    number(
        "logTailLines",
        "Log tail lines",
        200,
        "How many historical log lines to load when opening container logs.",
    ),
    secret_text(
        "sudoPassword",
        "sudo password",
        "",
        "Password piped to sudo -S when installing the Docker engine on this host. Leave empty for passwordless sudo.",
    ),
];

const MACRO_ITEM: &[SettingsField] = &[
    text("label", "Label", "", "Caption on the button."),
    text(
        "text",
        "Text to send",
        "",
        "Typed into the terminal when the button is pressed; Enter is pressed for you.",
    ),
    text(
        "group",
        "Group",
        "",
        "Heading the button is listed under; empty for ungrouped.",
    ),
];

const MACRO_PAD_APP: &[SettingsField] = &[
    item_list(
        "macros",
        "Buttons",
        "Label, text to send, and group per button. The Macros panel edits the same list.",
        "button",
        MACRO_ITEM,
    ),
    boolean(
        "ungroupedCollapsed",
        "Collapse ungrouped",
        false,
        "Whether the ungrouped macro section is collapsed.",
    ),
];

const MQTT_MESSAGE_ITEM: &[SettingsField] = &[
    text("label", "Label", "", "Caption shown in the saved-messages list."),
    text("topic", "Topic", "", "MQTT topic the message is published to."),
    SettingsField {
        key: "mode",
        label: "Format",
        kind: FieldKind::Select,
        default: FieldDefault::Text("text"),
        description: "How the payload is treated and validated.",
        secret: false,
        options: MQTT_MODES,
        children: &[],
        item_schema: &[],
        item_label: "",
    },
    text_area("payload", "Payload", "", "Text published when the message is sent."),
];

const MQTT_MODES: &[SelectOption] = &[
    SelectOption {
        value: "text",
        label: "Text",
    },
    SelectOption {
        value: "json",
        label: "JSON",
    },
];

const MQTT_APP: &[SettingsField] = &[
    item_list(
        "messages",
        "Saved messages",
        "Label, topic, format, and payload per saved publish message. The MQTT panel edits the same list.",
        "message",
        MQTT_MESSAGE_ITEM,
    ),
    boolean(
        "messagesCollapsed",
        "Collapse saved messages",
        true,
        "Whether the saved-messages section in the publish panel is collapsed.",
    ),
];

const MQTT_HOST: &[SettingsField] = &[
    text(
        "host",
        "Broker host",
        "127.0.0.1",
        "Broker address as seen from the remote SSH host.",
    ),
    text("username", "MQTT username", "", "Optional MQTT username for this host."),
    secret_text(
        "password",
        "MQTT password",
        "",
        "Optional MQTT password for this host.",
    ),
    number("port", "Server port", 1883, "Plain MQTT port (no TLS)."),
];

const SERVER_MONITOR_HOST: &[SettingsField] = &[
    boolean(
        "showGauges",
        "Show gauges",
        true,
        "CPU, memory, and swap gauges, per-core bars, and memory breakdown.",
    ),
    boolean(
        "showSparks",
        "Show history graphs",
        true,
        "Sparkline history inside CPU, network, and disk panels.",
    ),
    boolean(
        "showStatus",
        "Show status",
        true,
        "OS, uptime, load, temperatures, kernel, and task counts.",
    ),
    boolean(
        "showProcesses",
        "Show process list",
        true,
        "Top processes with state, nice, threads, and sort by CPU/mem.",
    ),
    boolean(
        "showNetwork",
        "Show network",
        true,
        "Network panel with rate history, totals, and per-interface stats.",
    ),
];

/// How a plugin's settings section is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    /// One row per field, drawn inline in the section.
    Fields,
    /// A single "Configure…" row, like the original app's plugins that own a
    /// dedicated settings editor.
    Editor,
}

/// Presentation of a plugin's Options section.
///
/// Only the MQTT analyser keeps its whole schema in a dialog (its saved messages are a
/// record list that needs the room); every other plugin's settings are ordinary rows.
pub fn app_presentation(id: PluginId) -> Presentation {
    match id {
        PluginId::MqttAnalyser => Presentation::Editor,
        _ => Presentation::Fields,
    }
}

/// Whether a plugin's Options section draws a provider list above its settings.
///
/// The AI agent's provider list is app-wide state edited in a list view, so it cannot be
/// a label/control row. It is still part of the section — only adding a single provider
/// opens a dialog, and the plugin's own editor draws that.
pub fn draws_provider_list(id: PluginId) -> bool {
    matches!(id, PluginId::AiAgent)
}

/// Fields Options shows for a plugin, app-wide.
pub fn app_schema(id: PluginId) -> &'static [SettingsField] {
    match id {
        PluginId::AiAgent => AI_AGENT_APP,
        PluginId::ConnectionLogger => CONNECTION_LOGGER_APP,
        PluginId::DockerManager => DOCKER_APP,
        PluginId::MacroPad => MACRO_PAD_APP,
        PluginId::MqttAnalyser => MQTT_APP,
        // Files and Scratchpad are configured in their own panel.
        PluginId::Sftp | PluginId::ServerMonitor | PluginId::Scratchpad => &[],
    }
}

/// Fields Host settings shows for a plugin, per host.
pub fn host_schema(id: PluginId) -> &'static [SettingsField] {
    match id {
        PluginId::ConnectionLogger => CONNECTION_LOGGER_HOST,
        PluginId::DockerManager => DOCKER_HOST,
        PluginId::MqttAnalyser => MQTT_HOST,
        PluginId::ServerMonitor => SERVER_MONITOR_HOST,
        PluginId::AiAgent => AI_AGENT_HOST,
        PluginId::Sftp | PluginId::MacroPad | PluginId::Scratchpad => &[],
    }
}

/// Section title of a plugin's Options section.
pub fn app_heading(id: PluginId) -> &'static str {
    match id {
        PluginId::ConnectionLogger => "Connection logger",
        PluginId::DockerManager => "Docker",
        PluginId::MacroPad => "Macro pad",
        PluginId::MqttAnalyser => "MQTT Analyser",
        PluginId::ServerMonitor => "Server monitor",
        PluginId::AiAgent => "AI agent",
        PluginId::Sftp => "Files",
        PluginId::Scratchpad => "Scratchpad",
    }
}

/// Section title of a plugin's Host settings section.
pub fn host_heading(id: PluginId) -> &'static str {
    match id {
        PluginId::ConnectionLogger => "Connection logger",
        PluginId::DockerManager => "Docker",
        PluginId::MqttAnalyser => "MQTT",
        PluginId::ServerMonitor => "Server monitor",
        PluginId::AiAgent => "AI agent",
        PluginId::Sftp => "Files",
        PluginId::MacroPad => "Macro pad",
        PluginId::Scratchpad => "Scratchpad",
    }
}

/// Every field of a schema, descending into groups (Electron's `flattenFields`).
pub fn flatten(schema: &[SettingsField]) -> Vec<&SettingsField> {
    let mut out = Vec::new();
    for field in schema {
        out.push(field);
        if field.kind == FieldKind::Group {
            out.extend(flatten(field.children));
        }
    }
    out
}

/// The schema defaults of one document kind.
pub fn defaults(schema: &[SettingsField]) -> Map<String, Value> {
    flatten(schema)
        .into_iter()
        .map(|field| (field.key.to_owned(), field.default_value()))
        .collect()
}

/// The value to show for `field`: the stored one, else its default.
pub fn value_of(values: &Map<String, Value>, field: &SettingsField) -> Value {
    values
        .get(field.key)
        .cloned()
        .unwrap_or_else(|| field.default_value())
}

/// Fill `target` with the schema values of `id`.
///
/// Schema defaults only fill keys the pane has not written itself, while a
/// value the user configured in Options or Host settings always wins — that is
/// the same precedence the Electron `mergePluginSettings` chain gives.
pub fn apply_to(
    id: PluginId,
    app_values: Option<&Map<String, Value>>,
    host_values: Option<&Map<String, Value>>,
    target: &mut Map<String, Value>,
) {
    let app_fields = flatten(app_schema(id));
    let host_fields = flatten(host_schema(id));
    // A secret field is stored in the credential vault, never in plugin data.
    for field in app_fields
        .iter()
        .chain(host_fields.iter())
        .filter(|field| !field.secret)
    {
        target
            .entry(field.key.to_owned())
            .or_insert_with(|| field.default_value());
    }
    for field in &app_fields {
        if field.secret {
            continue;
        }
        if let Some(value) = app_values.and_then(|values| values.get(field.key)) {
            target.insert(field.key.to_owned(), value.clone());
        }
    }
    for field in &host_fields {
        if field.secret {
            continue;
        }
        if let Some(value) = host_values.and_then(|values| values.get(field.key)) {
            target.insert(field.key.to_owned(), value.clone());
        }
    }
    // Keys a plugin draws with its own widget have no row, so no field above copies them.
    for key in list_owned_app_keys(id) {
        if let Some(value) = app_values.and_then(|values| values.get(*key)) {
            target.insert((*key).to_owned(), value.clone());
        }
    }
}

/// App-wide keys a plugin edits with its own widget instead of a settings row.
///
/// They are not schema fields — there is no label, so no row — but they are still
/// app-wide settings that Options owns and the pane reads, so [`apply_to`] has to carry
/// them: without this the AI agent never saw the providers configured in Options and kept
/// claiming none were set up.
fn list_owned_app_keys(id: PluginId) -> &'static [&'static str] {
    match id {
        PluginId::AiAgent => &[crate::ai_agent::PROVIDERS_KEY],
        _ => &[],
    }
}

/// Every secret field a plugin declares, with the scope it belongs to.
///
/// `true` marks an app-wide secret (stored under the `app` scope), `false` a
/// per-host one.
pub fn secret_fields(id: PluginId) -> Vec<(&'static str, bool)> {
    let app = flatten(app_schema(id))
        .into_iter()
        .filter(|field| field.secret)
        .map(|field| (field.key, true));
    let host = flatten(host_schema(id))
        .into_iter()
        .filter(|field| field.secret)
        .map(|field| (field.key, false));
    app.chain(host).collect()
}

/// Whether `key` is a secret field of `id`, and whether it is app-wide.
pub fn secret_scope(id: PluginId, key: &str) -> Option<bool> {
    secret_fields(id)
        .into_iter()
        .find(|(field, _)| *field == key)
        .map(|(_, app)| app)
}

/// Schema defaults overlaid with the configured app and host values.
pub fn resolve(
    id: PluginId,
    app_values: Option<&Map<String, Value>>,
    host_values: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut values = Map::new();
    apply_to(id, app_values, host_values, &mut values);
    values
}

/// Key one plugin secret field is filed under in [`PluginSecretDrafts`].
pub fn secret_field_key(plugin: &str, field: &str) -> String {
    format!("{plugin}|{field}")
}

/// A plugin secret field while a settings dialog is open.
///
/// The value never goes into the settings map: the host seals it into the
/// credential vault when the dialog is saved, exactly like a host password.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretDraft {
    /// Text typed in this dialog, empty to keep whatever is stored.
    pub draft: String,
    /// Whether the vault already holds a value.
    pub stored: bool,
    /// The user asked to forget the stored value.
    pub clear: bool,
}

impl SecretDraft {
    /// A draft for a field the vault already holds.
    pub fn stored() -> Self {
        Self {
            stored: true,
            ..Self::default()
        }
    }
}

/// Every plugin secret a dialog is editing, keyed by [`secret_field_key`].
pub type PluginSecretDrafts = std::collections::BTreeMap<String, SecretDraft>;

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn every_schema_key_is_unique_and_labelled() {
        for id in PluginId::ALL {
            for schema in [app_schema(id), host_schema(id)] {
                let fields = flatten(schema);
                let mut keys: Vec<_> = fields.iter().map(|field| field.key).collect();
                let count = keys.len();
                keys.sort_unstable();
                keys.dedup();
                assert_eq!(keys.len(), count, "{} repeats a settings key", id.as_str());
                assert!(
                    fields.iter().all(|field| !field.label.is_empty()),
                    "{} has an unlabelled field",
                    id.as_str()
                );
            }
        }
    }

    #[test]
    fn only_plugins_with_fields_get_a_section() {
        // Options lists a plugin only when it has app-wide fields.
        let with_app: Vec<_> = PluginId::ALL
            .into_iter()
            .filter(|id| !app_schema(*id).is_empty())
            .map(|id| id.as_str())
            .collect();
        assert_eq!(
            with_app,
            [
                "ai-agent",
                "docker-manager",
                "mqtt-analyser",
                "macro-pad",
                "connection-logger"
            ]
        );
        // Host settings do the same for per-host fields.
        let with_host: Vec<_> = PluginId::ALL
            .into_iter()
            .filter(|id| !host_schema(*id).is_empty())
            .map(|id| id.as_str())
            .collect();
        assert_eq!(
            with_host,
            [
                "ai-agent",
                "server-monitor",
                "docker-manager",
                "mqtt-analyser",
                "connection-logger"
            ]
        );
    }

    #[test]
    fn the_plugin_order_matches_the_original_registration() {
        let order: Vec<_> = PluginId::UI_ORDER.iter().map(|id| id.as_str()).collect();
        assert_eq!(
            order,
            [
                "server-monitor",
                "scratchpad",
                "macro-pad",
                "mqtt-analyser",
                "sftp",
                "ai-agent",
                "connection-logger",
                "docker-manager"
            ]
        );
        // Same set as ALL, only reordered.
        let mut sorted = order.clone();
        sorted.sort_unstable();
        let mut all: Vec<_> = PluginId::ALL.iter().map(|id| id.as_str()).collect();
        all.sort_unstable();
        assert_eq!(sorted, all);
    }

    #[test]
    fn only_the_plugins_with_a_dedicated_editor_use_one() {
        let editors: Vec<_> = PluginId::ALL
            .into_iter()
            .filter(|id| app_presentation(*id) == Presentation::Editor)
            .map(|id| id.as_str())
            .collect();
        assert_eq!(editors, ["mqtt-analyser"]);
    }

    #[test]
    fn the_provider_list_reaches_the_pane_from_the_app_settings() {
        // The provider list has no schema row (the AI agent draws it itself), so it used to
        // be dropped on the way from Options to a pane: the pane kept saying no provider was
        // configured however many were added.
        let app: Map<String, Value> = [(
            crate::ai_agent::PROVIDERS_KEY.to_owned(),
            serde_json::json!([{ "id": "ollama", "name": "Ollama", "protocol": "openai",
                                 "baseUrl": "http://127.0.0.1:11434/v1",
                                 "models": ["llama3.1"] }]),
        )]
        .into_iter()
        .collect();
        let mut pane = Map::new();
        pane.insert("globalPrompt".to_owned(), "keep me".to_owned().into());
        apply_to(PluginId::AiAgent, Some(&app), None, &mut pane);
        assert!(
            pane.get(crate::ai_agent::PROVIDERS_KEY)
                .and_then(Value::as_array)
                .is_some_and(|providers| providers.len() == 1),
            "the configured providers must reach the pane: {pane:?}"
        );
        // Its ordinary rows still resolve as before.
        assert_eq!(pane["globalPrompt"], Value::from("keep me"));
        // Nothing was invented for plugins that have no such key.
        let mut docker = Map::new();
        apply_to(PluginId::DockerManager, Some(&app), None, &mut docker);
        assert!(!docker.contains_key(crate::ai_agent::PROVIDERS_KEY));
    }

    #[test]
    fn only_the_ai_agent_draws_a_provider_list() {
        // Its settings are ordinary label/control rows now, so the section lists them.
        assert_eq!(app_presentation(PluginId::AiAgent), Presentation::Fields);
        assert!(!app_schema(PluginId::AiAgent).is_empty());
        // Its provider list is a list view, drawn inside the same section.
        assert!(draws_provider_list(PluginId::AiAgent));
        // Nothing else has one.
        let with_list: Vec<_> = PluginId::ALL
            .into_iter()
            .filter(|id| draws_provider_list(*id))
            .map(|id| id.as_str())
            .collect();
        assert_eq!(with_list, ["ai-agent"]);
    }

    #[test]
    fn the_ai_agent_settings_match_the_original_manifest() {
        let labels: Vec<_> = app_schema(PluginId::AiAgent)
            .iter()
            .map(|field| field.label)
            .collect();
        assert_eq!(
            labels,
            [
                "Global prompt text",
                "Global knowledge",
                "Default allow rules",
                "Default deny rules",
                "Web search"
            ]
        );
        let values = resolve(PluginId::AiAgent, None, None);
        assert_eq!(values["globalKnowledgeGroup"], Value::Bool(true));
        assert_eq!(
            values["defaultAllowRules"],
            Value::Array(vec![
                Value::from("pwd"),
                Value::from("whoami"),
                Value::from("id*"),
                Value::from("hostname*"),
                Value::from("uname*"),
                Value::from("date"),
                Value::from("uptime"),
                Value::from("ls*"),
                Value::from("lscpu*"),
                Value::from("free*"),
                Value::from("df*"),
                Value::from("ps*"),
                Value::from("git status*"),
                Value::from("git log*"),
                Value::from("git diff*"),
                Value::from("git branch*"),
                Value::from("git remote*"),
                Value::from("git show*"),
                Value::from("git rev-parse*"),
            ])
        );
        assert_eq!(values["webSearchProvider"], Value::from("bing"));
    }

    #[test]
    fn the_lists_the_panels_edit_are_the_ones_the_dialogs_own() {
        // The macro pad writes `macros` and the MQTT pane writes `messages`;
        // Options edits exactly those keys, so mirroring keeps them in step.
        assert!(
            app_schema(PluginId::MacroPad)
                .iter()
                .any(|field| field.key == "macros")
        );
        assert!(
            app_schema(PluginId::MqttAnalyser)
                .iter()
                .any(|field| field.key == "messages")
        );
        // The broker settings belong to the host document, like the panel keys.
        for key in ["host", "port", "username"] {
            assert!(
                host_schema(PluginId::MqttAnalyser)
                    .iter()
                    .any(|field| field.key == key),
                "MQTT host settings should own {key}"
            );
        }
    }

    #[test]
    fn secret_fields_name_their_scope() {
        // The MQTT password belongs to one host, the agent's search key to the
        // whole app; the dialog needs to know which vault scope to use.
        assert_eq!(secret_fields(PluginId::MqttAnalyser), [("password", false)]);
        assert_eq!(
            secret_fields(PluginId::DockerManager),
            [("sudoPassword", false)]
        );
        assert_eq!(
            secret_fields(PluginId::AiAgent),
            [("webSearchApiKey", true)]
        );
        assert_eq!(secret_scope(PluginId::AiAgent, "webSearchApiKey"), Some(true));
        assert_eq!(secret_scope(PluginId::MqttAnalyser, "password"), Some(false));
        assert_eq!(secret_scope(PluginId::MqttAnalyser, "host"), None);

        // No secret field reaches a resolved settings map.
        for id in PluginId::ALL {
            let values = resolve(id, None, None);
            for (key, _) in secret_fields(id) {
                assert!(
                    !values.contains_key(key),
                    "{} leaked {key} into its settings",
                    id.as_str()
                );
            }
        }
    }

    #[test]
    fn record_lists_describe_their_item_fields() {
        let macros = app_schema(PluginId::MacroPad)
            .iter()
            .find(|field| field.key == "macros")
            .expect("macros field");
        assert_eq!(macros.kind, FieldKind::ItemList);
        let keys: Vec<_> = macros.item_schema.iter().map(|field| field.key).collect();
        assert_eq!(keys, ["label", "text", "group"]);
        assert_eq!(macros.item_label, "button");

        let messages = app_schema(PluginId::MqttAnalyser)
            .iter()
            .find(|field| field.key == "messages")
            .expect("messages field");
        let keys: Vec<_> = messages.item_schema.iter().map(|field| field.key).collect();
        assert_eq!(keys, ["label", "topic", "mode", "payload"]);
        assert_eq!(messages.item_schema[2].kind, FieldKind::Select);
        assert_eq!(messages.item_label, "message");
        // Item fields are record members, not top-level values.
        assert!(!defaults(app_schema(PluginId::MqttAnalyser)).contains_key("payload"));
    }

    #[test]
    fn defaults_come_from_the_manifests() {
        let values = resolve(PluginId::DockerManager, None, None);
        assert_eq!(values["socketPath"], Value::from("/var/run/docker.sock"));
        assert_eq!(values["showStopped"], Value::Bool(false));
        assert_eq!(values["refreshIntervalMs"], Value::from(5000));
        // A secret field is not a setting: it lives in the credential vault.
        assert!(!values.contains_key("sudoPassword"));

        let values = resolve(PluginId::MqttAnalyser, None, None);
        assert_eq!(values["host"], Value::from("127.0.0.1"));
        assert_eq!(values["port"], Value::from(1883));
        assert!(!values.contains_key("password"));
        // The saved-messages list starts empty, like the original app.
        assert_eq!(values["messages"], Value::Array(Vec::new()));
    }

    #[test]
    fn configured_values_win_over_defaults() {
        let app = map(&[
            ("confirmDestructive", Value::Bool(false)),
            ("socketPath", Value::from("ignored")),
        ]);
        let host = map(&[
            ("socketPath", Value::from("/run/user/1000/docker.sock")),
            ("logTailLines", Value::from(50)),
        ]);
        let values = resolve(PluginId::DockerManager, Some(&app), Some(&host));
        assert_eq!(values["confirmDestructive"], Value::Bool(false));
        assert_eq!(
            values["socketPath"],
            Value::from("/run/user/1000/docker.sock"),
            "the host value is the more specific one"
        );
        assert_eq!(values["logTailLines"], Value::from(50));
    }

    #[test]
    fn a_value_the_dialog_never_touched_keeps_what_the_panel_wrote() {
        // The MQTT panel types the broker address into its own storage; until
        // somebody fills the Host settings field the panel value must survive.
        let mut stored = map(&[("host", Value::from("10.0.0.9"))]);
        apply_to(PluginId::MqttAnalyser, None, None, &mut stored);
        assert_eq!(stored["host"], Value::from("10.0.0.9"));
        assert_eq!(stored["port"], Value::from(1883));

        // Once the dialog holds a value, that value wins.
        let host = map(&[("host", Value::from("broker.lan"))]);
        apply_to(PluginId::MqttAnalyser, None, Some(&host), &mut stored);
        assert_eq!(stored["host"], Value::from("broker.lan"));
    }

    #[test]
    fn a_plugins_own_keys_are_left_alone() {
        let mut stored = map(&[("events", Value::Array(Vec::new()))]);
        let app = map(&[("events", Value::Bool(true))]);
        // `events` is not part of any schema, so even a stray app value is ignored.
        apply_to(PluginId::ConnectionLogger, Some(&app), None, &mut stored);
        assert_eq!(stored["events"], Value::Array(Vec::new()));
        assert_eq!(stored["enabled"], Value::Bool(true));
        assert_eq!(stored["maxEntries"], Value::from(500));
    }

    #[test]
    fn a_group_default_is_kept_and_its_children_flattened() {
        const CHILD: &[SettingsField] = &[text("inner", "Inner", "x", "desc")];
        const SCHEMA: &[SettingsField] = &[group("outer", "Outer", true, "desc", CHILD)];
        let values = defaults(SCHEMA);
        assert_eq!(values["outer"], Value::Bool(true));
        assert_eq!(values["inner"], Value::from("x"));
        assert_eq!(flatten(SCHEMA).len(), 2);
    }
}
