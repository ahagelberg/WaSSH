//! The index of tools, grouped by the kind of work they do.
//!
//! Every tool a host allows is callable from inside `run_script`, one line each in the script
//! API, so the prompt never has to carry every schema. The tools the chat wants on the turn
//! are *loaded*, and only those are sent as schemas: `search_tools` loads the few a task
//! needs, and a tool the conversation stops using is released again after a few turns. The
//! groups here are what the search matches against and what tells the model which kinds of
//! work exist at all.

use std::collections::BTreeMap;

use super::permissions::Permissions;
use super::protocol::{
    TOOL_ASK_USER, TOOL_CHANGE_DIR, TOOL_CURRENT_TIME, TOOL_DEV_CREATE, TOOL_DEV_DIFF, TOOL_DEV_EDIT,
    TOOL_HOSTS_LIST, TOOL_DEV_FIND,
    TOOL_DEV_GREP, TOOL_DEV_LIST, TOOL_DEV_VIEW, TOOL_ENV_FIND, TOOL_ENV_OUTLINE,
    TOOL_ENV_OVERVIEW, TOOL_ENV_SCAN, TOOL_ENV_SEARCH, TOOL_ENV_SYSTEM, TOOL_GIT_BRANCHES,
    TOOL_GIT_CHECKOUT, TOOL_GIT_COMMIT, TOOL_GIT_DIFF, TOOL_GIT_LOG, TOOL_GIT_STATUS,
    TOOL_DEV_IMAGE,
    TOOL_LOCAL_DELETE,
    TOOL_LOCAL_EDIT, TOOL_LOCAL_GREP, TOOL_LOCAL_LIST, TOOL_LOCAL_MOVE, TOOL_LOCAL_MULTI_EDIT,
    TOOL_LOCAL_PATCH, TOOL_LOCAL_READ, TOOL_LOCAL_REPLACE, TOOL_LOCAL_RUN, TOOL_LOCAL_WRITE,
    TOOL_RAG_SEARCH, TOOL_REMOTE_DELETE, TOOL_REMOTE_EDIT, TOOL_REMOTE_LIST, TOOL_REMOTE_MOVE,
    TOOL_REMOTE_PATCH, TOOL_REMOTE_READ, TOOL_REMOTE_STAT, TOOL_REMOTE_WRITE, TOOL_REMOTE_ZIP_EXTRACT,
    TOOL_REMOTE_ZIP_LIST, TOOL_REMOTE_ZIP_READ, TOOL_RUN_COMMAND, TOOL_RUN_COMMAND_BACKGROUND,
    TOOL_RUN_SCRIPT, TOOL_RUN_SUDO, TOOL_SCRIPT_RESULT, TOOL_SPAWN_SUBAGENT, TOOL_SYSTEM_DISK,
    TOOL_SYSTEM_MEMORY,
    TOOL_SYSTEM_OVERVIEW, TOOL_SYSTEM_PROCESSES, TOOL_TERMINAL_KEYS, TOOL_TERMINAL_READ,
    TOOL_TERMINAL_WAIT, TOOL_TERMINAL_WRITE, TOOL_WAIT_SUBAGENT, TOOL_WEB_FETCH, TOOL_WEB_SEARCH,
};
use crate::{PluginId, plugin};

/// The scope whose tools are always tools of the turn: the commands and the prompt.
pub const ROOT: &str = "core";

/// One kind of work the index knows.
pub struct Scope {
    /// Id, and the word a search matches on.
    pub id: &'static str,
    /// One line shown in a search that matched nothing.
    pub summary: &'static str,
    /// Tools of this crate's own, by the names the model calls.
    pub tools: &'static [&'static str],
    /// A pane whose tools this scope offers instead of (or as well as) named ones.
    pub pane: Option<PluginId>,
}

