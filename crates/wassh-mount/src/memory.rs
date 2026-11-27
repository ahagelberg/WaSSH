//! A remote folder held in memory, for tests.
//!
//! It answers the same [`RemoteFs`](crate::RemoteFs) calls the SFTP adapter does, so the
//! driver's rules can be exercised without a host, a network or a kernel driver.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Attrs, FileId, FsError, FsResult, OpenMode, RemoteFs};

/// Something in the tree.
#[derive(Clone, Debug)]
enum Node {
    File {
        data: Vec<u8>,
        modified: u64,
        mode: u32,
    },
    Dir,
}

/// One tree, and the handles open on it.
pub struct MemoryFs {
    nodes: Mutex<BTreeMap<String, Node>>,
    open: Mutex<BTreeMap<FileId, String>>,
    next: AtomicU64,
}

impl MemoryFs {
    /// A tree holding only the root.
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_owned(), Node::Dir);
        Self {
            nodes: Mutex::new(nodes),
            open: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Put a file with `data` at `path`, creating the directories above it.
    pub fn write(&self, path: &str, data: &[u8]) {
        let mut nodes = self.nodes.lock().expect("the tree");
        make_parents(&mut nodes, path);
        nodes.insert(
            path.to_owned(),
            Node::File {
                data: data.to_vec(),
                modified: 1,
                mode: 0o644,
            },
        );
    }

    /// What `path` holds, for an assertion.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.nodes.lock().expect("the tree").get(path) {
            Some(Node::File { data, .. }) => Some(data.clone()),
            _ => None,
        }
    }

    /// Every path in the tree, for an assertion.
    pub fn paths(&self) -> Vec<String> {
        self.nodes
            .lock()
            .expect("the tree")
            .keys()
            .cloned()
            .collect()
    }

    /// The path behind an open handle.
    fn path_of(&self, file: FileId) -> FsResult<String> {
        self.open
            .lock()
            .expect("the open table")
            .get(&file)
            .cloned()
            .ok_or(FsError::NotFound)
    }
}

impl Default for MemoryFs {
    fn default() -> Self {
        Self::new()
    }
}

/// Create the directories above `path` in `nodes`.
fn make_parents(nodes: &mut BTreeMap<String, Node>, path: &str) {
    let mut current = String::new();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        current.push('/');
        current.push_str(part);
        nodes.entry(current.clone()).or_insert(Node::Dir);
    }
}

/// What one node looks like to the filesystem.
fn attrs_of(node: &Node) -> Attrs {
    match node {
        Node::Dir => Attrs {
            is_dir: true,
            mode: 0o755,
            ..Attrs::default()
        },
        Node::File { data, modified, mode } => Attrs {
            is_dir: false,
            size: data.len() as u64,
            modified: *modified,
            mode: *mode,
            ..Attrs::default()
        },
    }
}

impl RemoteFs for MemoryFs {
    fn stat(&self, path: &str) -> FsResult<Attrs> {
        let nodes = self.nodes.lock().expect("the tree");
        nodes.get(path).map(attrs_of).ok_or(FsError::NotFound)
    }

    fn open(
        &self,
        path: &str,
        _mode: OpenMode,
        create: bool,
        truncate: bool,
    ) -> FsResult<FileId> {
        let mut nodes = self.nodes.lock().expect("the tree");
        if !nodes.contains_key(path) {
            if !create {
                return Err(FsError::NotFound);
            }
            make_parents(&mut nodes, path);
            nodes.insert(
                path.to_owned(),
                Node::File {
                    data: Vec::new(),
                    modified: 1,
                    mode: 0o644,
                },
            );
        }
        match nodes.get_mut(path) {
            Some(Node::File { data, .. }) => {
                if truncate {
                    data.clear();
                }
            }
            Some(Node::Dir) => return Err(FsError::IsADirectory),
            None => return Err(FsError::NotFound),
        }
        let file = FileId(self.next.fetch_add(1, Ordering::Relaxed));
        self.open
            .lock()
            .expect("the open table")
            .insert(file, path.to_owned());
        Ok(file)
    }

