//! The environment map: a per-host graph of the files behind the session.
//!
//! Built by `env_scan`, which asks the host for one `find` listing and one symbol grep
//! rather than letting the model discover the tree with `ls`, `grep` and `find` of its
//! own. `env_find`, `env_outline` and `env_overview` answer from the map, so a question
//! about where something lives costs no host call at all.
//!
//! The map is host state, not conversation state: it outlives the chat that built it and
//! is written back to the host's plugin data.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Version of the stored shape.
pub const MAP_VERSION: u32 = 1;
/// Most files one map keeps. A scan that finds more is marked truncated.
pub const MAX_NODES: usize = 5_000;
/// Deepest directory walk a scan performs.
pub const MAX_DEPTH: u32 = 6;
/// Most symbols kept per file.
pub const MAX_SYMBOLS: usize = 60;
/// Most symbol matches one scan collects.
pub const MAX_SCAN_SYMBOLS: usize = 4_000;
/// Characters of one symbol line kept.
pub const MAX_SYMBOL_CHARS: usize = 120;
/// Hits `env_find` reports at most.
pub const MAX_FIND_HITS: usize = 100;
/// Score a query token adds when a file's path holds it.
const PATH_WEIGHT: usize = 3;
/// Score a query token adds when one of a file's declarations holds it.
const SYMBOL_WEIGHT: usize = 2;
/// Entries one system scan keeps per section.
pub const MAX_SYSTEM_ENTRIES: usize = 60;
/// Directories between two lines of an overview.
pub const OVERVIEW_DEPTH: usize = 2;
/// Group name for the files that sit in the root itself.
pub const TOP_LEVEL: &str = "(top level)";
/// Files the overview names as recently changed.
pub const OVERVIEW_RECENT: usize = 12;

/// The line the scan command prints between its two sections.
pub const SECTION_MARKER: &str = "__wassh_symbols__";

/// One file in the map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Size in bytes.
    pub size: u64,
    /// Modification time, in seconds since the epoch.
    pub mtime: u64,
    /// Declaration lines: `line␉text`.
    pub symbols: Vec<String>,
}

/// A host's map of its own files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvMap {
    /// Directory the map was built from.
    pub root: String,
    /// When it was built, in milliseconds since the epoch.
    pub built_ms: u64,
    /// Every file, keyed by path.
    pub nodes: BTreeMap<String, Node>,
    /// Whether the scan stopped at its caps.
    pub truncated: bool,
    /// Bumped on every change, so the pane knows when what it stored is stale.
    pub revision: u64,
}

