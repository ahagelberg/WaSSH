//! Connections to other hosts than the tab's own, opened on a pane's request.
//!
//! The app owns these. A pane names a saved host; the app resolves its credentials, jump
//! hosts and host keys the way a tab does, keeps one connection per host so two chats share
//! it, and hands the pane a [`SessionAccess`] over it. Nothing here prompts: a connection
//! that needs a password the vault does not hold reports [`RemoteState::NeedsPassword`] and
//! waits for the pane to collect one.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use wassh_core::{
    AuthMethod, ConnectionParams, HostProfile, HostsDocument, Identity, Platform, SessionStatus,
};
use wassh_plugins::{
    CommandAccess, CommandOutput, MonitorAccess, RemoteHost, RemoteHosts, RemoteState,
    SessionAccess, SftpAccess, SftpEntry, SftpListing, TcpAccess, TerminalAccess,
    files::FileContent,
};
use wassh_ssh::{
    ExecJob, HostKeyPolicy, JumpHop, KnownHosts, SftpJob, SshAuth, SshEvent, SshOptions, SshSession,
};

use super::session::{Secrets, jump_chain, resolve_auth};

/// How long a connection nobody used is kept before it is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Terminal size a side connection reports. It opens no shell, so the size is nominal.
const SIDE_COLS: u16 = 80;
/// Rows, as above.
const SIDE_ROWS: u16 = 24;
/// Port an SSH connection uses when the profile leaves it unset.
const DEFAULT_PORT: u16 = 22;

/// What one saved host needs before it can be dialled.
enum Reach {
    /// Ready: parameters, the secrets they need, and the jump hosts in front of it.
    Ready {
        params: Box<ConnectionParams>,
        secrets: Secrets,
        jumps: Vec<JumpHop>,
    },
    /// The parameters need a password the vault does not hold.
    NeedsPassword {
        params: Box<ConnectionParams>,
        jumps: Vec<JumpHop>,
    },
    /// It cannot be opened, and this is why.
    Blocked(String),
}

/// One connection, as a pane handle left it.
struct Side {
    host_id: String,
    state: RemoteState,
    session: Option<SideSession>,
    /// When the pane last touched it, for the idle timeout.
    used: Instant,
}

/// Every connection opened for a pane, and what the app can offer it.
pub struct RemoteSessions {
    /// The saved hosts, rebuilt on [`RemoteSessions::sync`].
    hosts: Vec<RemoteHost>,
    reach: HashMap<String, Reach>,
    sides: HashMap<String, Side>,
    known_hosts: Arc<KnownHosts>,
    idle: Duration,
}

impl RemoteSessions {
    /// A broker that checks host keys against `known_hosts`.
    pub fn new(known_hosts: Arc<KnownHosts>) -> Self {
        Self {
            hosts: Vec::new(),
            reach: HashMap::new(),
            sides: HashMap::new(),
            known_hosts,
            idle: IDLE_TIMEOUT,
        }
    }

    /// Rebuild the inventory from the app's configuration.
    ///
    /// Called when the host list may have changed, not per frame: it resolves every saved
    /// host's credentials and jump chain once, so a connection request costs nothing.
    pub fn sync(
        &mut self,
        hosts: &HostsDocument,
        identities: &[Identity],
        vault: Option<&wassh_store::SecretsStore>,
    ) {
        self.hosts.clear();
        self.reach.clear();
        for host in &hosts.hosts {
            let (reach, problem) = self.classify(host, hosts, identities, vault);
            self.reach.insert(host.id.clone(), reach);
            self.hosts.push(RemoteHost {
                id: host.id.clone(),
                name: host.display_name().to_owned(),
                address: address_of(host),
                tags: host.tags.clone(),
                problem,
            });
        }
        // A connection to a host that no longer exists has nothing left to reach.
        self.sides
            .retain(|_, side| self.reach.contains_key(&side.host_id));
    }

