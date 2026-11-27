//! A remote folder served over the session's SFTP channel.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use wassh_ssh::{RemoteAttrs, SftpClient, SftpClientError, SftpOpenOptions};

use crate::{Attrs, FileId, FsError, FsResult, OpenMode, RemoteFs};

/// A remote folder reached through one long-lived SFTP channel.
///
/// The channel belongs to a live session. A mount that reconnects swaps a fresh client in
/// place, so a drive can stall and then carry on instead of vanishing; while there is no
/// client every call answers [`FsError::Disconnected`].
pub struct SftpFs {
    client: Mutex<Option<SftpClient>>,
    next: AtomicU64,
    /// Server handles by the id the filesystem handed out.
    open: Mutex<HashMap<FileId, String>>,
}

impl SftpFs {
    /// Serve `client`.
    pub fn new(client: SftpClient) -> Self {
        Self {
            client: Mutex::new(Some(client)),
            next: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
        }
    }

    /// Serve a client that has just connected, in place of the old one.
    ///
    /// Handles opened on the old channel mean nothing on the new one, so they are dropped; a
    /// program holding one sees its next call fail rather than read the wrong file.
    pub fn replace_client(&self, client: SftpClient) {
        *self.client.lock().expect("the client") = Some(client);
        self.open.lock().expect("the open table").clear();
    }

    /// Stop serving: the connection is gone and no client has arrived yet.
    pub fn clear_client(&self) {
        *self.client.lock().expect("the client") = None;
        self.open.lock().expect("the open table").clear();
    }

    /// The client to ask, or the failure a mount reports while reconnecting.
    fn client(&self) -> FsResult<SftpClient> {
        self.client
            .lock()
            .expect("the client")
            .clone()
            .ok_or(FsError::Disconnected)
    }

    /// The server handle behind `file`.
    fn handle(&self, file: FileId) -> FsResult<String> {
        self.open
            .lock()
            .expect("the open table")
            .get(&file)
            .cloned()
            .ok_or(FsError::NotFound)
    }
}

/// Translate a failed SFTP call into the kinds a filesystem distinguishes.
fn fail(error: SftpClientError) -> FsError {
    match error {
        SftpClientError::NotFound => FsError::NotFound,
        SftpClientError::Denied => FsError::Denied,
        SftpClientError::Unsupported => FsError::Unsupported,
        SftpClientError::Closed => FsError::Disconnected,
        SftpClientError::Io(message) => FsError::Io(message),
    }
}

/// Translate the server's metadata into the crate's.
fn attrs(source: RemoteAttrs) -> Attrs {
    Attrs {
        is_dir: source.is_dir,
        is_link: source.is_link,
        size: source.size,
        modified: source.modified,
        accessed: source.accessed,
        mode: source.mode,
    }
}

impl RemoteFs for SftpFs {
    fn stat(&self, path: &str) -> FsResult<Attrs> {
        self.client()?.stat(path).map(attrs).map_err(fail)
    }

    fn open(
        &self,
        path: &str,
        mode: OpenMode,
        create: bool,
        truncate: bool,
    ) -> FsResult<FileId> {
        let options = SftpOpenOptions {
            read: mode.reads(),
            write: mode.writes(),
            create,
            truncate: truncate && mode.writes(),
            append: false,
            exclusive: false,
        };
        let handle = self.client()?.open(path, options).map_err(fail)?;
        let file = FileId(self.next.fetch_add(1, Ordering::Relaxed));
        self.open
            .lock()
            .expect("the open table")
            .insert(file, handle);
        Ok(file)
    }

    fn read(&self, file: FileId, offset: u64, len: u32) -> FsResult<Vec<u8>> {
        let handle = self.handle(file)?;
        self.client()?.read_at(&handle, offset, len).map_err(fail)
    }

    fn write(&self, file: FileId, offset: u64, data: &[u8]) -> FsResult<u32> {
        let handle = self.handle(file)?;
        self.client()?
            .write_at(&handle, offset, data)
            .map_err(fail)
    }

    fn flush(&self, file: FileId) -> FsResult<()> {
        let handle = self.handle(file)?;
        self.client()?.sync(&handle).map_err(fail)
    }

    fn set_len(&self, file: FileId, size: u64) -> FsResult<()> {
        let handle = self.handle(file)?;
        self.client()?.set_len(&handle, size).map_err(fail)
    }

    fn set_times(&self, path: &str, accessed: u64, modified: u64) -> FsResult<()> {
        self.client()?
            .set_times(path, accessed, modified)
            .map_err(fail)
    }

    fn close(&self, file: FileId) -> FsResult<()> {
        let handle = self
            .open
            .lock()
            .expect("the open table")
            .remove(&file)
            .ok_or(FsError::NotFound)?;
        self.client()?.close(&handle).map_err(fail)
    }

    fn read_dir(&self, path: &str) -> FsResult<Vec<(String, Attrs)>> {
        let entries = self.client()?.read_dir(path).map_err(fail)?;
        Ok(entries
            .into_iter()
            .map(|(name, attrs)| (name, self::attrs(attrs)))
            .collect())
    }

    fn mkdir(&self, path: &str) -> FsResult<()> {
        self.client()?.mkdir(path).map_err(fail)
    }

    fn remove(&self, path: &str) -> FsResult<()> {
        self.client()?.remove(path).map_err(fail)
    }

    fn rmdir(&self, path: &str) -> FsResult<()> {
        self.client()?.rmdir(path).map_err(fail)
    }

    fn rename(&self, from: &str, to: &str) -> FsResult<()> {
        self.client()?.rename(from, to).map_err(fail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filesystem with no client, the state a mount is in while it reconnects.
    fn no_client() -> SftpFs {
        SftpFs {
            client: Mutex::new(None),
            next: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn a_filesystem_without_a_client_reports_the_connection() {
        let fs = no_client();
        assert_eq!(fs.stat("/"), Err(FsError::Disconnected));
        assert_eq!(fs.close(FileId(1)), Err(FsError::NotFound));
    }

    #[test]
    fn clearing_the_client_forgets_every_open_file() {
        let fs = no_client();
        fs.open
            .lock()
            .expect("the open table")
            .insert(FileId(7), "h7".to_owned());
        fs.clear_client();
        assert!(fs.open.lock().expect("the open table").is_empty());
        // The handle belonged to the old channel, so it is gone rather than retried.
        assert_eq!(fs.read(FileId(7), 0, 1), Err(FsError::NotFound));
        // And the folder itself waits for the next connection.
        assert_eq!(fs.stat("/"), Err(FsError::Disconnected));
    }
}
