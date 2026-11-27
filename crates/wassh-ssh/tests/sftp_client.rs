//! End-to-end tests for the blocking SFTP client, against an in-process server.
//!
//! The server serves a real directory, so the client is exercised the way a mount will use
//! it: open handles, offset reads and writes, listings, renames and deletes over a real SSH
//! connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use russh::keys::PrivateKey;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Packet, Status, StatusCode, Version,
};
use wassh_core::SessionStatus;
use wassh_ssh::{
    SftpClient, SftpClientError, SftpOpenOptions, SshAuth, SshEvent, SshOptions,
    SshSession,
};

/// Throwaway ed25519 host key used only by these tests.
const TEST_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACC3YJilmtTL1Yzxj1k9A91pbvonkDdGrD49/iE9WfkUBAAAAJAA4kMiAOJD\nIgAAAAtzc2gtZWQyNTUxOQAAACC3YJilmtTL1Yzxj1k9A91pbvonkDdGrD49/iE9WfkUBA\nAAAECoQUbFK7LflE6UdHX/68zm8oBEXxSmq7niKCwA/yVCNLdgmKWa1MvVjPGPWT0D3Wlu\n+ieQN0asPj3+IT1Z+RQEAAAACndhc3NoLXRlc3QBAgM=\n-----END OPENSSH PRIVATE KEY-----\n";

/// A server that runs a filesystem-backed SFTP subsystem.
#[derive(Clone)]
struct SftpServer {
    root: Arc<PathBuf>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl server::Handler for SftpServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == "secret" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels
            .lock()
            .expect("channel table")
            .insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            let _ = session.channel_failure(channel_id);
            return Ok(());
        }
        let channel = self
            .channels
            .lock()
            .expect("channel table")
            .remove(&channel_id);
        let Some(channel) = channel else {
            let _ = session.channel_failure(channel_id);
            return Ok(());
        };
        let _ = session.channel_success(channel_id);
        let files = Files::new((*self.root).clone());
        tokio::spawn(async move {
            let _ = russh_sftp::server::run(channel.into_stream(), files).await;
        });
        Ok(())
    }
}

/// One open file, as the server knows it.
struct OpenFile {
    file: std::fs::File,
    path: PathBuf,
}

/// The SFTP session state: a real directory behind a virtual root.
struct Files {
    root: PathBuf,
    next: u64,
    open: HashMap<String, OpenFile>,
    dirs: HashMap<String, Vec<File>>,
}

impl Files {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            next: 1,
            open: HashMap::new(),
            dirs: HashMap::new(),
        }
    }

    /// The directory entry a virtual path names.
    fn local(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    fn handle(&mut self) -> String {
        let handle = format!("h{}", self.next);
        self.next += 1;
        handle
    }
}

impl russh_sftp::server::Handler for Files {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.local(&filename);
        let mut options = std::fs::OpenOptions::new();
        options.read(pflags.contains(OpenFlags::READ));
        options.write(pflags.contains(OpenFlags::WRITE));
        options.append(pflags.contains(OpenFlags::APPEND));
        if pflags.contains(OpenFlags::CREATE) {
            if pflags.contains(OpenFlags::EXCLUDE) {
                options.create_new(true);
            } else {
                options.create(true);
            }
        }
        options.truncate(pflags.contains(OpenFlags::TRUNCATE));
        let file = options.open(&path).map_err(status_of)?;
        let handle = self.handle();
        self.open.insert(handle.clone(), OpenFile { file, path });
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.open.remove(&handle);
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let entry = self.open.get(&handle).ok_or(StatusCode::Failure)?;
        let data = pread(&entry.file, offset, len).map_err(status_of)?;
        if data.is_empty() {
            return Err(StatusCode::Eof);
        }
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let entry = self.open.get(&handle).ok_or(StatusCode::Failure)?;
        pwrite(&entry.file, offset, &data).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = std::fs::metadata(self.local(&path)).map_err(status_of)?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = std::fs::symlink_metadata(self.local(&path)).map_err(status_of)?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let entry = self.open.get(&handle).ok_or(StatusCode::Failure)?;
        let metadata = entry.file.metadata().map_err(status_of)?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let local = self.local(&path);
        apply(&local, attrs)?;
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let entry = self.open.get(&handle).ok_or(StatusCode::Failure)?;
        apply(&entry.path, attrs)?;
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let local = self.local(&path);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&local).map_err(status_of)? {
            let entry = entry.map_err(status_of)?;
            let metadata = entry.metadata().map_err(status_of)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.push(File::new(name.clone(), attributes(&metadata)));
        }
        let handle = self.handle();
        self.dirs.insert(handle.clone(), entries);
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.dirs.remove(&handle) {
            Some(files) if !files.is_empty() => Ok(Name { id, files }),
            _ => Err(StatusCode::Eof),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.local(&filename)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.local(&path)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.local(&path)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        std::fs::rename(self.local(&oldpath), self.local(&newpath)).map_err(status_of)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let absolute = format!("/{}", path.trim_start_matches('/'));
        Ok(Name {
            id,
            files: vec![File::dummy(absolute)],
        })
    }

    async fn extended(
        &mut self,
        _id: u32,
        _request: String,
        _data: Vec<u8>,
    ) -> Result<Packet, Self::Error> {
        Err(StatusCode::OpUnsupported)
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_owned(),
        language_tag: "en-US".to_owned(),
    }
}