    /// What opening `host` would need, and what stands in the way when something does.
    fn classify(
        &self,
        host: &HostProfile,
        hosts: &HostsDocument,
        identities: &[Identity],
        vault: Option<&wassh_store::SecretsStore>,
    ) -> (Reach, Option<String>) {
        let (params, secrets) =
            match resolve_auth(&host.connection, Some(&host.id), identities, vault) {
                Ok(resolved) => resolved,
                Err(reason) => return (Reach::Blocked(reason.clone()), Some(reason)),
            };
        let jumps = match jump_chain(hosts, &host.connection, identities, vault) {
            Ok(jumps) => jumps,
            Err(reason) => return (Reach::Blocked(reason.clone()), Some(reason)),
        };
        if self.known_hosts.get(&params.host, params.port).is_none() {
            let reason = "its host key is not trusted yet — connect once from a terminal tab and \
                          accept it"
                .to_owned();
            return (Reach::Blocked(reason.clone()), Some(reason));
        }
        if params.auth == AuthMethod::Password && secrets.password.is_empty() {
            return (
                Reach::NeedsPassword {
                    params: Box::new(params),
                    jumps,
                },
                Some("needs a password".to_owned()),
            );
        }
        (
            Reach::Ready {
                params: Box::new(params),
                secrets,
                jumps,
            },
            None,
        )
    }

    /// Poll every connection: transport events, running commands and file requests, and the
    /// idle timeout. Called once per frame, beside `poll_commands`.
    pub fn poll(&mut self) {
        let now = Instant::now();
        for side in self.sides.values_mut() {
            if let Some(session) = side.session.as_mut() {
                session.pump();
                let describe = session.describe();
                for event in session.session.events() {
                    if let Some(state) = next_state(&event, &describe) {
                        side.state = state;
                    }
                }
            }
            let idle = matches!(side.state, RemoteState::Open { .. })
                && now.duration_since(side.used) > self.idle;
            if idle {
                side.state = RemoteState::Idle;
            }
            if (idle || matches!(side.state, RemoteState::Failed(_)))
                && let Some(mut session) = side.session.take()
            {
                session.session.disconnect();
            }
        }
    }

    /// Start a connection under `handle`, or answer why it cannot start.
    fn dial(&mut self, handle: &str, host_id: &str, password: Option<String>) {
        let (state, session) = self.open(host_id, password);
        self.sides.insert(
            handle.to_owned(),
            Side {
                host_id: host_id.to_owned(),
                state,
                session,
                used: Instant::now(),
            },
        );
    }

    /// The connection itself: either a dialled session or the reason there is none.
    fn open(&self, host_id: &str, password: Option<String>) -> (RemoteState, Option<SideSession>) {
        let Some(reach) = self.reach.get(host_id) else {
            return (
                RemoteState::Failed(format!("'{host_id}' is not a saved host")),
                None,
            );
        };
        let (params, secrets, jumps) = match reach {
            Reach::Blocked(reason) => {
                return (
                    RemoteState::Failed(format!("{host_id} cannot be opened: {reason}")),
                    None,
                );
            }
            Reach::NeedsPassword { params, jumps } => match password {
                Some(password) => (
                    params.as_ref().clone(),
                    Secrets {
                        password,
                        key_passphrase: String::new(),
                    },
                    jumps.clone(),
                ),
                None => return (RemoteState::NeedsPassword, None),
            },
            Reach::Ready {
                params,
                secrets,
                jumps,
            } => (params.as_ref().clone(), secrets.clone(), jumps.clone()),
        };
        let platform = params.platform();
        let options = side_options(&params, &secrets, jumps, Arc::clone(&self.known_hosts));
        match SshSession::connect(options) {
            Ok(session) => (
                RemoteState::Connecting,
                Some(SideSession::new(session, params, platform)),
            ),
            Err(error) => (RemoteState::Failed(format!("{error:#}")), None),
        }
    }
}

impl RemoteHosts for RemoteSessions {
    fn hosts(&self) -> &[RemoteHost] {
        &self.hosts
    }

    fn connect(&mut self, handle: &str, host_id: &str) {
        if let Some(side) = self.sides.get_mut(handle)
            && side.host_id == host_id
            && matches!(
                side.state,
                RemoteState::Connecting | RemoteState::Open { .. } | RemoteState::NeedsPassword
            )
        {
            side.used = Instant::now();
            return;
        }
        self.dial(handle, host_id, None);
    }

    fn state(&self, handle: &str) -> RemoteState {
        self.sides
            .get(handle)
            .map_or(RemoteState::Idle, |side| side.state.clone())
    }

    fn supply_password(&mut self, handle: &str, password: &str) {
        let Some(host_id) = self.sides.get(handle).map(|side| side.host_id.clone()) else {
            return;
        };
        self.dial(handle, &host_id, Some(password.to_owned()));
    }

