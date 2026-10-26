//! The Files pane on a Local terminal.
//!
//! A local tab has no SFTP server, so the same [`SftpRequest`]s run against this
//! machine's filesystem instead. Paths behave like the reference app's
//! `LocalFileSession`: `~` is the home directory, a relative path is resolved
//! from it, and every path the pane sees uses `/` as its separator.
//!
//! Requests run on their own thread and answer over the same channels an SFTP
//! request uses, so the pane cannot tell the two apart and a large copy never
//! blocks the window.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

use wassh_ssh::{SftpEntry, SftpJob, SftpOutcome, SftpProgress, SftpRequest};

/// Size of one read/write block, matching the SFTP transfer path.
const BLOCK: usize = 64 * 1024;

/// The user's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Directory the browser opens first: the user's home directory.
pub fn start_dir() -> String {
    display(&std::fs::canonicalize(home()).unwrap_or_else(|_| home()))
}

/// Resolve `path` to an absolute path with `/` separators.
pub fn canonical(path: &str) -> Result<String, String> {
    std::fs::canonicalize(local_path(path))
        .map(|resolved| display(&resolved))
        .map_err(|error| format!("resolving {path}: {error}"))
}

/// Run `request` on its own thread, reporting progress as it goes.
pub fn spawn(request: SftpRequest) -> SftpJob {
    let (outcome_tx, outcome) = channel();
    let (progress_tx, progress) = channel();
    let unused = outcome_tx.clone();
    let spawned = std::thread::Builder::new()
        .name("wassh-local-files".to_owned())
        .spawn(move || {
            let mut report = Report::new(Some(progress_tx));
            let _ = outcome_tx.send(run(request, &mut report));
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the file worker: {error}")));
    }
    SftpJob { outcome, progress }
}

/// The local filesystem path a pane path means.
fn local_path(text: &str) -> PathBuf {
    let trimmed = text.trim();
    let home = home();
    let expanded = if trimmed.is_empty() || trimmed == "~" {
        home.clone()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        home.join(expanded)
    }
}

/// A path as the pane shows it: always `/` separated.
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Join a shown directory and a name with exactly one `/` between them.
fn join_shown(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    }
}

/// POSIX mode bits of an entry, when the platform reports them.
fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Where a request reports its progress, and how far it has got.
#[derive(Default)]
struct Report {
    sender: Option<Sender<SftpProgress>>,
    done: u64,
    total: Option<u64>,
}

impl Report {
    fn new(sender: Option<Sender<SftpProgress>>) -> Self {
        Self {
            sender,
            done: 0,
            total: None,
        }
    }

    /// Announce the total, so the bar can be determinate from the start.
    fn total(&mut self, total: u64) {
        self.total = Some(total);
        self.send();
    }

    /// Start counting from `offset` bytes that are already in place.
    fn skip(&mut self, offset: u64) {
        self.done = offset;
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
            });
        }
    }
}

/// Which side of a transfer failed.
#[derive(Debug)]
enum TransferError {
    Read(std::io::Error),
    Write(std::io::Error),
}