/// Every kind of work, the root first.
pub const SCOPES: &[Scope] = &[
    Scope {
        id: ROOT,
        summary: "Run shell commands on the connected host (also as root) and set the directory \
they work in, reach another saved host the same way, run a JavaScript program that calls the \
other tools itself, ask the user a question, read the clock, and delegate work to a background \
sub-agent.",
        tools: &[
            TOOL_RUN_COMMAND,
            TOOL_CHANGE_DIR,
            TOOL_HOSTS_LIST,
            TOOL_RUN_COMMAND_BACKGROUND,
            TOOL_RUN_SCRIPT,
            TOOL_SCRIPT_RESULT,
            TOOL_RUN_SUDO,
            TOOL_ASK_USER,
            TOOL_SPAWN_SUBAGENT,
            TOOL_WAIT_SUBAGENT,
            TOOL_CURRENT_TIME,
        ],
        pane: None,
    },
    Scope {
        id: "env",
        summary: "The environment map: every file under the project root with its size, age and \
declarations, plus the summary and connections a mapping pass writes for the files that matter, \
built by env_scan and queried with env_search, env_find, env_outline and env_overview without \
touching the host, and env_system for the host's ports and services. Cheaper than \
dev_grep_search or dev_find_files for \"where is X\".",
        tools: &[
            TOOL_ENV_SCAN,
            TOOL_ENV_SEARCH,
            TOOL_ENV_FIND,
            TOOL_ENV_OUTLINE,
            TOOL_ENV_OVERVIEW,
            TOOL_ENV_SYSTEM,
        ],
        pane: None,
    },
    Scope {
        id: "host-files",
        summary: "Read, search and change files on the connected host with dev_view (line \
numbers), dev_edit (replacement, reported as a diff), dev_create, dev_list (tree), \
dev_grep, dev_find and dev_diff, plus remote_fs_* for writing and deleting. Use these \
instead of cat, grep, ls, find or sed.",
        tools: &[
            TOOL_REMOTE_READ,
            TOOL_REMOTE_WRITE,
            TOOL_REMOTE_LIST,
            TOOL_REMOTE_DELETE,
            TOOL_REMOTE_EDIT,
            TOOL_REMOTE_STAT,
            TOOL_REMOTE_MOVE,
            TOOL_REMOTE_PATCH,
            TOOL_REMOTE_ZIP_LIST,
            TOOL_REMOTE_ZIP_READ,
            TOOL_REMOTE_ZIP_EXTRACT,
            TOOL_GIT_STATUS,
            TOOL_GIT_DIFF,
            TOOL_GIT_LOG,
            TOOL_GIT_BRANCHES,
            TOOL_GIT_COMMIT,
            TOOL_GIT_CHECKOUT,
            TOOL_DEV_VIEW,
            TOOL_DEV_IMAGE,
            TOOL_DEV_EDIT,
            TOOL_DEV_CREATE,
            TOOL_DEV_LIST,
            TOOL_DEV_GREP,
            TOOL_DEV_FIND,
            TOOL_DEV_DIFF,
        ],
        pane: None,
    },
    Scope {
        id: "host-system",
        summary: "Load, memory, disk and process statistics for the connected host.",
        tools: &[
            TOOL_SYSTEM_OVERVIEW,
            TOOL_SYSTEM_PROCESSES,
            TOOL_SYSTEM_DISK,
            TOOL_SYSTEM_MEMORY,
        ],
        pane: None,
    },
    Scope {
        id: "host-terminal",
        summary: "Read and drive the terminal the user is watching.",
        tools: &[
            TOOL_TERMINAL_READ,
            TOOL_TERMINAL_WRITE,
            TOOL_TERMINAL_KEYS,
            TOOL_TERMINAL_WAIT,
        ],
        pane: None,
    },
    Scope {
        id: "local",
        summary: "Read, search and change files on this computer.",
        tools: &[
            TOOL_LOCAL_READ,
            TOOL_LOCAL_WRITE,
            TOOL_LOCAL_LIST,
            TOOL_LOCAL_GREP,
            TOOL_LOCAL_EDIT,
            TOOL_LOCAL_DELETE,
            TOOL_LOCAL_MOVE,
            TOOL_LOCAL_MULTI_EDIT,
            TOOL_LOCAL_PATCH,
            TOOL_LOCAL_REPLACE,
            TOOL_LOCAL_RUN,
        ],
        pane: None,
    },
    Scope {
        id: "web",
        summary: "Fetch pages and search the web.",
        tools: &[TOOL_WEB_FETCH, TOOL_WEB_SEARCH],
        pane: None,
    },
    Scope {
        id: "knowledge",
        summary: "Search the configured knowledge-base folders.",
        tools: &[TOOL_RAG_SEARCH],
        pane: None,
    },
    Scope {
        id: "docker",
        summary: "Inspect containers, images, volumes and networks through the docker CLI.",
        tools: &[],
        pane: Some(PluginId::DockerManager),
    },
];

