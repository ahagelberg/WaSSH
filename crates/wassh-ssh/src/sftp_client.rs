//! A blocking SFTP client for callers that cannot await.
//!
//! A mounted remote folder is served from callbacks that must answer a file request right
//! away, while the session's own work runs on its tokio runtime. This client bridges the
//! two: every method sends one call to the runtime and waits for its answer.
//!
//! Unlike [`crate::sftp`], which opens a channel per request and moves whole files, this
//! keeps one SFTP channel open for the lifetime of its owner and works with handles and
//! offsets, the way a filesystem needs.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use russh::client::Handle;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};

use crate::session::ClientHandler;

/// Largest payload one read or write carries.
///
/// Servers commonly refuse a packet past 32 KiB, and nothing here needs larger pieces.
pub const CHUNK: u32 = 32 * 1024;

/// Seconds one call may wait for the server before it fails.
const TIMEOUT: u64 = 30;

/// Permission bits a created directory is asked for; the server applies its umask.
const DIRECTORY_MODE: u32 = 0o777;

/// Why a call failed: the kinds a filesystem has to tell apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SftpClientError {
    /// Nothing exists at the path.
    NotFound,
    /// The server refused the operation for this user.
    Denied,
    /// The server does not implement the operation.
    Unsupported,
    /// The SFTP channel or the connection under it is gone.
    Closed,
    /// The server failed the request for another reason, or did not answer in time.
    Io(String),
}

impl SftpClientError {
    /// Whether the connection, rather than the request, is at fault.
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

impl std::fmt::Display for SftpClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no such file"),
            Self::Denied => formatter.write_str("permission denied"),
            Self::Unsupported => formatter.write_str("operation not supported by the server"),
            Self::Closed => formatter.write_str("the SFTP connection is closed"),
            Self::Io(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SftpClientError {}

/// The answer of one call.
pub type SftpResult<T> = Result<T, SftpClientError>;

/// Metadata of one remote file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteAttrs {
    /// Whether this is a directory.
    pub is_dir: bool,
    /// Whether this is a symbolic link (only from [`SftpClient::lstat`]).
    pub is_link: bool,
    /// Length in bytes.
    pub size: u64,
    /// Seconds since the Unix epoch, or 0 when the server sent no time.
    pub modified: u64,
    /// Seconds since the Unix epoch, or 0 when the server sent no time.
    pub accessed: u64,
    /// Unix permission bits, or 0 when the server sent none.
    pub mode: u32,
}

impl RemoteAttrs {
    fn of(attributes: &FileAttributes) -> Self {
        Self {
            is_dir: attributes.is_dir(),
            is_link: attributes.is_symlink(),
            size: attributes.len(),
            modified: attributes.mtime.map_or(0, u64::from),
            accessed: attributes.atime.map_or(0, u64::from),
            mode: attributes.permissions.unwrap_or(0),
        }
    }
}

/// How a file is opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Open for reading.
    pub read: bool,
    /// Open for writing.
    pub write: bool,
    /// Create the file when it does not exist.
    pub create: bool,
    /// Empty the file when it is opened.
    pub truncate: bool,
    /// Write at the end of the file.
    pub append: bool,
    /// Fail when the file exists, together with `create`.
    pub exclusive: bool,
}

impl OpenOptions {
    /// Open an existing file for reading.
    pub fn read_only() -> Self {
        Self {
            read: true,
            ..Self::default()
        }
    }

    /// Open a file for reading and writing, creating it when missing.
    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            create: true,
            ..Self::default()
        }
    }

    fn flags(self) -> OpenFlags {
        let mut flags = OpenFlags::default();
        if self.read {
            flags |= OpenFlags::READ;
        }
        if self.write {
            flags |= OpenFlags::WRITE;
        }
        if self.append {
            flags |= OpenFlags::APPEND;
        }
        if self.create {
            flags |= OpenFlags::CREATE;
        }
        if self.truncate {
            flags |= OpenFlags::TRUNCATE;
        }
        if self.exclusive {
            flags |= OpenFlags::EXCLUDE;
        }
        flags
    }
}

/// The answer channel of one call.
type Reply<T> = Sender<SftpResult<T>>;

