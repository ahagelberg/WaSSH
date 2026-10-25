//! SSH session transport.
//!
//! `russh` is asynchronous; the UI is not. `SshSession` therefore owns a dedicated
//! runtime thread and exposes a small synchronous facade: send commands in, drain events
//! out. No UI thread ever blocks on the network.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use russh::client::{self, AuthResult, ChannelOpenHandle, Msg};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, ssh_key};
use russh::{ChannelId, ChannelMsg, Disconnect};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use wassh_core::SessionStatus;

use crate::hostkey::HostKeyFingerprint;
use crate::known_hosts::KnownHosts;
use crate::x11::{self, X11Config};

/// How to authenticate.
#[derive(Clone, Debug)]
pub enum SshAuth {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    /// Use the running OpenSSH agent (`SSH_AUTH_SOCK`).
    Agent,
    /// Challenge-response (keyboard-interactive); prompts are answered by the UI.
    KeyboardInteractive,
}

impl SshAuth {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Password(_) => "password",
            Self::PrivateKey { .. } => "private key",
            Self::Agent => "ssh-agent",
            Self::KeyboardInteractive => "keyboard-interactive",
        }
    }
}

/// One hop in a jump chain (an SSH connection that tunnels the next one).
#[derive(Clone, Debug)]
pub struct JumpHop {
    /// Hostname or IP address of this hop.
    pub host: String,
    /// TCP port of this hop.
    pub port: u16,
    /// Login name on this hop.
    pub username: String,
    /// How to authenticate to this hop.
    pub auth: SshAuth,
}

/// Everything needed to open a shell session.
#[derive(Clone, Debug)]
pub struct SshOptions {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    pub connect_timeout: Duration,
    pub keepalive_interval: Option<Duration>,
    pub keepalive_max: usize,
    /// X11 forwarding configuration; `None` disables forwarding.
    pub x11: Option<X11Config>,
    /// Command to run instead of an interactive login shell.
    ///
    /// Used for tmux/screen attach (`tmux new-session -A -s main`), and for any tab that
    /// should start in a specific program.
    pub command: Option<String>,
    /// Port forwards to establish once authenticated.
    pub tunnels: Vec<crate::tunnels::TunnelRequest>,
    /// Jump hosts to traverse before reaching the target, outermost first.
    pub jumps: Vec<JumpHop>,
    /// How to handle server host keys.
    pub host_key_policy: HostKeyPolicy,
}

impl SshOptions {
    pub fn new(host: impl Into<String>, username: impl Into<String>, auth: SshAuth) -> Self {
        Self {
            host: host.into(),
            port: 22,
            username: username.into(),
            auth,
            term: "xterm-256color".to_owned(),
            cols: 80,
            rows: 24,
            connect_timeout: Duration::from_secs(20),
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            x11: None,
            command: None,
            tunnels: Vec::new(),
            jumps: Vec::new(),
            host_key_policy: HostKeyPolicy::AcceptAny,
        }
    }
}

/// What to do when the server presents a host key.
#[derive(Clone)]
pub enum HostKeyPolicy {
    /// Accept anything. Tests and throwaway connections only.
    AcceptAny,
    /// Trust unknown keys silently and remember them (no interactive prompt).
    AcceptNew(Arc<KnownHosts>),
    /// Ask the user for unknown or changed keys, then remember accepted ones.
    Ask(Arc<KnownHosts>),
}

impl std::fmt::Debug for HostKeyPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AcceptAny => f.write_str("AcceptAny"),
            Self::AcceptNew(_) => f.write_str("AcceptNew"),
            Self::Ask(_) => f.write_str("Ask"),
        }
    }
}

