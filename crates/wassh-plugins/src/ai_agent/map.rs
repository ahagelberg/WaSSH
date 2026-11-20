//! The environment map: a per-host graph of the files behind the session.
//!
//! Built in two steps. `env_scan` asks the host for one `find` listing and one symbol grep
//! rather than letting the model discover the tree with `ls`, `grep` and `find` of its own;
//! with no directory given, the host finds the project root itself. A mapping pass then has
//! a sub-agent read the files that matter and write a summary and the connections of each
//! into the map, bounded by [`MAX_MAPPED_FILES`] so it costs a known number of tokens.
//!
//! `env_find`, `env_search`, `env_outline` and `env_overview` answer from the map, so a
//! question about where something lives, or what talks to what, costs no host call at all.
//!
//! The map is host state, not conversation state: it outlives the chat that built it and
//! is written back to the host's plugin data, so every chat on the connection shares it.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Version of the stored shape. 2 adds the summaries and the links between files.
pub const MAP_VERSION: u32 = 2;
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
/// Score a query token adds when a file's summary holds it.
const SUMMARY_WEIGHT: usize = 2;
/// Entries one system scan keeps per section.
pub const MAX_SYSTEM_ENTRIES: usize = 60;
/// Directories between two lines of an overview.
pub const OVERVIEW_DEPTH: usize = 2;
/// Group name for the files that sit in the root itself.
pub const TOP_LEVEL: &str = "(top level)";
/// Files the overview names as recently changed.
pub const OVERVIEW_RECENT: usize = 12;
/// Files one mapping pass may describe. Past this the pass costs more than it saves.
pub const MAX_MAPPED_FILES: usize = 100;
/// Characters of one file's summary kept.
pub const MAX_MAP_SUMMARY_CHARS: usize = 600;
/// Connections kept per file.
pub const MAX_MAP_LINKS: usize = 16;
/// Characters of a mapping answer read back at most.
pub const MAX_MAP_ANSWER_CHARS: usize = 200_000;

/// Line the scan command prints between its two sections.
pub const SECTION_MARKER: &str = "__wassh_symbols__";

/// Prefix of the line the scan command prints the root it walked on.
pub const ROOT_MARKER: &str = "__wassh_root__";

/// Files that mark a directory as the root of a project.
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "Cargo.toml",
    "go.mod",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "Makefile",
    "CMakeLists.txt",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
    "mix.exs",
    "deno.json",
    "Package.swift",
];

/// Shell that prints the project root.
///
/// The nearest directory at or above the working directory carrying one of
/// [`PROJECT_MARKERS`] wins, which is the repository a session was started in. When there is
/// none — a session opened in a home directory, say — the one project folder directly inside
/// the working directory is used instead. Anything else stays where the session is, because
/// guessing further would map the wrong tree.
fn root_probe() -> String {
    let mut markers = String::new();
    for marker in PROJECT_MARKERS {
        markers.push_str(&format!("{marker} "));
    }
    format!(
        "root=$PWD; found=; \
         while [ \"$root\" != / ] && [ \"$root\" != \"$HOME\" ]; do \
           for m in {markers}; do [ -e \"$root/$m\" ] && {{ found=1; break; }}; done; \
           [ -n \"$found\" ] && break; root=$(dirname \"$root\"); \
         done; \
         if [ -z \"$found\" ]; then \
           picks=0; pick=; \
           for d in */; do \
             [ -d \"$d\" ] || continue; \
             for m in {markers}; do \
               [ -e \"$d/$m\" ] && {{ picks=$((picks+1)); pick=$d; break; }}; \
             done; \
           done; \
           [ \"$picks\" = 1 ] && root=\"$PWD/${{pick%/}}\"; \
         fi; \
         printf '{ROOT_MARKER}%s\\n' \"$root\""
    )
}

/// The seconds of a `find -printf %T@` stamp, which carries a fractional part.
fn whole_seconds(stamp: &str) -> Option<u64> {
    stamp.split('.').next()?.parse().ok()
}

/// One file in the map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Size in bytes.
    pub size: u64,
    /// Modification time, in seconds since the epoch.
    pub mtime: u64,
    /// Declaration lines: `line␉text`.
    pub symbols: Vec<String>,
    /// What the file is for, in the mapping pass's words.
    pub summary: String,
    /// What it connects to: the files, modules or services it imports, calls or feeds.
    pub links: Vec<String>,
}