impl EnvMap {
    /// Whether nothing has been scanned yet.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// How many files it holds.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Age of the map in whole minutes, for the popup and the prompt.
    pub fn age_minutes(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.built_ms) / 60_000
    }

    /// Forget everything, keeping no root.
    pub fn clear(&mut self) {
        let revision = self.revision + 1;
        *self = Self {
            revision,
            ..Self::default()
        };
    }

    /// Drop one path, as a change to it invalidates what the map knows.
    pub fn forget(&mut self, path: &str) {
        if self.nodes.remove(path).is_some() {
            self.revision += 1;
        }
    }

    /// Drop every path that ends with `path`, for a write that named it relative to a
    /// root the map was built from rather than absolutely.
    pub fn forget_suffix(&mut self, path: &str) {
        let suffix = format!("/{}", path.trim_start_matches("./").trim_start_matches('/'));
        if suffix == "/" {
            return;
        }
        let before = self.nodes.len();
        self.nodes.retain(|key, _| !key.ends_with(&suffix));
        if self.nodes.len() != before {
            self.revision += 1;
        }
    }

    /// The command that builds the map: one `find` for the files, one `grep` for the
    /// declarations, both cut short by their caps.
    ///
    /// `root` is the directory to walk; it is quoted by the caller's rules.
    pub fn scan_command(root: &str) -> String {
        format!(
            "find {root} -maxdepth {MAX_DEPTH} {skipped} -type f -printf '%s %T@ %p\\n' \
             2>/dev/null | head -n {MAX_NODES}; \
             echo {SECTION_MARKER}; \
             grep -rnIE {pattern} {root} 2>/dev/null | head -n {MAX_SCAN_SYMBOLS}",
            skipped = SKIPPED_DIRS,
            pattern = shell_quote(SYMBOL_PATTERN),
        )
    }

    /// Read a scan's output back. Returns the map and whether a cap was hit.
    pub fn parse(root: &str, built_ms: u64, output: &str) -> Self {
        let mut map = Self {
            root: root.to_owned(),
            built_ms,
            revision: 1,
            ..Self::default()
        };
        let mut truncated = false;
        let mut section = true;
        for line in output.lines() {
            if line.trim() == SECTION_MARKER {
                section = false;
                continue;
            }
            if section {
                let mut parts = line.splitn(3, ' ');
                let (Some(size), Some(mtime), Some(path)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let (Ok(size), Ok(mtime)) = (size.parse::<u64>(), mtime.parse::<u64>()) else {
                    continue;
                };
                if map.nodes.len() >= MAX_NODES {
                    truncated = true;
                    continue;
                }
                map.nodes.insert(
                    path.to_owned(),
                    Node {
                        size,
                        mtime,
                        symbols: Vec::new(),
                    },
                );
            } else {
                // `path:line:text`, where the path may itself hold colons.
                let mut parts = line.rsplitn(3, ':');
                let (Some(text), Some(at), Some(path)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let Ok(at) = at.parse::<u64>() else {
                    continue;
                };
                let Some(node) = map.nodes.get_mut(path) else {
                    continue;
                };
                if node.symbols.len() >= MAX_SYMBOLS {
                    truncated = true;
                    continue;
                }
                let text: String = text.trim().chars().take(MAX_SYMBOL_CHARS).collect();
                node.symbols.push(format!("{at}\t{text}"));
            }
        }
        map.truncated = truncated;
        map
    }

    /// A directory digest: how many files each top-level directory holds, and which files
    /// changed most recently.
    pub fn overview(&self) -> String {
        if self.nodes.is_empty() {
            return "The environment map is empty: scan it first.".to_owned();
        }
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for path in self.nodes.keys() {
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .trim_start_matches('/');
            let group = match relative.rsplit_once('/') {
                Some((dir, _)) => dir
                    .split('/')
                    .take(OVERVIEW_DEPTH)
                    .collect::<Vec<_>>()
                    .join("/"),
                None => TOP_LEVEL.to_owned(),
            };
            *counts.entry(group).or_default() += 1;
        }
        let mut lines = vec![format!(
            "{} ({} files{})",
            self.root,
            self.len(),
            if self.truncated {
                ", capped"
            } else {
                ""
            }
        )];
        for (group, count) in &counts {
            if group == TOP_LEVEL {
                lines.push(format!("{group}  {count}"));
            } else {
                lines.push(format!("{group}/  {count}"));
            }
        }
        let mut recent: Vec<(&String, &Node)> = self.nodes.iter().collect();
        recent.sort_by_key(|(_, node)| std::cmp::Reverse(node.mtime));
        lines.push("Most recently changed:".to_owned());
        for (path, node) in recent.into_iter().take(OVERVIEW_RECENT) {
            match outline_of(&node.symbols) {
                Some(outline) => lines.push(format!("{path}  {outline}")),
                None => lines.push(path.clone()),
            }
        }
        lines.join("\n")
    }

    /// Paths whose name, or declarations, match `needle`, best first.
    pub fn find(&self, needle: &str, limit: usize) -> Vec<String> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, MAX_FIND_HITS);
        let mut hits: Vec<(u8, String)> = Vec::new();
        for (path, node) in &self.nodes {
            let name = path.to_lowercase();
            if name.contains(&needle) {
                hits.push((0, path.clone()));
                continue;
            }
            if let Some(symbol) = node
                .symbols
                .iter()
                .find(|symbol| symbol.to_lowercase().contains(&needle))
            {
                hits.push((1, format!("{path}: {}", symbol.replace('\t', " "))));
            }
        }
        hits.sort();
        hits.into_iter()
            .take(limit)
            .map(|(_, hit)| hit)
            .collect()
    }

    /// Files ranked by how well their path and declarations answer `query`.
    ///
    /// A token in the path counts for more than one in a declaration, and every token a
    /// file matches adds to its score, so a phrase like "render tab title" puts the file
    /// that names both first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<String> {
        let tokens: Vec<String> = query
            .to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect();
        if tokens.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, MAX_FIND_HITS);
        let mut scored: Vec<(usize, String)> = Vec::new();
        for (path, node) in &self.nodes {
            let name = path.to_lowercase();
            let symbols: String = node.symbols.join(" ").to_lowercase();
            let mut score = 0usize;
            for token in &tokens {
                if name.contains(token.as_str()) {
                    score += PATH_WEIGHT;
                }
                if symbols.contains(token.as_str()) {
                    score += SYMBOL_WEIGHT;
                }
            }
            if score > 0 {
                scored.push((score, path.clone()));
            }
        }
        // Best first, and alphabetical among equals so the answer is stable.
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, path)| path)
            .collect()
    }

    /// What the map knows about one file: its size, age and declarations.
    pub fn outline(&self, path: &str) -> Option<String> {
        let node = self.nodes.get(path)?;
        let mut text = format!(
            "{path}\n{} bytes, changed {}",
            node.size,
            node.mtime
        );
        if let Some(outline) = outline_of(&node.symbols) {
            text.push('\n');
            text.push_str(&outline);
        }
        Some(text)
    }

    /// The stored shape.
    pub fn to_json(&self) -> Value {
        let nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|(path, node)| {
                json!([path, node.size, node.mtime, node.symbols])
            })
            .collect();
        json!({
            "version": MAP_VERSION,
            "root": self.root,
            "builtMs": self.built_ms,
            "revision": self.revision,
            "truncated": self.truncated,
            "nodes": nodes,
        })
    }

    /// Read a stored map back.
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let mut map = Self {
            root: value["root"].as_str().unwrap_or_default().to_owned(),
            built_ms: value["builtMs"].as_u64().unwrap_or_default(),
            revision: value["revision"].as_u64().unwrap_or_default(),
            truncated: value["truncated"].as_bool().unwrap_or(false),
            ..Self::default()
        };
        for entry in value["nodes"].as_array().into_iter().flatten() {
            let Some(path) = entry[0].as_str() else {
                continue;
            };
            let symbols: Vec<String> = entry[3]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|symbol| symbol.as_str().map(str::to_owned))
                .collect();
            map.nodes.insert(
                path.to_owned(),
                Node {
                    size: entry[1].as_u64().unwrap_or_default(),
                    mtime: entry[2].as_u64().unwrap_or_default(),
                    symbols,
                },
            );
        }
        map
    }
}