/// Commands the UI can send to the session.
#[derive(Debug)]
enum SshCommand {
    Write(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Disconnect,
    /// A remote file request; the answer arrives on `reply`.
    Sftp {
        request: crate::sftp::SftpRequest,
        reply: std::sync::mpsc::Sender<Result<crate::sftp::SftpOutcome, String>>,
        progress: std::sync::mpsc::Sender<crate::sftp::SftpProgress>,
    },
    /// A one-shot remote command; the answer arrives on `reply`.
    Exec {
        command: String,
        /// Text written to the command's stdin before its channel is closed.
        input: Option<String>,
        reply: std::sync::mpsc::Sender<Result<ExecOutput, String>>,
    },
    /// Open a loopback forward to a service on the remote host.
    ForwardTcp {
        host: String,
        port: u16,
        reply: std::sync::mpsc::Sender<Result<(String, u16), String>>,
    },
}

/// One remote file request: its answer, and how far it has got.
pub struct SftpJob {
    /// The answer, once the request finishes.
    pub outcome: std::sync::mpsc::Receiver<Result<crate::sftp::SftpOutcome, String>>,
    /// Bytes moved as it runs.
    pub progress: std::sync::mpsc::Receiver<crate::sftp::SftpProgress>,
}

/// Events the session reports to the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshEvent {
    Status {
        status: SessionStatus,
        message: Option<String>,
    },
    Data(Vec<u8>),
    /// The server's key is unknown or differs from the recorded one; answer with
    /// [`SshSession::respond_host_key`].
    HostKeyPrompt {
        host: String,
        port: u16,
        fingerprint: String,
        mismatch: bool,
    },
    /// Challenge-response authentication needs answers; reply with
    /// [`SshSession::respond_auth`], one entry per prompt.
    AuthPrompt {
        name: String,
        instructions: String,
        prompts: Vec<AuthPromptField>,
    },
    Closed {
        reason: Option<String>,
    },
    /// The credentials were rejected. The caller may ask for another password and
    /// reconnect; a `Closed` event still follows so transports are always released.
    AuthFailed {
        detail: String,
    },
    Error(String),
}

/// Marker error distinguishing rejected credentials from transport failures, so a caller
/// can tell "wrong password" from "cannot reach the host".
#[derive(Debug)]
struct AuthFailure(String);

impl std::fmt::Display for AuthFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AuthFailure {}

/// One keyboard-interactive prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPromptField {
    pub label: String,
    /// When false the UI must mask the input (passwords, one-time codes).
    pub echo: bool,
}

/// Synchronous handle to a background SSH session.
pub struct SshSession {
    commands: UnboundedSender<SshCommand>,
    events: Receiver<SshEvent>,
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    auth_response: Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
    size: (u16, u16),
}

