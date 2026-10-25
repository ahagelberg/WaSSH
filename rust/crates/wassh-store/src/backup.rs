//! Configuration backups: the data directory as a single archive.
//!
//! The archive holds everything in the data directory, including `secret.key` and
//! `secrets.json`, because a backup that loses passwords is not a backup. Treat the file like
//! a password store.

use std::path::{Path, PathBuf};

use crate::StoreError;
use crate::zip;

/// Files larger than this are skipped rather than blowing up the archive.
const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
/// Deepest subdirectory a backup walks.
const MAX_DEPTH: usize = 4;

/// What a backup covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupSummary {
    /// Archive that was written or read.
    pub path: PathBuf,
    /// Number of files involved.
    pub files: usize,
    /// Bytes in the archive.
    pub bytes: u64,
}

/// Write every file under `dir` into the archive at `target`.
pub fn export(dir: impl AsRef<Path>, target: impl AsRef<Path>) -> Result<BackupSummary, StoreError> {
    let dir = dir.as_ref();
    let target = target.as_ref();
    if !dir.is_dir() {
        return Err(StoreError::io(
            dir,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a directory", dir.display()),
            ),
        ));
    }

    let mut writer = zip::ZipWriter::new();
    collect(dir, dir, 0, &mut writer)?;
    if writer.is_empty() {
        return Err(StoreError::io(
            dir,
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} holds no files to back up", dir.display()),
            ),
        ));
    }
    let files = writer.len();
    let bytes = writer.finish();
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
    }
    std::fs::write(target, &bytes).map_err(|error| StoreError::io(target, error))?;
    Ok(BackupSummary {
        path: target.to_path_buf(),
        files,
        bytes: bytes.len() as u64,
    })
}

/// Recursively add the files under `current`, naming them relative to `root`.
fn collect(
    root: &Path,
    current: &Path,
    depth: usize,
    writer: &mut zip::ZipWriter,
) -> Result<(), StoreError> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(current)
        .map_err(|error| StoreError::io(current, error))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    // Directory order is not guaranteed; sort so two backups of the same data match.
    entries.sort();

    for path in entries {
        let metadata = std::fs::metadata(&path).map_err(|error| StoreError::io(&path, error))?;
        if metadata.is_dir() {
            collect(root, &path, depth + 1, writer)?;
            continue;
        }
        if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if !zip::is_safe_entry_name(&name) {
            continue;
        }
        writer.add(&name, std::fs::read(&path).map_err(|error| StoreError::io(&path, error))?);
    }
    Ok(())
}

/// Restore the archive at `source` into `dir`, overwriting files that already exist.
pub fn import(source: impl AsRef<Path>, dir: impl AsRef<Path>) -> Result<BackupSummary, StoreError> {
    let source = source.as_ref();
    let dir = dir.as_ref();
    let bytes = std::fs::read(source).map_err(|error| StoreError::io(source, error))?;
    let entries = zip::read(&bytes).map_err(|error| {
        StoreError::io(
            source,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: {error}", source.display()),
            ),
        )
    })?;
    if entries.is_empty() {
        return Err(StoreError::io(
            source,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} holds no files", source.display()),
            ),
        ));
    }

    std::fs::create_dir_all(dir).map_err(|error| StoreError::io(dir, error))?;
    let mut files = 0usize;
    for (name, data) in entries {
        if !zip::is_safe_entry_name(&name) {
            return Err(StoreError::io(
                source,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{name}: refusing to write outside the data directory"),
                ),
            ));
        }
        let target = dir.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
        }
        std::fs::write(&target, data).map_err(|error| StoreError::io(&target, error))?;
        files += 1;
    }
    Ok(BackupSummary {
        path: source.to_path_buf(),
        files,
        bytes: bytes.len() as u64,
    })
}

/// A timestamped backup name in `dir`, such as `wassh-backup-20261024-1530.zip`.
pub fn default_archive_path(dir: impl AsRef<Path>, now_ms: u64) -> PathBuf {
    let seconds = now_ms / 1000;
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = wassh_core::time::civil_from_days(days);
    let time = seconds % 86_400;
    dir.as_ref().join(format!(
        "wassh-backup-{year:04}{month:02}{day:02}-{:02}{:02}.zip",
        time / 3600,
        (time % 3600) / 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that cleans itself up.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "wassh-backup-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_backup_restores_every_file() {
        let source = TempDir::new("source");
        std::fs::write(source.path().join("hosts.json"), b"{\"hosts\":[]}").expect("write");
        std::fs::write(source.path().join("settings.json"), b"{}").expect("write");
        std::fs::create_dir_all(source.path().join("plugins")).expect("dir");
        std::fs::write(source.path().join("plugins/plugin-sftp.json"), b"{}").expect("write");

        let archive = TempDir::new("archive");
        let target = archive.path().join("backup.zip");
        let summary = export(source.path(), &target).expect("export");
        assert_eq!(summary.files, 3);
        assert!(summary.bytes > 0);
        assert!(target.is_file());

        let restored = TempDir::new("restored");
        let summary = import(&target, restored.path()).expect("import");
        assert_eq!(summary.files, 3);
        assert_eq!(
            std::fs::read(restored.path().join("hosts.json")).expect("read"),
            b"{\"hosts\":[]}"
        );
        assert!(restored.path().join("plugins/plugin-sftp.json").is_file());
    }

    #[test]
    fn importing_over_existing_files_overwrites_them() {
        let source = TempDir::new("overwrite-source");
        std::fs::write(source.path().join("hosts.json"), b"new").expect("write");
        let archive = TempDir::new("overwrite-archive");
        let target = archive.path().join("backup.zip");
        export(source.path(), &target).expect("export");

        let restored = TempDir::new("overwrite-target");
        std::fs::write(restored.path().join("hosts.json"), b"old").expect("write");
        import(&target, restored.path()).expect("import");
        assert_eq!(
            std::fs::read(restored.path().join("hosts.json")).expect("read"),
            b"new"
        );
    }

    #[test]
    fn an_empty_directory_is_not_a_backup() {
        let empty = TempDir::new("empty");
        let archive = TempDir::new("empty-archive");
        let error = export(empty.path(), archive.path().join("x.zip")).expect_err("no files");
        assert!(error.to_string().contains("no files"), "{error}");
    }

    #[test]
    fn importing_garbage_fails_cleanly() {
        let archive = TempDir::new("garbage");
        let target = archive.path().join("junk.zip");
        std::fs::write(&target, b"definitely not a zip").expect("write");
        let restored = TempDir::new("garbage-target");
        let error = import(&target, restored.path()).expect_err("not a zip");
        assert!(error.to_string().contains("not a zip"), "{error}");
    }

    #[test]
    fn archive_names_are_timestamped() {
        // 2023-11-14 22:13:20 UTC
        let path = default_archive_path("/tmp", 1_700_000_000_000);
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("wassh-backup-20231114-2213.zip")
        );
    }
}
