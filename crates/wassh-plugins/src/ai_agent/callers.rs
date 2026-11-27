//! Which way a tool is called: as a tool of the turn, or from inside `run_script`.
//!
//! The choice is per tool, so a chat can keep a cheap lookup on the direct path and send
//! the tools whose results are large, or that fan out, through a script. A chat that has
//! made no choice runs on a default.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::protocol::{
    SETTING_HOST_SCRIPT_MODE, SETTING_HOST_TOOL_CALLER_DEFAULT,
    SETTING_SCRIPT_MODE, SETTING_TOOL_CALLERS, SETTING_TOOL_CALLER_DEFAULT,
};

/// How one tool is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// A tool of the turn.
    Direct,
    /// Only from inside `run_script`.
    Script,
    /// Both.
    Both,
}

impl Caller {
    /// How the caller is stored.
    pub fn key(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Script => "script",
            Self::Both => "both",
        }
    }

    /// Read a stored caller.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "direct" => Some(Self::Direct),
            "script" => Some(Self::Script),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Whether the tool is offered to the turn.
    pub fn direct(self) -> bool {
        matches!(self, Self::Direct | Self::Both)
    }

    /// Whether the tool is offered to a script.
    pub fn script(self) -> bool {
        matches!(self, Self::Script | Self::Both)
    }
}

/// What a chat does with a tool it has no explicit choice for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallerMode {
    /// Every tool of the turn.
    #[default]
    Direct,
    /// Every tool from inside `run_script`.
    Script,
    /// Every tool both ways.
    Both,
}

impl CallerMode {
    /// How the mode is stored.
    pub fn key(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Script => "script",
            Self::Both => "both",
        }
    }

    /// Read a stored mode.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "direct" => Some(Self::Direct),
            "script" => Some(Self::Script),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// The mode a stored switch means, for the keys script mode used before this setting.
    fn from_legacy(settings: &Map<String, Value>, key: &str) -> Option<Self> {
        settings.get(key).and_then(Value::as_bool).map(|on| {
            if on {
                Self::Script
            } else {
                Self::Direct
            }
        })
    }
}

/// The caller policy of one chat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Callers {
    mode: CallerMode,
    overrides: BTreeMap<String, Caller>,
}

impl Callers {
    /// A policy that calls everything the same way.
    pub fn every(mode: CallerMode) -> Self {
        Self {
            mode,
            overrides: BTreeMap::new(),
        }
    }

    /// Read the policy, the host's settings winning over the app's.
    ///
    /// The retired script-mode switches are still read when neither new key is stored, so
    /// an install that had script mode on keeps calling its tools from scripts.
    pub fn from_settings(app: &Map<String, Value>, host: Option<&Map<String, Value>>) -> Self {
        let mode = host
            .and_then(|host| stored_mode(host, SETTING_HOST_TOOL_CALLER_DEFAULT))
            .or_else(|| host.and_then(|host| CallerMode::from_legacy(host, SETTING_HOST_SCRIPT_MODE)))
            .or_else(|| stored_mode(app, SETTING_TOOL_CALLER_DEFAULT))
            .or_else(|| CallerMode::from_legacy(app, SETTING_SCRIPT_MODE))
            .unwrap_or_default();
        Self {
            mode,
            overrides: overrides_from(app),
        }
    }

    /// The default this chat applies.
    pub fn mode(&self) -> CallerMode {
        self.mode
    }

    /// The stored choice for one tool, when the chat has one.
    pub fn explicit(&self, tool: &str) -> Option<Caller> {
        self.overrides.get(tool).copied()
    }

    /// How `tool` is called.
    pub fn caller(&self, tool: &str) -> Caller {
        if let Some(chosen) = self.explicit(tool) {
            return chosen;
        }
        match self.mode {
            CallerMode::Direct => Caller::Direct,
            CallerMode::Script => Caller::Script,
            CallerMode::Both => Caller::Both,
        }
    }
}

/// A stored default, if the key holds one.
fn stored_mode(settings: &Map<String, Value>, key: &str) -> Option<CallerMode> {
    CallerMode::from_key(settings.get(key)?.as_str()?)
}

/// The per-tool choices a stored map holds.
fn overrides_from(settings: &Map<String, Value>) -> BTreeMap<String, Caller> {
    let Some(stored) = settings.get(SETTING_TOOL_CALLERS).and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    stored
        .iter()
        .filter_map(|(tool, value)| Some((tool.clone(), Caller::from_key(value.as_str()?)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn a_chat_without_choices_calls_its_tools_on_the_turn() {
        let callers = Callers::from_settings(&Map::new(), None);
        assert_eq!(callers.mode(), CallerMode::Direct);
        assert_eq!(callers.caller("run_command"), Caller::Direct);
    }

    #[test]
    fn an_explicit_choice_wins_over_the_default() {
        let app = map(json!({ "toolCallers": { "run_command": "both" } }));
        let callers = Callers::from_settings(&app, None);
        assert_eq!(callers.caller("run_command"), Caller::Both);
        assert_eq!(callers.caller("hosts_list"), Caller::Direct);
    }

    #[test]
    fn the_hosts_default_wins_over_the_apps() {
        let app = map(json!({ "toolCallerDefault": "direct" }));
        let host = map(json!({ "hostToolCallerDefault": "script" }));
        assert_eq!(
            Callers::from_settings(&app, Some(&host)).caller("hosts_list"),
            Caller::Script
        );
    }

    #[test]
    fn the_retired_script_mode_switch_still_decides() {
        let on = map(json!({ "scriptMode": true }));
        let off = map(json!({ "scriptMode": false }));
        assert_eq!(Callers::from_settings(&on, None).mode(), CallerMode::Script);
        assert_eq!(Callers::from_settings(&off, None).mode(), CallerMode::Direct);
        // A new key beside it wins.
        let both = map(json!({ "scriptMode": true, "toolCallerDefault": "both" }));
        assert_eq!(Callers::from_settings(&both, None).mode(), CallerMode::Both);
    }

    #[test]
    fn a_stored_caller_round_trips() {
        for caller in [Caller::Direct, Caller::Script, Caller::Both] {
            assert_eq!(Caller::from_key(caller.key()), Some(caller));
        }
        for mode in [
            CallerMode::Direct,
            CallerMode::Script,
            CallerMode::Both,
        ] {
            assert_eq!(CallerMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(Caller::from_key("nonsense"), None);
    }
}
