//! The graph of tool scopes the model navigates.
//!
//! Sending every tool schema in every request costs the prompt a few thousand tokens before
//! the user has typed anything. Instead the tools are grouped into scopes, and only the
//! scopes loaded for this conversation have their schemas sent. The root scope is always
//! loaded; from there the model follows the graph with `load_scope`.
//!
//! A scope is *visible* while one of the scopes around it is loaded: a child of a loaded
//! scope is visible, and so is a scope one of its `related` edges points at. Only `children`
//! edges nest, so unloading a scope takes its whole subtree with it; a `related` edge only
//! widens what may be loaded next and never loads anything by itself.

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
    TOOL_SET_SCRIPT_MODE,
};
use crate::{PluginId, plugin};

/// The scope that is always loaded: the commands themselves and the user prompt.
pub const ROOT: &str = "core";

/// One node of the tool graph.
pub struct Scope {
    /// Id the model passes to `load_scope`.
    pub id: &'static str,
    /// One line shown in the index while the scope is not loaded.
    pub summary: &'static str,
    /// Tools of this crate's own, by the names the model calls.
    pub tools: &'static [&'static str],
    /// A pane whose tools this scope offers instead of (or as well as) named ones.
    pub pane: Option<PluginId>,
    /// Scopes this one nests.
    pub children: &'static [&'static str],
    /// Scopes worth offering alongside this one; visibility only, nothing loads.
    pub related: &'static [&'static str],
}

/// Every scope, parents before children.
pub const SCOPES: &[Scope] = &[
    Scope {
        id: ROOT,
        summary: "Run shell commands on the connected host (also as root) and set the directory \
they work in, reach another saved host the same way, run a JavaScript program that calls the \
other tools itself, ask the user a question, read the clock, and delegate work to a background \
sub-agent. set_script_mode decides whether the tools are called from one of those programs \
instead of as tools of the turn.",
        tools: &[
            TOOL_RUN_COMMAND,
            TOOL_CHANGE_DIR,
            TOOL_HOSTS_LIST,
            TOOL_RUN_COMMAND_BACKGROUND,
            TOOL_RUN_SCRIPT,
            TOOL_SCRIPT_RESULT,
            TOOL_SET_SCRIPT_MODE,
            TOOL_RUN_SUDO,
            TOOL_ASK_USER,
            TOOL_SPAWN_SUBAGENT,
            TOOL_WAIT_SUBAGENT,
            TOOL_CURRENT_TIME,
        ],
        pane: None,
        children: &["host", "local", "web", "knowledge", "docker"],
        related: &[],
    },
    Scope {
        id: "host",
        summary: "Work with the connected host: the environment map of its files, the files \
themselves (dev_view, dev_edit, dev_create, dev_list, dev_grep, dev_find, dev_diff), its \
system statistics, the live terminal.",
        tools: &[],
        pane: None,
        children: &["env", "host-files", "host-system", "host-terminal"],
        related: &["local", "docker"],
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
        children: &[],
        related: &["host-files", "host-system"],
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
        children: &[],
        related: &["host-terminal", "env"],
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
        children: &[],
        related: &["docker"],
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
        children: &[],
        related: &["host-files"],
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
        children: &[],
        related: &["host-files"],
    },
    Scope {
        id: "web",
        summary: "Fetch pages and search the web.",
        tools: &[TOOL_WEB_FETCH, TOOL_WEB_SEARCH],
        pane: None,
        children: &[],
        related: &[],
    },
    Scope {
        id: "knowledge",
        summary: "Search the configured knowledge-base folders.",
        tools: &[TOOL_RAG_SEARCH],
        pane: None,
        children: &[],
        related: &[],
    },
    Scope {
        id: "docker",
        summary: "Inspect containers, images, volumes and networks through the docker CLI.",
        tools: &[],
        pane: Some(PluginId::DockerManager),
        children: &[],
        related: &["host-system"],
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
///
/// A scope holding no tools of its own is available while one of its children is, so a
/// container never shows up as an empty stop with every branch switched off.
pub fn available(scope: &'static Scope, permissions: &Permissions) -> bool {
    tools_of(scope)
        .iter()
        .any(|name| permissions.offers(name))
        || scope
            .children
            .iter()
            .filter_map(|id| find(id))
            .any(|child| available(child, permissions))
}

/// The scopes the model may load next.
///
/// Everything a loaded scope nests or is related to, minus what is loaded already, in graph
/// order and only what the host may actually use.
pub fn visible(loaded: &Loaded, permissions: &Permissions) -> Vec<&'static Scope> {
    let mut reachable: Vec<&'static str> = Vec::new();
    for scope in loaded.scopes() {
        for id in scope.children.iter().chain(scope.related).copied() {
            if !loaded.contains(id) && !reachable.contains(&id) {
                reachable.push(id);
            }
        }
    }
    SCOPES
        .iter()
        .filter(|scope| reachable.contains(&scope.id) && available(scope, permissions))
        .collect()
}

/// The scopes loaded for one conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// Loaded ids, kept in graph order so the prompt and the request never reorder.
    ids: Vec<&'static str>,
}

impl Default for Loaded {
    fn default() -> Self {
        Self::new()
    }
}

impl Loaded {
    /// The set a conversation starts with: the root scope alone.
    pub fn new() -> Self {
        Self { ids: vec![ROOT] }
    }