/// One call waiting for the runtime to run it.
enum Call {
    Stat {
        path: String,
        reply: Reply<RemoteAttrs>,
    },
    Lstat {
        path: String,
        reply: Reply<RemoteAttrs>,
    },
    Realpath {
        path: String,
        reply: Reply<String>,
    },
    Open {
        path: String,
        options: OpenOptions,
        reply: Reply<String>,
    },
    Read {
        handle: String,
        offset: u64,
        len: u32,
        reply: Reply<Vec<u8>>,
    },
    Write {
        handle: String,
        offset: u64,
        data: Vec<u8>,
        reply: Reply<u32>,
    },
    Close {
        handle: String,
        reply: Reply<()>,
    },
    Sync {
        handle: String,
        reply: Reply<()>,
    },
    SetLen {
        handle: String,
        size: u64,
        reply: Reply<()>,
    },
    SetTimes {
        path: String,
        accessed: u64,
        modified: u64,
        reply: Reply<()>,
    },
    ReadDir {
        path: String,
        reply: Reply<Vec<(String, RemoteAttrs)>>,
    },
    Mkdir {
        path: String,
        reply: Reply<()>,
    },
    Remove {
        path: String,
        reply: Reply<()>,
    },
    Rmdir {
        path: String,
        reply: Reply<()>,
    },
    Rename {
        from: String,
        to: String,
        reply: Reply<()>,
    },
}

impl Call {
    /// Run one call against the shared channel and answer it.
    async fn serve(self, sftp: &RawSftpSession) {
        match self {
            Self::Stat { path, reply } => {
                let _ = reply.send(stat(sftp, path, false).await);
            }
            Self::Lstat { path, reply } => {
                let _ = reply.send(stat(sftp, path, true).await);
            }
            Self::Realpath { path, reply } => {
                let _ = reply.send(realpath(sftp, path).await);
            }
            Self::Open {
                path,
                options,
                reply,
            } => {
                let answer = sftp
                    .open(path, options.flags(), FileAttributes::default())
                    .await
                    .map(|handle| handle.handle)
                    .map_err(fail);
                let _ = reply.send(answer);
            }
            Self::Read {
                handle,
                offset,
                len,
                reply,
            } => {
                let _ = reply.send(read_at(sftp, &handle, offset, len).await);
            }
            Self::Write {
                handle,
                offset,
                data,
                reply,
            } => {
                let _ = reply.send(write_at(sftp, &handle, offset, &data).await);
            }
            Self::Close { handle, reply } => {
                let _ = reply.send(sftp.close(handle).await.map(|_| ()).map_err(fail));
            }
            Self::Sync { handle, reply } => {
                let _ = reply.send(sync(sftp, &handle).await);
            }
            Self::SetLen {
                handle,
                size,
                reply,
            } => {
                let attributes = FileAttributes {
                    size: Some(size),
                    ..FileAttributes::default()
                };
                let answer = sftp
                    .fsetstat(handle, attributes)
                    .await
                    .map(|_| ())
                    .map_err(fail);
                let _ = reply.send(answer);
            }
            Self::SetTimes {
                path,
                accessed,
                modified,
                reply,
            } => {
                let attributes = FileAttributes {
                    atime: Some(seconds(accessed)),
                    mtime: Some(seconds(modified)),
                    ..FileAttributes::default()
                };
                let answer = sftp
                    .setstat(path, attributes)
                    .await
                    .map(|_| ())
                    .map_err(fail);
                let _ = reply.send(answer);
            }
            Self::ReadDir { path, reply } => {
                let _ = reply.send(read_dir(sftp, path).await);
            }
            Self::Mkdir { path, reply } => {
                let attributes = FileAttributes {
                    permissions: Some(DIRECTORY_MODE),
                    ..FileAttributes::default()
                };
                let answer = sftp
                    .mkdir(path, attributes)
                    .await
                    .map(|_| ())
                    .map_err(fail);
                let _ = reply.send(answer);
            }
            Self::Remove { path, reply } => {
                let _ = reply.send(sftp.remove(path).await.map(|_| ()).map_err(fail));
            }
            Self::Rmdir { path, reply } => {
                let _ = reply.send(sftp.rmdir(path).await.map(|_| ()).map_err(fail));
            }
            Self::Rename { from, to, reply } => {
                let _ = reply.send(sftp.rename(from, to).await.map(|_| ()).map_err(fail));
            }
        }
    }
}

/// A handle on one long-lived SFTP channel.
///
/// Cheap to clone and safe to share: every clone sends calls to the same channel and the
/// runtime runs them side by side.
#[derive(Clone)]
pub struct SftpClient {
    calls: tokio::sync::mpsc::UnboundedSender<Call>,
}

