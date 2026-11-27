//! A remote folder, as a filesystem.
//!
//! The crate is in two halves. [`RemoteFs`] and its types are platform-neutral: paths are
//! `/`-separated, times are Unix seconds, and nothing here knows about a driver. The
//! Windows half (see [`windows`]) turns one of those into a drive letter or a directory
//! through WinFsp, and it is the only place with `unsafe` code.
//!
//! Keeping the two apart is what makes the filesystem testable: the driver's rules live in
//! plain functions over [`RemoteFs`], so they can be exercised against an in-memory tree
//! without a driver, a host or a network.

#![deny(unsafe_op_in_unsafe_fn)]

mod sftp_fs;

#[cfg(test)]
mod memory;

#[cfg(windows)]
pub mod windows;

pub use sftp_fs::SftpFs;

use std::fmt;

/// What a remote file is, as a filesystem needs to know it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    /// Whether this is a directory.
    pub is_dir: bool,
    /// Whether the remote entry is a symbolic link.
    pub is_link: bool,
    /// Length in bytes.
    pub size: u64,
    /// Last change, in seconds since the Unix epoch; 0 when unknown.
    pub modified: u64,
    /// Last read, in seconds since the Unix epoch; 0 when unknown.
    pub accessed: u64,
    /// Unix permission bits, or 0 when the server sent none.
    pub mode: u32,
}

/// How a file is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// Read only.
    Read,
    /// Write only.
    Write,
    /// Both.
    ReadWrite,
}

impl OpenMode {
    /// Whether the handle may read.
    pub fn reads(self) -> bool {
        !matches!(self, Self::Write)
    }

    /// Whether the handle may write.
    pub fn writes(self) -> bool {
        !matches!(self, Self::Read)
    }
}

/// One open file, as the filesystem names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u64);

/// Why a filesystem call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsError {
    /// Nothing exists at the path.
    NotFound,
    /// The user may not do this.
    Denied,
    /// The name is already taken.
    Exists,
    /// The directory still holds entries.
    NotEmpty,
    /// The path names a directory where a file was expected.
    IsADirectory,
    /// The path names a file where a directory was expected.
    NotADirectory,
    /// The operation is not something a remote folder can do.
    Unsupported,
    /// The connection behind the filesystem is gone right now; the mount may come back.
    Disconnected,
    /// The server failed the call for another reason.
    Io(String),
}

impl FsError {
    /// Whether the connection, rather than the request, is at fault.
    pub fn is_disconnected(&self) -> bool {
        matches!(self, Self::Disconnected)
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no such file"),
            Self::Denied => formatter.write_str("permission denied"),
            Self::Exists => formatter.write_str("already exists"),
            Self::NotEmpty => formatter.write_str("the directory is not empty"),
            Self::IsADirectory => formatter.write_str("the path is a directory"),
            Self::NotADirectory => formatter.write_str("the path is not a directory"),
            Self::Unsupported => formatter.write_str("not supported on a remote folder"),
            Self::Disconnected => formatter.write_str("the connection is closed"),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for FsError {}

/// The answer of one filesystem call.
pub type FsResult<T> = Result<T, FsError>;

/// A remote folder, as a filesystem works with it.
///
/// Every method blocks: a driver calls it from a callback thread and waits for the answer.
pub trait RemoteFs: Send + Sync {
    /// What `path` is.
    fn stat(&self, path: &str) -> FsResult<Attrs>;

    /// Open `path`, answering the handle the other calls use.
    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        create: bool,
        truncate: bool,
    ) -> FsResult<FileId>;

    /// Up to `len` bytes at `offset`; a short answer means the file ends there.
    fn read(&self, file: FileId, offset: u64, len: u32) -> FsResult<Vec<u8>>;

    /// Write `data` at `offset`, answering how many bytes went out.
    fn write(&self, file: FileId, offset: u64, data: &[u8]) -> FsResult<u32>;

    /// Ask the far side to put earlier writes on its disk.
    fn flush(&self, file: FileId) -> FsResult<()>;

    /// Cut an open file to `size` bytes, extending it with zeros when it grows.
    fn set_len(&self, file: FileId, size: u64) -> FsResult<()>;

    /// Set the times of `path`, in seconds since the Unix epoch.
    fn set_times(&self, path: &str, accessed: u64, modified: u64) -> FsResult<()>;

    /// Let go of an open file.
    fn close(&self, file: FileId) -> FsResult<()>;

    /// Every entry of `path`, with its attributes.
    fn read_dir(&self, path: &str) -> FsResult<Vec<(String, Attrs)>>;

    /// Create one directory.
    fn mkdir(&self, path: &str) -> FsResult<()>;

    /// Delete one file.
    fn remove(&self, path: &str) -> FsResult<()>;

    /// Delete one empty directory.
    fn rmdir(&self, path: &str) -> FsResult<()>;

    /// Rename or move `from` to `to`.
    fn rename(&self, from: &str, to: &str) -> FsResult<()>;
}

/// The remote path a filesystem path stands for.
///
/// A driver names paths relative to the mounted folder, with backslashes and a leading
/// separator; the remote host wants forward slashes from its root. `None` means the path
/// climbs out of the mount, which no remote folder can resolve.
pub fn remote_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

/// The name a directory entry is shown under, refusing the two the listing itself returns.
pub fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_path_becomes_a_remote_one() {
        assert_eq!(remote_path("\\"), Some("/".to_owned()));
        assert_eq!(remote_path(""), Some("/".to_owned()));
        assert_eq!(remote_path("\\var\\log"), Some("/var/log".to_owned()));
        assert_eq!(remote_path("/var/log/"), Some("/var/log".to_owned()));
        assert_eq!(remote_path("\\a\\.\\b"), Some("/a/b".to_owned()));
        assert_eq!(remote_path("\\a\\..\\b"), Some("/b".to_owned()));
        assert_eq!(remote_path("\\..\\etc"), None);
    }

    #[test]
    fn a_child_of_the_root_keeps_one_separator() {
        assert_eq!(child_path("/", "etc"), "/etc");
        assert_eq!(child_path("/var", "log"), "/var/log");
    }
}