/// Declarations of a file, one per line, or nothing when it has none.
fn outline_of(symbols: &[String]) -> Option<String> {
    (!symbols.is_empty()).then(|| {
        symbols
            .iter()
            .map(|symbol| symbol.replace('\t', ": "))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// How the host itself is set up: what it runs and what listens.
///
/// The second half of the map. A files map answers "where is this code"; these facts answer
/// "what serves that port", which otherwise costs the agent a handful of `ss` and
/// `systemctl` calls per conversation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemFacts {
    /// Kernel and operating system line.
    pub os: String,
    /// When the scan ran, in milliseconds since the epoch.
    pub built_ms: u64,
    /// Listening sockets: address, port and process.
    pub listening: Vec<String>,
    /// Running services.
    pub services: Vec<String>,
    /// Bumped on every change, so the pane knows when its stored copy is stale.
    pub revision: u64,
}

impl SystemFacts {
    /// Whether nothing has been scanned yet.
    pub fn is_empty(&self) -> bool {
        self.os.is_empty() && self.listening.is_empty() && self.services.is_empty()
    }

    /// One command that prints the three sections, each optional on a host that lacks it.
    pub fn scan_command() -> String {
        format!(
            "echo {os}; uname -srm 2>/dev/null; \
             echo {ports}; (ss -lntup 2>/dev/null || netstat -lntup 2>/dev/null) \
             | tail -n +2 | head -n {max}; \
             echo {units}; (systemctl list-units --type=service --state=running \
             --no-legend --no-pager 2>/dev/null || service --status-all 2>/dev/null) \
             | head -n {max}",
            os = SYSTEM_OS_MARKER,
            ports = SYSTEM_PORTS_MARKER,
            units = SYSTEM_UNITS_MARKER,
            max = MAX_SYSTEM_ENTRIES,
        )
    }

    /// Read a scan's output back.
    pub fn parse(built_ms: u64, output: &str) -> Self {
        let mut facts = Self {
            built_ms,
            revision: 1,
            ..Self::default()
        };
        let mut section = "";
        for line in output.lines() {
            match line.trim() {
                SYSTEM_OS_MARKER => section = "os",
                SYSTEM_PORTS_MARKER => section = "listening",
                SYSTEM_UNITS_MARKER => section = "services",
                _ => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match section {
                        "os" => {
                            if facts.os.is_empty() {
                                facts.os = line.to_owned();
                            }
                        }
                        "listening" if facts.listening.len() < MAX_SYSTEM_ENTRIES => {
                            facts.listening.push(line.to_owned());
                        }
                        "services" if facts.services.len() < MAX_SYSTEM_ENTRIES => {
                            facts.services.push(line.to_owned());
                        }
                        _ => {}
                    }
                }
            }
        }
        facts
    }

    /// What the prompt and the popup say about it.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "Nothing scanned yet. env_system reads the host's ports and services."
                .to_owned();
        }
        let mut lines = vec![format!("Host: {}", self.os)];
        if !self.listening.is_empty() {
            lines.push("Listening:".to_owned());
            lines.extend(self.listening.iter().cloned());
        }
        if !self.services.is_empty() {
            lines.push("Running services:".to_owned());
            lines.extend(self.services.iter().cloned());
        }
        lines.join("\n")
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        let revision = self.revision + 1;
        *self = Self {
            revision,
            ..Self::default()
        };
    }

    /// The stored shape.
    pub fn to_json(&self) -> Value {
        json!({
            "version": MAP_VERSION,
            "os": self.os,
            "builtMs": self.built_ms,
            "revision": self.revision,
            "listening": self.listening,
            "services": self.services,
        })
    }

    /// Read a stored copy back.
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let lines = |key: &str| -> Vec<String> {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect()
        };
        Self {
            os: value["os"].as_str().unwrap_or_default().to_owned(),
            built_ms: value["builtMs"].as_u64().unwrap_or_default(),
            revision: value["revision"].as_u64().unwrap_or_default(),
            listening: lines("listening"),
            services: lines("services"),
        }
    }
}