    fn session(&mut self, handle: &str) -> Option<&mut dyn SessionAccess> {
        let side = self.sides.get_mut(handle)?;
        side.used = Instant::now();
        let session = side.session.as_mut()?;
        Some(session as &mut dyn SessionAccess)
    }

    fn close(&mut self, handle: &str) {
        if let Some(mut side) = self.sides.remove(handle)
            && let Some(session) = side.session.as_mut()
        {
            session.session.disconnect();
        }
    }
}

/// How a transport event changes what a pane is told, if it does.
fn next_state(event: &SshEvent, environment: &str) -> Option<RemoteState> {
    match event {
        SshEvent::Status { status, message } => match status {
            SessionStatus::Connected => Some(RemoteState::Open {
                environment: environment.to_owned(),
            }),
            SessionStatus::Failed => Some(RemoteState::Failed(
                message
                    .clone()
                    .unwrap_or_else(|| "the connection failed".to_owned()),
            )),
            SessionStatus::Closed | SessionStatus::Disconnected => {
                Some(RemoteState::Failed("the connection closed".to_owned()))
            }
            _ => None,
        },
        SshEvent::AuthFailed { detail } => Some(RemoteState::Failed(format!(
            "the host refused the credentials: {detail}"
        ))),
        SshEvent::HostKeyPrompt { mismatch, .. } => Some(RemoteState::Failed(if *mismatch {
            "the host key changed since it was trusted; connect from a terminal tab to look at it"
                .to_owned()
        } else {
            "the host key is not trusted yet; connect once from a terminal tab and accept it"
                .to_owned()
        })),
        SshEvent::Error(error) => Some(RemoteState::Failed(error.clone())),
        _ => None,
    }
}

/// `user@host:port`, as a person reads an address.
fn address_of(host: &HostProfile) -> String {
    let port = match host.connection.port {
        0 => DEFAULT_PORT,
        port => port,
    };
    match host.connection.username.trim() {
        "" => format!("{}:{port}", host.connection.host),
        user => format!("{user}@{}:{port}", host.connection.host),
    }
}

/// Transport options for a connection opened without a terminal in front of it.
fn side_options(
    params: &ConnectionParams,
    secrets: &Secrets,
    jumps: Vec<JumpHop>,
    known_hosts: Arc<KnownHosts>,
) -> SshOptions {
    let auth = match params.auth {
        AuthMethod::Password => SshAuth::Password(secrets.password.clone()),
        AuthMethod::PrivateKey => SshAuth::PrivateKey {
            path: params.private_key_path.clone().unwrap_or_default().into(),
            passphrase: if secrets.key_passphrase.is_empty() {
                None
            } else {
                Some(secrets.key_passphrase.clone())
            },
        },
        AuthMethod::None => SshAuth::Agent,
    };
    let mut options = SshOptions::new(params.host.clone(), params.username.clone(), auth);
    options.port = params.port;
    options.cols = SIDE_COLS;
    options.rows = SIDE_ROWS;
    options.jumps = jumps;
    // Only a key that is already trusted: nobody is watching to accept a new one.
    options.host_key_policy = HostKeyPolicy::Known(known_hosts);
    options
}

/// One command started on a side connection.
struct CommandJob {
    job: ExecJob,
    output: Option<CommandOutput>,
    failure: Option<String>,
}

/// What the file request in flight is for.
enum FileWhat {
    Listing(String),
    Read(String, u64),
    Write(String),
}

/// One file request started on a side connection.
struct FileJob {
    what: FileWhat,
    job: SftpJob,
}

/// A connection to one host, as the agent's tools see it: commands and files, no terminal.
struct SideSession {
    session: SshSession,
    params: ConnectionParams,
    platform: Platform,
    commands: HashMap<String, CommandJob>,
    file: Option<FileJob>,
    listing: Option<SftpListing>,
    listing_revision: u64,
    content: Option<(String, u64, FileContent)>,
    error: Option<String>,
    transfer_status: Option<String>,
    forwards: HashMap<String, String>,
}

impl SideSession {
    fn new(session: SshSession, params: ConnectionParams, platform: Platform) -> Self {
        Self {
            session,
            params,
            platform,
            commands: HashMap::new(),
            file: None,
            listing: None,
            listing_revision: 0,
            content: None,
            error: None,
            transfer_status: None,
            forwards: HashMap::new(),
        }
    }