    fn read(&self, file: FileId, offset: u64, len: u32) -> FsResult<Vec<u8>> {
        let path = self.path_of(file)?;
        let nodes = self.nodes.lock().expect("the tree");
        let Some(Node::File { data, .. }) = nodes.get(&path) else {
            return Err(FsError::NotFound);
        };
        let start = (offset as usize).min(data.len());
        let end = (start + len as usize).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn write(&self, file: FileId, offset: u64, data: &[u8]) -> FsResult<u32> {
        let path = self.path_of(file)?;
        let mut nodes = self.nodes.lock().expect("the tree");
        let Some(Node::File { data: stored, .. }) = nodes.get_mut(&path) else {
            return Err(FsError::NotFound);
        };
        let start = offset as usize;
        if stored.len() < start + data.len() {
            stored.resize(start + data.len(), 0);
        }
        stored[start..start + data.len()].copy_from_slice(data);
        Ok(data.len() as u32)
    }

    fn flush(&self, file: FileId) -> FsResult<()> {
        self.path_of(file).map(|_| ())
    }

    fn set_len(&self, file: FileId, size: u64) -> FsResult<()> {
        let path = self.path_of(file)?;
        let mut nodes = self.nodes.lock().expect("the tree");
        let Some(Node::File { data, .. }) = nodes.get_mut(&path) else {
            return Err(FsError::NotFound);
        };
        data.resize(size as usize, 0);
        Ok(())
    }

    fn set_times(&self, path: &str, _accessed: u64, modified: u64) -> FsResult<()> {
        let mut nodes = self.nodes.lock().expect("the tree");
        match nodes.get_mut(path) {
            Some(Node::File { modified: time, .. }) => {
                *time = modified;
                Ok(())
            }
            Some(Node::Dir) => Ok(()),
            None => Err(FsError::NotFound),
        }
    }

    fn close(&self, file: FileId) -> FsResult<()> {
        self.open
            .lock()
            .expect("the open table")
            .remove(&file)
            .map(|_| ())
            .ok_or(FsError::NotFound)
    }

    fn read_dir(&self, path: &str) -> FsResult<Vec<(String, Attrs)>> {
        let nodes = self.nodes.lock().expect("the tree");
        match nodes.get(path) {
            Some(Node::Dir) => {}
            Some(Node::File { .. }) => return Err(FsError::NotADirectory),
            None => return Err(FsError::NotFound),
        }
        let prefix = if path == "/" {
            "/".to_owned()
        } else {
            format!("{path}/")
        };
        let mut entries = Vec::new();
        for (entry, node) in nodes.iter() {
            let Some(name) = entry.strip_prefix(&prefix) else {
                continue;
            };
            if name.is_empty() || name.contains('/') {
                continue;
            }
            entries.push((name.to_owned(), attrs_of(node)));
        }
        Ok(entries)
    }

    fn mkdir(&self, path: &str) -> FsResult<()> {
        let mut nodes = self.nodes.lock().expect("the tree");
        if nodes.contains_key(path) {
            return Err(FsError::Exists);
        }
        make_parents(&mut nodes, path);
        nodes.insert(path.to_owned(), Node::Dir);
        Ok(())
    }

    fn remove(&self, path: &str) -> FsResult<()> {
        let mut nodes = self.nodes.lock().expect("the tree");
        match nodes.get(path) {
            Some(Node::File { .. }) => {
                nodes.remove(path);
                Ok(())
            }
            Some(Node::Dir) => Err(FsError::IsADirectory),
            None => Err(FsError::NotFound),
        }
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        let mut nodes = self.nodes.lock().expect("the tree");
        match nodes.get(path) {
            Some(Node::Dir) => {
                let prefix = format!("{path}/");
                if nodes.keys().any(|entry| entry.starts_with(&prefix)) {
                    return Err(FsError::NotEmpty);
                }
                nodes.remove(path);
                Ok(())
            }
            Some(Node::File { .. }) => Err(FsError::NotADirectory),
            None => Err(FsError::NotFound),
        }
    }

    fn rename(&self, from: &str, to: &str) -> FsResult<()> {
        let mut nodes = self.nodes.lock().expect("the tree");
        let node = nodes.remove(from).ok_or(FsError::NotFound)?;
        nodes.insert(to.to_owned(), node);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_answers_stat_open_and_read() {
        let fs = MemoryFs::new();
        fs.write("/etc/hosts", b"127.0.0.1 localhost");
        let attrs = fs.stat("/etc/hosts").expect("stat");
        assert!(!attrs.is_dir);
        assert_eq!(attrs.size, 19);
        assert!(fs.stat("/etc").expect("dir").is_dir);
        assert_eq!(fs.stat("/nope"), Err(FsError::NotFound));
    }

    #[test]
    fn a_listing_shows_the_children_of_one_directory() {
        let fs = MemoryFs::new();
        fs.write("/etc/hosts", b"x");
        fs.write("/etc/ssh/ssh_config", b"y");
        fs.write("/var/log/syslog", b"z");
        let mut names: Vec<_> = fs
            .read_dir("/etc")
            .expect("listing")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["hosts".to_owned(), "ssh".to_owned()]);
    }

    #[test]
    fn handles_write_at_offsets_and_grow_the_file() {
        let fs = MemoryFs::new();
        let file = fs
            .open("/new.txt", OpenMode::ReadWrite, true, true)
            .expect("created");
        assert_eq!(RemoteFs::write(&fs, file, 5, b"world").expect("wrote"), 5);
        assert_eq!(RemoteFs::write(&fs, file, 0, b"hello").expect("wrote"), 5);
        fs.close(file).expect("closed");
        assert_eq!(fs.read("/new.txt").expect("data"), b"helloworld");
    }

    #[test]
    fn a_directory_cannot_be_removed_while_it_holds_entries() {
        let fs = MemoryFs::new();
        fs.write("/a/b", b"x");
        assert_eq!(fs.rmdir("/a"), Err(FsError::NotEmpty));
        fs.remove("/a/b").expect("removed");
        fs.rmdir("/a").expect("removed the empty directory");
    }

    #[test]
    fn renaming_keeps_the_contents() {
        let fs = MemoryFs::new();
        fs.write("/one", b"body");
        fs.rename("/one", "/two").expect("renamed");
        assert_eq!(fs.read("/two").expect("data"), b"body");
        assert!(fs.stat("/one").is_err());
    }
}
