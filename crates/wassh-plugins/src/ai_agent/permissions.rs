//! What the agent is allowed to do on one host.
//!
//! Port of the reference's `pluginApiTools.ts` and `apiMethods.ts`. Capabilities
//! are grouped: a group has one on/off setting, and every tool it holds has an
//! `allow`/`ask`/`deny` setting. Groups start off, so a fresh host can do
//! nothing until the user switches a capability on.
//!
//! Tools bundled into one setting resolve through the first tool's key, so a
//! single switch covers a family such as "read files". The setting keys are
//! spelled exactly as the reference spells them (`apiGroup:ai-agent:<group>`,
//! `apiMethod:ai-agent:<tool>`), so saved settings carry over.

use serde_json::{Map, Value};

use super::protocol::{
    TOOL_CURRENT_TIME, TOOL_DEV_CREATE, TOOL_DEV_DIFF, TOOL_DEV_EDIT, TOOL_DEV_FIND, TOOL_DEV_GREP,
    TOOL_DEV_LIST, TOOL_DEV_VIEW, TOOL_LOCAL_EDIT, TOOL_LOCAL_LIST, TOOL_LOCAL_READ,
    TOOL_LOCAL_WRITE, TOOL_RAG_SEARCH, TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST,
    TOOL_REMOTE_READ, TOOL_REMOTE_WRITE, TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ,
    TOOL_TERMINAL_WAIT, TOOL_TERMINAL_WRITE, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};
use crate::PluginId;

/// Live-terminal tools.
pub const GROUP_TERMINAL: &str = "terminal";
/// Tools for the connected SSH server's files.
pub const GROUP_REMOTE_FS: &str = "remote-fs";
/// Tools for files on this computer.
pub const GROUP_LOCAL_FS: &str = "local-fs";
/// Fetching pages and searching the web.
pub const GROUP_WEB_ACCESS: &str = "web-access";
/// Searching the knowledge-base folders.
pub const GROUP_KNOWLEDGE_BASE: &str = "knowledge-base";

/// The groups, in the order the settings schema lists them.
pub const GROUPS: &[&str] = &[
    GROUP_TERMINAL,
    GROUP_REMOTE_FS,
    GROUP_LOCAL_FS,
    GROUP_WEB_ACCESS,
    GROUP_KNOWLEDGE_BASE,
];

/// A tool's answer to "may I?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Run it.
    Allow,
    /// Ask the user first.
    Ask,
    /// Never run it.
    Deny,
}

