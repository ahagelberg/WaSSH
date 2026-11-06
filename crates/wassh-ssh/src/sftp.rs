//! Remote file browsing over SFTP.
//!
//! Each request opens its own subsystem channel, uses it and lets it close, so a listing
//! never interferes with the interactive shell and a stale request cannot wedge the session.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use russh::client::Handle;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::session::ClientHandler;

/// Stops a file request running on an SSH or local worker.
#[derive(Clone, Debug, Default)]
pub struct SftpStop(Arc<SftpStopState>);

#[derive(Debug, Default)]
struct SftpStopState {
    stopped: AtomicBool,
    notify: tokio::sync::Notify,
}

impl SftpStop {
    /// Ask the worker to stop.
    pub fn stop(&self) {
        self.0.stopped.store(true, Ordering::Release);
        self.0.notify.notify_one();
    }

    /// Whether a stop has been requested.
    pub fn is_stopped(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        if !self.is_stopped() {
            self.0.notify.notified().await;
        }
    }
}

/// Error returned when the user stops a file request.
pub const SFTP_STOPPED: &str = "File transfer stopped by the user.";

/// One directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpEntry {
    /// File name without its directory.
    pub name: String,
    /// Full remote path.
    pub path: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether this entry is a symbolic link.
    pub is_link: bool,
    /// Size in bytes (`0` for directories).
    pub size: u64,
    /// Modification time in seconds since the Unix epoch, when reported.
    pub modified: Option<u64>,
    /// POSIX permission bits, when reported.
    pub permissions: Option<u32>,
}

/// Kind of work represented by a progress report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SftpProgressPhase {
    /// Moving file bytes.
    #[default]
    Transfer,
    /// Enumerating a tree before moving it.
    Scanning,
    /// Adding a staged file to an archive.
    Packing,
    /// Writing the completed archive.
    Writing,
}

/// How far a request has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SftpProgress {
    /// Bytes moved so far.
    pub done: u64,
    /// Bytes to move in total, when the request knows up front.
    pub total: Option<u64>,
    /// File being moved right now, for the pane's row.
    pub label: Option<String>,
    /// Kind of work represented by this report.
    pub phase: SftpProgressPhase,
    /// Number of tree entries seen while scanning.
    pub items: Option<u64>,
}

impl SftpProgress {
    /// Completion from 0 to 1, when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => {
                Some((self.done as f64 / total as f64).clamp(0.0, 1.0) as f32)
            }
            _ => None,
        }
    }
}

/// Where a request reports its progress, and how far it has got.
#[derive(Default)]
struct Report {
    sender: Option<std::sync::mpsc::Sender<SftpProgress>>,
    done: u64,
    total: Option<u64>,
    label: Option<String>,
    phase: SftpProgressPhase,
    items: Option<u64>,
}

impl Report {
    fn new(sender: Option<std::sync::mpsc::Sender<SftpProgress>>) -> Self {
        Self {
            sender,
            done: 0,
            total: None,
            label: None,
            phase: SftpProgressPhase::Transfer,
            items: None,
        }
    }

    /// Say which file the request is moving now.
    fn saying(&mut self, name: &str) {
        self.phase = SftpProgressPhase::Transfer;
        self.items = None;
        self.label = Some(name.to_owned());
        self.send();
    }

    /// Report how many entries have been inspected so far.
    fn scanning(&mut self, items: u64) {
        self.phase = SftpProgressPhase::Scanning;
        self.items = Some(items);
        self.label = None;
        self.send();
    }

    /// Add `bytes` to what this request has to move.
    ///
    /// A request states its whole size before the first byte moves, so one bar covers
    /// the whole job instead of restarting for every file in it.
    fn total(&mut self, bytes: u64) {
        self.phase = SftpProgressPhase::Transfer;
        self.items = None;
        *self.total.get_or_insert(0) += bytes;
        self.send();
    }

    /// Count `bytes` that are already in place, as a resumed transfer starts there.
    fn skip(&mut self, bytes: u64) {
        self.done += bytes;
        self.send();
    }