    /// One line describing the host, for the state a pane reads.
    fn describe(&self) -> String {
        let params = &self.params;
        match params.username.trim() {
            "" => format!("{} (SSH)", params.host),
            user => format!("{user}@{} (SSH)", params.host),
        }
    }

    /// Take in whatever the running commands and the file request have produced.
    fn pump(&mut self) {
        for command in self.commands.values_mut() {
            if command.output.is_some() || command.failure.is_some() {
                continue;
            }
            match command.job.output.try_recv() {
                Ok(Ok(output)) => {
                    command.output = Some(CommandOutput {
                        stdout: output.stdout,
                        stderr: output.stderr,
                        code: output.exit_status,
                    });
                }
                Ok(Err(problem)) => command.failure = Some(problem),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    command.failure = Some("The command channel closed.".to_owned());
                }
            }
        }
        let Some(job) = self.file.as_mut() else {
            return;
        };
        let outcome = match job.job.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("The file channel closed.".to_owned())
            }
        };
        let what = std::mem::replace(&mut job.what, FileWhat::Write(String::new()));
        self.file = None;
        match (outcome, what) {
            (Ok(wassh_ssh::SftpOutcome::Listed(entries)), FileWhat::Listing(path)) => {
                self.listing = Some(SftpListing {
                    path,
                    entries: entries
                        .into_iter()
                        .map(|entry| SftpEntry {
                            name: entry.name,
                            path: entry.path,
                            is_dir: entry.is_dir,
                            is_link: entry.is_link,
                            size: entry.size,
                            modified: entry.modified,
                            permissions: entry.permissions,
                        })
                        .collect(),
                });
                self.listing_revision += 1;
                self.error = None;
            }
            (Ok(wassh_ssh::SftpOutcome::Content { data, truncated }), FileWhat::Read(path, max)) => {
                self.content = Some((path, max, FileContent { data, truncated }));
                self.error = None;
            }
            (Ok(_), FileWhat::Write(path)) => {
                self.transfer_status = Some(format!("Wrote {path}."));
                self.error = None;
            }
            (Ok(_), _) => {}
            (Err(problem), _) => {
                self.transfer_status = Some(problem.clone());
                self.error = Some(problem);
            }
        }
    }

    /// Start a file request unless the same one is already in flight.
    fn start_file(&mut self, what: FileWhat, request: wassh_ssh::sftp::SftpRequest) {
        if self
            .file
            .as_ref()
            .is_some_and(|job| same_request(&job.what, &what))
        {
            return;
        }
        self.file = Some(FileJob {
            what,
            job: self.session.sftp(request),
        });
    }
}

/// Whether the request in flight is the one being asked for again.
fn same_request(pending: &FileWhat, wanted: &FileWhat) -> bool {
    match (pending, wanted) {
        (FileWhat::Listing(a), FileWhat::Listing(b)) => a == b,
        (FileWhat::Read(a, am), FileWhat::Read(b, bm)) => a == b && am == bm,
        (FileWhat::Write(a), FileWhat::Write(b)) => a == b,
        _ => false,
    }
}

impl CommandAccess for SideSession {
    fn exec(&mut self, key: &str, command: &str) {
        let job = self.session.exec(command);
        self.commands.insert(
            key.to_owned(),
            CommandJob {
                job,
                output: None,
                failure: None,
            },
        );
    }

    fn exec_input(&mut self, key: &str, command: &str, input: &str) {
        let job = self.session.exec_with_input(command, Some(input.to_owned()));
        self.commands.insert(
            key.to_owned(),
            CommandJob {
                job,
                output: None,
                failure: None,
            },
        );
    }

    fn output(&self, key: &str) -> Option<&CommandOutput> {
        self.commands.get(key)?.output.as_ref()
    }

    fn failure(&self, key: &str) -> Option<&str> {
        self.commands.get(key)?.failure.as_deref()
    }

    fn is_running(&self, key: &str) -> bool {
        self.commands
            .get(key)
            .is_some_and(|command| command.output.is_none() && command.failure.is_none())
    }

    fn cancel(&mut self, key: &str) {
        if let Some(command) = self.commands.get_mut(key) {
            command.job.stop.stop();
            if command.output.is_none() && command.failure.is_none() {
                command.failure = Some(wassh_ssh::STOPPED_COMMAND.to_owned());
            }
        }
    }