/// The scope with this id.
pub fn find(id: &str) -> Option<&'static Scope> {
    SCOPES.iter().find(|scope| scope.id == id)
}

/// The scope a tool belongs to.
pub fn scope_of(tool: &str) -> Option<&'static Scope> {
    SCOPES.iter().find(|scope| {
        scope.tools.contains(&tool)
            || scope
                .pane
                .is_some_and(|pane| plugin::tools_of(pane).iter().any(|entry| entry.name == tool))
    })
}

/// Every tool a scope offers, named ones first.
fn tools_of(scope: &'static Scope) -> Vec<&'static str> {
    let mut names = scope.tools.to_vec();
    if let Some(pane) = scope.pane {
        names.extend(plugin::tools_of(pane).iter().map(|tool| tool.name));
    }
    names
}

/// Whether the host may use any tool of this scope.
pub fn scope_available(scope: &'static Scope, permissions: &Permissions) -> bool {
    tools_of(scope)
        .iter()
        .any(|name| permissions.offers(name))
}

/// The kinds of work the index knows that this host can do, for a search that matched nothing.
pub fn summaries(permissions: &Permissions) -> Vec<(&'static str, &'static str)> {
    SCOPES
        .iter()
        .filter(|scope| scope.id != ROOT && scope_available(scope, permissions))
        .map(|scope| (scope.id, scope.summary))
        .collect()
}

/// The tools loaded for one conversation: the ones offered as tools of the turn.
///
/// Everything else the host allows is still callable from `run_script`, so a tool is loaded
/// only when the chat wants it on the turn, and it is released once the conversation has not
/// used it for [`crate::ai_agent::agent::TOOL_IDLE_TURNS`] turns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    /// Loaded tool names, with the user turn each was last used in. Sorted by name, so the
    /// request never reorders between rounds.
    tools: BTreeMap<&'static str, u64>,
}