    /// Add `bytes` to the total moved.
    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        self.send();
    }

    fn send(&self) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(SftpProgress {
                done: self.done,
                total: self.total,
                label: self.label.clone(),
                phase: self.phase,
                items: self.items,
            });
        }
    }
}

/// A request run against the remote SFTP subsystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SftpRequest {
    /// List one directory.
    List {
        /// Directory to list.
        path: String,
    },
    /// Resolve `path` (a `.` means the login directory) to an absolute path.
    RealPath {
        /// Path to resolve.
        path: String,
    },
    /// Copy a remote file to `local`.
    Download {
        /// Remote file to read.
        remote: String,
        /// Local file to write (its directory is created when missing).
        local: PathBuf,
        /// Continue from the local file's current length instead of restarting.
        resume: bool,
    },
    /// Copy a local file to `remote`.
    Upload {
        /// Local file to read.
        local: PathBuf,
        /// Remote file to write.
        remote: String,
        /// Continue from the remote file's current size instead of restarting.
        resume: bool,
    },
    /// Replace a remote file with owned bytes.
    Write {
        /// Destination path.
        path: String,
        /// File contents.
        content: Vec<u8>,
    },
    /// Create one directory.
    Mkdir {
        /// Directory to create.
        path: String,
    },
    /// Rename or move `from` to `to`.
    Rename {
        /// Current path.
        from: String,
        /// New path.
        to: String,
    },
    /// Delete a file, or a directory and everything under it.
    Remove {
        /// Path to delete.
        path: String,
        /// Whether `path` is a directory.
        is_dir: bool,
    },
    /// Change the Unix mode bits of `path`.
    Chmod {
        /// File or directory to change.
        path: String,
        /// Mode bits, e.g. `0o644`.
        mode: u32,
    },
    /// Read the first `max_bytes` of one file.
    Read {
        /// File to read.
        path: String,
        /// Largest number of bytes to return.
        max_bytes: u64,
    },
    /// Upload several local files in order.
    UploadMany {
        /// `(local, remote)` pairs.
        items: Vec<(PathBuf, String)>,
        /// Continue interrupted files instead of restarting them.
        resume: bool,
    },
    /// Download several remote files in order.
    DownloadMany {
        /// `(remote, local)` pairs.
        items: Vec<(String, PathBuf)>,
        /// Continue interrupted files instead of restarting them.
        resume: bool,
    },
    /// Download each path (a file or a whole directory tree) into `dir`.
    DownloadTree {
        /// Remote paths to fetch.
        items: Vec<String>,
        /// Local directory that receives them, keeping their names.
        dir: PathBuf,
    },
    /// Copy remote files to other remote paths.
    ///
    /// SFTP has no server-side copy, so each file is streamed through `scratch`
    /// on this machine. With `move_items` the source is removed by renaming
    /// instead, which needs no data to move at all.
    CopyMany {
        /// `(from, to)` pairs.
        items: Vec<(String, String)>,
        /// Local file each copy is streamed through.
        scratch: PathBuf,
        /// Rename instead of copying when set.
        move_items: bool,
    },
}

/// What a request produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SftpOutcome {
    /// Directory entries, answering [`SftpRequest::List`].
    Listed(Vec<SftpEntry>),
    /// Byte count moved by a transfer.
    Transferred {
        /// Bytes copied.
        bytes: u64,
    },
    /// A change that has nothing to report.
    Done,
    /// An absolute path, answering [`SftpRequest::RealPath`].
    RealPath(String),
    /// A file prefix, answering [`SftpRequest::Read`].
    Content {
        /// Bytes read, at most the requested maximum.
        data: Vec<u8>,
        /// Whether the file continues past what was returned.
        truncated: bool,
    },
}

/// Size of a single read/write block.
const BLOCK: usize = 64 * 1024;
/// Tree scans report periodically rather than once per entry.
const SCAN_REPORT_EVERY: u64 = 64;