/// A host's map of its own files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvMap {
    /// Directory the map was built from.
    pub root: String,
    /// When it was built, in milliseconds since the epoch.
    pub built_ms: u64,
    /// When a mapping pass last described files, in milliseconds since the epoch.
    pub mapped_ms: u64,
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

    /// Whether the tree is listed but nothing in it has been described yet, which is when a
    /// mapping pass is worth its tokens.
    pub fn needs_summaries(&self) -> bool {
        !self.nodes.is_empty() && self.nodes.values().all(|node| node.summary.is_empty())
    }

    /// How many files the mapping pass described.
    pub fn summarised(&self) -> usize {
        self.nodes
            .values()
            .filter(|node| !node.summary.is_empty())
            .count()
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
    /// `root` is the directory to walk; it is quoted by the caller's rules. An empty `root`
    /// asks the host to find the project root itself, so a session started deep inside a
    /// repository maps the repository. The root actually walked is printed on a marker line.
    pub fn scan_command(root: &str) -> String {
        let root = root.trim();
        let locate = if root.is_empty() {
            root_probe()
        } else {
            format!(
                "root={}; printf '{ROOT_MARKER}%s\\n' \"$root\"",
                shell_quote(root)
            )
        };
        format!(
            "{locate}; \
             find \"$root\" -maxdepth {MAX_DEPTH} {skipped} -type f -printf '%s %T@ %p\\n' \
             2>/dev/null | head -n {MAX_NODES}; \
             echo {SECTION_MARKER}; \
             grep -rnIE {pattern} \"$root\" 2>/dev/null | head -n {MAX_SCAN_SYMBOLS}",
            skipped = SKIPPED_DIRS,
            pattern = shell_quote(SYMBOL_PATTERN),
        )
    }

    /// Read a scan's output back. Returns the map and whether a cap was hit.
    ///
    /// The root the command walked wins over `root`, because a command given no directory
    /// finds the project root itself.
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
            if let Some(found) = line.strip_prefix(ROOT_MARKER) {
                let found = found.trim();
                if !found.is_empty() {
                    map.root = found.to_owned();
                }
                continue;
            }
            if line.trim() == SECTION_MARKER {
                section = false;
                continue;
            }
            if section {
                let mut parts = line.splitn(3, ' ');
                let (Some(size), Some(stamp), Some(path)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let (Ok(size), Some(mtime)) = (size.parse::<u64>(), whole_seconds(stamp)) else {
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
                        ..Node::default()
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
            "{} ({} files{}{})",
            self.root,
            self.len(),
            if self.truncated { ", capped" } else { "" },
            match self.summarised() {
                0 => ", no summaries yet".to_owned(),
                described => format!(", {described} described"),
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
            let symbols = format!("{} {}", node.symbols.join(" "), node.summary).to_lowercase();
            let mut score = 0usize;
            for token in &tokens {
                if name.contains(token.as_str()) {
                    score += PATH_WEIGHT;
                }
                if symbols.contains(token.as_str()) {
                    score += SYMBOL_WEIGHT;
                }
                if node.summary.to_lowercase().contains(token.as_str()) {
                    score += SUMMARY_WEIGHT;
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
            .map(|(_, path)| match self.nodes.get(&path) {
                // A described file says what it is for, so the model often needs no other call.
                Some(node) if !node.summary.is_empty() => format!("{path} — {}", node.summary),
                _ => path,
            })
            .collect()
    }

    /// What the map knows about one file: its size, age, summary, connections and
    /// declarations.
    pub fn outline(&self, path: &str) -> Option<String> {
        let node = self.nodes.get(path)?;
        let mut text = format!("{path}\n{} bytes, changed {}", node.size, node.mtime);
        if !node.summary.is_empty() {
            text.push('\n');
            text.push_str(&node.summary);
        }
        if !node.links.is_empty() {
            text.push_str("\nConnects to: ");
            text.push_str(&node.links.join(", "));
        }
        if let Some(outline) = outline_of(&node.symbols) {
            text.push('\n');
            text.push_str(&outline);
        }
        Some(text)
    }

    /// Take a mapping pass's descriptions into the map.
    ///
    /// Only files under `root` are kept, so an answer that wandered outside the tree cannot
    /// grow the map. Returns how many files were described.
    pub fn merge_summaries(&mut self, root: &str, mapped_ms: u64, files: &[MappedFile]) -> usize {
        let root = root.trim().trim_end_matches('/');
        let mut described = 0usize;
        for file in files.iter().take(MAX_MAPPED_FILES) {
            let path = if file.path.starts_with('/') {
                file.path.clone()
            } else {
                format!("{root}/{}", file.path.trim_start_matches("./"))
            };
            if !path.starts_with(root) || path.len() <= root.len() {
                continue;
            }
            let summary: String = file
                .summary
                .trim()
                .chars()
                .take(MAX_MAP_SUMMARY_CHARS)
                .collect();
            if summary.is_empty() {
                continue;
            }
            let links: Vec<String> = file
                .links
                .iter()
                .map(|link| link.trim())
                .filter(|link| !link.is_empty())
                .take(MAX_MAP_LINKS)
                .map(str::to_owned)
                .collect();
            let node = self.nodes.entry(path).or_default();
            node.summary = summary;
            node.links = links;
            described += 1;
        }
        if described > 0 {
            if self.root.is_empty() {
                self.root = root.to_owned();
            }
            self.mapped_ms = mapped_ms;
            self.revision += 1;
        }
        described
    }

    /// The stored shape.
    pub fn to_json(&self) -> Value {
        let nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|(path, node)| {
                json!([
                    path,
                    node.size,
                    node.mtime,
                    node.symbols,
                    node.summary,
                    node.links
                ])
            })
            .collect();
        json!({
            "version": MAP_VERSION,
            "root": self.root,
            "builtMs": self.built_ms,
            "mappedMs": self.mapped_ms,
            "revision": self.revision,
            "truncated": self.truncated,
            "nodes": nodes,
        })
    }

    /// Read a stored map back. A version 1 map has no summaries: those nodes keep empty
    /// ones and ask for a mapping pass.
    pub fn from_json(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let mut map = Self {
            root: value["root"].as_str().unwrap_or_default().to_owned(),
            built_ms: value["builtMs"].as_u64().unwrap_or_default(),
            mapped_ms: value["mappedMs"].as_u64().unwrap_or_default(),
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
            let links: Vec<String> = entry[5]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|link| link.as_str().map(str::to_owned))
                .collect();
            map.nodes.insert(
                path.to_owned(),
                Node {
                    size: entry[1].as_u64().unwrap_or_default(),
                    mtime: entry[2].as_u64().unwrap_or_default(),
                    symbols,
                    summary: entry[4].as_str().unwrap_or_default().to_owned(),
                    links,
                },
            );
        }
        map
    }
}

/// One file as a mapping pass describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MappedFile {
    /// The file, absolute or relative to the map's root.
    pub path: String,
    /// What it is for.
    pub summary: String,
    /// What it connects to.
    pub links: Vec<String>,
}

/// Read the `{"files": [...]}` object a mapping pass answers with.
///
/// The model may fence it or wrap it in a sentence, so every fenced block is tried and then
/// the widest brace-delimited run of the text.
pub fn parse_mapped(answer: &str) -> Vec<MappedFile> {
    let answer: String = answer.chars().take(MAX_MAP_ANSWER_CHARS).collect();
    let mut candidates: Vec<&str> = Vec::new();
    let mut rest = answer.as_str();
    while let Some(open) = rest.find("```") {
        rest = &rest[open + 3..];
        let Some(close) = rest.find("```") else {
            break;
        };
        let block = &rest[..close];
        // Drop the fence's language tag.
        candidates.push(match block.split_once('\n') {
            Some((first, body)) if !first.contains('{') => body,
            _ => block,
        });
        rest = &rest[close + 3..];
    }
    if let (Some(start), Some(end)) = (answer.find('{'), answer.rfind('}')) {
        candidates.push(&answer[start..=end]);
    }
    candidates
        .iter()
        .find_map(|candidate| read_files(candidate))
        .unwrap_or_default()
}

/// The files of one `{"files": …}` object, or `None` when it does not hold one.
fn read_files(text: &str) -> Option<Vec<MappedFile>> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let files = value.get("files")?.as_array()?;
    let mapped: Vec<MappedFile> = files
        .iter()
        .filter_map(|file| {
            let path = file["path"].as_str()?.trim();
            if path.is_empty() {
                return None;
            }
            let links: Vec<String> = file["connects"]
                .as_array()
                .or_else(|| file["connections"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|link| link.as_str().map(str::to_owned))
                .collect();
            Some(MappedFile {
                path: path.to_owned(),
                summary: file["summary"].as_str().unwrap_or_default().to_owned(),
                links,
            })
        })
        .collect();
    (!mapped.is_empty()).then_some(mapped)
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
    fn a_mapping_pass_describes_files_and_their_links() {
        let mut map = EnvMap::parse("/srv/app", 42, SCAN);
        assert!(map.needs_summaries(), "a fresh scan has nothing described");
        let files = vec![
            MappedFile {
                path: "/srv/app/src/main.rs".to_owned(),
                summary: "Starts the daemon and reads its settings.".to_owned(),
                links: vec!["src/lib.rs".to_owned(), "settings".to_owned()],
            },
            MappedFile {
                path: "src/lib.rs".to_owned(),
                summary: "Runs a job from the queue.".to_owned(),
                links: vec![],
            },
            // Outside the map's tree: an answer may wander, the map does not follow.
            MappedFile {
                path: "/etc/passwd".to_owned(),
                summary: "not ours".to_owned(),
                links: vec![],
            },
        ];
        let described = map.merge_summaries("/srv/app", 99, &files);
        assert_eq!(described, 2, "only the files under the root are taken");
        assert!(!map.needs_summaries());
        assert_eq!(map.summarised(), 2);
        assert_eq!(map.mapped_ms, 99);
        assert_eq!(
            map.nodes["/srv/app/src/lib.rs"].summary,
            "Runs a job from the queue."
        );
        assert_eq!(
            map.nodes["/srv/app/src/main.rs"].links,
            ["src/lib.rs", "settings"]
        );
        // The descriptions answer questions the declarations cannot.
        let hits = map.search("queue", 5);
        assert!(hits.iter().any(|hit| hit == "/srv/app/src/lib.rs — Runs a job from the queue."), "{hits:?}");
        let outline = map.outline("/srv/app/src/main.rs").expect("an outline");
        assert!(outline.contains("Starts the daemon"), "{outline}");
        assert!(outline.contains("Connects to: src/lib.rs, settings"), "{outline}");
        // And they survive the store.
        let restored = EnvMap::from_json(Some(&map.to_json()));
        assert_eq!(restored, map);
    }

    #[test]
    fn a_mapping_answer_is_read_out_of_its_fence() {
        let answer = "Here is the map:\n\n```json\n{\"files\": [\
            {\"path\": \"src/main.rs\", \"summary\": \"Entry point.\", \"connects\": [\"lib\"]},\
            {\"path\": \"src/lib.rs\", \"summary\": \"Library.\"}]}\n```\n";
        let files = parse_mapped(answer);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].summary, "Entry point.");
        assert_eq!(files[0].links, ["lib"]);
        assert!(files[1].links.is_empty());
        assert_eq!(parse_mapped("no map here"), Vec::new());
    }

    #[test]
    fn the_scan_command_walks_one_root_and_marks_its_sections() {
        let command = EnvMap::scan_command("/srv/app");
        assert!(command.contains("root='/srv/app'"), "{command}");
        assert!(command.contains(&format!("printf '{ROOT_MARKER}%s")), "{command}");
        assert!(command.contains("find \"$root\" -maxdepth"), "{command}");
        assert!(command.contains("-type f"));
        assert!(command.contains(SECTION_MARKER));
        assert!(command.contains("grep -rnIE"));
        assert!(command.contains("head -n 5000"));
    }

    #[test]
    fn a_scan_without_a_root_finds_the_project_root_itself() {
        let command = EnvMap::scan_command("");
        assert!(command.contains("Cargo.toml"), "no project marker: {command}");
        assert!(command.contains(ROOT_MARKER), "{command}");
        assert!(!command.contains("root=''"), "{command}");
    }

    /// A POSIX shell, where the machine has one.
    fn shell() -> Option<&'static str> {
        ["/bin/sh", "C:/Program Files/Git/bin/sh.exe"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
    }

    #[test]
    fn the_probe_prefers_the_project_inside_or_below_the_session() {
        // Shell paths and Windows paths mean different things to `sh`, so the tree is built on
        // disk and every comparison happens inside the shell.
        let Some(shell) = shell() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("wassh-probe-{}", std::process::id()));
        std::fs::create_dir_all(base.join("home").join("ayla").join(".git")).expect("a project");
        std::fs::create_dir_all(base.join("home").join("downloads")).expect("another directory");
        let script = "fail=; home=$(cd home && pwd -P) || exit 1
            check() { out=$(__PROBE__); root=${out#__wassh_root__}; [ \"$root\" = \"$1\" ] || fail=\"$fail $2\"; }
            cd \"$home\" && HOME=$home && check \"$home/ayla\" a-session-in-the-home-directory
            cd \"$home/ayla/.git\" && HOME=$home && check \"$home/ayla\" a-session-inside-the-project
            mkdir -p \"$home/other/.git\"
            cd \"$home\" && HOME=$home && check \"$home\" two-projects-side-by-side
            echo \"${fail:-pass}\"";
        let script = script.replace("__PROBE__", &root_probe());
        let output = std::process::Command::new(shell)
            .arg("-c")
            .arg(script)
            .current_dir(&base)
            .output()
            .expect("the shell runs");
        let said = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(said, "pass", "the probe picked the wrong root:{said}");
    }

    #[test]
    fn a_scan_reports_the_root_it_walked() {
        let output = format!("{ROOT_MARKER}/srv/app\n12 100 /srv/app/a.rs\n{SECTION_MARKER}\n");
        let map = EnvMap::parse("", 7, &output);
        assert_eq!(map.root, "/srv/app", "the marker names the root");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn the_whole_seconds_of_a_find_stamp_are_read() {
        // `find -printf '%T@'` prints the fraction too, and a map that drops those lines is
        // empty whatever the tree holds.
        assert_eq!(whole_seconds("1795170949.2881875000"), Some(1_795_170_949));
        assert_eq!(whole_seconds("1795170949"), Some(1_795_170_949));
        assert_eq!(whole_seconds("not a stamp"), None);
        let scan = format!(
            "{ROOT_MARKER}/srv/app\n12 1795170949.2881875000 /srv/app/a.rs\n{SECTION_MARKER}\n"
        );
        assert_eq!(
            EnvMap::parse("", 7, &scan)
                .nodes
                .get("/srv/app/a.rs")
                .map(|node| node.size),
            Some(12)
        );
    }

    #[test]
    fn a_real_scan_lists_the_files_and_their_declarations() {
        let Some(shell) = shell() else {
            return;
        };
        let base = std::env::temp_dir().join(format!("wassh-scan-{}", std::process::id()));
        let tree = base.join("tree");
        std::fs::create_dir_all(tree.join(".git")).expect("a project");
        std::fs::create_dir_all(tree.join("src")).expect("a source directory");
        std::fs::write(tree.join("src").join("main.rs"), "fn main() {\n    run();\n}\n")
            .expect("a source file");
        std::fs::write(tree.join("README.md"), "# Notes\n").expect("a readme");
        let command = EnvMap::scan_command("");
        let output = std::process::Command::new(shell)
            .arg("-c")
            .arg(format!("cd \"$1\" && {command}", command = command))
            .arg("sh")
            .arg(&tree)
            .output()
            .expect("the shell runs");
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let map = EnvMap::parse("", 7, &text);
        let _ = std::fs::remove_dir_all(&base);
        assert!(map.root.ends_with("tree"), "walked the project root: {}", map.root);
        let main = map
            .nodes
            .iter()
            .find(|(path, _)| path.ends_with("src/main.rs"))
            .map(|(_, node)| node)
            .unwrap_or_else(|| panic!("the tree is not in the map: {:?}", map.nodes.keys()));
        assert!(main.size > 0, "the file's size was read");
        assert!(main.mtime > 0, "the file's age was read");
        assert!(
            main.symbols.iter().any(|line| line.contains("fn main")),
            "the declarations were found: {:?}",
            main.symbols
        );
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
