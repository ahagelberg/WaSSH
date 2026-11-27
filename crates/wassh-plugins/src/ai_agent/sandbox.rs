//! What a command can reach once it has been allowed to run.
//!
//! The policy in [`super::policy`] decides whether a command runs. This decides what it can
//! then touch. On Linux the kernel enforces it (Landlock): writes outside the working tree,
//! the chat's scratch directory and the system's temporary directories fail with EACCES, so
//! being allowed to run is no longer the same as being allowed to write anywhere. Reads stay
//! open — the policy already refuses the secrets — because a build reads `/usr`, `/etc`,
//! `/proc` and its whole dependency tree.
//!
//! A ruleset restricts the thread that applies it and everything that thread starts, so it is
//! applied on the worker thread that runs the command — never on the thread that draws the
//! window, which would confine wassh itself.

use std::path::PathBuf;

/// Directories a build expects to write in, besides the working tree.
const TEMPORARY_DIRS: &[&str] = &["/tmp", "/var/tmp", "/dev/shm"];

/// Device nodes a shell writes to without meaning to: a redirect to `/dev/null`, a terminal.
const WRITABLE_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/tty",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
];

/// What a command's confinement turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confinement {
    /// The kernel confines the command's writes to [`writable`].
    Enforced,
    /// Nothing confines it, and this is why.
    None(&'static str),
}

/// The paths a confined command may write: `roots` (the working tree and the chat's scratch
/// directory) plus the places a build writes to.
pub fn writable(roots: &[&str]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|root| !root.trim().is_empty())
        .map(PathBuf::from)
        .chain(TEMPORARY_DIRS.iter().map(PathBuf::from))
        .chain(WRITABLE_DEVICES.iter().map(PathBuf::from))
        .collect()
}

/// Confine this thread, and so every command it starts from now on, to write only under
/// `paths`. Call it on the thread that runs the command.
pub fn confine(paths: &[PathBuf]) -> Confinement {
    platform::confine(paths)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use landlock::{
        ABI, AccessFs, BitFlags, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus,
    };

    use super::Confinement;

    /// The write rights a command may take, and the only ones the kernel watches.
    fn rights() -> BitFlags<AccessFs> {
        AccessFs::from_write(ABI::V10)
    }

    /// A ruleset allowing `paths` and nothing else, or why this system has none.
    fn ruleset(paths: &[PathBuf]) -> Result<landlock::RulesetCreated, &'static str> {
        let mut created = Ruleset::default()
            .handle_access(rights())
            .map_err(|_| "this kernel does not enforce a ruleset")?
            .create()
            .map_err(|_| "a ruleset was refused")?;
        for path in paths {
            // A path that is not there cannot be allowed, and that is not an error: `/dev/shm`
            // is missing in some containers.
            let Ok(fd) = PathFd::new(path) else {
                continue;
            };
            created = created
                .add_rule(PathBeneath::new(fd, rights()))
                .map_err(|_| "a rule was refused")?;
        }
        Ok(created)
    }

    /// Whether a ruleset is enforced here, asked once by trying one on a thread of its own:
    /// applying a ruleset confines the thread, so the attempt cannot run in this one.
    fn enforced() -> bool {
        static ENFORCED: OnceLock<bool> = OnceLock::new();
        *ENFORCED.get_or_init(|| {
            std::thread::spawn(|| {
                ruleset(&[PathBuf::from("/")])
                    .and_then(|created| created.restrict_self().map_err(|_| "a rule was refused"))
                    .is_ok_and(|status| status.ruleset != RulesetStatus::NotEnforced)
            })
            .join()
            .unwrap_or(false)
        })
    }

    pub(super) fn confine(paths: &[PathBuf]) -> Confinement {
        if !enforced() {
            return Confinement::None("this kernel has no Landlock");
        }
        let Ok(created) = ruleset(paths) else {
            return Confinement::None("this kernel refused a ruleset");
        };
        match created.restrict_self() {
            Ok(status) if status.ruleset != RulesetStatus::NotEnforced => Confinement::Enforced,
            _ => Confinement::None("this kernel refused the ruleset"),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::path::Path;
        use std::process::Command;

        use super::*;

        #[test]
        fn a_ruleset_allows_what_it_names_and_refuses_the_rest() {
            if !enforced() {
                return;
            }
            let base = std::env::temp_dir().join(format!("wassh-sandbox-{}", std::process::id()));
            let inside = base.join("in");
            let outside = base.join("out");
            std::fs::create_dir_all(&inside).expect("a directory");
            std::fs::create_dir_all(&outside).expect("a directory");
            let allowed = inside.join("a");
            let refused = outside.join("b");
            // The ruleset confines this thread, so it is applied here and the command is
            // started after it: exactly what the worker thread does.
            let mut shell = Command::new("sh");
            shell.arg("-c").arg(format!(
                "echo a > '{}'; echo b > '{}'",
                allowed.display(),
                refused.display()
            ));
            assert_eq!(confine(&[inside]), Confinement::Enforced);
            let output = shell.output().expect("the shell ran");
            assert!(Path::new(&allowed).exists(), "the allowed write failed");
            assert!(!Path::new(&refused).exists(), "the refused write happened");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Permission denied"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::path::PathBuf;

    use super::Confinement;

    pub(super) fn confine(_paths: &[PathBuf]) -> Confinement {
        Confinement::None("only Linux confines a command")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_working_tree_comes_first_and_the_temporaries_follow() {
        let paths = writable(&["/srv/app", "", "/srv/scratch"]);
        assert_eq!(paths[0], PathBuf::from("/srv/app"));
        assert_eq!(paths[1], PathBuf::from("/srv/scratch"));
        assert!(paths.contains(&PathBuf::from("/tmp")));
        assert!(paths.contains(&PathBuf::from("/dev/null")));
    }
}