impl SftpClient {
    /// Metadata of `path`, following a symbolic link.
    pub fn stat(&self, path: &str) -> SftpResult<RemoteAttrs> {
        self.call(|reply| Call::Stat {
            path: path.to_owned(),
            reply,
        })
    }

    /// Metadata of `path` itself, describing a symbolic link rather than its target.
    pub fn lstat(&self, path: &str) -> SftpResult<RemoteAttrs> {
        self.call(|reply| Call::Lstat {
            path: path.to_owned(),
            reply,
        })
    }

    /// The absolute path `path` stands for, with `~` and relative parts resolved.
    pub fn realpath(&self, path: &str) -> SftpResult<String> {
        self.call(|reply| Call::Realpath {
            path: path.to_owned(),
            reply,
        })
    }

    /// Open `path`, answering the server's handle for it.
    pub fn open(&self, path: &str, options: OpenOptions) -> SftpResult<String> {
        self.call(|reply| Call::Open {
            path: path.to_owned(),
            options,
            reply,
        })
    }

    /// Read up to `len` bytes of an open file at `offset`.
    ///
    /// A short answer means the file ends there.
    pub fn read_at(&self, handle: &str, offset: u64, len: u32) -> SftpResult<Vec<u8>> {
        self.call(|reply| Call::Read {
            handle: handle.to_owned(),
            offset,
            len,
            reply,
        })
    }

    /// Write `data` to an open file at `offset`, answering how many bytes went out.
    pub fn write_at(&self, handle: &str, offset: u64, data: &[u8]) -> SftpResult<u32> {
        self.call(|reply| Call::Write {
            handle: handle.to_owned(),
            offset,
            data: data.to_vec(),
            reply,
        })
    }

    /// Close an open file, waiting for the server to confirm its writes.
    pub fn close(&self, handle: &str) -> SftpResult<()> {
        self.call(|reply| Call::Close {
            handle: handle.to_owned(),
            reply,
        })
    }

    /// Ask the server to put an open file's data on its disk.
    pub fn sync(&self, handle: &str) -> SftpResult<()> {
        self.call(|reply| Call::Sync {
            handle: handle.to_owned(),
            reply,
        })
    }

    /// Cut an open file to `size` bytes.
    pub fn set_len(&self, handle: &str, size: u64) -> SftpResult<()> {
        self.call(|reply| Call::SetLen {
            handle: handle.to_owned(),
            size,
            reply,
        })
    }

    /// Set the access and modification times of `path`, in seconds since the Unix epoch.
    pub fn set_times(&self, path: &str, accessed: u64, modified: u64) -> SftpResult<()> {
        self.call(|reply| Call::SetTimes {
            path: path.to_owned(),
            accessed,
            modified,
            reply,
        })
    }

    /// Every entry of `path`, with its metadata.
    pub fn read_dir(&self, path: &str) -> SftpResult<Vec<(String, RemoteAttrs)>> {
        self.call(|reply| Call::ReadDir {
            path: path.to_owned(),
            reply,
        })
    }

    /// Create one directory.
    pub fn mkdir(&self, path: &str) -> SftpResult<()> {
        self.call(|reply| Call::Mkdir {
            path: path.to_owned(),
            reply,
        })
    }

    /// Delete one file.
    pub fn remove(&self, path: &str) -> SftpResult<()> {
        self.call(|reply| Call::Remove {
            path: path.to_owned(),
            reply,
        })
    }

    /// Delete one empty directory.
    pub fn rmdir(&self, path: &str) -> SftpResult<()> {
        self.call(|reply| Call::Rmdir {
            path: path.to_owned(),
            reply,
        })
    }

    /// Rename or move `from` to `to`.
    pub fn rename(&self, from: &str, to: &str) -> SftpResult<()> {
        self.call(|reply| Call::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
            reply,
        })
    }

    /// Send one call and wait for its answer.
    fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Call) -> SftpResult<T> {
        let (reply, answer) = channel();
        self.calls
            .send(make(reply))
            .map_err(|_| SftpClientError::Closed)?;
        answer.recv().map_err(|_| SftpClientError::Closed)?
    }
}