fn status_of(error: std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => StatusCode::Failure,
        _ => StatusCode::Failure,
    }
}

/// Applies the parts of `attrs` a server can honour: length and times.
fn apply(path: &Path, attrs: FileAttributes) -> Result<(), StatusCode> {
    if let Some(size) = attrs.size {
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(status_of)?;
        handle.set_len(size).map_err(status_of)?;
    }
    if attrs.atime.is_some() || attrs.mtime.is_some() {
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(status_of)?;
        let mut times = std::fs::FileTimes::new();
        if let Some(accessed) = attrs.atime {
            times = times.set_accessed(UNIX_EPOCH + Duration::from_secs(u64::from(accessed)));
        }
        if let Some(modified) = attrs.mtime {
            times = times.set_modified(UNIX_EPOCH + Duration::from_secs(u64::from(modified)));
        }
        handle.set_times(times).map_err(status_of)?;
    }
    Ok(())
}

/// Metadata as the protocol carries it.
fn attributes(metadata: &std::fs::Metadata) -> FileAttributes {
    let mut attrs = FileAttributes {
        size: Some(metadata.len()),
        permissions: Some(mode_of(metadata)),
        ..FileAttributes::default()
    };
    if let Ok(modified) = metadata.modified()
        && let Ok(since) = modified.duration_since(UNIX_EPOCH)
    {
        attrs.mtime = Some(since.as_secs() as u32);
    }
    if let Ok(accessed) = metadata.accessed()
        && let Ok(since) = accessed.duration_since(UNIX_EPOCH)
    {
        attrs.atime = Some(since.as_secs() as u32);
    }
    attrs
}

#[cfg(unix)]
fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    let directory = if metadata.is_dir() { 0o040000 } else { 0o100000 };
    let readonly = metadata.permissions().readonly();
    directory
        | if readonly {
            0o444
        } else if metadata.is_dir() {
            0o755
        } else {
            0o644
        }
}

#[cfg(unix)]
fn pread(file: &std::fs::File, offset: u64, len: u32) -> std::io::Result<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    let mut buffer = vec![0; len as usize];
    let read = file.read_at(&mut buffer, offset)?;
    buffer.truncate(read);
    Ok(buffer)
}

#[cfg(windows)]
fn pread(file: &std::fs::File, offset: u64, len: u32) -> std::io::Result<Vec<u8>> {
    use std::os::windows::fs::FileExt;
    let mut buffer = vec![0; len as usize];
    let read = file.seek_read(&mut buffer, offset)?;
    buffer.truncate(read);
    Ok(buffer)
}

#[cfg(unix)]
fn pwrite(file: &std::fs::File, offset: u64, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)
}

#[cfg(windows)]
fn pwrite(file: &std::fs::File, offset: u64, data: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    file.seek_write(data, offset).map(|_| ())
}

/// Start the server and answer its address.
async fn spawn_server(root: PathBuf) -> (String, u16) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::from_openssh(TEST_HOST_KEY).expect("parse the test host key")],
            ..Default::default()
        });
        let handler = SftpServer {
            root: Arc::new(root),
            channels: Arc::new(Mutex::new(HashMap::new())),
        };
        let _ = server::run_stream(config, stream, handler).await;
    });
    (address.ip().to_string(), address.port())
}

fn options(host: String, port: u16) -> SshOptions {
    let mut options = SshOptions::new(host, "tester", SshAuth::Password("secret".to_owned()));
    options.port = port;
    options.connect_timeout = Duration::from_secs(5);
    options.shell = false;
    options
}

