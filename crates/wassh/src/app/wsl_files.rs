//! The Files pane on a WSL terminal.
//!
//! A WSL tab's host is its distribution, so its files are the distribution's, not this
//! machine's. Windows publishes that filesystem under `\\wsl.localhost\<distro>`, which
//! means the local file backend can serve it unchanged: each pane path is translated to
//! the share, and everything that comes back is translated again. One backend, one set of
//! tests, and uploads, downloads, trees and archives keep working.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Mutex, OnceLock};

use wassh_ssh::{SftpEntry, SftpJob, SftpOutcome, SftpRequest};

use crate::app::local_files;

/// The prefixes Windows publishes distributions under, newest first.
const SHARES: [&str; 2] = [r"\\wsl.localhost", r"\\wsl$"];
/// The user's home inside the distribution, asked of its own shell.
const HOME_COMMAND: &str = "echo $HOME";

/// Directory the Files pane opens first for this distribution: its home.
pub fn start_dir(distro: Option<&str>) -> String {
    match root(distro) {
        Ok(root) => root.home,
        Err(_) => "/".to_owned(),
    }
}

/// Resolve `path` inside the distribution to an absolute `/`-separated path.
pub fn canonical(distro: Option<&str>, path: &str) -> Result<String, String> {
    let root = root(distro)?;
    let host = host_path(&root, path);
    let resolved = std::fs::canonicalize(&host)
        .map_err(|error| format!("resolving {path}: {error}"))?;
    Ok(to_distro(&root.shown, &resolved.to_string_lossy()))
}