    fn platform(&self) -> Platform {
        self.platform
    }
}

impl SftpAccess for SideSession {
    fn request_listing(&mut self, path: &str) {
        self.start_file(
            FileWhat::Listing(path.to_owned()),
            wassh_ssh::sftp::SftpRequest::List {
                path: path.to_owned(),
            },
        );
    }

    fn refresh(&mut self, path: &str) {
        self.listing = None;
        self.request_listing(path);
    }

    fn listing(&self) -> Option<&SftpListing> {
        self.listing.as_ref()
    }

    fn listing_revision(&self) -> u64 {
        self.listing_revision
    }

    fn is_loading(&self) -> bool {
        self.file.is_some()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn upload(&mut self, local: &str, remote: &str) {
        self.start_file(
            FileWhat::Write(remote.to_owned()),
            wassh_ssh::sftp::SftpRequest::Upload {
                local: local.into(),
                remote: remote.to_owned(),
                resume: false,
            },
        );
    }

    fn write_file(&mut self, remote: &str, content: Vec<u8>) {
        self.start_file(
            FileWhat::Write(remote.to_owned()),
            wassh_ssh::sftp::SftpRequest::Write {
                path: remote.to_owned(),
                content,
            },
        );
    }

    fn download(&mut self, remote: &str, local: &str) {
        self.start_file(
            FileWhat::Write(remote.to_owned()),
            wassh_ssh::sftp::SftpRequest::Download {
                remote: remote.to_owned(),
                local: local.into(),
                resume: false,
            },
        );
    }

    fn is_transferring(&self) -> bool {
        matches!(
            self.file.as_ref().map(|job| &job.what),
            Some(FileWhat::Read(..) | FileWhat::Write(..))
        )
    }

    fn transfer_status(&self) -> Option<&str> {
        self.transfer_status.as_deref()
    }

    fn report_problem(&mut self, message: &str) {
        self.error = Some(message.to_owned());
    }

    fn resolve(&mut self, path: &str) -> Option<String> {
        // The tools that use a side connection hand it absolute paths; asking the host for
        // each one would cost a round trip for nothing.
        Some(path.to_owned())
    }

    fn mkdir(&mut self, path: &str) {
        self.start_file(
            FileWhat::Write(path.to_owned()),
            wassh_ssh::sftp::SftpRequest::Mkdir {
                path: path.to_owned(),
            },
        );
    }

    fn rename(&mut self, from: &str, to: &str) {
        self.start_file(
            FileWhat::Write(to.to_owned()),
            wassh_ssh::sftp::SftpRequest::Rename {
                from: from.to_owned(),
                to: to.to_owned(),
            },
        );
    }

    fn remove(&mut self, path: &str, is_dir: bool) {
        self.start_file(
            FileWhat::Write(path.to_owned()),
            wassh_ssh::sftp::SftpRequest::Remove {
                path: path.to_owned(),
                is_dir,
            },
        );
    }

    fn chmod(&mut self, path: &str, mode: u32) {
        self.start_file(
            FileWhat::Write(path.to_owned()),
            wassh_ssh::sftp::SftpRequest::Chmod {
                path: path.to_owned(),
                mode,
            },
        );
    }

    fn read(&mut self, path: &str, max_bytes: u64) -> Option<FileContent> {
        if let Some((read_path, read_max, content)) = &self.content
            && read_path == path
            && *read_max == max_bytes
        {
            return Some(content.clone());
        }
        self.start_file(
            FileWhat::Read(path.to_owned(), max_bytes),
            wassh_ssh::sftp::SftpRequest::Read {
                path: path.to_owned(),
                max_bytes,
            },
        );
        None
    }

    fn upload_many(&mut self, items: Vec<(String, String)>) {
        for (local, remote) in items {
            self.upload(&local, &remote);
        }
    }

    fn download_many(&mut self, items: Vec<(String, String)>) {
        for (remote, local) in items {
            self.download(&remote, &local);
        }
    }

    fn copy_many(&mut self, items: Vec<(String, String)>, move_items: bool) {
        // One request at a time, and a move is a rename.
        for (from, to) in items {
            if move_items {
                self.rename(&from, &to);
            }
        }
    }

    fn progress(&self) -> Option<(u64, Option<u64>)> {
        None
    }

    fn download_tree(&mut self, items: Vec<String>, dir: &str) {
        for item in items {
            let name = item.rsplit('/').next().unwrap_or(&item).to_owned();
            self.download(&item, &format!("{}/{name}", dir.trim_end_matches('/')));
        }
    }

    fn download_zip(&mut self, _items: Vec<String>, _archive: &str) {}

    fn transfers(&self) -> Vec<wassh_plugins::files::TransferRow> {
        Vec::new()
    }

    fn retry(&mut self) {}
}

impl TerminalAccess for SideSession {
    fn screen_text(&self) -> String {
        // A side connection opens no shell, so there is no screen to read.
        String::new()
    }