impl SshSession {
    /// Start connecting in the background. Progress is reported through [`Self::events`].
    pub fn connect(options: SshOptions) -> Result<Self> {
        let (command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = channel();
        let decision = Arc::new(Mutex::new(None));
        let auth_response = Arc::new(Mutex::new(None));
        let size = (options.cols, options.rows);
        let thread_decision = decision.clone();
        let thread_auth = auth_response.clone();

        std::thread::Builder::new()
            .name("wassh-ssh".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_tx.send(SshEvent::Error(format!(
                            "cannot start the async runtime: {error}"
                        )));
                        return;
                    }
                };
                runtime.block_on(run(
                    options,
                    command_rx,
                    event_tx,
                    thread_decision,
                    thread_auth,
                ));
            })
            .context("starting the SSH thread")?;

        Ok(Self {
            commands: command_tx,
            events: event_rx,
            decision,
            auth_response,
            size,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.commands.send(SshCommand::Write(bytes.to_vec()));
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != self.size {
            self.size = (cols, rows);
            let _ = self.commands.send(SshCommand::Resize { cols, rows });
        }
    }

    /// Answer a [`SshEvent::HostKeyPrompt`]. Later answers are ignored.
    pub fn respond_host_key(&self, accept: bool) {
        let sender = self
            .decision
            .lock()
            .expect("host-key decision lock")
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(accept);
        }
    }

    /// Answer a [`SshEvent::AuthPrompt`] with one entry per prompt.
    pub fn respond_auth(&self, answers: Vec<String>) {
        let sender = self
            .auth_response
            .lock()
            .expect("auth response lock")
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(answers);
        }
    }

    /// Run a remote file request, returning a receiver for the answer.
    ///
    /// The request runs on its own task so a slow listing or transfer never blocks the
    /// shell.
    pub fn sftp(&self, request: crate::sftp::SftpRequest) -> SftpJob {
        let (tx, rx) = std::sync::mpsc::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let _ = self.commands.send(SshCommand::Sftp {
            request,
            reply: tx,
            progress: progress_tx,
        });
        SftpJob {
            outcome: rx,
            progress: progress_rx,
        }
    }

    /// Run a command on the remote host, returning a receiver for its output.
    ///
    /// Like SFTP requests this runs on its own task, so a slow probe never blocks the shell.
    pub fn exec(
        &self,
        command: impl Into<String>,
    ) -> std::sync::mpsc::Receiver<Result<ExecOutput, String>> {
        self.exec_with_input(command, None)
    }

    /// Run `command`, writing `input` to its stdin first.
    ///
    /// Used for `sudo -S`, which reads the password from stdin rather than argv.
    pub fn exec_with_input(
        &self,
        command: impl Into<String>,
        input: Option<String>,
    ) -> std::sync::mpsc::Receiver<Result<ExecOutput, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self.commands.send(SshCommand::Exec {
            command: command.into(),
            input,
            reply: tx,
        });
        rx
    }

    /// Ask for a loopback forward to `host:port` as seen from the remote host.
    ///
    /// The answer is the local address to dial; a pane that needs a plain TCP
    /// client (the MQTT analyser) connects there and the bytes travel over SSH.
    pub fn forward_tcp(
        &self,
        host: impl Into<String>,
        port: u16,
    ) -> std::sync::mpsc::Receiver<Result<(String, u16), String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self.commands.send(SshCommand::ForwardTcp {
            host: host.into(),
            port,
            reply: tx,
        });
        rx
    }

    pub fn disconnect(&mut self) {
        let _ = self.commands.send(SshCommand::Disconnect);
    }
    /// Drain pending events without blocking.
    pub fn events(&self) -> Vec<SshEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        let _ = self.commands.send(SshCommand::Disconnect);
    }
}

pub(crate) struct ClientHandler {
    policy: HostKeyPolicy,
    host: String,
    port: u16,
    events: Sender<SshEvent>,
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    x11: Option<X11Config>,
    x11_requested: bool,
    shell_channel: Option<ChannelId>,
    remote_forwards: crate::tunnels::RemoteForwards,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string(),
            PublicKeyOrCertificate::Certificate(certificate) => certificate
                .public_key()
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string(),
        };

        match &self.policy {
            HostKeyPolicy::AcceptAny => Ok(true),
            HostKeyPolicy::AcceptNew(known_hosts) => match known_hosts.get(&self.host, self.port) {
                Some(recorded) => Ok(recorded == fingerprint),
                None => {
                    if let Err(error) = known_hosts.remember(HostKeyFingerprint::new(
                        self.host.clone(),
                        self.port,
                        fingerprint,
                    )) {
                        eprintln!("could not record the host key: {error}");
                    }
                    Ok(true)
                }
            },
            HostKeyPolicy::Ask(known_hosts) => {
                let recorded = known_hosts.get(&self.host, self.port);
                if recorded.as_deref() == Some(fingerprint.as_str()) {
                    return Ok(true);
                }
                let mismatch = recorded.is_some();

                let (sender, receiver) = oneshot::channel();
                *self.decision.lock().expect("host-key decision lock") = Some(sender);
                let _ = self.events.send(SshEvent::HostKeyPrompt {
                    host: self.host.clone(),
                    port: self.port,
                    fingerprint: fingerprint.clone(),
                    mismatch,
                });

                let accepted = matches!(
                    tokio::time::timeout(Duration::from_secs(120), receiver).await,
                    Ok(Ok(true))
                );
                if accepted
                    && let Err(error) = known_hosts.remember(HostKeyFingerprint::new(
                        self.host.clone(),
                        self.port,
                        fingerprint,
                    ))
                {
                    eprintln!("could not record the host key: {error}");
                }
                Ok(accepted)
            }
        }
    }

    async fn channel_open_confirmation(
        &mut self,
        id: ChannelId,
        _max_packet_size: u32,
        _window_size: u32,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        self.shell_channel = Some(id);
        Ok(())
    }

    async fn channel_success(
        &mut self,
        channel: ChannelId,
        session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if !self.x11_requested
            && self.shell_channel == Some(channel)
            && let Some(config) = &self.x11
        {
            let _ = session.request_x11(
                channel,
                true,
                false,
                &config.protocol,
                &config.cookie_hex,
                config.screen,
            );
            self.x11_requested = true;
        }
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        if let Some(config) = &self.x11 {
            let display = config.display_number;
            tokio::spawn(async move {
                if let Err(error) = x11::bridge(channel, display).await {
                    eprintln!("X11 forwarding stopped: {error:#}");
                }
            });
        }
        Ok(())
    }

    /// A remote (`-R`) forward: the server accepted a connection on our behalf, so dial the
    /// destination from this machine and splice the two streams together.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some((host, port)) =
            crate::tunnels::resolve_remote(&self.remote_forwards, connected_address, connected_port)
        else {
            reply.reject(crate::tunnels::REJECT_FORWARD).await;
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let Ok(mut local) = tokio::net::TcpStream::connect((host.as_str(), port)).await else {
                return;
            };
            let mut remote = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
        });
        Ok(())
    }
}