/// Run `request` against the distribution's filesystem on its own thread.
pub fn spawn(distro: Option<&str>, request: SftpRequest) -> SftpJob {
    let root = match root(distro) {
        Ok(root) => root,
        Err(error) => return failed(error),
    };
    let SftpJob {
        outcome: answers,
        progress,
        stop,
    } = local_files::spawn(map_request(&root, request));
    let (sender, outcome) = channel();
    let unused = sender.clone();
    let shown = root.shown.clone();
    let spawned = std::thread::Builder::new()
        .name("wassh-wsl-files".to_owned())
        .spawn(move || {
            // Every answer names Windows paths; the pane only knows the distribution's.
            for answer in answers {
                let answer = answer
                    .map(|outcome| map_outcome(&shown, outcome))
                    .map_err(|error| to_distro(&shown, &error));
                if sender.send(answer).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the file worker: {error}")));
    }
    SftpJob {
        outcome,
        progress,
        stop,
    }
}

/// A job that answers with one failure.
fn failed(error: String) -> SftpJob {
    let (sender, outcome) = channel();
    let _ = sender.send(Err(error));
    let (_progress, progress) = channel();
    SftpJob {
        outcome,
        progress,
        stop: wassh_ssh::SftpStop::default(),
    }
}

/// Where a distribution's files sit, as this machine sees them.
struct Root {
    /// The share, e.g. `\\wsl.localhost\Ubuntu`.
    host: PathBuf,
    /// The prefix the local backend puts on every path, e.g. `//wsl.localhost/Ubuntu/`.
    shown: String,
    /// The distribution's home directory, e.g. `/home/ana`.
    home: String,
}

/// The share a WSL distribution is published on.
fn root(distro: Option<&str>) -> Result<Root, String> {
    let name = distro_name(distro)?;
    let host = SHARES
        .iter()
        .map(|prefix| PathBuf::from(format!(r"{prefix}\{name}")))
        .find(|path| path.is_dir())
        .ok_or_else(|| format!("The {name} distribution is not running."))?;
    let home = home_dir(&name);
    let shown = format!("{}/", host.to_string_lossy().replace('\\', "/"));
    Ok(Root { host, shown, home })
}

/// The name of the distribution a WSL tab runs: its own, or WSL's default.
fn distro_name(distro: Option<&str>) -> Result<String, String> {
    if let Some(name) = distro {
        return Ok(name.to_owned());
    }
    let listing = wassh_term::wsl::listing();
    listing
        .iter()
        .find(|info| info.default)
        .or_else(|| listing.first())
        .map(|info| info.name.clone())
        .ok_or_else(|| "No WSL distribution is installed.".to_owned())
}

/// The home directory of the distribution's user.
///
/// The shell inside the distribution is the only thing that knows it, so it is asked once
/// per distribution and remembered.
fn home_dir(name: &str) -> String {
    static HOMES: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    let homes = HOMES.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(home) = homes.lock().ok().and_then(|homes| homes.get(name).cloned()) {
        return home;
    }
    let home = wsl_output(name, HOME_COMMAND)
        .filter(|home| home.starts_with('/'))
        .unwrap_or_else(|| "/".to_owned());
    if let Ok(mut homes) = homes.lock() {
        homes.insert(name.to_owned(), home.clone());
    }
    home
}

/// Run one shell command in the distribution and return its first line of output.
fn wsl_output(name: &str, command: &str) -> Option<String> {
    let output = std::process::Command::new("wsl.exe")
        .args(["--distribution", name, "--exec", "/bin/sh", "-c", command])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(|line| line.trim().to_owned())
}

/// Where `path` lives on this machine, as a path the local backend understands.
fn host_path(root: &Root, path: &str) -> String {
    let trimmed = path.trim();
    let inside = if trimmed.is_empty() || trimmed == "~" {
        root.home.clone()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("{}/{rest}", root.home.trim_end_matches('/'))
    } else if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("{}/{trimmed}", root.home.trim_end_matches('/'))
    };
    let relative = inside.trim_start_matches('/').replace('/', r"\");
    format!(r"{}\{relative}", root.host.display())
}

/// A path from this machine, worded as the distribution's own.
fn to_distro(prefix: &str, text: &str) -> String {
    let forward = text.replace('\\', "/");
    match forward.strip_prefix(prefix) {
        Some(rest) => format!("/{rest}"),
        None => forward,
    }
}

/// Rewrite a request's pane paths as paths on this machine.
fn map_request(root: &Root, request: SftpRequest) -> SftpRequest {
    let host = |path: &str| host_path(root, path);
    match request {
        SftpRequest::List { path } => SftpRequest::List { path: host(&path) },
        SftpRequest::RealPath { path } => SftpRequest::RealPath { path: host(&path) },
        SftpRequest::Read { path, max_bytes } => SftpRequest::Read {
            path: host(&path),
            max_bytes,
        },
        SftpRequest::Mkdir { path } => SftpRequest::Mkdir { path: host(&path) },
        SftpRequest::Rename { from, to } => SftpRequest::Rename {
            from: host(&from),
            to: host(&to),
        },
        SftpRequest::Remove { path, is_dir } => SftpRequest::Remove {
            path: host(&path),
            is_dir,
        },
        SftpRequest::Chmod { path, mode } => SftpRequest::Chmod {
            path: host(&path),
            mode,
        },
        SftpRequest::Download {
            remote,
            local,
            resume,
        } => SftpRequest::Download {
            remote: host(&remote),
            local,
            resume,
        },
        SftpRequest::Upload {
            local,
            remote,
            resume,
        } => SftpRequest::Upload {
            local,
            remote: host(&remote),
            resume,
        },
        SftpRequest::Write { path, content } => SftpRequest::Write {
            path: host(&path),
            content,
        },
        SftpRequest::UploadMany { items, resume } => SftpRequest::UploadMany {
            items: items
                .into_iter()
                .map(|(local, remote)| (local, host(&remote)))
                .collect(),
            resume,
        },
        SftpRequest::DownloadMany { items, resume } => SftpRequest::DownloadMany {
            items: items
                .into_iter()
                .map(|(remote, local)| (host(&remote), local))
                .collect(),
            resume,
        },
        SftpRequest::DownloadTree { items, dir } => SftpRequest::DownloadTree {
            items: items.into_iter().map(|item| host(&item)).collect(),
            dir,
        },
        SftpRequest::CopyMany {
            items,
            scratch,
            move_items,
        } => SftpRequest::CopyMany {
            items: items
                .into_iter()
                .map(|(from, to)| (host(&from), host(&to)))
                .collect(),
            scratch,
            move_items,
        },
    }
}

/// Rewrite the paths an answer carries as the distribution's own.
fn map_outcome(prefix: &str, outcome: SftpOutcome) -> SftpOutcome {
    match outcome {
        SftpOutcome::Listed(entries) => SftpOutcome::Listed(
            entries
                .into_iter()
                .map(|entry| SftpEntry {
                    path: to_distro(prefix, &entry.path),
                    ..entry
                })
                .collect(),
        ),
        SftpOutcome::RealPath(path) => SftpOutcome::RealPath(to_distro(prefix, &path)),
        other => other,
    }
}

/// The local backend's root, for the tests that check the translation.
#[cfg(test)]
fn test_root(host: &str) -> Root {
    let host = PathBuf::from(host);
    let shown = format!("{}/", host.to_string_lossy().replace('\\', "/"));
    Root {
        host,
        shown,
        home: "/home/ana".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_paths_land_on_the_share() {
        let root = test_root(r"\\wsl.localhost\Ubuntu");
        assert_eq!(
            host_path(&root, "/etc/hosts"),
            r"\\wsl.localhost\Ubuntu\etc\hosts"
        );
        assert_eq!(
            host_path(&root, "~/project/main.rs"),
            r"\\wsl.localhost\Ubuntu\home\ana\project\main.rs"
        );
        assert_eq!(host_path(&root, "~"), r"\\wsl.localhost\Ubuntu\home\ana");
        // A relative path starts at the distribution's home, not at Windows' home.
        assert_eq!(
            host_path(&root, "notes.txt"),
            r"\\wsl.localhost\Ubuntu\home\ana\notes.txt"
        );
    }

    #[test]
    fn answers_are_worded_as_the_distribution_worded_them() {
        let root = test_root(r"\\wsl.localhost\Ubuntu");
        assert_eq!(
            to_distro(&root.shown, r"\\wsl.localhost\Ubuntu\home\ana\x"),
            "/home/ana/x"
        );
        assert_eq!(to_distro(&root.shown, "//wsl.localhost/Ubuntu/etc"), "/etc");
        // Something outside the share is left alone rather than mangled.
        assert_eq!(to_distro(&root.shown, r"C:\Users\ana"), "C:/Users/ana");
    }

    #[test]
    fn a_listing_comes_back_with_distribution_paths() {
        let root = test_root(r"\\wsl.localhost\Ubuntu");
        let entry = SftpEntry {
            path: r"\\wsl.localhost\Ubuntu\home\ana\main.rs".replace('\\', "/"),
            name: "main.rs".to_owned(),
            is_dir: false,
            is_link: false,
            size: 12,
            modified: None,
            permissions: None,
        };
        let SftpOutcome::Listed(entries) = map_outcome(&root.shown, SftpOutcome::Listed(vec![entry]))
        else {
            panic!("a listing comes back as a listing");
        };
        assert_eq!(entries[0].path, "/home/ana/main.rs");
        assert_eq!(entries[0].name, "main.rs");
        assert_eq!(entries[0].size, 12);
    }

    #[test]
    fn a_request_carries_the_share_to_the_backend() {
        let root = test_root(r"\\wsl.localhost\Ubuntu");
        let mapped = map_request(
            &root,
            SftpRequest::Upload {
                local: PathBuf::from(r"C:\Users\ana\main.rs"),
                remote: "/home/ana/main.rs".to_owned(),
                resume: false,
            },
        );
        let SftpRequest::Upload { local, remote, .. } = mapped else {
            panic!("an upload stays an upload");
        };
        assert_eq!(local, PathBuf::from(r"C:\Users\ana\main.rs"));
        assert_eq!(remote, r"\\wsl.localhost\Ubuntu\home\ana\main.rs");
    }

    #[test]
    fn a_missing_distribution_answers_with_the_reason() {
        let job = failed("No WSL distribution is installed.".to_owned());
        let error = job.outcome.recv().expect("an answer").expect_err("a failure");
        assert!(error.contains("No WSL distribution"), "{error}");
    }

    #[test]
    fn a_name_with_a_space_keeps_its_share() {
        let root = test_root(r"\\wsl.localhost\Ubuntu 22.04");
        assert_eq!(
            host_path(&root, "/tmp/x"),
            r"\\wsl.localhost\Ubuntu 22.04\tmp\x"
        );
    }

    /// The default distribution, when this machine has one to test against.
    #[cfg(windows)]
    fn default_distro() -> Option<String> {
        let listing = wassh_term::wsl::listing();
        listing
            .iter()
            .find(|info| info.default)
            .or_else(|| listing.first())
            .map(|info| info.name.clone())
    }

    /// Wait for one request's answer.
    #[cfg(windows)]
    fn answer(job: SftpJob) -> SftpOutcome {
        job.outcome
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("an answer")
            .expect("a successful request")
    }

    #[test]
    #[cfg(windows)]
    fn a_distribution_is_read_and_written_through_its_share() {
        let Some(distro) = default_distro() else {
            // Nothing to check on a machine without WSL.
            return;
        };
        let root = answer(spawn(Some(&distro), SftpRequest::List { path: "/".to_owned() }));
        let SftpOutcome::Listed(entries) = root else {
            panic!("a listing comes back as a listing");
        };
        assert!(
            entries.iter().any(|entry| entry.name == "etc"),
            "{:?}",
            entries.iter().map(|entry| &entry.name).collect::<Vec<_>>()
        );
        assert!(
            entries.iter().all(|entry| entry.path.starts_with('/')),
            "the pane only ever sees the distribution's own paths"
        );

        let path = format!("/tmp/wassh-wsl-files-{}.txt", std::process::id());
        answer(spawn(
            Some(&distro),
            SftpRequest::Write {
                path: path.clone(),
                content: b"hello from the pane".to_vec(),
            },
        ));
        let SftpOutcome::Content { data, .. } = answer(spawn(
            Some(&distro),
            SftpRequest::Read {
                path: path.clone(),
                max_bytes: 100,
            },
        )) else {
            panic!("a read comes back as content");
        };
        assert_eq!(String::from_utf8_lossy(&data), "hello from the pane");
        answer(spawn(
            Some(&distro),
            SftpRequest::Remove {
                path,
                is_dir: false,
            },
        ));
    }
}