/// Open a long-lived SFTP channel over `handle`.
pub(crate) async fn open(handle: Arc<Handle<ClientHandler>>) -> Result<SftpClient, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| format!("opening an SFTP channel: {error}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| format!("starting the SFTP subsystem: {error}"))?;
    let session = RawSftpSession::new(channel.into_stream());
    session
        .init()
        .await
        .map_err(|error| format!("initialising SFTP: {error}"))?;
    session.set_timeout(TIMEOUT);
    let session = Arc::new(session);
    let (calls, mut queue) = tokio::sync::mpsc::unbounded_channel::<Call>();
    tokio::spawn(async move {
        while let Some(call) = queue.recv().await {
            let session = Arc::clone(&session);
            tokio::spawn(async move { call.serve(&session).await });
        }
    });
    Ok(SftpClient { calls })
}

/// Read up to `len` bytes in pieces the server accepts.
async fn read_at(
    sftp: &RawSftpSession,
    handle: &str,
    offset: u64,
    len: u32,
) -> SftpResult<Vec<u8>> {
    let mut data = Vec::with_capacity(len as usize);
    while (data.len() as u32) < len {
        let want = (len - data.len() as u32).min(CHUNK);
        let answer = match sftp
            .read(handle, offset + data.len() as u64, want)
            .await
        {
            Ok(answer) => answer,
            // A read past the end is the normal way to find it.
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
            Err(error) => return Err(fail(error)),
        };
        if answer.data.is_empty() {
            break;
        }
        data.extend_from_slice(&answer.data);
    }
    Ok(data)
}

/// Write `data` in pieces the server accepts.
async fn write_at(
    sftp: &RawSftpSession,
    handle: &str,
    offset: u64,
    data: &[u8],
) -> SftpResult<u32> {
    let mut written = 0usize;
    while written < data.len() {
        let end = (written + CHUNK as usize).min(data.len());
        sftp.write(handle, offset + written as u64, data[written..end].to_vec())
            .await
            .map_err(fail)?;
        written = end;
    }
    Ok(written as u32)
}

/// Flush an open file, tolerating a server without the `fsync` extension.
async fn sync(sftp: &RawSftpSession, handle: &str) -> SftpResult<()> {
    match sftp.extended("fsync@openssh.com", ssh_string(handle)).await {
        Ok(_) => Ok(()),
        Err(error) => match fail(error) {
            SftpClientError::Unsupported => Ok(()),
            other => Err(other),
        },
    }
}

/// Metadata of `path`, through `lstat` when `link` asks for the link itself.
async fn stat(sftp: &RawSftpSession, path: String, link: bool) -> SftpResult<RemoteAttrs> {
    let answer = if link {
        sftp.lstat(path).await
    } else {
        sftp.stat(path).await
    };
    answer
        .map(|packet| RemoteAttrs::of(&packet.attrs))
        .map_err(fail)
}

/// Resolve `path` to the absolute path it names.
async fn realpath(sftp: &RawSftpSession, path: String) -> SftpResult<String> {
    let name = sftp.realpath(path.clone()).await.map_err(fail)?;
    name.files
        .first()
        .map(|file| file.filename.clone())
        .ok_or_else(|| SftpClientError::Io(format!("{path} resolved to nothing")))
}

/// Every entry of `path`, walking the name replies to the end of the listing.
async fn read_dir(sftp: &RawSftpSession, path: String) -> SftpResult<Vec<(String, RemoteAttrs)>> {
    let handle = sftp.opendir(path).await.map_err(fail)?;
    let mut entries = Vec::new();
    loop {
        match sftp.readdir(handle.handle.as_str()).await {
            Ok(name) => entries.extend(
                name.files
                    .into_iter()
                    .map(|file| (file.filename, RemoteAttrs::of(&file.attrs))),
            ),
            Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
            Err(error) => return Err(fail(error)),
        }
    }
    sftp.close(handle.handle).await.map_err(fail)?;
    Ok(entries)
}

/// An SSH string: a big-endian length followed by the bytes.
fn ssh_string(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + text.len());
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

/// A time the protocol can carry, which is a 32-bit second count.
fn seconds(time: u64) -> u32 {
    u32::try_from(time).unwrap_or(u32::MAX)
}

/// Translate a protocol failure into the kinds a filesystem distinguishes.
fn fail(error: SftpError) -> SftpClientError {
    match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => SftpClientError::NotFound,
            StatusCode::PermissionDenied => SftpClientError::Denied,
            StatusCode::OpUnsupported => SftpClientError::Unsupported,
            StatusCode::ConnectionLost | StatusCode::NoConnection => SftpClientError::Closed,
            other => SftpClientError::Io(format!("{other}: {}", status.error_message)),
        },
        SftpError::Timeout => SftpClientError::Io("the server did not answer in time".to_owned()),
        other => SftpClientError::Io(other.to_string()),
    }
}