async fn run(
    options: SshOptions,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<SshCommand>,
    events: Sender<SshEvent>,
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    auth_response: Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
) {
    let status = |status, message: Option<String>| {
        let _ = events.send(SshEvent::Status { status, message });
    };

    status(SessionStatus::Connecting, None);

    match connect_and_shell(&options, &mut commands, &events, &status, decision, auth_response)
        .await
    {
        Ok(()) => {
            let _ = events.send(SshEvent::Closed { reason: None });
        }
        Err(error) => {
            if error.downcast_ref::<AuthFailure>().is_some() {
                let _ = events.send(SshEvent::AuthFailed {
                    detail: format!("{error:#}"),
                });
            } else {
                let _ = events.send(SshEvent::Error(format!("{error:#}")));
            }
            let _ = events.send(SshEvent::Closed {
                reason: Some(format!("{error:#}")),
            });
        }
    }
}

async fn connect_and_shell(
    options: &SshOptions,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<SshCommand>,
    events: &Sender<SshEvent>,
    status: &impl Fn(SessionStatus, Option<String>),
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    auth_response: Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
) -> Result<()> {
    let forwards = crate::tunnels::remote_forwards();
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: options.keepalive_interval,
        keepalive_max: options.keepalive_max,
        ..Default::default()
    });

    status(SessionStatus::Authenticating, None);
    // Jump hosts bring up a stack of authenticated connections; the last one tunnels to
    // the target. Keep the outer handles alive for as long as the session runs.
    let (handle, _chain) = connect_chain(
        options,
        &config,
        events,
        decision,
        &auth_response,
        &forwards,
    )
    .await?;

    let handle = Arc::new(handle);
    // Port forwards come up before the shell so the status strip can report failures.
    for request in &options.tunnels {
        match crate::tunnels::start(handle.clone(), request, &forwards).await {
            Ok(description) => {
                status(SessionStatus::Connected, Some(format!("Forwarding {description}")));
            }
            Err(error) => {
                status(
                    SessionStatus::Connected,
                    Some(format!("Forward {} failed: {error:#}", request.summary())),
                );
            }
        }
    }

    let channel = handle
        .channel_open_session()
        .await
        .context("opening a session channel")?;
    channel
        .request_pty(
            true,
            &options.term,
            u32::from(options.cols),
            u32::from(options.rows),
            0,
            0,
            &[],
        )
        .await
        .context("requesting a PTY")?;
    match &options.command {
        Some(command) => {
            channel
                .exec(true, command.as_bytes())
                .await
                .context("requesting the remote command")?;
        }
        None => {
            channel
                .request_shell(true)
                .await
                .context("requesting a shell")?;
        }
    }

    status(SessionStatus::Connected, None);

    let (mut reader, writer) = channel.split();
    let mut writer = Some(writer);

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(SshCommand::Write(bytes)) => {
                        if let Some(writer) = writer.as_mut() {
                            writer.data(&bytes[..]).await.context("writing to the channel")?;
                        }
                    }
                    Some(SshCommand::Resize { cols, rows }) => {
                        if let Some(writer) = writer.as_mut() {
                            writer
                                .window_change(u32::from(cols), u32::from(rows), 0, 0)
                                .await
                                .context("resizing the remote PTY")?;
                        }
                    }
                    Some(SshCommand::Sftp {
                        request,
                        reply,
                        progress,
                    }) => {
                        let handle = handle.clone();
                        tokio::spawn(async move {
                            let answer = crate::sftp::run(handle, request, Some(progress)).await;
                            let _ = reply.send(answer);
                        });
                    }
                    Some(SshCommand::Exec {
                        command,
                        input,
                        reply,
                    }) => {
                        let handle = handle.clone();
                        tokio::spawn(async move {
                            let answer = crate::exec::run(handle, command, input).await;
                            let _ = reply.send(answer);
                        });
                    }
                    Some(SshCommand::ForwardTcp { host, port, reply }) => {
                        let handle = handle.clone();
                        tokio::spawn(async move {
                            let answer = crate::tunnels::start_local_forward(
                                handle, "127.0.0.1", 0, &host, port,
                            )
                            .await
                            .map_err(|error| format!("{error:#}"));
                            let _ = reply.send(answer);
                        });
                    }
                    Some(SshCommand::Disconnect) | None => break,
                }
            }
            message = reader.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) => {
                        let _ = events.send(SshEvent::Data(data.to_vec()));
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let _ = events.send(SshEvent::Data(data.to_vec()));
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        let _ = events.send(SshEvent::Closed {
                            reason: Some(format!("remote exit status {exit_status}")),
                        });
                        break;
                    }
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }

    drop(writer);
    let _ = handle
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(())
}

