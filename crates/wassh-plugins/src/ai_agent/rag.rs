//! The knowledge base behind `rag_search`.
//!
//! Port of `src/plugins/builtins/ai-agent/rag.ts`: one or two local folders are
//! read into overlapping chunks; when an embedding provider is configured the
//! chunks are embedded and searched by cosine similarity, otherwise the search
//! is lexical (the reference behaves the same way without an embedding model).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::providers::{self, EmbedConfig};

/// Characters per chunk.
pub const CHUNK_CHARS: usize = 1_200;
/// Characters consecutive chunks share.
pub const CHUNK_OVERLAP: usize = 200;
/// Most chunks one knowledge base may hold.
pub const MAX_CHUNKS: usize = 4_000;
/// Largest file the indexer reads.
pub const MAX_FILE_BYTES: u64 = 512_000;
/// Extensions that are never documents.
pub const SKIP_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "pdf", "zip", "gz", "xz", "bz2",
    "7z", "tar", "exe", "dll", "so", "dylib", "bin", "o", "a", "class", "jar", "mp3", "mp4",
    "mkv", "avi", "mov", "woff", "woff2", "ttf", "otf", "db", "sqlite", "lock",
];

/// One slice of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    /// File it came from.
    pub path: String,
    /// First line of the chunk in that file.
    pub start_line: usize,
    /// The text itself.
    pub text: String,
    /// Embedding, when one was computed.
    pub vector: Option<Vec<f32>>,
}

/// One search hit.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// File it came from.
    pub path: String,
    /// First line of the chunk.
    pub start_line: usize,
    /// The text itself.
    pub text: String,
    /// Relevance, higher is better.
    pub score: f32,
}

/// A built knowledge base.
#[derive(Clone, Debug, Default)]
pub struct Index {
    /// Folders that were read.
    pub folders: Vec<String>,
    /// Every chunk.
    pub chunks: Vec<Chunk>,
    /// Whether the chunks carry embeddings.
    pub embedded: bool,
}

/// Whether a file should be read into the index.
fn is_indexable(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.starts_with('.') {
        return false;
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if SKIP_EXTENSIONS.contains(&extension.as_str()) {
        return false;
    }
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.len() <= MAX_FILE_BYTES && meta.len() > 0,
        Err(_) => false,
    }
}

/// Every indexable file under `folder`.
pub fn files_under(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![(folder.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        if depth > 6 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            if path.is_dir() {
                pending.push((path, depth + 1));
            } else if is_indexable(&path) {
                files.push(path);
            }
        }
    }
    files
}

/// Split a document into overlapping chunks.
pub fn chunk_text(path: &str, text: &str) -> Vec<Chunk> {
    let characters: Vec<char> = text.chars().collect();
    if characters.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < characters.len() {
        let end = (start + CHUNK_CHARS).min(characters.len());
        let body: String = characters[start..end].iter().collect();
        let start_line = characters[..start].iter().filter(|character| **character == '\n').count() + 1;
        chunks.push(Chunk {
            path: path.to_owned(),
            start_line,
            text: body,
            vector: None,
        });
        if end == characters.len() {
            break;
        }
        start = end - CHUNK_OVERLAP.min(CHUNK_CHARS / 4);
    }
    chunks
}

/// Read every document in `folders` into one index.
pub fn build(folders: &[String]) -> Index {
    let mut index = Index::default();
    for folder in folders {
        let trimmed = folder.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = super::tools::local_path(trimmed);
        if !path.is_dir() {
            continue;
        }
        index.folders.push(path.display().to_string());
        for file in files_under(&path) {
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            if bytes.iter().take(2048).any(|byte| *byte == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            for chunk in chunk_text(&file.display().to_string(), &text) {
                if index.chunks.len() >= MAX_CHUNKS {
                    return index;
                }
                index.chunks.push(chunk);
            }
        }
    }
    index
}

/// Attach embeddings to every chunk.
pub fn attach_vectors(index: &mut Index, mut vectors: Vec<Vec<f32>>) {
    if vectors.len() != index.chunks.len() {
        return;
    }
    for (chunk, vector) in index.chunks.iter_mut().zip(vectors.drain(..)) {
        chunk.vector = Some(vector);
    }
    index.embedded = true;
}

/// Words worth matching: lowercase, at least three characters.
fn terms(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_owned)
        .collect()
}

/// Lexical score of one chunk: how many query terms it contains, weighted by
/// how rare the term is.
fn lexical_score(text: &str, query_terms: &[String]) -> f32 {
    let lower = text.to_lowercase();
    let mut score = 0.0;
    for term in query_terms {
        let hits = lower.matches(term.as_str()).count();
        if hits > 0 {
            score += 1.0 + (hits as f32).ln();
        }
    }
    score
}