    /// The loaded scopes in graph order.
    pub fn scopes(&self) -> impl Iterator<Item = &'static Scope> {
        SCOPES
            .iter()
            .filter(|scope| self.contains(scope.id))
    }

    /// The ids of everything loaded.
    pub fn ids(&self) -> &[&'static str] {
        &self.ids
    }

    /// Whether `id` is loaded.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(&id)
    }

    /// Whether `tool` comes from a loaded scope.
    pub fn holds(&self, tool: &str) -> bool {
        scope_of(tool).is_some_and(|scope| self.contains(scope.id))
    }

    /// Load a scope. Answers whether it was not loaded already.
    pub fn load(&mut self, scope: &'static Scope) -> bool {
        if self.contains(scope.id) {
            return false;
        }
        self.ids.push(scope.id);
        self.ids.sort_by_key(|id| {
            SCOPES
                .iter()
                .position(|scope| scope.id == *id)
                .unwrap_or(SCOPES.len())
        });
        true
    }

    /// Unload `id` and everything it nests.
    ///
    /// The root stays loaded whatever the model asks: the conversation needs the commands
    /// and the user prompt.
    pub fn unload(&mut self, id: &str) -> Vec<&'static str> {
        if id == ROOT {
            return Vec::new();
        }
        let mut removed = Vec::new();
        self.remove(id, &mut removed);
        self.ids.retain(|loaded| !removed.contains(loaded));
        removed
    }

    /// The stored ids. Unknown ones (a scope from an older build) are dropped on the way in.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::from(self.ids.clone())
    }

    /// Read a stored set back, which always holds the root.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let mut loaded = Self::new();
        for id in value.and_then(serde_json::Value::as_array).into_iter().flatten() {
            if let Some(scope) = id.as_str().and_then(find) {
                loaded.load(scope);
            }
        }
        loaded
    }

    fn remove(&self, id: &str, removed: &mut Vec<&'static str>) {
        let Some(scope) = find(id) else {
            return;
        };
        if !self.contains(scope.id) || removed.contains(&scope.id) {
            return;
        }
        removed.push(scope.id);
        for child in scope.children {
            self.remove(child, removed);
        }
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

    fn ids(scopes: &[&'static Scope]) -> Vec<&'static str> {
        scopes.iter().map(|scope| scope.id).collect()
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
    fn the_root_is_the_only_scope_a_conversation_starts_with() {
        let loaded = Loaded::new();
        assert!(loaded.contains(ROOT));
        assert_eq!(loaded.ids(), [ROOT]);
        assert!(loaded.holds(TOOL_RUN_COMMAND));
        assert!(!loaded.holds(TOOL_REMOTE_READ));
        assert!(!loaded.holds("docker_containers"));
    }

    #[test]
    fn the_root_offers_the_next_stops_but_not_their_tools() {
        let permissions = everything_on();
        let loaded = Loaded::new();
        assert_eq!(
            ids(&visible(&loaded, &permissions)),
            ["host", "local", "web", "knowledge", "docker"]
        );
    }

    #[test]
    fn loading_a_container_scope_reveals_its_children_and_nothing_else() {
        let permissions = everything_on();
        let mut loaded = Loaded::new();
        loaded.load(find("host").unwrap());
        let shown = ids(&visible(&loaded, &permissions));
        assert_eq!(
            shown,
            [
                "env",
                "host-files",
                "host-system",
                "host-terminal",
                "local",
                "web",
                "knowledge",
                "docker",
            ]
        );
        assert!(!loaded.holds(TOOL_DEV_VIEW));
        loaded.load(find("host-files").unwrap());
        assert!(loaded.holds(TOOL_DEV_VIEW));
    }

    #[test]
    fn a_related_scope_can_be_loaded_without_its_parent() {
        let permissions = everything_on();
        let mut loaded = Loaded::new();
        // `host-files` is only reachable through `host`, which is not loaded yet.
        assert!(!ids(&visible(&loaded, &permissions)).contains(&"host-files"));
        loaded.load(find("local").unwrap());
        // `local` is related to `host-files`, which makes it loadable on its own.
        assert!(ids(&visible(&loaded, &permissions)).contains(&"host-files"));
    }

    #[test]
    fn unloading_takes_the_loaded_subtree_and_leaves_the_root() {
        let mut loaded = Loaded::new();
        loaded.load(find("host").unwrap());
        loaded.load(find("host-files").unwrap());
        loaded.load(find("web").unwrap());
        assert_eq!(loaded.unload("host"), ["host", "host-files"]);
        assert!(loaded.contains(ROOT));
        assert!(loaded.contains("web"));
        assert!(loaded.unload(ROOT).is_empty());
        assert!(loaded.contains(ROOT));
    }

    #[test]
    fn a_scope_whose_group_is_off_is_invisible() {
        let permissions = permissions(json!({}));
        let mut loaded = Loaded::new();
        loaded.load(find("host").unwrap());
        let shown = ids(&visible(&loaded, &permissions));
        // Only the ungated system scope survives inside `host`.
        assert!(shown.contains(&"host-system"));
        assert!(!shown.contains(&"host-files"));
        assert!(!shown.contains(&"host-terminal"));
        assert!(!available(find("web").unwrap(), &permissions));
    }

    #[test]
    fn a_container_stays_available_while_one_of_its_children_is() {
        let permissions = permissions(json!({ group_key(GROUP_TERMINAL): true }));
        // remote-fs is off but the system probes are ungated, so the container is reachable.
        assert!(available(find("host").unwrap(), &permissions));
        assert!(!available(find("web").unwrap(), &permissions));
    }

    #[test]
    fn loaded_scopes_survive_a_round_trip_and_drop_unknown_ids() {
        let mut loaded = Loaded::new();
        loaded.load(find("host").unwrap());
        loaded.load(find("web").unwrap());
        let stored = json!([ROOT, "web", "host", "not-a-scope"]);
        let restored = Loaded::from_json(Some(&stored));
        assert_eq!(restored, loaded);
        assert_eq!(restored.ids(), [ROOT, "host", "web"]);
        assert_eq!(Loaded::from_json(None), Loaded::new());
    }
}