impl Permission {
    /// The decision a stored setting holds.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// How the setting stores it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// One gated tool.
pub struct Gate {
    /// The name the model calls.
    pub tool: &'static str,
    /// The group that must be on; `None` for a tool with no group of its own.
    pub group: Option<&'static str>,
    /// The tool whose setting the decision is stored under. Bundled tools name
    /// their bundle's first tool so one setting covers them all.
    pub leader: &'static str,
    /// The decision when nothing is stored.
    pub default: Permission,
}

/// Every gated tool, in the reference's order.
///
/// Reads and listings are allowed once their group is on; anything that
/// changes a file asks first. `run_command`, `run_sudo`, `ask_user` and the
/// system probes are not listed: the reference gates them with its command rule
/// lists instead, or not at all.
pub const GATES: &[Gate] = &[
    Gate {
        tool: TOOL_TERMINAL_READ,
        group: Some(GROUP_TERMINAL),
        leader: TOOL_TERMINAL_READ,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_TERMINAL_WAIT,
        group: Some(GROUP_TERMINAL),
        leader: TOOL_TERMINAL_WAIT,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_TERMINAL_WRITE,
        group: Some(GROUP_TERMINAL),
        leader: TOOL_TERMINAL_WRITE,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_TERMINAL_KEYS,
        group: Some(GROUP_TERMINAL),
        leader: TOOL_TERMINAL_KEYS,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_DEV_VIEW,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_VIEW,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_REMOTE_READ,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_VIEW,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_DEV_LIST,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_LIST,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_DEV_FIND,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_LIST,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_DEV_GREP,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_LIST,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_REMOTE_LIST,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_LIST,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_DEV_EDIT,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_DEV_CREATE,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_DEV_DIFF,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_REMOTE_WRITE,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_REMOTE_EDIT,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_REMOTE_DELETE,
        group: Some(GROUP_REMOTE_FS),
        leader: TOOL_DEV_EDIT,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_LOCAL_READ,
        group: Some(GROUP_LOCAL_FS),
        leader: TOOL_LOCAL_READ,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_LOCAL_LIST,
        group: Some(GROUP_LOCAL_FS),
        leader: TOOL_LOCAL_LIST,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_LOCAL_WRITE,
        group: Some(GROUP_LOCAL_FS),
        leader: TOOL_LOCAL_WRITE,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_LOCAL_EDIT,
        group: Some(GROUP_LOCAL_FS),
        leader: TOOL_LOCAL_WRITE,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_WEB_FETCH,
        group: Some(GROUP_WEB_ACCESS),
        leader: TOOL_WEB_FETCH,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_WEB_SEARCH,
        group: Some(GROUP_WEB_ACCESS),
        leader: TOOL_WEB_SEARCH,
        default: Permission::Ask,
    },
    Gate {
        tool: TOOL_RAG_SEARCH,
        group: Some(GROUP_KNOWLEDGE_BASE),
        leader: TOOL_RAG_SEARCH,
        default: Permission::Allow,
    },
    Gate {
        tool: TOOL_CURRENT_TIME,
        group: None,
        leader: TOOL_CURRENT_TIME,
        default: Permission::Allow,
    },
];

/// The setting that switches a group on.
pub fn group_key(group: &str) -> String {
    format!("apiGroup:{}:{group}", PluginId::AiAgent.as_str())
}

/// The setting that holds a tool's decision.
pub fn method_key(tool: &str) -> String {
    format!("apiMethod:{}:{tool}", PluginId::AiAgent.as_str())
}

/// The setting a tool's decision is stored under, which for a bundled tool is
/// its bundle leader's.
pub fn setting_key(tool: &str) -> Option<String> {
    gate(tool).map(|gate| method_key(gate.leader))
}

fn gate(tool: &str) -> Option<&'static Gate> {
    GATES.iter().find(|gate| gate.tool == tool)
}

/// One host's permissions.
///
/// Empty is "nothing gated yet", which is what a pane that has not read its
/// settings runs under; the pane replaces it from the settings every frame.
#[derive(Debug, Default)]
pub struct Permissions {
    /// The decision for every gated tool.
    decisions: Vec<(&'static str, Permission)>,
}

impl Permissions {
    /// Read the decisions out of a host's settings.
    pub fn from_settings(settings: &Map<String, Value>) -> Self {
        Self {
            decisions: GATES
                .iter()
                .map(|gate| (gate.tool, resolve(settings, gate)))
                .collect(),
        }
    }

    /// What to do with `tool` now.
    pub fn decision(&self, tool: &str) -> Permission {
        self.decisions
            .iter()
            .find(|(name, _)| *name == tool)
            .map_or(Permission::Allow, |(_, decision)| *decision)
    }