async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    username: &str,
    auth: &SshAuth,
    events: &Sender<SshEvent>,
    auth_response: &Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
) -> Result<()> {
    let success = match auth {
        SshAuth::Password(password) => matches!(
            handle
                .authenticate_password(username.to_owned(), password.clone())
                .await
                .context("password authentication")?,
            AuthResult::Success
        ),
        SshAuth::PrivateKey { path, passphrase } => {
            let key = russh::keys::load_secret_key(path, passphrase.as_deref())
                .with_context(|| format!("loading {}", path.display()))?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .context("negotiating the RSA hash")?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            matches!(
                handle
                    .authenticate_publickey(username.to_owned(), key)
                    .await
                    .context("public-key authentication")?,
                AuthResult::Success
            )
        }
        SshAuth::Agent => authenticate_agent(handle, username).await?,
        SshAuth::KeyboardInteractive => {
            authenticate_keyboard_interactive(handle, username, events, auth_response).await?
        }
    };

    if success {
        Ok(())
    } else {
        Err(anyhow::Error::new(AuthFailure(format!(
            "authentication failed for {username} ({})",
            auth.describe()
        ))))
    }
}

/// Build a handler for one connection in the chain.
fn make_handler(
    options: &SshOptions,
    host: &str,
    port: u16,
    events: &Sender<SshEvent>,
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    forwards: &crate::tunnels::RemoteForwards,
) -> ClientHandler {
    ClientHandler {
        policy: options.host_key_policy.clone(),
        host: host.to_owned(),
        port,
        events: events.clone(),
        decision,
        x11: options.x11.clone(),
        x11_requested: false,
        shell_channel: None,
        remote_forwards: forwards.clone(),
    }
}