/// Section markers the system scan prints.
const SYSTEM_OS_MARKER: &str = "__wassh_os__";
const SYSTEM_PORTS_MARKER: &str = "__wassh_ports__";
const SYSTEM_UNITS_MARKER: &str = "__wassh_units__";

/// Directories a scan leaves alone: the same set the search tools skip.
const SKIPPED_DIRS: &str =
    "-not -path '*/.*' -not -path '*/node_modules*' -not -path '*/dist*' -not -path '*/build*' \
     -not -path '*/target*'";

/// What counts as a declaration worth mapping, across the common languages.
const SYMBOL_PATTERN: &str = "^[[:space:]]*((pub|export|async|static|public|private|protected|\
const|let|impl)[[:space:]]+)*(fn|struct|enum|trait|impl|class|def|function|interface|type|\
mod|macro_rules!)[[:space:]]+[A-Za-z_]";

/// `text` as one shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN: &str = "\
1200 1700000000 /srv/app/src/main.rs
430 1700000001 /srv/app/src/lib.rs
99 1700000002 /srv/app/README.md
__wassh_symbols__
/srv/app/src/main.rs:12:fn main() {
/srv/app/src/main.rs:30:pub struct App {
/srv/app/src/lib.rs:4:pub fn run() {
";

    #[test]
    fn a_scan_becomes_files_with_their_declarations() {
        let map = EnvMap::parse("/srv/app", 1_700_000_100_000, SCAN);
        assert_eq!(map.len(), 3);
        let main = &map.nodes["/srv/app/src/main.rs"];
        assert_eq!(main.size, 1200);
        assert_eq!(main.mtime, 1_700_000_000);
        assert_eq!(main.symbols, ["12\tfn main() {", "30\tpub struct App {"]);
        assert!(map.nodes["/srv/app/README.md"].symbols.is_empty());
        assert!(!map.truncated);
    }

    #[test]
    fn a_path_with_a_colon_still_parses() {
        let scan = "10 1 /srv/a:b/c.rs\n__wassh_symbols__\n/srv/a:b/c.rs:7:fn x() {\n";
        let map = EnvMap::parse("/srv", 0, scan);
        assert_eq!(map.len(), 1, "{:?}", map.nodes);
        assert_eq!(map.nodes["/srv/a:b/c.rs"].symbols, ["7\tfn x() {"]);
    }

    #[test]
    fn the_overview_groups_directories_and_names_recent_files() {
        let map = EnvMap::parse("/srv/app", 0, SCAN);
        let overview = map.overview();
        assert!(overview.contains("3 files"), "{overview}");
        assert!(overview.contains("src/  2"), "{overview}");
        assert!(overview.contains("(top level)  1"), "{overview}");
        assert!(overview.contains("Most recently changed:"), "{overview}");
        assert!(
            overview.find("README.md") < overview.find("main.rs"),
            "the newest file comes first: {overview}"
        );
    }

    #[test]
    fn an_empty_map_says_so_instead_of_drawing_nothing() {
        assert!(EnvMap::default().overview().contains("empty"));
    }

    #[test]
    fn find_matches_paths_before_declarations() {
        let map = EnvMap::parse("/srv/app", 0, SCAN);
        let hits = map.find("main", 10);
        assert_eq!(hits[0], "/srv/app/src/main.rs");
        let by_symbol = map.find("run", 10);
        assert!(
            by_symbol.iter().any(|hit| hit.contains("pub fn run")),
            "{by_symbol:?}"
        );
        assert!(map.find("   ", 10).is_empty(), "a blank needle finds nothing");
        assert_eq!(map.find("main", 1).len(), 1, "the limit applies");
    }

    #[test]
    fn an_outline_names_the_file_its_size_and_its_declarations() {
        let map = EnvMap::parse("/srv/app", 0, SCAN);
        let outline = map.outline("/srv/app/src/lib.rs").expect("an outline");
        assert!(outline.contains("430 bytes"), "{outline}");
        assert!(outline.contains("4: pub fn run() {"), "{outline}");
        assert!(map.outline("/srv/app/missing.rs").is_none());
    }

    #[test]
    fn a_map_survives_a_round_trip() {
        let map = EnvMap::parse("/srv/app", 42, SCAN);
        let restored = EnvMap::from_json(Some(&map.to_json()));
        assert_eq!(restored, map);
        assert_eq!(EnvMap::from_json(None), EnvMap::default());
    }

    #[test]
    fn the_scan_command_walks_one_root_and_marks_its_sections() {
        let command = EnvMap::scan_command("'/srv/app'");
        assert!(command.contains("find '/srv/app' -maxdepth"));
        assert!(command.contains("-type f"));
        assert!(command.contains(SECTION_MARKER));
        assert!(command.contains("grep -rnIE"));
        assert!(command.contains("head -n 5000"));
    }

    #[test]
    fn a_change_forgets_one_path() {
        let mut map = EnvMap::parse("/srv/app", 0, SCAN);
        map.forget("/srv/app/src/main.rs");
        assert_eq!(map.len(), 2);
        map.clear();
        assert!(map.is_empty());
    }

    #[test]
    fn a_relative_write_forgets_the_mapped_file() {
        let mut map = EnvMap::parse("/srv/app", 0, SCAN);
        map.forget_suffix("./src/lib.rs");
        assert_eq!(map.len(), 2);
        assert!(map.nodes.contains_key("/srv/app/src/main.rs"));
        map.forget_suffix("/");
        assert_eq!(map.len(), 2, "a bare slash forgets nothing");
    }

    #[test]
    fn search_weights_the_path_above_the_declarations() {
        let map = EnvMap::parse("/srv/app", 0, SCAN);
        let hits = map.search("main.rs", 10);
        assert_eq!(hits.first().map(String::as_str), Some("/srv/app/src/main.rs"));
        // "run" is only a declaration, so it still finds the file.
        assert_eq!(map.search("run", 10), ["/srv/app/src/lib.rs"]);
        assert!(map.search("   ", 10).is_empty());
        assert!(map.search("nothing-like-this", 10).is_empty());
    }

    #[test]
    fn a_system_scan_becomes_ports_and_services() {
        const OUTPUT: &str = "__wassh_os__\nLinux 6.1.0 x86_64\n__wassh_ports__\n\
                              tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n\
                              tcp LISTEN 0 128 127.0.0.1:8080 0.0.0.0:* users:((\"nginx\",pid=2,fd=6))\n\
                              __wassh_units__\n\
                              ssh.service loaded active running OpenBSD Secure Shell server\n";
        let facts = SystemFacts::parse(7, OUTPUT);
        assert_eq!(facts.os, "Linux 6.1.0 x86_64");
        assert_eq!(facts.listening.len(), 2);
        assert!(facts.listening[1].contains(":8080"), "{:?}", facts.listening);
        assert_eq!(facts.services.len(), 1);
        let summary = facts.summary();
        assert!(summary.contains("Listening:"), "{summary}");
        assert!(summary.contains("Running services:"), "{summary}");
        assert!(!facts.is_empty());
    }

    #[test]
    fn a_host_without_ss_or_systemd_scans_empty_but_says_what_it_found() {
        let facts = SystemFacts::parse(0, "__wassh_os__\nLinux 6.1\n__wassh_ports__\n\
                                       __wassh_units__\n");
        assert_eq!(facts.os, "Linux 6.1");
        assert!(facts.listening.is_empty());
        assert!(!facts.is_empty());
        assert!(SystemFacts::default().summary().contains("Nothing scanned"));
    }

    #[test]
    fn system_facts_survive_a_round_trip_and_clearing_bumps_the_revision() {
        let mut facts = SystemFacts::parse(5, "__wassh_os__\nLinux\n__wassh_ports__\n:22\n");
        let restored = SystemFacts::from_json(Some(&facts.to_json()));
        assert_eq!(restored, facts);
        facts.clear();
        assert!(facts.is_empty());
        assert_eq!(facts.revision, 2);
        assert_eq!(SystemFacts::from_json(None), SystemFacts::default());
    }

    #[test]
    fn the_system_scan_asks_for_the_three_sections() {
        let command = SystemFacts::scan_command();
        assert!(command.contains(SYSTEM_OS_MARKER));
        assert!(command.contains("ss -lntup"), "{command}");
        assert!(command.contains("systemctl list-units"), "{command}");
    }
}