    /// Whether the tool is offered to the model at all.
    pub fn offers(&self, tool: &str) -> bool {
        self.decision(tool) != Permission::Deny
    }
}

fn resolve(settings: &Map<String, Value>, gate: &Gate) -> Permission {
    if let Some(group) = gate.group
        && !group_enabled(settings, group)
    {
        return Permission::Deny;
    }
    stored(settings, &method_key(gate.leader)).unwrap_or(gate.default)
}

fn group_enabled(settings: &Map<String, Value>, group: &str) -> bool {
    settings
        .get(&group_key(group))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn stored(settings: &Map<String, Value>, key: &str) -> Option<Permission> {
    settings
        .get(key)
        .and_then(Value::as_str)
        .and_then(Permission::parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_agent::protocol::TOOL_RUN_COMMAND;
    use serde_json::json;

    fn settings(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_fresh_host_may_use_nothing_that_is_grouped() {
        let permissions = Permissions::from_settings(&settings(json!({})));
        assert_eq!(permissions.decision(TOOL_WEB_FETCH), Permission::Deny);
        assert_eq!(permissions.decision(TOOL_TERMINAL_READ), Permission::Deny);
        assert!(!permissions.offers(TOOL_TERMINAL_READ));
        // Nothing to switch on, so the time tool keeps its own setting.
        assert_eq!(permissions.decision(TOOL_CURRENT_TIME), Permission::Allow);
    }

    #[test]
    fn a_group_switched_on_uses_the_stored_decisions() {
        let permissions = Permissions::from_settings(&settings(json!({
            group_key(GROUP_TERMINAL): true,
        })));
        assert_eq!(permissions.decision(TOOL_TERMINAL_READ), Permission::Allow);
        assert_eq!(permissions.decision(TOOL_TERMINAL_WRITE), Permission::Ask);
        assert!(permissions.offers(TOOL_TERMINAL_WRITE));
    }

    #[test]
    fn a_stored_denial_hides_the_tool() {
        let permissions = Permissions::from_settings(&settings(json!({
            group_key(GROUP_WEB_ACCESS): true,
            method_key(TOOL_WEB_SEARCH): "deny",
        })));
        assert!(!permissions.offers(TOOL_WEB_SEARCH));
        assert_eq!(permissions.decision(TOOL_WEB_FETCH), Permission::Ask);
    }

    #[test]
    fn one_setting_gates_a_whole_bundle() {
        let permissions = Permissions::from_settings(&settings(json!({
            group_key(GROUP_REMOTE_FS): true,
            method_key(TOOL_DEV_VIEW): "deny",
        })));
        assert_eq!(permissions.decision(TOOL_REMOTE_READ), Permission::Deny);
        // A different bundle keeps its own decision.
        assert_eq!(permissions.decision(TOOL_REMOTE_WRITE), Permission::Ask);
    }

    #[test]
    fn reads_run_and_writes_ask_once_the_group_is_on() {
        let permissions = Permissions::from_settings(&settings(json!({
            group_key(GROUP_REMOTE_FS): true,
            group_key(GROUP_LOCAL_FS): true,
            group_key(GROUP_TERMINAL): true,
        })));
        for tool in [
            TOOL_DEV_VIEW,
            TOOL_REMOTE_READ,
            TOOL_DEV_LIST,
            TOOL_DEV_FIND,
            TOOL_DEV_GREP,
            TOOL_REMOTE_LIST,
            TOOL_LOCAL_READ,
            TOOL_LOCAL_LIST,
            TOOL_TERMINAL_READ,
            TOOL_TERMINAL_WAIT,
        ] {
            assert_eq!(permissions.decision(tool), Permission::Allow, "{tool}");
        }
        for tool in [
            TOOL_DEV_EDIT,
            TOOL_DEV_CREATE,
            TOOL_DEV_DIFF,
            TOOL_REMOTE_WRITE,
            TOOL_REMOTE_EDIT,
            TOOL_REMOTE_DELETE,
            TOOL_LOCAL_WRITE,
            TOOL_LOCAL_EDIT,
            TOOL_TERMINAL_WRITE,
            TOOL_TERMINAL_KEYS,
        ] {
            assert_eq!(permissions.decision(tool), Permission::Ask, "{tool}");
        }
    }

    #[test]
    fn a_bundled_tool_persists_its_bundle_leader_setting() {
        assert_eq!(
            setting_key(TOOL_REMOTE_EDIT).as_deref(),
            Some("apiMethod:ai-agent:dev_edit_file")
        );
        assert_eq!(
            setting_key(TOOL_LOCAL_EDIT).as_deref(),
            Some("apiMethod:ai-agent:local_fs_write_file")
        );
        assert_eq!(setting_key(TOOL_RUN_COMMAND), None);
    }

    #[test]
    fn a_stored_value_wins_over_the_default() {
        let permissions = Permissions::from_settings(&settings(json!({
            method_key(TOOL_CURRENT_TIME): "deny",
        })));
        assert!(!permissions.offers(TOOL_CURRENT_TIME));
    }
}