/// Connect to the target, tunnelling through `options.jumps` when present.
///
/// Returns the target handle plus the handles of every jump host; the caller keeps the
/// chain alive for the lifetime of the session.
async fn connect_chain(
    options: &SshOptions,
    config: &Arc<client::Config>,
    events: &Sender<SshEvent>,
    decision: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    auth_response: &Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
    forwards: &crate::tunnels::RemoteForwards,
) -> Result<(client::Handle<ClientHandler>, Vec<client::Handle<ClientHandler>>)> {
    let timeout = options.connect_timeout;

    if options.jumps.is_empty() {
        let handler = make_handler(
            options,
            &options.host,
            options.port,
            events,
            decision,
            forwards,
        );
        let mut handle = tokio::time::timeout(
            timeout,
            client::connect(config.clone(), (options.host.as_str(), options.port), handler),
        )
        .await
        .map_err(|_| anyhow!("connection to {}:{} timed out", options.host, options.port))?
        .context("opening the SSH connection")?;
        authenticate(&mut handle, &options.username, &options.auth, events, auth_response)
            .await?;
        return Ok((handle, Vec::new()));
    }

    let mut chain: Vec<client::Handle<ClientHandler>> = Vec::with_capacity(options.jumps.len());
    let first = &options.jumps[0];
    let handler = make_handler(options, &first.host, first.port, events, decision.clone(), forwards);
    let mut current = tokio::time::timeout(
        timeout,
        client::connect(config.clone(), (first.host.as_str(), first.port), handler),
    )
    .await
    .map_err(|_| anyhow!("connection to jump host {}:{} timed out", first.host, first.port))?
    .with_context(|| format!("opening the jump host {}:{}", first.host, first.port))?;
    authenticate(&mut current, &first.username, &first.auth, events, auth_response).await?;

    // Each subsequent hop (and finally the target) is reached through the previous one.
    let hops: Vec<(String, u16, String, SshAuth)> = options
        .jumps
        .iter()
        .skip(1)
        .map(|hop| (hop.host.clone(), hop.port, hop.username.clone(), hop.auth.clone()))
        .chain(std::iter::once((
            options.host.clone(),
            options.port,
            options.username.clone(),
            options.auth.clone(),
        )))
        .collect();

    for (host, port, username, auth) in hops {
        let _ = events.send(SshEvent::Status {
            status: SessionStatus::Authenticating,
            message: Some(format!("Tunnelling through {host}:{port}…")),
        });
        let channel = current
            .channel_open_direct_tcpip(host.clone(), u32::from(port), "127.0.0.1", 0)
            .await
            .with_context(|| format!("opening a tunnel to {host}:{port}"))?;
        let stream = channel.into_stream();
        let handler = make_handler(options, &host, port, events, decision.clone(), forwards);
        let mut next = tokio::time::timeout(
            timeout,
            client::connect_stream(config.clone(), stream, handler),
        )
        .await
        .map_err(|_| anyhow!("connection to {host}:{port} timed out"))?
        .with_context(|| format!("opening the SSH connection to {host}:{port}"))?;
        authenticate(&mut next, &username, &auth, events, auth_response).await?;
        // The previous handle must stay alive: the new connection runs over its channel.
        chain.push(std::mem::replace(&mut current, next));
    }

    Ok((current, chain))
}

/// Run the challenge-response flow, asking the UI for each round of prompts.
async fn authenticate_keyboard_interactive(
    handle: &mut client::Handle<ClientHandler>,
    username: &str,
    events: &Sender<SshEvent>,
    auth_response: &Arc<Mutex<Option<oneshot::Sender<Vec<String>>>>>,
) -> Result<bool> {
    use client::KeyboardInteractiveAuthResponse as Response;

    let mut response = handle
        .authenticate_keyboard_interactive_start(username.to_owned(), None::<String>)
        .await
        .context("starting keyboard-interactive authentication")?;

    loop {
        match response {
            Response::Success => return Ok(true),
            Response::Failure { .. } => return Ok(false),
            Response::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let fields: Vec<AuthPromptField> = prompts
                    .iter()
                    .map(|prompt| AuthPromptField {
                        label: prompt.prompt.clone(),
                        echo: prompt.echo,
                    })
                    .collect();

                let (sender, receiver) = oneshot::channel();
                *auth_response.lock().expect("auth response lock") = Some(sender);
                let _ = events.send(SshEvent::AuthPrompt {
                    name,
                    instructions,
                    prompts: fields,
                });

                let answers = match tokio::time::timeout(Duration::from_secs(300), receiver)
                    .await
                {
                    Ok(Ok(answers)) => answers,
                    _ => return Ok(false),
                };

                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .context("answering keyboard-interactive prompts")?;
            }
        }
    }
}