/// Wait until the session reports it is connected.
async fn wait_connected(session: &mut SshSession) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        for event in session.events() {
            match event {
                SshEvent::Status {
                    status: SessionStatus::Connected,
                    ..
                } => return,
                SshEvent::Error(error) => panic!("ssh error: {error}"),
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the session never connected");
}

/// Ask the session for an SFTP client, waiting for it off the async thread.
async fn open_client(session: &SshSession) -> SftpClient {
    let answer = session.open_sftp_client();
    tokio::task::spawn_blocking(move || answer.recv_timeout(Duration::from_secs(10)))
        .await
        .expect("the wait finished")
        .expect("an answer")
        .expect("a client")
}

/// Run one blocking client call off the async thread.
async fn call<T, F>(client: &SftpClient, run: F) -> T
where
    T: Send + 'static,
    F: FnOnce(SftpClient) -> T + Send + 'static,
{
    let client = client.clone();
    tokio::task::spawn_blocking(move || run(client))
        .await
        .expect("the call finished")
}

/// One call that names an open handle, which the blocking closure has to own.
async fn on_handle<T, F>(client: &SftpClient, handle: &str, run: F) -> T
where
    T: Send + 'static,
    F: FnOnce(SftpClient, String) -> T + Send + 'static,
{
    let client = client.clone();
    let handle = handle.to_owned();
    tokio::task::spawn_blocking(move || run(client, handle))
        .await
        .expect("the call finished")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_works_with_handles_offsets_and_names() {
    let root = std::env::temp_dir().join(format!("wassh-sftp-client-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (host, port) = spawn_server(root.clone()).await;
    let mut session = SshSession::connect(options(host, port)).expect("spawn session");
    wait_connected(&mut session).await;
    let client = open_client(&session).await;

    let resolved = call(&client, |client| client.realpath("/")).await;
    assert_eq!(resolved, Ok("/".to_owned()));

    let written = SftpOpenOptions {
        write: true,
        create: true,
        truncate: true,
        ..SftpOpenOptions::default()
    };
    let handle = call(&client, move |client| client.open("/hello.txt", written))
        .await
        .expect("created");
    let wrote = on_handle(&client, &handle, |client, handle| {
        client.write_at(&handle, 6, b"world")
    })
    .await;
    assert_eq!(wrote, Ok(5));
    let wrote = on_handle(&client, &handle, |client, handle| {
        client.write_at(&handle, 0, b"hello ")
    })
    .await;
    assert_eq!(wrote, Ok(6));
    on_handle(&client, &handle, |client, handle| client.close(&handle))
        .await
        .expect("closed");

    let attrs = call(&client, |client| client.stat("/hello.txt"))
        .await
        .expect("stat");
    assert_eq!(attrs.size, 11);
    assert!(!attrs.is_dir);

    let handle = call(&client, |client| {
        client.open("/hello.txt", SftpOpenOptions::read_only())
    })
    .await
    .expect("opened");
    let read = on_handle(&client, &handle, |client, handle| {
        client.read_at(&handle, 6, 5)
    })
    .await;
    assert_eq!(read, Ok(b"world".to_vec()));
    let read = on_handle(&client, &handle, |client, handle| {
        client.read_at(&handle, 11, 5)
    })
    .await;
    assert_eq!(read, Ok(Vec::new()));
    on_handle(&client, &handle, |client, handle| client.close(&handle))
        .await
        .expect("closed");

    let entries = call(&client, |client| client.read_dir("/"))
        .await
        .expect("listing");
    assert!(
        entries
            .iter()
            .any(|(name, attrs)| name == "hello.txt" && attrs.size == 11),
        "listing: {entries:?}"
    );

    call(&client, |client| client.rename("/hello.txt", "/moved.txt"))
        .await
        .expect("renamed");
    call(&client, |client| client.mkdir("/sub")).await.expect("mkdir");
    let handle = call(&client, |client| {
        client.open("/moved.txt", SftpOpenOptions::read_write())
    })
    .await
    .expect("opened");
    on_handle(&client, &handle, |client, handle| client.set_len(&handle, 5))
        .await
        .expect("cut");
    on_handle(&client, &handle, |client, handle| client.sync(&handle))
        .await
        .expect("flushed");
    on_handle(&client, &handle, |client, handle| client.close(&handle))
        .await
        .expect("closed");
    assert_eq!(std::fs::read(root.join("moved.txt")).unwrap(), b"hello");

    call(&client, |client| client.remove("/moved.txt"))
        .await
        .expect("removed");
    call(&client, |client| client.rmdir("/sub")).await.expect("rmdir");
    assert!(!root.join("moved.txt").exists());
    let missing = call(&client, |client| client.stat("/moved.txt")).await;
    assert_eq!(missing, Err(SftpClientError::NotFound));

    session.disconnect();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_never_connected_reports_the_closure() {
    // Nothing listens on this port: the failure has to reach the caller as an event rather
    // than leaving a mount waiting for ever.
    let Ok(session) = SshSession::connect(options("127.0.0.1".to_owned(), 1)) else {
        return;
    };
    let start = Instant::now();
    let mut failed = false;
    while start.elapsed() < Duration::from_secs(10) {
        for event in session.events() {
            if matches!(event, SshEvent::Closed { .. } | SshEvent::Error(_)) {
                failed = true;
            }
        }
        if failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(failed, "the refused connection reported nothing");
}