impl Loaded {
    /// A conversation that has loaded nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The loaded tools, in the order the request lists them.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tools.keys().copied()
    }

    /// Whether `tool` is loaded, and so offered as a tool of the turn.
    pub fn holds(&self, tool: &str) -> bool {
        self.tools.contains_key(tool)
    }

    /// How many tools are loaded.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Load `tools`, remembering the turn that wanted them.
    pub fn load(&mut self, tools: impl IntoIterator<Item = &'static str>, turn: u64) {
        for tool in tools {
            self.tools.insert(tool, turn);
        }
    }

    /// Note that `tool` was used in `turn`, so it stays loaded.
    pub fn touch(&mut self, tool: &'static str, turn: u64) {
        self.tools.insert(tool, turn);
    }

    /// Drop one tool, so it goes back to being callable from a script alone.
    pub fn release_tool(&mut self, tool: &str) -> bool {
        self.tools.remove(tool).is_some()
    }

    /// Release what the conversation has not used for `idle` turns.
    pub fn release(&mut self, turn: u64, idle: u64) -> Vec<&'static str> {
        let mut released = Vec::new();
        self.tools.retain(|tool, used| {
            let keep = turn.saturating_sub(*used) < idle;
            if !keep {
                released.push(*tool);
            }
            keep
        });
        released
    }

    /// The stored names.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::from(self.tools.keys().copied().collect::<Vec<_>>())
    }

    /// Read a stored set back. Names this build does not know are dropped on the way in.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let mut loaded = Self::default();
        for name in value.and_then(serde_json::Value::as_array).into_iter().flatten() {
            let Some(name) = name.as_str().and_then(super::tools::static_name) else {
                continue;
            };
            loaded.tools.insert(name, 0);
        }
        loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_agent::permissions::{GROUP_LOCAL_FS, GROUP_TERMINAL, group_key};
    use serde_json::json;

    fn permissions(value: serde_json::Value) -> Permissions {
        Permissions::from_settings(&value.as_object().cloned().unwrap_or_default())
    }

    fn everything_on() -> Permissions {
        permissions(json!({
            group_key(GROUP_TERMINAL): true,
            group_key(GROUP_LOCAL_FS): true,
            group_key("remote-fs"): true,
            group_key("web-access"): true,
            group_key("knowledge-base"): true,
        }))
    }

    #[test]
    fn every_tool_belongs_to_exactly_one_scope() {
        let names = super::super::tools::TOOL_SPECS
            .iter()
            .map(|spec| spec.name)
            .chain(plugin::tools().map(|tool| tool.name));
        for name in names {
            let owners = SCOPES
                .iter()
                .filter(|scope| tools_of(scope).contains(&name))
                .count();
            assert_eq!(owners, 1, "{name} is in {owners} scopes");
        }
    }

    #[test]
    fn a_conversation_starts_with_nothing_loaded() {
        let loaded = Loaded::new();
        assert_eq!(loaded.len(), 0);
        assert!(!loaded.holds(TOOL_DEV_VIEW));
        assert!(!loaded.holds("docker_containers"));
    }

    #[test]
    fn a_tool_stays_loaded_until_it_goes_unused() {
        let mut loaded = Loaded::new();
        loaded.load([TOOL_DEV_VIEW, TOOL_DEV_GREP], 1);
        assert!(loaded.holds(TOOL_DEV_VIEW));
        assert_eq!(loaded.names().collect::<Vec<_>>(), [TOOL_DEV_GREP, TOOL_DEV_VIEW]);

        // One turn later it is still worth its schema.
        assert!(loaded.release(2, 2).is_empty());
        assert!(loaded.holds(TOOL_DEV_VIEW));
        // A call in the third turn keeps the one that was used; the other goes.
        loaded.touch(TOOL_DEV_GREP, 3);
        let released = loaded.release(4, 2);
        assert_eq!(released, [TOOL_DEV_VIEW]);
        assert!(loaded.holds(TOOL_DEV_GREP));
    }

    #[test]
    fn loaded_tools_survive_a_round_trip_through_json() {
        let mut loaded = Loaded::new();
        loaded.load([TOOL_DEV_VIEW, TOOL_DEV_GREP], 4);
        let stored = loaded.to_json();
        let read = Loaded::from_json(Some(&stored));
        assert_eq!(read.names().collect::<Vec<_>>(), loaded.names().collect::<Vec<_>>());
        // A name this build does not know is dropped rather than kept as a broken entry.
        let stale = json!(["host-files", TOOL_DEV_VIEW]);
        assert_eq!(Loaded::from_json(Some(&stale)).len(), 1);
    }

    #[test]
    fn the_index_names_what_the_host_can_do() {
        let shown = summaries(&everything_on());
        let ids: Vec<&str> = shown.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&"host-files"));
        assert!(ids.contains(&"docker"));
        assert!(!ids.contains(&ROOT));

        // A host with the families switched off offers no tool of theirs; the map tools and
        // the pane tools are not gated and stay.
        let off: Vec<&str> = summaries(&permissions(json!({})))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(!off.contains(&"host-files"));
        assert!(!off.contains(&"web"));
        assert!(!off.contains(&"terminal"));
    }
}