/// Try every identity the running OpenSSH agent offers.
async fn authenticate_agent(
    handle: &mut client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool> {
    let mut agent = AgentClient::connect_env()
        .await
        .context("connecting to the SSH agent (is SSH_AUTH_SOCK set?)")?;
    let identities = agent
        .request_identities()
        .await
        .context("listing SSH agent identities")?;
    if identities.is_empty() {
        return Ok(false);
    }

    let hash = handle
        .best_supported_rsa_hash()
        .await
        .context("negotiating the RSA hash")?
        .flatten();

    for identity in identities {
        let key = match &identity {
            AgentIdentity::PublicKey { key, .. } => key.clone(),
            AgentIdentity::Certificate { certificate, .. } => {
                certificate.public_key().clone().into()
            }
        };
        match handle
            .authenticate_publickey_with(username.to_owned(), key, hash, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => return Ok(true),
            Ok(AuthResult::Failure { .. }) => continue,
            Err(error) => {
                eprintln!("agent identity rejected: {error}");
                continue;
            }
        }
    }
    Ok(false)
}

/// Keep the `Msg` import used by the generic channel type honest.
type _Channel = russh::Channel<Msg>;

/// Output of a one-shot remote command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
}

/// Run `command` on the remote host and collect its output.
///
/// Opens its own connection, authenticates, runs one exec channel and disconnects. This
/// blocks the calling thread, so call it from a worker thread; `timeout` bounds the whole
/// exchange. Keyboard-interactive authentication is not supported here (there is no UI to
/// answer the prompts) and reports an error instead.
pub fn capture(
    options: SshOptions,
    command: impl Into<String>,
    timeout: Duration,
) -> Result<ExecOutput> {
    let command = command.into();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;
    runtime.block_on(capture_async(options, command, timeout))
}

async fn capture_async(options: SshOptions, command: String, timeout: Duration) -> Result<ExecOutput> {
    tokio::time::timeout(timeout, async move {
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            keepalive_interval: None,
            keepalive_max: 0,
            ..Default::default()
        });
        let (event_tx, _event_rx) = channel();
        let handler = ClientHandler {
            policy: HostKeyPolicy::AcceptNew(Arc::new(KnownHosts::in_memory())),
            host: options.host.clone(),
            port: options.port,
            events: event_tx.clone(),
            decision: Arc::new(Mutex::new(None)),
            x11: None,
            x11_requested: false,
            shell_channel: None,
            remote_forwards: crate::tunnels::remote_forwards(),
        };
        let mut handle = client::connect(
            config,
            (options.host.as_str(), options.port),
            handler,
        )
        .await
        .with_context(|| format!("opening {}:{}", options.host, options.port))?;

        let auth_response = Arc::new(Mutex::new(None));
        authenticate(
            &mut handle,
            &options.username,
            &options.auth,
            &event_tx,
            &auth_response,
        )
        .await?;

        let channel = handle
            .channel_open_session()
            .await
            .context("opening a session channel")?;
        channel
            .exec(true, command.as_bytes())
            .await
            .context("requesting the remote command")?;

        let (mut reader, writer) = channel.split();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        loop {
            match reader.wait().await {
                Some(ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
                Some(ChannelMsg::ExtendedData { data, .. }) => stderr.extend_from_slice(&data),
                Some(ChannelMsg::ExitStatus { exit_status: status }) => {
                    exit_status = Some(status);
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
        drop(writer);
        let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;

        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_status,
        })
    })
    .await
    .map_err(|_| anyhow!("remote command timed out after {timeout:?}"))?
}