/// Run `request` over a fresh SFTP channel.
pub(crate) async fn run(
    handle: Arc<Handle<ClientHandler>>,
    request: SftpRequest,
    progress: Option<std::sync::mpsc::Sender<SftpProgress>>,
) -> Result<SftpOutcome, String> {
    let mut report = Report::new(progress);
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| format!("opening an SFTP channel: {error}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| format!("starting the SFTP subsystem: {error}"))?;
    let session = SftpSession::new(channel.into_stream())
        .await
        .map_err(|error| format!("initialising SFTP: {error}"))?;

    match request {
        SftpRequest::List { path } => list(&session, &path).await,
        SftpRequest::RealPath { path } => session
            .canonicalize(path.clone())
            .await
            .map(SftpOutcome::RealPath)
            .map_err(|error| format!("resolving {path}: {error}")),
        SftpRequest::Download {
            remote,
            local,
            resume,
        } => {
            report.total(
                session
                    .metadata(remote.clone())
                    .await
                    .map(|meta| meta.size.unwrap_or(0))
                    .unwrap_or(0),
            );
            download(&session, &remote, &local, &mut report, resume).await
        }
        SftpRequest::Upload {
            local,
            remote,
            resume,
        } => {
            report.total(
                tokio::fs::metadata(&local)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            );
            upload(&session, &local, &remote, &mut report, resume).await
        }
        SftpRequest::Write { path, content } => {
            let bytes = content.len() as u64;
            report.total(bytes);
            let mut sink = session
                .create(path.clone())
                .await
                .map_err(|error| format!("writing {path}: {error}"))?;
            sink.write_all(&content)
                .await
                .map_err(|error| format!("writing {path}: {error}"))?;
            report.add(bytes);
            let _ = sink.sync_all().await;
            let _ = sink.close().await;
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::Mkdir { path } => {
            session
                .create_dir(path.clone())
                .await
                .map_err(|error| format!("creating {path}: {error}"))?;
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Rename { from, to } => {
            session
                .rename(from.clone(), to.clone())
                .await
                .map_err(|error| format!("renaming {from} to {to}: {error}"))?;
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Remove { path, is_dir } => {
            if is_dir {
                remove_tree(&session, &path).await?;
            } else {
                session
                    .remove_file(path.clone())
                    .await
                    .map_err(|error| format!("deleting {path}: {error}"))?;
            }
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Read { path, max_bytes } => read(&session, &path, max_bytes).await,
        SftpRequest::UploadMany { items, resume } => {
            let mut total = 0;
            for (local, _) in &items {
                total += tokio::fs::metadata(local)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0);
            }
            report.total(total);
            let mut bytes = 0;
            for (local, remote) in &items {
                if let SftpOutcome::Transferred { bytes: moved } =
                    upload(&session, local, remote, &mut report, resume).await?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::DownloadMany { items, resume } => {
            let mut total = 0;
            for (remote, _) in &items {
                total += session
                    .metadata(remote.clone())
                    .await
                    .map(|meta| meta.size.unwrap_or(0))
                    .unwrap_or(0);
            }
            report.total(total);
            let mut bytes = 0;
            for (remote, local) in &items {
                if let SftpOutcome::Transferred { bytes: moved } =
                    download(&session, remote, local, &mut report, resume).await?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::DownloadTree { items, dir } => {
            // Measure the tree from the listings first, so the bar covers all of it from
            // the first file, then copy it in one pass.
            let mut files = Vec::new();
            let mut scanned = 0;
            report.scanning(scanned);
            for item in &items {
                measure_tree(
                    &session,
                    item,
                    &dir,
                    &mut files,
                    &mut report,
                    &mut scanned,
                    None,
                )
                .await?;
            }
            report.total(files.iter().map(|(_, _, size)| *size).sum());
            let mut bytes = 0;
            for (remote, local, _) in &files {
                if let SftpOutcome::Transferred { bytes: moved } =
                    download(&session, remote, local, &mut report, false).await?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::CopyMany {
            items,
            scratch,
            move_items,
        } => {
            if !move_items {
                // A copy streams each file down and back up, so the bar covers both
                // directions of the whole batch.
                let mut total = 0;
                for (from, _) in &items {
                    total += session
                        .metadata(from.clone())
                        .await
                        .map(|meta| meta.size.unwrap_or(0))
                        .unwrap_or(0);
                }
                report.total(total * 2);
            }
            let mut bytes = 0;
            for (from, to) in &items {
                if move_items {
                    session
                        .rename(from.clone(), to.clone())
                        .await
                        .map_err(|error| format!("moving {from} to {to}: {error}"))?;
                    continue;
                }
                if let SftpOutcome::Transferred { bytes: moved } =
                    download(&session, from, &scratch, &mut report, false).await?
                {
                    bytes += moved;
                }
                upload(&session, &scratch, to, &mut report, false).await?;
            }
            let _ = tokio::fs::remove_file(&scratch).await;
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::Chmod { path, mode } => {
            session
                .set_metadata(
                    path.clone(),
                    russh_sftp::protocol::FileAttributes {
                        permissions: Some(mode),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| format!("changing the mode of {path}: {error}"))?;
            Ok(SftpOutcome::Done)
        }
    }
}

/// Delete `path` and everything under it, deepest first.
async fn remove_tree(session: &SftpSession, path: &str) -> Result<(), String> {
    let directory = session
        .read_dir(path.to_owned())
        .await
        .map_err(|error| format!("listing {path}: {error}"))?;
    let mut children: Vec<(String, bool)> = directory
        .map(|entry| (entry.path(), entry.metadata().is_dir()))
        .filter(|(child, _)| child.as_str() != path)
        .collect();
    children.sort();
    for (child, is_dir) in children {
        if is_dir {
            Box::pin(remove_tree(session, &child)).await?;
        } else {
            session
                .remove_file(child.clone())
                .await
                .map_err(|error| format!("deleting {child}: {error}"))?;
        }
    }
    session
        .remove_dir(path.to_owned())
        .await
        .map_err(|error| format!("deleting {path}: {error}"))
}

async fn list(session: &SftpSession, path: &str) -> Result<SftpOutcome, String> {
    let directory = session
        .read_dir(path.to_owned())
        .await
        .map_err(|error| format!("listing {path}: {error}"))?;
    let mut entries: Vec<SftpEntry> = directory
        .map(|entry| {
            let metadata = entry.metadata();
            SftpEntry {
                name: entry.file_name(),
                path: entry.path(),
                is_dir: metadata.is_dir(),
                is_link: metadata.is_symlink(),
                size: metadata.size.unwrap_or(0),
                modified: metadata.mtime.map(u64::from),
                permissions: metadata.permissions,
            }
        })
        // SFTP servers echo "." and ".."; navigation is handled by the UI.
        .filter(|entry| entry.name != "." && entry.name != "..")
        .collect();
    sort_entries(&mut entries);
    Ok(SftpOutcome::Listed(entries))
}

/// Read at most `max_bytes` of `path`, saying whether there is more.
async fn read(session: &SftpSession, path: &str, max_bytes: u64) -> Result<SftpOutcome, String> {
    let mut file = session
        .open(path)
        .await
        .map_err(|error| format!("opening {path}: {error}"))?;
    let mut buffer = Vec::with_capacity(max_bytes.min(BLOCK as u64 * 16) as usize);
    // One byte past the limit tells us whether the file continues.
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buffer)
        .await
        .map_err(|error| format!("reading {path}: {error}"))?;
    let truncated = buffer.len() as u64 > max_bytes;
    buffer.truncate(max_bytes as usize);
    let _ = file.close().await;
    Ok(SftpOutcome::Content {
        data: buffer,
        truncated,
    })
}

/// Last segment of a slash-separated path.
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("download")
}

/// Collect every file under `remote` as `(remote, local, size)`, keeping its name under
/// `dir`; directories are created as the walk goes.
///
/// A directory's listing carries each entry's type and size, so measuring a tree costs
/// one call per directory rather than one per file. Links are skipped: they point at
/// something else on the host, which is not part of the tree being copied.
async fn measure_tree(
    session: &SftpSession,
    remote: &str,
    dir: &std::path::Path,
    files: &mut Vec<(String, std::path::PathBuf, u64)>,
    report: &mut Report,
    scanned: &mut u64,
    entry_metadata: Option<(bool, u64)>,
) -> Result<(), String> {
    let target = dir.join(base_name(remote));
    let (is_dir, size) = match entry_metadata {
        Some(metadata) => metadata,
        None => {
            let metadata = session
                .symlink_metadata(remote.to_owned())
                .await
                .map_err(|error| format!("reading {remote}: {error}"))?;
            if metadata.is_symlink() {
                return Ok(());
            }
            (metadata.is_dir(), metadata.size.unwrap_or(0))
        }
    };
    if !is_dir {
        files.push((remote.to_owned(), target, size));
        return Ok(());
    }
    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|error| format!("creating {}: {error}", target.display()))?;
    let mut children: Vec<(String, bool, u64)> = Vec::new();
    let directory = session
        .read_dir(remote.to_owned())
        .await
        .map_err(|error| format!("listing {remote}: {error}"))?;
    for entry in directory {
        if entry.file_type().is_symlink() {
            continue;
        }
        children.push((
            entry.path(),
            entry.file_type().is_dir(),
            entry.metadata().len(),
        ));
        *scanned += 1;
        if (*scanned).is_multiple_of(SCAN_REPORT_EVERY) {
            report.scanning(*scanned);
        }
    }
    report.scanning(*scanned);
    children.sort_by(|left, right| left.0.cmp(&right.0));
    for (child, is_dir, size) in children {
        if is_dir {
            Box::pin(measure_tree(
                session,
                &child,
                &target,
                files,
                report,
                scanned,
                Some((is_dir, size)),
            ))
            .await?;
        } else {
            files.push((child.clone(), target.join(base_name(&child)), size));
        }
    }
    Ok(())
}

async fn download(
    session: &SftpSession,
    remote: &str,
    local: &std::path::Path,
    report: &mut Report,
    resume: bool,
) -> Result<SftpOutcome, String> {
    let mut source = session
        .open(remote)
        .await
        .map_err(|error| format!("opening {remote}: {error}"))?;
    if let Some(parent) = local.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    let size = session
        .metadata(remote.to_owned())
        .await
        .map(|meta| meta.size.unwrap_or(0))
        .unwrap_or(0);
    report.saying(base_name(remote));

    // Resume only when the local file is a genuine prefix of the remote one.
    let have = if resume {
        tokio::fs::metadata(local)
            .await
            .map(|meta| meta.len())
            .unwrap_or(0)
            .min(size)
    } else {
        0
    };
    if resume && have == size && size > 0 {
        report.skip(have);
        let _ = source.close().await;
        return Ok(SftpOutcome::Transferred { bytes: 0 });
    }

    let mut sink = if have > 0 {
        let file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(local)
            .await
            .map_err(|error| format!("writing {}: {error}", local.display()))?;
        tokio::io::AsyncSeekExt::seek(&mut source, std::io::SeekFrom::Start(have))
            .await
            .map_err(|error| format!("seeking {remote}: {error}"))?;
        report.skip(have);
        file
    } else {
        tokio::fs::File::create(local)
            .await
            .map_err(|error| format!("writing {}: {error}", local.display()))?
    };

    let bytes = copy(&mut source, &mut sink, report)
        .await
        .map_err(|error| match error {
            TransferError::Read(error) => format!("reading {remote}: {error}"),
            TransferError::Write(error) => format!("writing {}: {error}", local.display()),
        })?;
    let _ = source.close().await;
    Ok(SftpOutcome::Transferred { bytes })
}

async fn upload(
    session: &SftpSession,
    local: &std::path::Path,
    remote: &str,
    report: &mut Report,
    resume: bool,
) -> Result<SftpOutcome, String> {
    let mut source = tokio::fs::File::open(local)
        .await
        .map_err(|error| format!("reading {}: {error}", local.display()))?;
    let size = tokio::fs::metadata(local)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0);
    // Append to what the host already holds, when that is a prefix of ours.
    let have = if resume {
        session
            .metadata(remote.to_owned())
            .await
            .ok()
            .and_then(|meta| meta.size)
            .unwrap_or(0)
            .min(size)
    } else {
        0
    };
    let mut sink = if have > 0 {
        tokio::io::AsyncSeekExt::seek(&mut source, std::io::SeekFrom::Start(have))
            .await
            .map_err(|error| format!("seeking {}: {error}", local.display()))?;
        report.skip(have);
        session
            .open_with_flags(
                remote.to_owned(),
                russh_sftp::protocol::OpenFlags::WRITE | russh_sftp::protocol::OpenFlags::APPEND,
            )
            .await
            .map_err(|error| format!("writing {remote}: {error}"))?
    } else {
        report.skip(0);
        session
            .create(remote)
            .await
            .map_err(|error| format!("writing {remote}: {error}"))?
    };

    let bytes = copy(&mut source, &mut sink, report)
        .await
        .map_err(|error| match error {
            TransferError::Read(error) => format!("reading {}: {error}", local.display()),
            TransferError::Write(error) => format!("writing {remote}: {error}"),
        })?;
    let _ = sink.sync_all().await;
    let _ = sink.close().await;
    Ok(SftpOutcome::Transferred { bytes })
}

/// Which side of a transfer failed.
#[derive(Debug)]
enum TransferError {
    Read(std::io::Error),
    Write(std::io::Error),
}

/// Stream every byte of `source` into `sink`, returning the count.
async fn copy<R, W>(source: &mut R, sink: &mut W, report: &mut Report) -> Result<u64, TransferError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; BLOCK];
    let mut bytes = 0u64;
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(TransferError::Read)?;
        if read == 0 {
            break;
        }
        sink.write_all(&buffer[..read])
            .await
            .map_err(TransferError::Write)?;
        bytes += read as u64;
        report.add(read as u64);
    }
    sink.flush().await.map_err(TransferError::Write)?;
    Ok(bytes)
}

/// Directories first, then case-insensitive by name.
pub fn sort_entries(entries: &mut [SftpEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Human-readable size, as used by the file list.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> SftpEntry {
        SftpEntry {
            name: name.to_owned(),
            path: format!("/home/{name}"),
            is_dir,
            is_link: false,
            size: 0,
            modified: None,
            permissions: None,
        }
    }

    #[test]
    fn directories_sort_before_files() {
        let mut entries = vec![
            entry("zebra", false),
            entry("alpha", true),
            entry("Beta", false),
        ];
        sort_entries(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Beta", "zebra"]);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn a_resumed_transfer_counts_from_where_it_stopped() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut report = Report::new(Some(sender));
        report.total(1000);
        report.skip(400);
        report.add(600);
        let updates: Vec<SftpProgress> = receiver.try_iter().collect();
        assert_eq!(updates.first().map(|update| update.done), Some(0));
        assert_eq!(updates.last().map(|update| update.done), Some(1000));
        assert!(
            updates
                .iter()
                .all(|update| update.total == Some(1000) && update.fraction() <= Some(1.0))
        );
        // Only the remainder is announced as new work.
        assert!(
            updates
                .iter()
                .any(|update| update.done == 400 && update.fraction() == Some(0.4))
        );
    }

    #[test]
    fn transferred_outcomes_carry_the_byte_count() {
        let outcome = SftpOutcome::Transferred { bytes: 4096 };
        assert_eq!(outcome, SftpOutcome::Transferred { bytes: 4096 });
    }

    #[tokio::test]
    async fn copy_streams_every_byte_across_block_boundaries() {
        // Two and a bit blocks, so the reader returns short reads.
        let data = vec![7u8; BLOCK * 2 + 123];
        let mut source = std::io::Cursor::new(data.clone());
        let mut sink: Vec<u8> = Vec::new();
        let mut report = Report::default();
        let bytes = copy(&mut source, &mut sink, &mut report)
            .await
            .expect("copy");
        assert_eq!(bytes, data.len() as u64);
        assert_eq!(sink, data);
    }

    #[tokio::test]
    async fn an_empty_file_transfers_nothing() {
        let mut source = std::io::Cursor::new(Vec::new());
        let mut sink: Vec<u8> = Vec::new();
        let mut report = Report::default();
        assert_eq!(
            copy(&mut source, &mut sink, &mut report)
                .await
                .expect("copy"),
            0
        );
        assert!(sink.is_empty());
    }
}