    fn type_text(&mut self, _text: &str) {}

    fn is_closed(&self) -> bool {
        false
    }
}

impl TcpAccess for SideSession {
    fn open_forward(&mut self, key: &str, _host: &str, _port: u16) {
        self.forwards.insert(
            key.to_owned(),
            "A connection opened for the agent does not forward ports.".to_owned(),
        );
    }

    fn forward_address(&self, _key: &str) -> Option<&str> {
        None
    }

    fn forward_error(&self, key: &str) -> Option<&str> {
        self.forwards.get(key).map(String::as_str)
    }

    fn forward_pending(&self, _key: &str) -> bool {
        false
    }
}

impl MonitorAccess for SideSession {}

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::ConnectionType;

    /// A document holding one SSH host that wants a password.
    fn hosts_with(host_id: &str) -> HostsDocument {
        let mut hosts = HostsDocument::default();
        let mut profile = HostProfile {
            id: host_id.to_owned(),
            name: host_id.to_owned(),
            ..HostProfile::default()
        };
        profile.connection.kind = ConnectionType::Ssh;
        profile.connection.host = "10.0.0.4".to_owned();
        profile.connection.port = 22;
        profile.connection.username = "deploy".to_owned();
        profile.connection.auth = AuthMethod::Password;
        hosts.hosts.push(profile);
        hosts
    }

    #[test]
    fn the_inventory_names_each_host_and_what_it_needs() {
        let hosts = hosts_with("db-1");
        let mut broker = RemoteSessions::new(Arc::new(KnownHosts::in_memory()));
        broker.sync(&hosts, &[], None);
        assert_eq!(broker.hosts().len(), 1);
        let host = &broker.hosts()[0];
        assert_eq!(host.name, "db-1");
        assert_eq!(host.address, "deploy@10.0.0.4:22");
        // No stored password and no trusted key: the pane can say why before trying.
        assert!(host.problem.is_some(), "{host:?}");
    }

    #[test]
    fn an_untrusted_host_key_is_what_stops_the_connection_first() {
        let hosts = hosts_with("db-1");
        let mut broker = RemoteSessions::new(Arc::new(KnownHosts::in_memory()));
        broker.sync(&hosts, &[], None);
        broker.connect("host:db-1", "db-1");
        match broker.state("host:db-1") {
            RemoteState::Failed(reason) => {
                assert!(reason.contains("host key"), "{reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_host_fails_with_its_id() {
        let hosts = hosts_with("db-1");
        let mut broker = RemoteSessions::new(Arc::new(KnownHosts::in_memory()));
        broker.sync(&hosts, &[], None);
        broker.connect("host:web", "web");
        match broker.state("host:web") {
            RemoteState::Failed(reason) => assert!(reason.contains("'web'"), "{reason}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_second_request_under_the_same_handle_keeps_its_answer() {
        let hosts = hosts_with("db-1");
        let mut broker = RemoteSessions::new(Arc::new(KnownHosts::in_memory()));
        broker.sync(&hosts, &[], None);
        broker.connect("host:db-1", "db-1");
        let first = broker.state("host:db-1");
        broker.connect("host:db-1", "db-1");
        assert_eq!(broker.state("host:db-1"), first);
    }

    #[test]
    fn a_host_that_is_gone_leaves_nothing_behind() {
        let hosts = hosts_with("db-1");
        let mut broker = RemoteSessions::new(Arc::new(KnownHosts::in_memory()));
        broker.sync(&hosts, &[], None);
        broker.connect("host:db-1", "db-1");
        broker.sync(&HostsDocument::default(), &[], None);
        assert!(broker.hosts().is_empty());
        assert_eq!(broker.state("host:db-1"), RemoteState::Idle);
    }
}