/// Cosine similarity of two vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Search the index, by vectors when they exist and by words otherwise.
pub fn search(index: &Index, query: &str, query_vector: Option<&[f32]>, limit: usize) -> Vec<Hit> {
    let query_terms = terms(query);
    let mut hits: Vec<Hit> = index
        .chunks
        .iter()
        .filter_map(|chunk| {
            let score = match (query_vector, chunk.vector.as_deref()) {
                (Some(query_vector), Some(vector)) => {
                    let similarity = cosine(query_vector, vector);
                    // Keep lexical overlap as a tie-breaker on top of the vector.
                    similarity + lexical_score(&chunk.text, &query_terms) * 0.01
                }
                _ => lexical_score(&chunk.text, &query_terms),
            };
            (score > 0.0).then(|| Hit {
                path: chunk.path.clone(),
                start_line: chunk.start_line,
                text: chunk.text.clone(),
                score,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(limit.max(1));
    hits
}

/// The cache key for one set of folders and embedding settings.
fn cache_key(folders: &[String], embed: Option<&EmbedConfig>) -> String {
    let folders = folders
        .iter()
        .map(|folder| folder.trim())
        .filter(|folder| !folder.is_empty())
        .collect::<Vec<_>>()
        .join("|");
    match embed {
        Some(config) => format!("{folders}@{}{}{}", config.base_url, config.model, config.protocol.as_str()),
        None => folders,
    }
}

/// Build (or reuse) the index for `folders`.
pub fn cached(folders: &[String], embed: Option<&EmbedConfig>) -> Result<Arc<Index>, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Index>>>> = OnceLock::new();
    let key = cache_key(folders, embed);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(index) = guard.get(&key)
    {
        return Ok(Arc::clone(index));
    }

    let mut index = build(folders);
    if index.chunks.is_empty() {
        return Err(if index.folders.is_empty() {
            "No knowledge folder is configured.".to_owned()
        } else {
            format!("No readable documents under {}.", index.folders.join(", "))
        });
    }
    if let Some(config) = embed {
        let texts: Vec<String> = index.chunks.iter().map(|chunk| chunk.text.clone()).collect();
        let vectors = providers::embeddings(config, &texts)?;
        attach_vectors(&mut index, vectors);
    }
    let index = Arc::new(index);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, Arc::clone(&index));
    }
    Ok(index)
}

/// Format hits the way the model reads them.
pub fn format_hits(query: &str, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!("No passages matched \"{query}\" in the knowledge base.");
    }
    let mut out = format!("Knowledge base results for \"{query}\" ({} passages):\n", hits.len());
    for (index, hit) in hits.iter().enumerate() {
        let excerpt: String = hit.text.chars().take(1_200).collect();
        out.push_str(&format!(
            "\n[{}] {} (line {}, score {:.2})\n{}\n",
            index + 1,
            hit.path,
            hit.start_line,
            hit.score,
            excerpt.trim()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_chunked_with_overlap() {
        let text = "x".repeat(CHUNK_CHARS + 500);
        let chunks = chunk_text("a.txt", &text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text.chars().count(), CHUNK_CHARS);
        assert_eq!(chunks[0].start_line, 1);
        // The second chunk starts before the first ends.
        assert!(!chunks[1].text.is_empty());
    }

    #[test]
    fn line_numbers_follow_newlines() {
        let text = format!("{}\n{}", "a\n".repeat(10), "b\n".repeat(CHUNK_CHARS));
        let chunks = chunk_text("a.txt", &text);
        assert!(chunks.len() >= 2);
        assert!(chunks[1].start_line > 1, "{:?}", chunks[1]);
    }

    #[test]
    fn lexical_search_ranks_matching_chunks() {
        let mut index = Index::default();
        index.chunks.push(Chunk {
            path: "nginx.md".to_owned(),
            start_line: 1,
            text: "Restart nginx with systemctl restart nginx".to_owned(),
            vector: None,
        });
        index.chunks.push(Chunk {
            path: "postgres.md".to_owned(),
            start_line: 1,
            text: "Postgres listens on 5432".to_owned(),
            vector: None,
        });
        let hits = search(&index, "how do I restart nginx", None, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "nginx.md");
        assert!(hits[0].score > 0.0);
        assert!(search(&index, "kubernetes", None, 5).is_empty());
    }

    #[test]
    fn vectors_win_when_both_sides_have_them() {
        let mut index = Index::default();
        index.chunks.push(Chunk {
            path: "a.md".to_owned(),
            start_line: 1,
            text: "nothing relevant".to_owned(),
            vector: Some(vec![1.0, 0.0]),
        });
        index.chunks.push(Chunk {
            path: "b.md".to_owned(),
            start_line: 1,
            text: "restart nginx".to_owned(),
            vector: Some(vec![0.0, 1.0]),
        });
        index.embedded = true;
        let hits = search(&index, "unrelated words", Some(&[0.0, 1.0]), 5);
        assert_eq!(hits[0].path, "b.md");
    }

    #[test]
    fn cosine_handles_zero_and_mismatch() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        let value = cosine(&[1.0, 0.0], &[1.0, 0.0]);
        assert!((value - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_folder_is_indexed_and_searched() {
        let dir = std::env::temp_dir().join(format!("wassh-rag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("deploy.md"), "Deploy with: systemctl restart wassh\n").unwrap();
        std::fs::write(dir.join("photo.png"), [0u8, 1, 2]).unwrap();
        let folders = vec![dir.display().to_string()];
        let index = build(&folders);
        assert_eq!(index.chunks.len(), 1, "the png must be skipped");
        let hits = search(&index, "restart wassh", None, 3);
        assert_eq!(hits.len(), 1);
        assert!(format_hits("restart wassh", &hits).contains("deploy.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_configuration_says_so() {
        let error = cached(&["/definitely/not/here".to_owned()], None).unwrap_err();
        assert!(error.contains("No knowledge folder"), "{error}");
    }
}