/// Run one request against this machine's filesystem.
fn run(request: SftpRequest, report: &mut Report) -> Result<SftpOutcome, String> {
    match request {
        SftpRequest::List { path } => list(&path),
        SftpRequest::RealPath { path } => canonical(&path).map(SftpOutcome::RealPath),
        SftpRequest::Read { path, max_bytes } => read(&path, max_bytes),
        SftpRequest::Mkdir { path } => {
            std::fs::create_dir(local_path(&path))
                .map_err(|error| format!("creating {path}: {error}"))?;
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Rename { from, to } => {
            std::fs::rename(local_path(&from), local_path(&to))
                .map_err(|error| format!("renaming {from} to {to}: {error}"))?;
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Remove { path, is_dir } => {
            let target = local_path(&path);
            if is_dir {
                remove_tree(&target)?;
            } else {
                std::fs::remove_file(&target)
                    .map_err(|error| format!("deleting {path}: {error}"))?;
            }
            Ok(SftpOutcome::Done)
        }
        SftpRequest::Chmod { path, mode } => chmod(&path, mode),
        SftpRequest::Download {
            remote,
            local,
            resume,
        } => {
            let source = local_path(&remote);
            report.total(size_of(&source));
            copy_file(&source, &local, resume, report)
        }
        SftpRequest::Upload {
            local,
            remote,
            resume,
        } => {
            report.total(size_of(&local));
            copy_file(&local, &local_path(&remote), resume, report)
        }
        SftpRequest::UploadMany { items, resume } => {
            let mut bytes = 0;
            for (local, remote) in &items {
                report.total(size_of(local));
                if let SftpOutcome::Transferred { bytes: moved } =
                    copy_file(local, &local_path(remote), resume, report)?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::DownloadMany { items, resume } => {
            let mut bytes = 0;
            for (remote, local) in &items {
                let source = local_path(remote);
                report.total(size_of(&source));
                if let SftpOutcome::Transferred { bytes: moved } =
                    copy_file(&source, local, resume, report)?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        SftpRequest::DownloadTree { items, dir } => {
            let mut bytes = 0;
            for item in &items {
                bytes += download_tree(item, &dir, report)?;
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
        // A local copy moves no bytes between machines, so the scratch file the
        // SFTP path streams through is not needed: `fs::copy` does the whole job.
        SftpRequest::CopyMany {
            items, move_items, ..
        } => {
            let mut bytes = 0;
            for (from, to) in &items {
                let source = local_path(from);
                let target = local_path(to);
                if move_items {
                    std::fs::rename(&source, &target)
                        .map_err(|error| format!("moving {from} to {to}: {error}"))?;
                    continue;
                }
                if let SftpOutcome::Transferred { bytes: moved } =
                    copy_file(&source, &target, false, report)?
                {
                    bytes += moved;
                }
            }
            Ok(SftpOutcome::Transferred { bytes })
        }
    }
}

/// Size of `path`, or `0` when it cannot be read.
fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn list(path: &str) -> Result<SftpOutcome, String> {
    let directory = local_path(path);
    let shown = display(&directory);
    let read = std::fs::read_dir(&directory).map_err(|error| format!("listing {path}: {error}"))?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| format!("listing {path}: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `lstat`, so a symlink is reported as a link rather than what it points at.
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let is_dir = meta.is_dir();
        entries.push(SftpEntry {
            path: join_shown(&shown, &name),
            name,
            is_dir,
            is_link: meta.file_type().is_symlink(),
            size: if is_dir { 0 } else { meta.len() },
            modified: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_secs()),
            permissions: mode_of(&meta),
        });
    }
    sort_entries(&mut entries);
    Ok(SftpOutcome::Listed(entries))
}

/// Directories first, then case-insensitive by name.
fn sort_entries(entries: &mut [SftpEntry]) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

/// Read at most `max_bytes` of `path`, saying whether there is more.
fn read(path: &str, max_bytes: u64) -> Result<SftpOutcome, String> {
    let mut file =
        File::open(local_path(path)).map_err(|error| format!("opening {path}: {error}"))?;
    let mut buffer = Vec::with_capacity(max_bytes.min(BLOCK as u64 * 16) as usize);
    // One byte past the limit tells us whether the file continues.
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buffer)
        .map_err(|error| format!("reading {path}: {error}"))?;
    let truncated = buffer.len() as u64 > max_bytes;
    buffer.truncate(max_bytes as usize);
    Ok(SftpOutcome::Content {
        data: buffer,
        truncated,
    })
}

fn chmod(path: &str, mode: u32) -> Result<SftpOutcome, String> {
    let target = local_path(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            .map_err(|error| format!("changing the mode of {path}: {error}"))?;
        Ok(SftpOutcome::Done)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, mode);
        Err(format!(
            "changing the mode of {path} is not supported on this system"
        ))
    }
}

/// Delete `path` and everything under it, deepest first.
fn remove_tree(path: &Path) -> Result<(), String> {
    let shown = display(path);
    let read = std::fs::read_dir(path).map_err(|error| format!("listing {shown}: {error}"))?;
    let mut children: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        // A symlink is removed itself, never the tree it points at.
        let is_dir = std::fs::symlink_metadata(&child).is_ok_and(|meta| meta.is_dir());
        if is_dir {
            remove_tree(&child)?;
        } else {
            std::fs::remove_file(&child)
                .map_err(|error| format!("deleting {}: {error}", display(&child)))?;
        }
    }
    std::fs::remove_dir(path).map_err(|error| format!("deleting {shown}: {error}"))
}

/// Copy `source` into `dir`, keeping its name; directories are walked.
///
/// Returns the bytes moved, so a tree of files shows its real size.
fn download_tree(item: &str, dir: &Path, report: &mut Report) -> Result<u64, String> {
    let source = local_path(item);
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    let target = dir.join(&name);
    let is_dir = std::fs::symlink_metadata(&source)
        .is_ok_and(|meta| meta.is_dir());
    if !is_dir {
        return match copy_file(&source, &target, false, report)? {
            SftpOutcome::Transferred { bytes } => Ok(bytes),
            _ => Ok(0),
        };
    }
    std::fs::create_dir_all(&target)
        .map_err(|error| format!("creating {}: {error}", display(&target)))?;
    let read = std::fs::read_dir(&source).map_err(|error| format!("listing {item}: {error}"))?;
    let mut children: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    children.sort();
    let mut bytes = 0;
    for child in children {
        bytes += download_tree(&display(&child), &target, report)?;
    }
    Ok(bytes)
}

/// Copy one file, optionally continuing a partial copy that is already there.
fn copy_file(
    source: &Path,
    target: &Path,
    resume: bool,
    report: &mut Report,
) -> Result<SftpOutcome, String> {
    let mut from =
        File::open(source).map_err(|error| format!("reading {}: {error}", display(source)))?;
    let size = from.metadata().map(|meta| meta.len()).unwrap_or(0);
    report.total(size);
    if let Some(parent) = target.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", display(parent)))?;
    }

    // Resume only when the target is a genuine prefix of the source.
    let have = if resume {
        size_of(target).min(size)
    } else {
        0
    };
    if resume && have == size && size > 0 {
        report.skip(have);
        return Ok(SftpOutcome::Transferred { bytes: 0 });
    }
    if have > 0 {
        from.seek(SeekFrom::Start(have))
            .map_err(|error| format!("reading {}: {error}", display(source)))?;
    }
    let mut to = if have > 0 {
        OpenOptions::new().append(true).open(target)
    } else {
        File::create(target)
    }
    .map_err(|error| format!("writing {}: {error}", display(target)))?;
    report.skip(have);

    let bytes = copy(&mut from, &mut to, report).map_err(|error| match error {
        TransferError::Read(error) => format!("reading {}: {error}", display(source)),
        TransferError::Write(error) => format!("writing {}: {error}", display(target)),
    })?;
    let _ = to.sync_all();
    Ok(SftpOutcome::Transferred { bytes })
}

/// Stream every byte of `source` into `sink`, returning the count.
fn copy<R, W>(source: &mut R, sink: &mut W, report: &mut Report) -> Result<u64, TransferError>
where
    R: Read,
    W: Write,
{
    let mut buffer = vec![0u8; BLOCK];
    let mut bytes = 0u64;
    loop {
        let read = source.read(&mut buffer).map_err(TransferError::Read)?;
        if read == 0 {
            break;
        }
        sink.write_all(&buffer[..read])
            .map_err(TransferError::Write)?;
        bytes += read as u64;
        report.add(read as u64);
    }
    sink.flush().map_err(TransferError::Write)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh directory under the system temp dir, removed when it is dropped.
    struct Sandbox {
        path: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "wassh-local-{name}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("sandbox");
            Self { path }
        }

        /// The directory as the pane writes it.
        fn shown(&self) -> String {
            display(&self.path)
        }

        /// A path the pane can ask for.
        fn at(&self, name: &str) -> String {
            join_shown(&self.shown(), name)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn execute(request: SftpRequest) -> Result<SftpOutcome, String> {
        run(request, &mut Report::new(None))
    }

    fn transferred(outcome: Result<SftpOutcome, String>) -> u64 {
        match outcome.expect("a finished request") {
            SftpOutcome::Transferred { bytes } => bytes,
            other => panic!("expected a transfer, got {other:?}"),
        }
    }

    #[test]
    fn a_relative_path_starts_at_the_home_directory() {
        let home = home();
        assert_eq!(local_path("~"), home);
        assert_eq!(local_path("~/notes.txt"), home.join("notes.txt"));
        assert_eq!(local_path("notes.txt"), home.join("notes.txt"));
        assert_eq!(local_path(""), home);
        assert_eq!(local_path("/etc/hosts"), PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn the_browser_starts_in_the_home_directory() {
        let home = std::fs::canonicalize(home()).unwrap_or_else(|_| home());
        assert_eq!(start_dir(), display(&home));
    }

    #[test]
    fn a_listing_reports_type_size_and_mode() {
        let sandbox = Sandbox::new("list");
        std::fs::write(sandbox.path.join("alpha.txt"), b"hello").expect("write");
        std::fs::create_dir(sandbox.path.join("sub")).expect("mkdir");

        let SftpOutcome::Listed(entries) = execute(SftpRequest::List { path: sandbox.shown() })
            .expect("listing")
        else {
            panic!("expected a listing");
        };
        assert_eq!(
            entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            vec!["sub", "alpha.txt"],
            "folders come first"
        );
        let file = &entries[1];
        assert_eq!(file.path, sandbox.at("alpha.txt"));
        assert!(!file.is_dir && !file.is_link);
        assert_eq!(file.size, 5);
        assert!(file.modified.is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(sandbox.path.join("alpha.txt"))
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(file.permissions, Some(mode), "the mode is the file's own");
            assert_eq!(mode & 0o170000, 0o100000, "a regular file");
        }
        assert_eq!(entries[0].size, 0, "a folder reports no size");
    }

    #[test]
    fn folders_can_be_made_renamed_and_deleted() {
        let sandbox = Sandbox::new("ops");
        execute(SftpRequest::Mkdir {
            path: sandbox.at("first"),
        })
        .expect("mkdir");
        assert!(sandbox.path.join("first").is_dir());

        std::fs::write(sandbox.path.join("first").join("note.txt"), b"hi").expect("write");
        execute(SftpRequest::Rename {
            from: sandbox.at("first"),
            to: sandbox.at("second"),
        })
        .expect("rename");
        assert!(!sandbox.path.join("first").exists());
        assert!(sandbox.path.join("second").join("note.txt").is_file());

        execute(SftpRequest::Remove {
            path: sandbox.at("second"),
            is_dir: true,
        })
        .expect("delete");
        assert!(!sandbox.path.join("second").exists(), "the whole tree is gone");
    }

    #[test]
    fn a_symlink_is_reported_as_a_link() {
        let sandbox = Sandbox::new("link");
        std::fs::write(sandbox.path.join("real.txt"), b"x").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            sandbox.path.join("real.txt"),
            sandbox.path.join("link.txt"),
        )
        .expect("symlink");
        #[cfg(not(unix))]
        return;

        let SftpOutcome::Listed(entries) = execute(SftpRequest::List { path: sandbox.shown() })
            .expect("listing")
        else {
            panic!("expected a listing");
        };
        let link = entries
            .iter()
            .find(|entry| entry.name == "link.txt")
            .expect("the link");
        assert!(link.is_link && !link.is_dir);
    }

    #[test]
    fn a_upload_and_a_download_round_trip() {
        let sandbox = Sandbox::new("transfer");
        let source = sandbox.path.join("source.bin");
        let payload: Vec<u8> = (0..200_000u32).map(|byte| byte as u8).collect();
        std::fs::write(&source, &payload).expect("write");

        let target = sandbox.at("copy.bin");
        let sent = transferred(execute(SftpRequest::Upload {
            local: source.clone(),
            remote: target.clone(),
            resume: false,
        }));
        assert_eq!(sent, payload.len() as u64);
        assert_eq!(std::fs::read(sandbox.path.join("copy.bin")).expect("read"), payload);

        let back = sandbox.path.join("back.bin");
        let pulled = transferred(execute(SftpRequest::Download {
            remote: target,
            local: back.clone(),
            resume: false,
        }));
        assert_eq!(pulled, payload.len() as u64);
        assert_eq!(std::fs::read(&back).expect("read"), payload);
    }

    #[test]
    fn a_resumed_upload_continues_where_it_stopped() {
        let sandbox = Sandbox::new("resume");
        let source = sandbox.path.join("source.bin");
        std::fs::write(&source, b"0123456789").expect("write");
        // The target already holds the first four bytes.
        std::fs::write(sandbox.path.join("target.bin"), b"0123").expect("write");

        let bytes = transferred(execute(SftpRequest::Upload {
            local: source,
            remote: sandbox.at("target.bin"),
            resume: true,
        }));
        assert_eq!(bytes, 6, "only the missing tail moves");
        assert_eq!(
            std::fs::read(sandbox.path.join("target.bin")).expect("read"),
            b"0123456789"
        );

        // A finished copy moves nothing at all.
        let again = transferred(execute(SftpRequest::Upload {
            local: sandbox.path.join("target.bin"),
            remote: sandbox.at("target.bin"),
            resume: true,
        }));
        assert_eq!(again, 0);
    }

    #[test]
    fn a_tree_download_keeps_the_folder_layout() {
        let sandbox = Sandbox::new("tree");
        std::fs::create_dir_all(sandbox.path.join("docs").join("inner")).expect("mkdir");
        std::fs::write(sandbox.path.join("docs").join("a.txt"), b"a").expect("write");
        std::fs::write(sandbox.path.join("docs").join("inner").join("b.txt"), b"b").expect("write");
        let target = Sandbox::new("tree-target");

        let bytes = transferred(execute(SftpRequest::DownloadTree {
            items: vec![sandbox.at("docs")],
            dir: target.path.clone(),
        }));
        assert_eq!(bytes, 2);
        assert_eq!(
            std::fs::read(target.path.join("docs").join("inner").join("b.txt")).expect("read"),
            b"b"
        );
    }

    #[test]
    fn copy_many_moves_when_asked_and_copies_otherwise() {
        let sandbox = Sandbox::new("copy");
        std::fs::write(sandbox.path.join("one.txt"), b"one").expect("write");
        std::fs::write(sandbox.path.join("two.txt"), b"two").expect("write");

        let bytes = transferred(execute(SftpRequest::CopyMany {
            items: vec![(sandbox.at("one.txt"), sandbox.at("one-copy.txt"))],
            scratch: sandbox.path.join("scratch"),
            move_items: false,
        }));
        assert_eq!(bytes, 3);
        assert!(sandbox.path.join("one.txt").is_file(), "a copy keeps the source");
        assert!(sandbox.path.join("one-copy.txt").is_file());

        execute(SftpRequest::CopyMany {
            items: vec![(sandbox.at("two.txt"), sandbox.at("two-moved.txt"))],
            scratch: sandbox.path.join("scratch"),
            move_items: true,
        })
        .expect("move");
        assert!(!sandbox.path.join("two.txt").exists());
        assert!(sandbox.path.join("two-moved.txt").is_file());
    }

    #[test]
    fn a_read_stops_at_the_limit_and_says_so() {
        let sandbox = Sandbox::new("read");
        std::fs::write(sandbox.path.join("note.txt"), b"0123456789").expect("write");

        let SftpOutcome::Content { data, truncated } = execute(SftpRequest::Read {
            path: sandbox.at("note.txt"),
            max_bytes: 4,
        })
        .expect("read")
        else {
            panic!("expected content");
        };
        assert_eq!(data, b"0123");
        assert!(truncated);

        let SftpOutcome::Content { data, truncated } = execute(SftpRequest::Read {
            path: sandbox.at("note.txt"),
            max_bytes: 10,
        })
        .expect("read")
        else {
            panic!("expected content");
        };
        assert_eq!(data.len(), 10);
        assert!(!truncated, "a file that fits exactly is not truncated");
    }

    #[test]
    fn a_missing_path_reports_the_failure_instead_of_panicking() {
        let sandbox = Sandbox::new("missing");
        let error = execute(SftpRequest::List {
            path: sandbox.at("nope"),
        })
        .expect_err("a missing directory");
        assert!(error.starts_with("listing "), "{error}");
        assert!(error.contains("nope"), "{error}");
    }

    #[test]
    fn a_mode_change_reaches_the_file() {
        let sandbox = Sandbox::new("mode");
        let file = sandbox.path.join("script.sh");
        std::fs::write(&file, b"#!/bin/sh\n").expect("write");
        execute(SftpRequest::Chmod {
            path: sandbox.at("script.sh"),
            mode: 0o755,
        })
        .expect("chmod");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn resolving_a_path_returns_an_absolute_one() {
        let home = std::fs::canonicalize(home()).unwrap_or_else(|_| home());
        assert_eq!(
            execute(SftpRequest::RealPath {
                path: "~".to_owned()
            })
            .expect("resolve"),
            SftpOutcome::RealPath(display(&home))
        );
    }
}
