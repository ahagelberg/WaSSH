//! One open session: an emulator plus its transport.
//!
//! A session owns a [`Terminal`] (alacritty-backed emulator with an event channel), a
//! [`TerminalView`] for drawing and a [`Backend`] that is either a local PTY or an SSH
//! channel. The shell polls [`SessionEntry::pump`] every frame; nothing here blocks.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use super::{local_exec, local_files, wsl_files};
use wassh_core::{
    AuthMethod, ConnectionParams, ConnectionType, DockEdge, DockPlacement, LocalShell, Palette,
    SessionStatus, SessionStyleDefaults, TabPluginLayout,
};
use wassh_plugins::files::{TransferDirection, TransferState};
use wassh_ui::dock::HeaderHealth;
use wassh_plugins::{Plugin, PluginContext, PluginId, SftpEntry, SftpListing};
use wassh_ssh::{KnownHosts, SshAuth, SshEvent, SshOptions, SshSession};
use wassh_term::{
    CellSnapshot, Key, PtySession, ScreenSnapshot, Terminal, TerminalEvent, TerminalSize, encode,
};
use wassh_ui::TerminalView;

/// How many status changes one session keeps for the connection log plugin.
const MAX_STATUS_HISTORY: usize = 200;

/// Secrets map for a pane that declares none, so the context can borrow a reference.
static NO_SECRETS: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

/// Credentials typed by the user for this session only.
#[derive(Clone, Debug, Default)]
pub struct Secrets {
    pub password: String,
    pub key_passphrase: String,
}

/// One plugin pane in a tab: its instance plus the data it persists.
struct PluginPane {
    id: PluginId,
    plugin: Box<dyn Plugin>,
    data: PluginData,
    /// Plugin secrets the pane sealed or cleared this frame, keyed by field.
    secret_writes: Vec<(String, Option<String>)>,
    /// App-wide settings the pane changed while drawing.
    app_writes: Vec<(String, serde_json::Value)>,
}

/// A pane's stored data, and which of it is not the pane's own.
///
/// A pane reads one flat map, but a value in it can come from three places: the pane's own
/// `plugin-<id>.<scope>.json`, the app-wide plugin settings Options owns, or Host settings.
/// Only the pane's own keys are written back, so the keys the dialogs supplied are kept in
/// `from_dialogs` and stripped from the payload.
#[derive(Default)]
struct PluginData {
    /// What the pane reads and writes.
    values: serde_json::Map<String, serde_json::Value>,
    /// `values` as last written, so an unchanged pane writes nothing.
    written: serde_json::Map<String, serde_json::Value>,
    /// Values the dialogs supplied, with what they supplied, rather than the file.
    from_dialogs: serde_json::Map<String, serde_json::Value>,
    /// Whether the file has been read yet.
    loaded: bool,
}

impl PluginData {
    /// Merge a stored file and the dialog values into what the pane will read.
    fn load(
        &mut self,
        stored: serde_json::Map<String, serde_json::Value>,
        id: PluginId,
        values: &PluginValues,
    ) {
        let mut merged = stored;
        let before = merged.clone();
        // The dialogs own these values; whatever the pane wrote itself survives until a
        // dialog field actually holds something.
        wassh_plugins::settings::apply_to(
            id,
            values.app.get(id.as_str()),
            values.host.get(id.as_str()),
            &mut merged,
        );
        self.from_dialogs = merged
            .iter()
            .filter(|(key, value)| before.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.written.clone_from(&merged);
        self.values = merged;
        self.loaded = true;
    }

    /// The payload to write, when the pane changed something the file owns.
    fn take_changes(&mut self) -> Option<serde_json::Map<String, serde_json::Value>> {
        if !self.loaded || self.values == self.written {
            return None;
        }
        self.written.clone_from(&self.values);
        let mut payload = self.values.clone();
        for (key, value) in &self.from_dialogs {
            if payload.get(key) == Some(value) {
                payload.remove(key);
            }
        }
        Some(payload)
    }

    /// Take a value the dialogs just supplied, without letting it reach the file.
    fn adopt(&mut self, key: &str, value: serde_json::Value) {
        self.values.insert(key.to_owned(), value.clone());
        self.written.insert(key.to_owned(), value.clone());
        self.from_dialogs.insert(key.to_owned(), value);
    }
}

/// App- and host-level plugin settings for one session: what Options stores for
/// every plugin plus what Host settings stores for this host. Panes read them
/// through their own settings map, so a plugin keeps working with one lookup.
#[derive(Clone, Debug, Default)]
pub struct PluginValues {
    pub app: wassh_core::PluginSettings,
    pub host: wassh_core::PluginSettings,
    /// Decrypted `secret: true` settings, per plugin id and field key.
    ///
    /// Kept out of [`Self::app`] and [`Self::host`] so the plain text never
    /// reaches `settings.json`, `hosts.json` or a plugin's data file.
    pub secrets: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

/// An in-terminal password prompt.
///
/// Like PuTTY, the prompt is printed into the terminal itself and the typed password is
/// never echoed, so the user answers it where the session output appears.
pub struct LoginPrompt {
    /// How to connect once a password is entered, kept for retries.
    open: SshOpen,
    /// Characters typed so far.
    buffer: String,
    /// Attempts made so far; used to print "Permission denied" on retries.
    attempts: u32,
    /// True while waiting for typing, false once the attempt is running.
    active: bool,
}

/// What the user did at a login prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    /// Still typing.
    Waiting,
    /// Enter was pressed; the connection attempt has started.
    Submitted,
    /// Escape or Ctrl+C; the attempt was abandoned.
    Cancelled,
}

/// Transport behind a session.
pub enum Backend {
    /// A shell on the local machine.
    Local(PtySession),
    /// An SSH channel.
    Ssh(SshSession),
    /// A Telnet connection.
    Telnet(wassh_telnet::TelnetSession),
    /// A serial port.
    Serial(wassh_serial::SerialSession),
    /// Nothing to talk to (failed to start, or already gone).
    Closed,
}

/// A host-key decision the user still has to make.
#[derive(Clone, Debug)]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub mismatch: bool,
}

/// A keyboard-interactive challenge.
#[derive(Clone, Debug)]
pub struct AuthPrompt {
    pub name: String,
    pub instructions: String,
    pub fields: Vec<wassh_ssh::AuthPromptField>,
    pub answers: Vec<String>,
}

/// Why a session's transport ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    /// Text to show as the tab's error, if any.
    pub reason: Option<String>,
    /// True when the shell exited by itself (`exit`, `logout`) rather than the
    /// transport failing. A tab whose shell exited is closed; a failed one stays and
    /// reconnects.
    pub from_shell: bool,
}

/// Something the shell should react to after pumping a session.
#[derive(Clone, Debug, Default)]
pub struct Pumped {
    /// The bell rang.
    pub bell: bool,
    /// The transport closed, and why.
    pub closed: Option<Exit>,
    /// The session produced something worth showing: output, a status change, a
    /// prompt or an exit. The window repaints promptly while this is being set
    /// and idles slowly when it is not.
    pub wake: bool,
}

/// Everything needed to open an SSH session.
#[derive(Clone)]
pub struct SshOpen {
    /// Connection parameters, already normalised.
    pub params: ConnectionParams,
    pub resolved: Option<ConnectionParams>,
    /// Saved host this session came from, if any.
    pub host_id: Option<String>,
    /// Credentials typed for this session only.
    pub secrets: Secrets,
    /// Trusted host keys.
    pub known_hosts: Arc<KnownHosts>,
    /// Jump hosts to traverse, outermost first.
    pub jumps: Vec<wassh_ssh::JumpHop>,
    /// Why the jump chain could not be resolved, if it could not.
    pub jump_error: Option<String>,
}

/// A tab's worth of session state.
pub struct SessionEntry {
    /// Stable id, also used as the egui id salt.
    pub id: String,
    /// Title shown in the tab strip.
    pub title: String,
    /// Current lifecycle state.
    pub status: SessionStatus,
    /// Last error message, if any.
    pub error: Option<String>,
    /// The emulator.
    pub terminal: Terminal,
    /// Emulator events (title, bell, clipboard, wakeups).
    pub events: Receiver<TerminalEvent>,
    /// Renderer settings for this session.
    pub view: TerminalView,
    /// Transport.
    pub backend: Backend,
    /// Parameters the session was opened with.
    pub connection: ConnectionParams,
    resolved: Option<ConnectionParams>,
    /// Saved host this tab came from, if any.
    pub host_id: Option<String>,
    /// Pending host-key decision.
    pub host_key_prompt: Option<HostKeyPrompt>,
    /// Pending keyboard-interactive challenge.
    pub auth_prompt: Option<AuthPrompt>,
    /// Whether the process exit has already been reported to the user.
    pub exit_reported: bool,
    /// Credentials used to open this session, kept for automatic reconnects.
    pub secrets: Secrets,
    identity_prompt_password: bool,
    /// Message shown alongside the status, e.g. a tmux fallback notice.
    pub status_message: Option<String>,
    /// How many reconnects have been attempted since the last successful login.
    pub reconnect_attempt: u32,
    /// When the next reconnect should run.
    pub retry_at: Option<std::time::Instant>,
    /// Command to run instead of a login shell (tmux/screen attach).
    command_override: Option<String>,
    /// Jump hosts used by this session, resolved when the tab was opened.
    jumps: Vec<wassh_ssh::JumpHop>,
    /// Why the jump hosts could not be resolved; the session reports it instead of
    /// connecting directly to a host that is only reachable through them.
    jump_error: Option<String>,
    /// A tmux/screen probe still running in the background.
    probe: Option<Receiver<wassh_ssh::Presence>>,
    /// Render-ready snapshot of the visible grid, with the emulator revision it was taken
    /// at, so one frame draws and searches the same copy.
    snapshot: std::cell::RefCell<Option<(u64, ScreenSnapshot)>>,
    /// How to open the connection once the probe finishes.
    pending_open: Option<SshOpen>,
    /// Palette the terminal and plugins draw with.
    pub palette: Palette,
    /// Remote file browsing state for this session.
    files: RemoteFiles,
    /// Status changes this session went through, for the connection log plugin.
    status_history: std::sync::Arc<Vec<wassh_core::ConnectionEvent>>,
    /// Commands other panes asked the session to run.
    commands: RemoteCommands,
    /// Loopback forwards panes asked for (MQTT broker, and any later pane).
    forwards: RemoteForwards,
    /// Pending password prompt shown in the terminal, if any.
    login: Option<LoginPrompt>,
    /// True when the next `Closed` event belongs to a rejected password and must not
    /// end the tab.
    suppress_close: bool,
    /// Which plugin panes this tab shows, and where.
    pub layout: TabPluginLayout,
    /// Live plugin instances, created on demand and kept for the tab's lifetime.
    panes: Vec<PluginPane>,
    /// Toasts the panes raised while they were drawn, waiting for the window to show them.
    plugin_toasts: Vec<(wassh_ui::widgets::ToastKind, String)>,
    /// Host-level plugin values this tab changed in Session settings.
    ///
    /// Kept in the tab: the saved host and its other sessions keep their own.
    plugin_overrides: wassh_core::PluginSettings,
    /// Plugin secrets Session settings typed for this tab, per plugin and field key.
    plugin_secret_overrides: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    >,
}

impl SessionEntry {
    /// Spawn a local shell tab, belonging to the built-in local terminal host.
    pub fn new_local(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        term_type: &str,
        size: TerminalSize,
        params: ConnectionParams,
    ) -> Self {
        let mut entry = Self::prepare(
            id,
            palette,
            defaults,
            size,
            params,
            Some(wassh_core::LOCAL_HOST_ID.to_owned()),
        );
        entry.start_local(term_type);
        entry
    }

    /// Open an SSH tab from saved or ad-hoc parameters.
    pub fn new_ssh(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        term_type: &str,
        size: TerminalSize,
        open: SshOpen,
    ) -> Self {
        let secrets = open.secrets.clone();
        let jumps = open.jumps.clone();
        let mut entry = Self::prepare(id, palette, defaults, size, open.params.clone(), open.host_id.clone());
        entry.secrets = secrets;
        entry.resolved = open.resolved.clone();
        entry.jumps = jumps;
        entry.jump_error = open.jump_error.clone();
        entry.begin(term_type, open);
        entry
    }

    /// Open an SSH tab that asks for its password in the terminal.
    ///
    /// The tab is created immediately so the prompt is visible in the session the user is
    /// about to use, the way PuTTY does.
    pub fn new_ssh_prompt(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        size: TerminalSize,
        open: SshOpen,
    ) -> Self {
        let mut entry = Self::prepare(
            id,
            palette,
            defaults,
            size,
            open.params.clone(),
            open.host_id.clone(),
        );
        entry.jumps = open.jumps.clone();
        entry.resolved = open.resolved.clone();
        entry.prompt_login(open);
        entry
    }

    /// Open a Telnet tab.
    pub fn new_telnet(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        term_type: &str,
        size: TerminalSize,
        params: ConnectionParams,
        host_id: Option<String>,
    ) -> Self {
        let mut entry = Self::prepare(id, palette, defaults, size, params, host_id);
        entry.start_telnet(term_type);
        entry
    }

    /// Open a serial tab.
    pub fn new_serial(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        size: TerminalSize,
        params: ConnectionParams,
        host_id: Option<String>,
    ) -> Self {
        let mut entry = Self::prepare(id, palette, defaults, size, params, host_id);
        entry.start_serial();
        entry
    }

    /// Build the emulator and bookkeeping shared by every transport.
    fn prepare(
        id: impl Into<String>,
        palette: &Palette,
        defaults: &SessionStyleDefaults,
        size: TerminalSize,
        params: ConnectionParams,
        host_id: Option<String>,
    ) -> Self {
        let (events_tx, events) = channel();
        let terminal = Terminal::new(
            size,
            params
                .style
                .scrollback
                .unwrap_or(defaults.scrollback) as usize,
            events_tx,
            palette,
        );
        let title = params.subtitle();
        let mut entry = Self {
            id: id.into(),
            title,
            status: SessionStatus::Connecting,
            error: None,
            terminal,
            events,
            view: TerminalView::with_font_size(f32::from(
                params.style.font_size.unwrap_or(defaults.font_size),
            )),
            backend: Backend::Closed,
            palette: *palette,
            connection: params,
            resolved: None,
            host_id,
            host_key_prompt: None,
            auth_prompt: None,
            exit_reported: false,
            secrets: Secrets::default(),
            identity_prompt_password: false,
            status_message: None,
            reconnect_attempt: 0,
            retry_at: None,
            command_override: None,
            jumps: Vec::new(),
            jump_error: None,
            probe: None,
            pending_open: None,
            files: RemoteFiles::default(),
            status_history: std::sync::Arc::new(Vec::new()),
            commands: RemoteCommands::default(),
            forwards: RemoteForwards::default(),
            login: None,
            suppress_close: false,
            layout: TabPluginLayout::empty(),
            panes: Vec::new(),
            plugin_toasts: Vec::new(),
            plugin_overrides: wassh_core::PluginSettings::new(),
            plugin_secret_overrides: std::collections::BTreeMap::new(),
            snapshot: std::cell::RefCell::new(None),
        };
        entry.apply_style(defaults);
        entry
    }

    /// Whether a title the shell reported should rename the tab.
    ///
    /// A Windows shell's console title is its executable path, so those tabs keep the name
    /// the connection gave them. A WSL shell reports the user and machine it runs on, which
    /// is worth keeping.
    fn accepts_title(&self, title: &str) -> bool {
        if title.trim().is_empty() {
            return false;
        }
        match self.connection.kind {
            ConnectionType::Local => matches!(self.connection.local, LocalShell::Wsl { .. }),
            _ => true,
        }
    }

    /// Name a title the shell reported, adding the environment a WSL tab runs in.
    fn titled(connection: &ConnectionParams, title: String) -> String {
        match &connection.local {
            LocalShell::Wsl { distro: Some(name) }
                if connection.kind == ConnectionType::Local =>
            {
                format!("{title} ({name})")
            }
            _ => title,
        }
    }

    fn start_telnet(&mut self, term_type: &str) {
        let params = &self.connection;
        let mut options = wassh_telnet::TelnetOptions::new(params.host.clone(), params.port);
        options.term = term_type.to_owned();
        options.cols = self.terminal.size().cols;
        options.rows = self.terminal.size().rows;
        self.status = SessionStatus::Connecting;
        match wassh_telnet::TelnetSession::connect(options) {
            Ok(session) => self.backend = Backend::Telnet(session),
            Err(error) => {
                self.backend = Backend::Closed;
                self.status = SessionStatus::Failed;
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    fn start_serial(&mut self) {
        let config = self.connection.serial.clone();
        self.status = SessionStatus::Connecting;
        match wassh_serial::SerialSession::open(&config) {
            Ok(session) => {
                self.status_message = Some(session.description());
                self.backend = Backend::Serial(session);
                self.status = SessionStatus::Connected;
            }
            Err(error) => {
                self.backend = Backend::Closed;
                self.status = SessionStatus::Failed;
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    /// Start a connection, probing for tmux/screen first when the host asks for it.
    fn begin(&mut self, term_type: &str, open: SshOpen) {
        self.command_override = None;
        if let Some(error) = self.jump_error.clone() {
            self.backend = Backend::Closed;
            self.status = SessionStatus::Failed;
            self.error = Some(error);
            return;
        }
        if self.connection.remote_session.enabled {
            let kind = self.connection.remote_session.multiplexer;
            let name = self.connection.remote_session.name.clone();
            let mut probe_options = self.options_for(term_type, &open.secrets, open.known_hosts.clone());
            probe_options.command = None;
            let command = wassh_ssh::remote_session::probe_command(kind);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("wassh-remote-probe".to_owned())
                .spawn(move || {
                    let presence = match wassh_ssh::capture_command(
                        probe_options,
                        command,
                        std::time::Duration::from_secs(15),
                    ) {
                        Ok(output) => wassh_ssh::remote_session::classify(
                            kind,
                            &name,
                            output.exit_status,
                            &output.stdout,
                            &output.stderr,
                        ),
                        Err(_) => wassh_ssh::Presence::Unknown,
                    };
                    let _ = tx.send(presence);
                    wassh_core::wake::notify();
                })
                .ok();
            self.status = SessionStatus::Connecting;
            self.status_message = Some(format!(
                "Checking {} on the remote host…",
                wassh_ssh::remote_session::multiplexer_label(kind)
            ));
            self.probe = Some(rx);
            self.pending_open = Some(open);
            return;
        }
        self.start_ssh(term_type, &open.secrets, open.known_hosts);
    }

    /// Resolve a finished tmux/screen probe and open the transport.
    fn finish_probe(&mut self, term_type: &str, presence: wassh_ssh::Presence) -> bool {
        let Some(open) = self.pending_open.take() else {
            return false;
        };
        let remote = self.connection.remote_session.clone();
        let plan = wassh_ssh::plan_remote_session(
            remote.multiplexer,
            &remote.name,
            remote.busy,
            presence,
        );
        self.status_message = plan.message;
        self.command_override = plan.command;
        self.start_ssh(term_type, &open.secrets, open.known_hosts);
        true
    }

    /// Restart the transport of a failed or closed session.
    pub fn restart(
        &mut self,
        term_type: &str,
        secrets: &Secrets,
        known_hosts: Arc<KnownHosts>,
    ) {
        self.error = None;
        self.host_key_prompt = None;
        self.auth_prompt = None;
        self.status_message = None;
        self.command_override = None;
        self.probe = None;
        self.pending_open = None;
        self.exit_reported = false;
        self.login = None;
        self.suppress_close = false;
        if !secrets.password.is_empty() {
            self.secrets = secrets.clone();
        }
        match self.connection.kind {
            ConnectionType::Local => self.start_local(term_type),
            ConnectionType::Telnet => self.start_telnet(term_type),
            ConnectionType::Serial => self.start_serial(),
            ConnectionType::Ssh => {
                self.connection = self.connection.clone().normalized();
                let open = SshOpen {
                    params: self.connection.clone(),
                    resolved: self.resolved.clone(),
                    host_id: self.host_id.clone(),
                    secrets: self.secrets.clone(),
                    known_hosts,
                    jumps: self.jumps.clone(),
                    jump_error: self.jump_error.clone(),
                };
                if self.jump_error.is_none()
                    && self.resolved.as_ref().unwrap_or(&self.connection).auth == AuthMethod::Password
                    && self.secrets.password.is_empty()
                {
                    // Nothing to reuse: ask again in the terminal.
                    self.prompt_login(open);
                } else {
                    self.begin(term_type, open);
                }
            }
        }
    }

    fn start_local(&mut self, term_type: &str) {
        // The command prompt and WSL shells exist on Windows only; a configuration
        // carried over from Windows keeps the value but cannot launch it here.
        if !cfg!(windows) && !matches!(self.connection.local, LocalShell::System) {
            self.backend = Backend::Closed;
            self.status = SessionStatus::Failed;
            self.error = Some("This local shell is only available on Windows.".to_owned());
            return;
        }
        let (shell, args) = match &self.connection.local {
            LocalShell::System => PtySession::default_shell(),
            LocalShell::Cmd => wassh_term::pty::cmd_shell(),
            LocalShell::Wsl { distro } => wassh_term::wsl::launch(distro.as_deref()),
        };
        match PtySession::spawn(&shell, &args, self.terminal.size(), term_type) {
            Ok(pty) => {
                self.backend = Backend::Local(pty);
                self.status = SessionStatus::Connected;
            }
            Err(error) => {
                self.backend = Backend::Closed;
                self.status = SessionStatus::Failed;
                self.error = Some(format!("failed to start {shell}: {error}"));
            }
        }
    }

    fn start_ssh(&mut self, term_type: &str, secrets: &Secrets, known_hosts: Arc<KnownHosts>) {
        let mut options = self.options_for(term_type, secrets, known_hosts);
        options.command = self.command_override.clone();

        self.status = SessionStatus::Connecting;
        match SshSession::connect(options) {
            Ok(session) => self.backend = Backend::Ssh(session),
            Err(error) => {
                self.backend = Backend::Closed;
                self.status = SessionStatus::Failed;
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    /// Build the transport options for this session's parameters.
    fn options_for(
        &self,
        term_type: &str,
        secrets: &Secrets,
        known_hosts: Arc<KnownHosts>,
    ) -> SshOptions {
        let params = self.resolved.as_ref().unwrap_or(&self.connection);
        let auth = match params.auth {
            AuthMethod::Password => SshAuth::Password(secrets.password.clone()),
            AuthMethod::PrivateKey => SshAuth::PrivateKey {
                path: params
                    .private_key_path
                    .clone()
                    .unwrap_or_default()
                    .into(),
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
        options.term = term_type.to_owned();
        options.cols = self.terminal.size().cols;
        options.rows = self.terminal.size().rows;
        options.x11 = params.x11.then(wassh_ssh::x11::local_config).flatten();
        options.tunnels = params
            .tunnels
            .iter()
            .filter(|tunnel| tunnel.enabled)
            .filter_map(wassh_ssh::TunnelRequest::from_saved)
            .collect();
        options.jumps = self.jumps.clone();
        options.host_key_policy = wassh_ssh::HostKeyPolicy::Ask(known_hosts);
        options
    }

    /// Feed pending transport output into the emulator and collect shell-level events.
    pub fn pump(&mut self, term_type: &str) -> Pumped {
        let mut pumped = Pumped::default();

        // A tmux/screen probe still running holds off the actual connection.
        if self.probe.is_some() {
            let outcome = self.probe.as_ref().map(|receiver| match receiver.try_recv() {
                Ok(presence) => Some(presence),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(wassh_ssh::Presence::Unknown)
                }
            });
            match outcome {
                Some(Some(presence)) => {
                    self.probe = None;
                    self.finish_probe(term_type, presence);
                }
                Some(None) => return pumped,
                None => return pumped,
            }
        }

        let mut local_exited = false;

        match &mut self.backend {
            Backend::Local(pty) => {
                for chunk in pty.read_pending() {
                    if !chunk.is_empty() {
                        pumped.wake = true;
                        self.terminal.feed(&chunk);
                    }
                }
                local_exited = pty.has_exited();
            }
            Backend::Ssh(session) => {
                for event in session.events() {
                    match event {
                        SshEvent::Data(bytes) => {
                            if !bytes.is_empty() {
                                pumped.wake = true;
                                self.terminal.feed(&bytes);
                            }
                        }
                        SshEvent::Status { status, .. } => {
                            pumped.wake |= self.status != status;
                            self.status = status;
                        }
                        SshEvent::HostKeyPrompt {
                            host,
                            port,
                            fingerprint,
                            mismatch,
                        } => {
                            pumped.wake = true;
                            self.host_key_prompt = Some(HostKeyPrompt {
                                host,
                                port,
                                fingerprint,
                                mismatch,
                            });
                        }
                        SshEvent::AuthPrompt {
                            name,
                            instructions,
                            prompts,
                        } => {
                            pumped.wake = true;
                            self.auth_prompt = Some(AuthPrompt {
                                name,
                                instructions,
                                answers: vec![String::new(); prompts.len()],
                                fields: prompts,
                            });
                        }
                        SshEvent::AuthFailed { detail } => {
                            pumped.wake = true;
                            // A rejected password re-prompts instead of ending the tab.
                            if !self.retry_login() {
                                self.error = Some(detail);
                                self.status = SessionStatus::Failed;
                                pumped.closed = Some(Exit {
                                    reason: None,
                                    from_shell: false,
                                });
                            }
                        }
                        SshEvent::Closed { reason, end } => {
                            pumped.wake = true;
                            if self.suppress_close {
                                self.suppress_close = false;
                            } else {
                                // A failed transport keeps its status, so the shell's
                                // reconnect policy applies; an `exit` closes the tab.
                                if end.by_shell() {
                                    self.status = SessionStatus::Closed;
                                } else if self.status == SessionStatus::Connected {
                                    self.status = SessionStatus::Disconnected;
                                }
                                self.host_key_prompt = None;
                                self.auth_prompt = None;
                                pumped.closed = Some(Exit {
                                    reason,
                                    from_shell: end.by_shell(),
                                });
                            }
                        }
                        SshEvent::Error(error) => {
                            pumped.wake = true;
                            // A transport failure ends the prompt flow for good.
                            self.login = None;
                            self.suppress_close = false;
                            self.error = Some(error);
                            self.status = SessionStatus::Failed;
                            pumped.closed = Some(Exit {
                                reason: None,
                                from_shell: false,
                            });
                        }
                    }
                }
            }
            Backend::Telnet(session) => {
                for event in session.events() {
                    match event {
                        wassh_telnet::TelnetEvent::Data(bytes) => {
                            if !bytes.is_empty() {
                                pumped.wake = true;
                                self.terminal.feed(&bytes);
                            }
                        }
                        wassh_telnet::TelnetEvent::Status(status) => {
                            pumped.wake |= self.status != status;
                            self.status = status;
                        }
                        wassh_telnet::TelnetEvent::Closed { reason, by_remote } => {
                            pumped.wake = true;
                            // A server that hangs up ended the session; a failure keeps its
                            // status, so the reconnect policy takes over.
                            if by_remote {
                                self.status = SessionStatus::Closed;
                            } else if self.status == SessionStatus::Connected {
                                self.status = SessionStatus::Disconnected;
                            }
                            pumped.closed = Some(Exit {
                                reason,
                                from_shell: by_remote,
                            });
                        }
                        wassh_telnet::TelnetEvent::Error(error) => {
                            pumped.wake = true;
                            self.error = Some(error);
                            self.status = SessionStatus::Failed;
                            pumped.closed = Some(Exit {
                                reason: None,
                                from_shell: false,
                            });
                        }
                    }
                }
            }
            Backend::Serial(session) => {
                for chunk in session.read_pending() {
                    if !chunk.is_empty() {
                        pumped.wake = true;
                        self.terminal.feed(&chunk);
                    }
                }
            }
            Backend::Closed => {}
        }

        // A local shell that exits was told to: no reconnect, the tab goes away.
        if local_exited && self.status == SessionStatus::Connected {
            pumped.wake = true;
            self.status = SessionStatus::Closed;
            pumped.closed = Some(Exit {
                reason: None,
                from_shell: true,
            });
        }

        if self.status == SessionStatus::Connected {
            self.reconnect_attempt = 0;
            self.retry_at = None;
            self.login = None;
            self.suppress_close = false;
            self.error = None;
        }

        let mut replies = Vec::new();
        for event in self.events.try_iter() {
            match event {
                TerminalEvent::Title(title) => {
                    pumped.wake = true;
                    if self.accepts_title(&title) {
                        self.title = Self::titled(&self.connection, title);
                    }
                }
                TerminalEvent::Bell => {
                    pumped.wake = true;
                    pumped.bell = true;
                }
                // The shell is waiting for this answer, e.g. ConPTY blocks on `CSI 6 n`.
                TerminalEvent::PtyWrite(text) => {
                    pumped.wake = true;
                    replies.push(text);
                }
                // The grid changed under us (a redraw request from the application).
                TerminalEvent::Wakeup => pumped.wake = true,
            }
        }
        for reply in replies {
            self.send(reply.as_bytes());
        }

        self.record_status();
        pumped
    }

    /// Note a status change so the connection log plugin can report it.
    ///
    /// Runs once per frame, so a transition that happens and reverts inside one frame is not
    /// logged — which is what a user wants from a "when was this host up?" log.
    fn record_status(&mut self) {
        if !self.status.is_loggable() {
            return;
        }
        if self
            .status_history
            .last()
            .is_some_and(|event| event.status == self.status)
        {
            return;
        }
        let message = match self.status {
            SessionStatus::Failed | SessionStatus::Disconnected => self.error.clone(),
            _ => None,
        };
        let history = std::sync::Arc::make_mut(&mut self.status_history);
        history.push(wassh_core::ConnectionEvent {
            at_ms: wassh_core::time::now_ms(),
            status: self.status,
            message,
        });
        if history.len() > MAX_STATUS_HISTORY {
            let excess = history.len() - MAX_STATUS_HISTORY;
            history.drain(..excess);
        }
    }

    /// Send encoded input to the transport.
    pub fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        match &mut self.backend {
            Backend::Local(pty) => {
                let _ = pty.write(bytes);
            }
            Backend::Ssh(session) => session.write(bytes),
            Backend::Telnet(session) => session.write(bytes),
            Backend::Serial(session) => {
                let _ = session.write(bytes);
            }
            Backend::Closed => {}
        }
    }

    /// Encode and send a paste, honouring bracketed paste mode.
    pub fn send_paste(&mut self, text: &str) {
        if self.terminal.modes().bracketed_paste {
            let mut wrapped = Vec::with_capacity(text.len() + 12);
            wrapped.extend_from_slice(b"\x1b[200~");
            wrapped.extend_from_slice(text.as_bytes());
            wrapped.extend_from_slice(b"\x1b[201~");
            self.send(&wrapped);
        } else {
            self.send(text.as_bytes());
        }
    }

    /// Send a key stroke using the current terminal modes.
    pub fn send_key(&mut self, stroke: &wassh_term::KeyStroke) {
        let bytes = encode(stroke, self.terminal.modes());
        self.send(&bytes);
    }

    /// Scroll the scrollback when the wheel turns over the terminal.
    ///
    /// The delta is taken out of the frame's input, so a pane's own scroll area laid out
    /// beside the terminal never sees the same turn: whoever handles the wheel consumes it.
    pub fn scroll_from_wheel(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if !response.contains_pointer() {
            return;
        }
        let delta = ui
            .ctx()
            .input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
        let (_, cell_h) = self.view.cell_size(ui);
        let lines = (delta / cell_h).round() as i32;
        if lines != 0 {
            self.terminal.scroll(lines);
        }
    }

    /// Turn a click or drag across the grid into a text selection.
    pub fn track_selection(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        size: TerminalSize,
    ) {
        let cell = response
            .interact_pointer_pos()
            .and_then(|pos| self.view.cell_at(ui, response.rect, size, pos));
        if response.drag_started() {
            if let Some((row, col)) = cell {
                self.terminal.begin_selection(row, col);
            }
        } else if response.dragged() {
            if let Some((row, col)) = cell {
                self.terminal.extend_selection(row, col);
            }
        } else if response.clicked() {
            // A plain click drops the old selection, like every other terminal.
            self.terminal.clear_selection();
        }
    }

    /// Resize the emulator and the transport.
    pub fn resize(&mut self, size: TerminalSize) {
        if size == self.terminal.size() {
            return;
        }
        self.terminal.resize(size);
        match &mut self.backend {
            Backend::Local(pty) => {
                let _ = pty.resize(size);
            }
            Backend::Ssh(session) => session.resize(size.cols, size.rows),
            Backend::Telnet(session) => session.resize(size.cols, size.rows),
            Backend::Serial(_) => {}
            Backend::Closed => {}
        }
    }

    /// Answer a host-key prompt.
    pub fn respond_host_key(&mut self, accept: bool) {
        if let Backend::Ssh(session) = &self.backend {
            session.respond_host_key(accept);
        }
        self.host_key_prompt = None;
        if !accept {
            self.status = SessionStatus::Disconnected;
        }
    }

    /// Answer a keyboard-interactive challenge.
    pub fn respond_auth(&mut self, answers: Vec<String>) {
        if let Backend::Ssh(session) = &self.backend {
            session.respond_auth(answers);
        }
        self.auth_prompt = None;
    }

    /// Close the transport but keep the tab (and its scrollback) around.
    pub fn disconnect(&mut self) {
        match &mut self.backend {
            Backend::Local(pty) => pty.kill(),
            Backend::Ssh(session) => session.disconnect(),
            Backend::Telnet(session) => session.disconnect(),
            Backend::Serial(_) => {}
            Backend::Closed => {}
        }
        self.backend = Backend::Closed;
        self.host_key_prompt = None;
        self.auth_prompt = None;
        self.login = None;
        self.suppress_close = false;
        if !matches!(
            self.status,
            SessionStatus::Failed | SessionStatus::Disconnected
        ) {
            self.status = SessionStatus::Closed;
        }
    }

    /// True while the session can accept keyboard input.
    pub fn is_live(&self) -> bool {
        self.status == SessionStatus::Connected && !matches!(self.backend, Backend::Closed)
    }

    /// Adopt an edited host's connection settings.
    ///
    /// The tab keeps its emulator and scrollback. Style the renderer can honour right away
    /// updates immediately; everything else is used the next time this session connects.
    pub fn apply_connection(
        &mut self,
        connection: ConnectionParams,
        defaults: &SessionStyleDefaults,
    ) {
        if self.connection.identity_id != connection.identity_id {
            self.secrets = Secrets::default();
            self.identity_prompt_password = false;
        }
        self.connection = connection;
        self.apply_style(defaults);
    }

    pub fn set_resolved_auth(&mut self, resolved: Option<ConnectionParams>, mut secrets: Secrets, jumps: Vec<wassh_ssh::JumpHop>, error: Option<String>) {
        if self.connection.identity_id.is_some() && self.identity_prompt_password && secrets.password.is_empty() {
            secrets.password.clone_from(&self.secrets.password);
        } else {
            self.identity_prompt_password = false;
        }
        self.resolved = resolved;
        self.secrets = secrets;
        self.jumps = jumps;
        self.jump_error = error;
    }

    /// Recompute what the renderer shows from this session's overrides plus the app defaults.
    ///
    /// Font size and the terminal colours apply immediately; scrollback and font family are
    /// baked into the emulator and the font atlas at construction.
    pub fn apply_style(&mut self, defaults: &SessionStyleDefaults) {
        let style = &self.connection.style;
        self.view.font_size = f32::from(style.font_size.unwrap_or(defaults.font_size));

        let mut palette = self.palette;
        if let Some(colour) = style.term_background.or(defaults.term_background) {
            palette.bg_term = colour;
        }
        if let Some(colour) = style.term_foreground.or(defaults.term_foreground) {
            palette.term_fg = colour;
        }
        self.terminal.set_palette(&palette);
    }

    /// Per-session accent colour for tabs and host rows.
    pub fn accent(&self) -> Option<wassh_core::Rgba> {
        self.connection.style.tab_color
    }

    /// Render-ready snapshot of the visible screen.
    ///
    /// The emulator only copies the grid when something changed, so the frame's draw and
    /// the find bar share one snapshot instead of building theirs separately.
    pub fn snapshot(&self) -> std::cell::Ref<'_, ScreenSnapshot> {
        let revision = self.terminal.revision();
        let stale = self
            .snapshot
            .borrow()
            .as_ref()
            .is_none_or(|(cached, _)| *cached != revision);
        if stale {
            let fresh = self.terminal.snapshot();
            *self.snapshot.borrow_mut() = Some((revision, fresh));
        }
        std::cell::Ref::map(self.snapshot.borrow(), |slot| {
            &slot.as_ref().expect("the snapshot was just filled").1
        })
    }

    /// Revision of the visible grid; it changes whenever the snapshot would.
    pub fn revision(&self) -> u64 {
        self.terminal.revision()
    }

    /// Plain text of the visible screen, used by the find bar.
    pub fn visible_text(&self) -> String {
        let snapshot = self.snapshot();
        let capacity = usize::from(snapshot.size.rows) * (usize::from(snapshot.size.cols) + 1);
        let mut text = String::with_capacity(capacity);
        for line in grid_lines(&snapshot) {
            text.push_str(&line);
            text.push('\n');
        }
        text
    }
}

impl SessionEntry {
    /// Start `request` on whichever filesystem backs this session.
    ///
    /// An SSH tab talks to its SFTP subsystem; a Local tab reads and writes this
    /// machine. `None` means the transport (telnet, serial, closed) has no files.
    fn file_job(&self, request: wassh_ssh::SftpRequest) -> Option<wassh_ssh::SftpJob> {
        match &self.backend {
            Backend::Ssh(session) => Some(session.sftp(request)),
            // A WSL tab's files are its distribution's, published on a share this machine
            // can read like any other directory.
            Backend::Local(_) => Some(match &self.connection.local {
                LocalShell::Wsl { distro } => wsl_files::spawn(distro.as_deref(), request),
                _ => local_files::spawn(request),
            }),
            _ => None,
        }
    }

    /// Whether this session has a filesystem to browse.
    fn can_browse_files(&self) -> bool {
        matches!(self.backend, Backend::Ssh(_) | Backend::Local(_))
    }

    /// Run `command` on the host behind this tab.
    ///
    /// An SSH tab runs it on its host; a Local tab's host is this machine, so the
    /// command goes through the local shell, like the reference's
    /// `runHostCommand`.
    fn start_command(&mut self, key: &str, command: &str, input: Option<&str>) {
        self.commands.poll();
        // A new request replaces whatever the pane read last.
        self.commands.results.remove(key);
        let (pending, stop) = match &self.backend {
            Backend::Ssh(session) => {
                let job = match input {
                    Some(input) => {
                        session.exec_with_input(command.to_owned(), Some(format!("{input}\n")))
                    }
                    None => session.exec(command.to_owned()),
                };
                (job.output, CommandStop::Ssh(job.stop))
            }
            _ if self.connection.kind == ConnectionType::Local => {
                let stdin = input.map(|input| format!("{input}\n"));
                let job = match &self.connection.local {
                    // A WSL tab's host is its distribution, which has its own docker, its
                    // own /proc and its own package manager.
                    LocalShell::Wsl { distro } => {
                        local_exec::exec_in_wsl(distro.as_deref(), command, stdin.as_deref())
                    }
                    _ => local_exec::exec(command, stdin.as_deref()),
                };
                (job.output, CommandStop::Local(job.stop))
            }
            _ => {
                self.commands.results.insert(
                    key.to_owned(),
                    CommandResult {
                        output: None,
                        failure: Some(COMMAND_TRANSPORT_ERROR.to_owned()),
                    },
                );
                return;
            }
        };
        self.commands.pending.push((key.to_owned(), pending));
        self.commands.stops.push((key.to_owned(), stop));
    }

    /// Take the slot for a started job, so the next frames collect its answer.
    fn adopt_job(&mut self, job: wassh_ssh::SftpJob) {
        self.files.pending = Some(job.outcome);
        self.files.progress = Some(job.progress);
        self.files.stop = Some(job.stop);
        self.files.moved = None;
        self.files.stop_requested = false;
    }

    /// Queue a transfer and start it when the current one finishes.
    fn enqueue_transfer(
        &mut self,
        request: wassh_ssh::SftpRequest,
        direction: TransferDirection,
        path: String,
    ) {
        self.enqueue_transfer_with_done(request, direction, path.clone(), path);
    }

    fn enqueue_transfer_with_done(
        &mut self,
        request: wassh_ssh::SftpRequest,
        direction: TransferDirection,
        path: String,
        done: String,
    ) {
        if !self.can_browse_files() {
            self.files.error = Some(FILES_TRANSPORT_ERROR.to_owned());
            return;
        }
        self.files.queue.push_back(QueuedTransfer {
            request,
            direction,
            path,
            done,
        });
        self.files.status = None;
        // The file this transfer writes is about to change, so stop answering reads of it
        // from the snapshot taken before.
        self.files.forget_content();
        self.start_next_transfer();
    }

    /// Start the next queued transfer, if nothing is running.
    fn start_next_transfer(&mut self) {
        // Packing an archive holds the slot too: one request is collected per frame.
        if self.files.pending.is_some() || self.files.pack.is_some() {
            return;
        }
        let Some(next) = self.files.queue.pop_front() else {
            return;
        };
        let Some(job) = self.file_job(next.request.clone()) else {
            return;
        };
        self.adopt_job(job);
        self.files.request = Some(next.request);
        self.files.active = Some((next.direction, next.path.clone()));
        self.files.job = Some(FileJob::Transfer { done: next.done });
        self.files.error = None;
    }

    /// Fetch a tree and pack it, instead of reloading the listing afterwards.
    fn start_zip(
        &mut self,
        request: wassh_ssh::SftpRequest,
        dir: std::path::PathBuf,
        archive: std::path::PathBuf,
    ) {
        let Some(job) = self.file_job(request) else {
            self.files.error = Some(FILES_TRANSPORT_ERROR.to_owned());
            return;
        };
        self.adopt_job(job);
        // A ZIP is a transfer like any other, so the pane shows it while it runs.
        self.files.active = Some((TransferDirection::Zip, archive.display().to_string()));
        self.files.job = Some(FileJob::Zip { dir, archive });
        self.files.error = None;
        self.files.status = None;
    }

    /// Run a change on the browsed filesystem, reloading the listing after it.
    fn start_file_action(&mut self, request: wassh_ssh::SftpRequest, done: String) {
        let Some(job) = self.file_job(request) else {
            self.files.error = Some(FILES_TRANSPORT_ERROR.to_owned());
            return;
        };
        self.adopt_job(job);
        self.files.job = Some(FileJob::Action { done });
        self.files.forget_content();
    }

    fn start_file_request(&mut self, path: &str) {
        if let Some(stop) = self.files.listing_stop.take() {
            stop.stop();
        }
        let Some(job) = self.file_job(wassh_ssh::SftpRequest::List {
            path: path.to_owned(),
        }) else {
            self.files.error = Some(FILES_TRANSPORT_ERROR.to_owned());
            return;
        };
        self.files.listing_pending = Some((path.to_owned(), job.outcome));
        self.files.listing_stop = Some(job.stop);
        self.files.requested = Some(path.to_owned());
        self.files.listing_error = None;
    }

    /// Whether a file or command request is still running.
    pub fn requests_pending(&self) -> bool {
        self.files.listing_pending.is_some()
            || self.files.pending.is_some()
            || self.files.pack.is_some()
            || !self.commands.pending.is_empty()
    }

    /// Scope this tab's plugin data is filed under: the saved host, or the tab itself.
    fn plugin_scope(&self) -> String {
        self.host_id
            .clone()
            .unwrap_or_else(|| format!("tab:{}", self.id))
    }

    /// Ensure a plugin instance exists for `id` and return its index.
    fn ensure_pane(&mut self, id: PluginId) -> usize {
        if let Some(slot) = self.panes.iter().position(|pane| pane.id == id) {
            return slot;
        }
        self.panes.push(PluginPane {
            id,
            plugin: wassh_plugins::create(id),
            data: PluginData::default(),
            secret_writes: Vec::new(),
            app_writes: Vec::new(),
        });
        self.panes.len() - 1
    }

    /// Load a pane's stored data the first time it is drawn.
    fn load_pane_data(
        &mut self,
        slot: usize,
        store: Option<&wassh_store::Config>,
        values: &PluginValues,
    ) {
        let scope = self.plugin_scope();
        let Some(pane) = self.panes.get_mut(slot) else {
            return;
        };
        if pane.data.loaded {
            return;
        }
        let stored = store
            .and_then(|store| match store.plugin_data(pane.id.as_str(), Some(&scope)) {
                Ok(Some(data)) => Some(data),
                Ok(None) if pane.id == PluginId::Scratchpad => {
                    let data = store.plugin_data(pane.id.as_str(), None).ok().flatten()?;
                    let text = data.get(wassh_plugins::plugin::SCRATCHPAD_TEXT_KEY)?.as_str()?;
                    if text.is_empty() || store.save_plugin_data(pane.id.as_str(), Some(&scope), &data).is_err() {
                        return None;
                    }
                    let _ = store.save_plugin_data(pane.id.as_str(), None, &serde_json::Value::Null);
                    Some(data)
                }
                _ => None,
            })
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let id = pane.id;
        pane.data.load(stored, id, values);
    }

    /// Show `id` in this tab, docking it where its manifest asks.
    pub fn open_plugin(&mut self, id: PluginId) {
        self.ensure_pane(id);
        let manifest = id.manifest();
        match manifest.placement {
            DockPlacement::SplitLeft => {
                self.layout.dock_on_edge(id.as_str(), DockEdge::Left, None);
            }
            DockPlacement::SplitRight => {
                self.layout.dock_on_edge(id.as_str(), DockEdge::Right, None);
            }
            DockPlacement::SplitTop => {
                self.layout.dock_on_edge(id.as_str(), DockEdge::Top, None);
            }
            DockPlacement::SplitBottom => {
                self.layout.dock_on_edge(id.as_str(), DockEdge::Bottom, None);
            }
            DockPlacement::Overlay => self.layout.move_to_overlay(id.as_str()),
        }
    }

    /// Close the pane showing `plugin_id`.
    pub fn close_plugin(&mut self, plugin_id: &str) {
        self.layout.remove_plugin(plugin_id);
    }

    /// Secrets the panes sealed or cleared since the last call.
    ///
    /// Returned as `(plugin id, field key, value)`; `None` means the plugin
    /// asked for the stored value to be forgotten.
    pub fn take_dirty_plugin_secrets(&mut self) -> Vec<(PluginId, String, Option<String>)> {
        let mut writes = Vec::new();
        for pane in &mut self.panes {
            for (field, value) in std::mem::take(&mut pane.secret_writes) {
                writes.push((pane.id, field, value));
            }
        }
        writes
    }

    /// App-wide plugin settings the panes changed since the last call.
    pub fn take_dirty_app_settings(&mut self) -> Vec<(PluginId, String, serde_json::Value)> {
        let mut writes = Vec::new();
        for pane in &mut self.panes {
            for (key, value) in std::mem::take(&mut pane.app_writes) {
                writes.push((pane.id, key, value));
            }
        }
        writes
    }

    /// Title for a pane header.
    pub fn plugin_title(&self, plugin_id: &str) -> String {
        PluginId::parse(plugin_id)
            .map(|id| id.manifest().view_title.to_owned())
            .unwrap_or_else(|| plugin_id.to_owned())
    }

    /// State line a plugin pane wants in its header, if any.
    pub fn plugin_status(&self, plugin_id: &str) -> Option<(&str, HeaderHealth)> {
        let id = PluginId::parse(plugin_id)?;
        self.panes
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.plugin.header_status())
    }

    /// Button a plugin pane wants in its header, if any.
    pub fn plugin_header_action(&self, plugin_id: &str) -> Option<wassh_plugins::HeaderButton> {
        let id = PluginId::parse(plugin_id)?;
        self.panes
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.plugin.header_action())
    }

    /// The user clicked a pane's header button.
    pub fn click_plugin_header_action(&mut self, plugin_id: &str) {
        let Some(id) = PluginId::parse(plugin_id) else {
            return;
        };
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
            pane.plugin.header_action_clicked();
        }
    }

    /// Draw the contents of a plugin pane.
    pub fn show_plugin(
        &mut self,
        ui: &mut egui::Ui,
        plugin_id: &str,
        store: Option<&wassh_store::Config>,
        values: &PluginValues,
    ) {
        let Some(id) = PluginId::parse(plugin_id) else {
            return;
        };
        let slot = self.ensure_pane(id);
        self.load_pane_data(slot, store, values);
        let scope = self.plugin_scope();
        let palette = self.palette;
        let host_label = self.title.clone();
        let environment = self.connection.environment();
        let status = self.status;
        // The pane is lent out so the session itself can serve remote file requests while
        // the plugin draws.
        if slot >= self.panes.len() {
            return;
        }
        let mut pane = self.panes.swap_remove(slot);
        let events = std::sync::Arc::clone(&self.status_history);
        let secrets = values.secrets.get(id.as_str()).unwrap_or(&NO_SECRETS);
        let typed;
        let repaint_after;
        {
            let mut context = PluginContext {
                palette,
                host_label,
                environment,
                status,
                settings: &mut pane.data.values,
                scope,
                session: Some(self),
                events: &events,
                pending_input: Vec::new(),
                repaint_after: None,
                secrets,
                secret_writes: Vec::new(),
            };
            pane.plugin.show(ui, &mut context);
            typed = std::mem::take(&mut context.pending_input);
            repaint_after = context.repaint_after;
            let writes = std::mem::take(&mut context.secret_writes);
            if !writes.is_empty() {
                pane.secret_writes.extend(writes);
            }
        }
        if let Some(after) = repaint_after {
            ui.ctx().request_repaint_after(after);
        }
        let app_writes = pane.plugin.take_app_settings();
        if !app_writes.is_empty() {
            pane.app_writes.extend(app_writes);
        }
        let toasts = pane.plugin.take_toasts();
        self.plugin_toasts.extend(toasts);
        self.panes.push(pane);
        // A macro or plugin action types into the shell as if the user had.
        if !typed.is_empty() {
            self.send(&typed);
        }
    }

    /// Take the toasts the panes raised while they were drawn.
    pub fn take_plugin_toasts(&mut self) -> Vec<(wassh_ui::widgets::ToastKind, String)> {
        std::mem::take(&mut self.plugin_toasts)
    }

    /// Collect finished pane commands.
    pub fn poll_commands(&mut self) {
        self.commands.poll();
        self.forwards.poll();
    }

    /// Handle one file-request cycle.
    pub fn poll_files(&mut self) {
        self.files.poll_listing();
        match self.files.poll() {
            Some(path) => {
                // A transfer landed: let the next queued one start, then show
                // the directory as it is now.
                self.start_next_transfer();
                let already_listing = self
                    .files
                    .listing_pending
                    .as_ref()
                    .is_some_and(|(requested, _)| requested == &path);
                if self.files.pending.is_none() && !already_listing {
                    self.start_file_request(&path);
                } else {
                    self.files.requested = Some(path);
                }
            }
            // Nothing is running, so a queued transfer may start now.
            None => self.start_next_transfer(),
        }
    }

    /// Take an app-wide value a pane refreshed into the pane's own copy.
    ///
    /// The pane reads its settings from a copy loaded once, so a value it just
    /// refreshed in Options would otherwise be overwritten on the next frame.
    /// It counts as supplied by the dialogs, so it stays out of the pane's file.
    pub fn adopt_app_setting(&mut self, id: PluginId, key: &str, value: serde_json::Value) {
        for pane in self.panes.iter_mut().filter(|pane| pane.id == id) {
            pane.data.adopt(key, value.clone());
        }
    }

    /// Panes whose data changed since the last save, ready to be written.
    pub fn take_dirty_plugins(
        &mut self,
    ) -> Vec<(PluginId, String, serde_json::Map<String, serde_json::Value>)> {
        let scope = self.plugin_scope();
        let mut dirty = Vec::new();
        for pane in &mut self.panes {
            if let Some(payload) = pane.data.take_changes() {
                dirty.push((pane.id, scope.clone(), payload));
            }
        }
        dirty
    }

    /// Drop the cached plugin data so every pane reloads with the current
    /// settings values on its next draw.
    pub fn reload_pane_settings(&mut self) {
        for pane in &mut self.panes {
            pane.data.loaded = false;
        }
    }

    /// Lay this tab's plugin values over the host's, for Session settings to show.
    pub fn with_plugin_overrides(&self, values: &mut wassh_core::PluginSettings) {
        for (id, changed) in self.plugin_overrides() {
            values.entry(id.clone()).or_default().extend(changed.clone());
        }
    }

    /// The plugin values this tab set for itself.
    pub fn plugin_overrides(&self) -> &wassh_core::PluginSettings {
        &self.plugin_overrides
    }

    /// Plugin secrets this tab set, layered over what the vault holds.
    pub fn plugin_secret_overrides(
        &self,
    ) -> &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
        &self.plugin_secret_overrides
    }

    /// Keep Session settings' plugin values in the tab, and only there.
    pub fn set_plugin_settings(
        &mut self,
        values: wassh_core::PluginSettings,
        secrets: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    ) {
        self.plugin_overrides = values;
        self.plugin_secret_overrides = secrets;
        self.reload_pane_settings();
    }

    /// Forget the tab's own plugin values, so the host's apply again.
    pub fn clear_plugin_overrides(&mut self) {
        if self.plugin_overrides.is_empty() && self.plugin_secret_overrides.is_empty() {
            return;
        }
        self.plugin_overrides.clear();
        self.plugin_secret_overrides.clear();
        self.reload_pane_settings();
    }

    /// Whether this tab holds its own value for one plugin setting.
    pub fn has_plugin_override(&self, id: &str, key: &str) -> bool {
        self.plugin_overrides
            .get(id)
            .is_some_and(|values| values.contains_key(key))
    }

    /// Keep a value a pane changed in the tab's own map, so it stays what the tab shows.
    pub fn set_plugin_override(&mut self, id: &str, key: &str, value: serde_json::Value) {
        self.plugin_overrides
            .entry(id.to_owned())
            .or_default()
            .insert(key.to_owned(), value);
    }

    /// Restore the dock layout a tab was saved with.
    pub fn restore_layout(&mut self, layout: TabPluginLayout) {
        self.layout = layout;
    }
}

impl SessionEntry {
    /// Whether the terminal is waiting for a password.
    pub fn awaiting_login(&self) -> bool {
        self.login.as_ref().is_some_and(|login| login.active)
    }

    /// Print the password prompt into the terminal and wait for typing.
    ///
    /// Nothing is echoed while the user types, and a retry prints the same
    /// "Permission denied" line OpenSSH and PuTTY use.
    pub fn prompt_login(&mut self, open: SshOpen) {
        let attempts = self.login.as_ref().map_or(0, |login| login.attempts);
        let mut text = String::new();
        if attempts > 0 {
            text.push_str("\r\nPermission denied, please try again.\r\n");
        }
        text.push_str(&format!(
            "{}@{}'s password: ",
            open.resolved.as_ref().unwrap_or(&open.params).username, open.params.host
        ));
        self.login = Some(LoginPrompt {
            open,
            buffer: String::new(),
            attempts,
            active: true,
        });
        self.status = SessionStatus::Authenticating;
        self.error = None;
        self.backend = Backend::Closed;
        self.terminal.feed(text.as_bytes());
    }

    /// Feed keystrokes to the password prompt.
    ///
    /// Returns [`LoginOutcome::Submitted`] once the password has been handed to a new
    /// connection attempt.
    pub fn login_input(
        &mut self,
        inputs: &[wassh_ui::TerminalInput],
        term_type: &str,
    ) -> LoginOutcome {
        let Some(login) = self.login.as_mut() else {
            return LoginOutcome::Waiting;
        };
        if !login.active {
            return LoginOutcome::Waiting;
        }
        let mut submit = false;
        let mut cancel = false;
        for input in inputs {
            match input {
                wassh_ui::TerminalInput::Text(text) => {
                    for ch in text.chars() {
                        match ch {
                            '\r' | '\n' => submit = true,
                            '\u{7f}' => {
                                login.buffer.pop();
                            }
                            '\u{3}' => cancel = true,
                            ch if !ch.is_control() => login.buffer.push(ch),
                            _ => {}
                        }
                    }
                }
                wassh_ui::TerminalInput::Paste(text) => {
                    login.buffer.extend(text.chars().filter(|ch| !ch.is_control()));
                }
                wassh_ui::TerminalInput::Key(stroke) => match stroke.key {
                    Key::Enter => submit = true,
                    Key::Backspace => {
                        login.buffer.pop();
                    }
                    Key::Escape => cancel = true,
                    Key::Char('c') if stroke.ctrl => cancel = true,
                    Key::Char('u') if stroke.ctrl => login.buffer.clear(),
                    _ => {}
                },
            }
        }
        if cancel {
            self.cancel_login();
            return LoginOutcome::Cancelled;
        }
        if submit {
            self.submit_login(term_type);
            return LoginOutcome::Submitted;
        }
        LoginOutcome::Waiting
    }

    /// Abandon the prompt, leaving the tab closed but readable.
    fn cancel_login(&mut self) {
        if self.login.take().is_none() {
            return;
        }
        self.disconnect();
        self.error = Some("Login cancelled.".to_owned());
        self.terminal.feed(b"\r\nLogin cancelled.\r\n");
    }

    /// Hand the typed password to a fresh connection attempt.
    fn submit_login(&mut self, term_type: &str) {
        let Some(mut login) = self.login.take() else {
            return;
        };
        let password = std::mem::take(&mut login.buffer);
        login.active = false;
        login.attempts += 1;
        login.open.secrets.password.clone_from(&password);
        let open = login.open.clone();
        // Keep the password for automatic reconnects and for the retry prompt.
        self.secrets.password.clone_from(&password);
        self.identity_prompt_password = self.connection.identity_id.is_some();
        self.login = Some(login);
        self.terminal.feed(b"\r\n");
        self.begin(term_type, open);
    }

    /// Handle a rejected password: print the denial and prompt again.
    fn retry_login(&mut self) -> bool {
        let Some(open) = self.login.as_ref().map(|login| login.open.clone()) else {
            return false;
        };
        self.suppress_close = true;
        self.prompt_login(open);
        true
    }
}

/// A finished command, kept so panes can read it on later frames.
struct CommandResult {
    /// Output when the command ran.
    output: Option<wassh_plugins::CommandOutput>,
    /// Why it did not run, or the reason it failed.
    failure: Option<String>,
}

/// Commands panes asked the session to run, keyed by the pane's own label.
#[derive(Default)]
struct RemoteCommands {
    /// Commands still running, with the key they were asked under.
    pending: Vec<(String, std::sync::mpsc::Receiver<Result<wassh_ssh::ExecOutput, String>>)>,
    /// How to stop each of them.
    stops: Vec<(String, CommandStop)>,
    /// Last result per key.
    results: std::collections::HashMap<String, CommandResult>,
}

/// How to stop a command the session is running.
enum CommandStop {
    /// Close the channel of a command on the remote host.
    Ssh(wassh_ssh::ExecStop),
    /// Kill the shell of a command on this machine.
    Local(local_exec::LocalStop),
}

impl CommandStop {
    fn stop(&self) {
        match self {
            Self::Ssh(stop) => stop.stop(),
            Self::Local(stop) => stop.stop(),
        }
    }
}

/// What a forward request answers with: the local address, or why it failed.
type ForwardAnswer = Result<(String, u16), String>;

/// Why a forward never opened: the thread that owned it went away.
const FORWARD_CLOSED: &str = "The forward closed before it opened.";

/// Why a command never ran: the thread that owned it went away.
const COMMAND_CLOSED: &str = "the session closed before the command finished";

/// Take every answer that has arrived, leaving the rest pending.
///
/// Every pane request follows the same shape: a receiver per request, keyed by what the
/// pane asked under, collected when the answer lands. A receiver whose sender went away
/// reports `closed`, so a pane never waits on a request whose thread died.
fn drain_answers<T>(
    pending: &mut Vec<(String, Receiver<Result<T, String>>)>,
    closed: &str,
    mut landed: impl FnMut(&str, Result<T, String>),
) {
    pending.retain_mut(|(key, receiver)| match receiver.try_recv() {
        Ok(answer) => {
            landed(key, answer);
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            landed(key, Err(closed.to_owned()));
            false
        }
    });
}

/// Loopback forwards panes asked for, keyed per pane.
#[derive(Default)]
struct RemoteForwards {
    /// Forwards still being opened, with the key they were asked under.
    pending: Vec<(String, std::sync::mpsc::Receiver<ForwardAnswer>)>,
    /// Local address per key once a forward is up.
    addresses: std::collections::HashMap<String, String>,
    /// Why a forward failed.
    errors: std::collections::HashMap<String, String>,
}

impl RemoteForwards {
    /// Take every answer that has arrived.
    fn poll(&mut self) {
        let Self {
            pending,
            addresses,
            errors,
        } = self;
        drain_answers(pending, FORWARD_CLOSED, |key, answer| match answer {
            Ok((host, port)) => {
                errors.remove(key);
                addresses.insert(key.to_owned(), format!("{host}:{port}"));
            }
            Err(error) => {
                errors.insert(key.to_owned(), error);
            }
        });
    }
}

impl RemoteCommands {
    /// Take every answer that has arrived.
    fn poll(&mut self) {
        let Self {
            pending,
            stops,
            results,
        } = self;
        drain_answers(pending, COMMAND_CLOSED, |key, answer| {
            stops.retain(|(running, _)| running != key);
            let result = match answer {
                Ok(output) => CommandResult {
                    output: Some(wassh_plugins::CommandOutput {
                        stdout: output.stdout,
                        stderr: output.stderr,
                        code: output.exit_status,
                    }),
                    failure: None,
                },
                Err(failure) => CommandResult {
                    output: None,
                    failure: Some(failure),
                },
            };
            results.insert(key.to_owned(), result);
        });
    }

    /// Whether a command for `key` is still running.
    fn is_running(&self, key: &str) -> bool {
        self.pending.iter().any(|(running, _)| running == key)
    }

    /// Stop the command for `key` where it runs.
    fn cancel(&mut self, key: &str) {
        if let Some(position) = self.stops.iter().position(|(running, _)| running == key) {
            self.stops.remove(position).1.stop();
        }
    }
}

/// What the in-flight remote-file request is for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FileJob {
    /// A transfer described by `done` once it finishes.
    Transfer { done: String },
    /// A change described by `done` once it finishes.
    Action { done: String },
    /// Resolving the login directory.
    Resolve,
    /// Reading the first bytes of `path` for the viewer.
    Read { path: String, max_bytes: u64 },
    /// A tree download that becomes `archive` once it lands in `dir`.
    Zip {
        /// Temporary directory the tree is downloaded into.
        dir: std::path::PathBuf,
        /// Archive to write from it.
        archive: std::path::PathBuf,
    },
}

/// Pack `dir` into `archive` on its own thread, reporting both visible phases: adding
/// each staged file, then writing the completed archive.
fn spawn_pack(
    dir: std::path::PathBuf,
    archive: std::path::PathBuf,
    stop: wassh_ssh::SftpStop,
) -> (
    std::sync::mpsc::Receiver<Result<String, String>>,
    std::sync::mpsc::Receiver<wassh_ssh::SftpProgress>,
) {
    let (answers, answer) = std::sync::mpsc::channel();
    let (progress, progress_rx) = std::sync::mpsc::channel();
    let unused = answers.clone();
    let packed_dir = dir.clone();
    let worker_stop = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("wassh-zip".to_owned())
        .spawn(move || {
            let packed = pack_zip(&packed_dir, &archive, Some(&progress), &worker_stop);
            // The tree was only ever a staging copy.
            let _ = std::fs::remove_dir_all(&packed_dir);
            let _ = answers.send(packed);
        });
    if let Err(error) = spawned {
        let _ = unused.send(Err(format!("starting the packing thread: {error}")));
    }
    (answer, progress_rx)
}

/// Bytes written per report while an archive lands, so the bar moves smoothly.
const ZIP_WRITE_BLOCK: usize = 64 * 1024;
const ZIP_SCAN_REPORT_EVERY: u64 = 64;

/// Pack `dir` into `archive`, describing what was saved or why it could not be.
///
/// Every file under `dir` lands in the archive under its path relative to it, so the
/// caller hands in the folder's contents and the archive holds them at its root. An
/// empty folder still produces a readable (empty) archive. The archive is written in
/// blocks, so the bar restarts at zero for the archive-write phase and advances as it
/// lands instead of jumping to the end.
fn pack_zip(
    dir: &std::path::Path,
    archive: &std::path::Path,
    progress: Option<&std::sync::mpsc::Sender<wassh_ssh::SftpProgress>>,
    stop: &wassh_ssh::SftpStop,
) -> Result<String, String> {
    let mut entries = Vec::new();
    let mut walk = vec![dir.to_path_buf()];
    let mut scanned = 0;
    let report_scan = |items| {
        if let Some(sender) = progress {
            let _ = sender.send(wassh_ssh::SftpProgress {
                done: 0,
                total: None,
                label: None,
                phase: wassh_ssh::SftpProgressPhase::Scanning,
                items: Some(items),
            });
        }
    };
    report_scan(scanned);
    while let Some(next) = walk.pop() {
        if stop.is_stopped() {
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        let Ok(read) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in read.flatten() {
            if stop.is_stopped() {
                return Err(wassh_ssh::SFTP_STOPPED.to_owned());
            }
            scanned += 1;
            if scanned.is_multiple_of(ZIP_SCAN_REPORT_EVERY) {
                report_scan(scanned);
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let path = entry.path();
            if metadata.is_dir() {
                walk.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                entries.push((path, name, label, metadata.len()));
            }
        }
    }
    if !scanned.is_multiple_of(ZIP_SCAN_REPORT_EVERY) {
        report_scan(scanned);
    }
    let total: u64 = entries.iter().map(|(_, _, _, size)| size).sum();
    let names_size = entries.iter().map(|(_, name, _, _)| name.len()).sum();
    let payload_size = usize::try_from(total)
        .map_err(|error| format!("archive is too large to pack: {error}"))?;
    let mut writer = wassh_store::zip::ZipWriter::with_capacity_for(
        payload_size,
        entries.len(),
        names_size,
    )
    .map_err(|error| format!("reserving ZIP output: {error}"))?;
    let report = |done: u64, total: u64, label: Option<String>| {
        if let Some(sender) = progress {
            let phase = if label.is_some() {
                wassh_ssh::SftpProgressPhase::Packing
            } else {
                wassh_ssh::SftpProgressPhase::Writing
            };
            let _ = sender.send(wassh_ssh::SftpProgress {
                done,
                total: Some(total),
                label,
                phase,
                items: None,
            });
        }
    };
    let mut files = 0;
    let mut packed = 0;
    for (index, (path, name, label, size)) in entries.into_iter().enumerate() {
        if stop.is_stopped() {
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        if index == 0 {
            report(packed, total, Some(label.clone()));
        }
        let data = match read_zip_entry(&path, stop) {
            Ok(Some(data)) => data,
            Ok(None) => {
                packed += size;
                report(packed, total, Some(label));
                continue;
            }
            Err(reason) => return Err(reason),
        };
        if stop.is_stopped() {
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        packed += data.len() as u64;
        writer.add(&name, data);
        if stop.is_stopped() {
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        files += 1;
        report(packed, total, Some(label));
    }
    let bytes = writer.finish();
    if stop.is_stopped() {
        return Err(wassh_ssh::SFTP_STOPPED.to_owned());
    }
    if let Some(parent) = archive.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let failure = |error: std::io::Error| format!("Could not write {}: {error}", archive.display());
    let total = bytes.len() as u64;
    let mut file = std::fs::File::create(archive).map_err(failure)?;
    let mut written = 0;
    report(written, total, None);
    for block in bytes.chunks(ZIP_WRITE_BLOCK) {
        if stop.is_stopped() {
            drop(file);
            let _ = std::fs::remove_file(archive);
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        file.write_all(block).map_err(failure)?;
        written += block.len() as u64;
        report(written, total, None);
    }
    if stop.is_stopped() {
        drop(file);
        let _ = std::fs::remove_file(archive);
        return Err(wassh_ssh::SFTP_STOPPED.to_owned());
    }
    file.sync_all().map_err(failure)?;
    if stop.is_stopped() {
        drop(file);
        let _ = std::fs::remove_file(archive);
        return Err(wassh_ssh::SFTP_STOPPED.to_owned());
    }
    Ok(format!(
        "Saved {} ({} file{})",
        archive.display(),
        files,
        if files == 1 { "" } else { "s" }
    ))
}

fn read_zip_entry(
    path: &std::path::Path,
    stop: &wassh_ssh::SftpStop,
) -> Result<Option<Vec<u8>>, String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Ok(None);
    };
    let mut data = Vec::new();
    let mut block = vec![0; ZIP_WRITE_BLOCK];
    loop {
        if stop.is_stopped() {
            return Err(wassh_ssh::SFTP_STOPPED.to_owned());
        }
        let read = match file.read(&mut block) {
            Ok(read) => read,
            Err(_) => return Ok(None),
        };
        if read == 0 {
            return Ok(Some(data));
        }
        data.extend_from_slice(&block[..read]);
    }
}

/// File-browsing state for one session.
///
/// The same state serves an SSH tab (over SFTP) and a Local tab (over this
/// machine's filesystem), so the pane never needs to know which it has.
#[derive(Default)]
struct RemoteFiles {
    /// Listing request, independent of transfers and other file operations.
    listing_pending: Option<(
        String,
        std::sync::mpsc::Receiver<Result<wassh_ssh::SftpOutcome, String>>,
    )>,
    /// Stops a listing when navigation replaces it.
    listing_stop: Option<wassh_ssh::SftpStop>,
    /// Listing error, separate from transfer failures.
    listing_error: Option<String>,
    /// Answer for the request in flight.
    pending: Option<std::sync::mpsc::Receiver<Result<wassh_ssh::SftpOutcome, String>>>,
    /// What the in-flight request is for.
    job: Option<FileJob>,
    /// Path the last listing request asked for.
    requested: Option<String>,
    /// Most recent successful listing.
    listing: Option<wassh_plugins::SftpListing>,
    /// Bumped whenever [`Self::listing`] is replaced, so a pane can tell whether the copy
    /// it holds is still current.
    listing_revision: u64,
    /// Last failure.
    error: Option<String>,
    /// Result line of the last finished transfer or change.
    status: Option<String>,
    /// Absolute login directory, once resolved.
    resolved: Option<String>,
    /// Path, read limit and prefix of the file the viewer last read, so the pane does not
    /// ask for it again every frame.
    content: Option<(String, u64, wassh_plugins::files::FileContent)>,
    /// Answer of the thread packing an archive, while it runs.
    pack: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// Bytes moved by the request in flight.
    progress: Option<std::sync::mpsc::Receiver<wassh_ssh::SftpProgress>>,
    /// Latest progress value, cleared when the request finishes.
    moved: Option<wassh_ssh::SftpProgress>,
    /// Stops the active file worker.
    stop: Option<wassh_ssh::SftpStop>,
    /// Whether the active download has been asked to stop.
    stop_requested: bool,
    /// Transfers waiting their turn.
    queue: std::collections::VecDeque<QueuedTransfer>,
    /// The transfer in flight: which way it goes and the remote path.
    active: Option<(TransferDirection, String)>,
    /// The last few transfers that failed, newest last, so the pane can offer to resume
    /// them.
    failed: Vec<wassh_plugins::files::TransferRow>,
    /// Request, direction and path of the last failure, so it can be resumed.
    retryable: Option<(wassh_ssh::SftpRequest, TransferDirection, String)>,
    /// Request of the transfer in flight, kept for a retry.
    request: Option<wassh_ssh::SftpRequest>,
}

/// One transfer waiting its turn.
struct QueuedTransfer {
    request: wassh_ssh::SftpRequest,
    direction: TransferDirection,
    path: String,
    done: String,
}

/// How many failed transfers the pane keeps offering to resume.
const FAILED_TRANSFERS: usize = 6;

/// Shown when a tab has neither an SFTP connection nor a local filesystem.
const FILES_TRANSPORT_ERROR: &str = "Files are available for SSH and Local sessions";

/// Shown when a tab's transport cannot run a shell command.
const COMMAND_TRANSPORT_ERROR: &str = "Commands need an SSH or local session";

impl RemoteFiles {
    /// Forget the file that was read: a write, a change or a reload may have replaced it.
    fn forget_content(&mut self) {
        self.content = None;
    }

    fn poll_listing(&mut self) {
        let Some((path, receiver)) = self.listing_pending.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(wassh_ssh::SftpOutcome::Listed(entries))) => {
                self.listing = Some(SftpListing {
                    path: path.clone(),
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
                self.listing_revision = self.listing_revision.wrapping_add(1);
                self.requested = Some(path);
                self.listing_error = None;
                self.listing_stop = None;
            }
            Ok(Ok(_)) => {
                self.listing_error = Some("the listing request returned an unexpected result".to_owned());
                self.listing_stop = None;
            }
            Ok(Err(error)) => {
                self.listing_error = Some(error);
                self.listing_stop = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.listing_pending = Some((path, receiver));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.listing_error = Some("the listing ended without a response".to_owned());
                self.listing_stop = None;
            }
        }
    }

    /// Remember how a transfer ended, keeping the last few rows.
    fn record(&mut self) {
        let Some((direction, path)) = self.active.clone() else {
            return;
        };
        let row = wassh_plugins::files::TransferRow {
            direction,
            path,
            done: self.moved.as_ref().map(|moved| moved.done).unwrap_or(0),
            total: self.moved.as_ref().and_then(|moved| moved.total),
            progress_text: self.scan_progress_text(),
            state: TransferState::Failed,
        };
        self.failed.push(row);
        while self.failed.len() > FAILED_TRANSFERS {
            self.failed.remove(0);
        }
    }

    /// Rows the pane lists: the failures worth resuming, then the one running, then the
    /// queue.
    fn rows(&self) -> Vec<wassh_plugins::files::TransferRow> {
        let mut rows = self.failed.clone();
        if !self.stop_requested
            && let Some((direction, path)) = &self.active
        {
            let (done, total) = self
                .moved
                .as_ref()
                .map(|moved| (moved.done, moved.total))
                .unwrap_or((0, None));
            rows.push(wassh_plugins::files::TransferRow {
                direction: *direction,
                path: self.running_message(path),
                done,
                total,
                progress_text: self.scan_progress_text(),
                state: TransferState::Running,
            });
        }
        for queued in &self.queue {
            rows.push(wassh_plugins::files::TransferRow {
                direction: queued.direction,
                path: queued.path.clone(),
                done: 0,
                total: None,
                progress_text: None,
                state: TransferState::Queued,
            });
        }
        rows
    }

    /// What the running row says.
    ///
    /// An archive is two steps, so the row names the one it is in: the files being
    /// packed, then the archive being downloaded.
    fn running_message(&self, path: &str) -> String {
        let archive = std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());
        if self.pack.is_some() {
            return match self.moved.as_ref() {
                Some(progress) => match progress.phase {
                    wassh_ssh::SftpProgressPhase::Scanning => "Scanning files".to_owned(),
                    wassh_ssh::SftpProgressPhase::Packing => format!(
                        "Packing {}",
                        progress.label.as_deref().unwrap_or("files")
                    ),
                    wassh_ssh::SftpProgressPhase::Writing => format!("Downloading {archive}"),
                    wassh_ssh::SftpProgressPhase::Transfer => "Downloading files".to_owned(),
                },
                None => "Packing files".to_owned(),
            };
        }
        if self
            .active
            .as_ref()
            .is_some_and(|(direction, _)| *direction == TransferDirection::Zip)
        {
            if self
                .moved
                .as_ref()
                .is_some_and(|progress| progress.phase == wassh_ssh::SftpProgressPhase::Scanning)
            {
                return "Scanning files".to_owned();
            }
            return self
                .moved
                .as_ref()
                .and_then(|progress| progress.label.as_deref())
                .map(|name| format!("Downloading {name}"))
                .unwrap_or_else(|| "Downloading files".to_owned());
        }
        let is_tree_download = self
            .request
            .as_ref()
            .is_some_and(|request| matches!(request, wassh_ssh::SftpRequest::DownloadTree { .. }));
        if self
            .moved
            .as_ref()
            .is_some_and(|progress| progress.phase == wassh_ssh::SftpProgressPhase::Scanning)
            || (is_tree_download && self.moved.is_none())
        {
            return "Scanning files".to_owned();
        }
        if is_tree_download {
            return self
                .moved
                .as_ref()
                .and_then(|progress| progress.label.as_deref())
                .map(|name| format!("Downloading {name}"))
                .unwrap_or_else(|| "Downloading files".to_owned());
        }
        path.to_owned()
    }

    /// Alternate right-column text while the byte total is not known yet.
    fn scan_progress_text(&self) -> Option<String> {
        self.moved.as_ref().and_then(|progress| {
            (progress.phase == wassh_ssh::SftpProgressPhase::Scanning)
                .then(|| progress.items.map(|items| format!("{items} items inspected")))
                .flatten()
        })
    }

    /// Take the answer of a finished request.
    ///
    /// Returns the directory to list again when a transfer landed in it.
    fn poll(&mut self) -> Option<String> {
        self.take_progress();
        if self.pack.is_some() {
            return self.poll_pack();
        }
        let receiver = self.pending.take()?;
        let answer = receiver.try_recv();
        let answered = !matches!(answer, Err(std::sync::mpsc::TryRecvError::Empty));
        if answered {
            // The request is over: its bar has nothing left to show.
            self.progress = None;
            self.moved = None;
            let completed_before_stop = matches!(&answer, Ok(Ok(_)))
                && !matches!(self.job, Some(FileJob::Zip { .. }));
            if self.stop_requested && !completed_before_stop {
                self.finish_cancelled();
                return None;
            }
        }
        match answer {
            Ok(Ok(outcome)) => {
                let job = self.job.take();
                self.error = None;
                match (outcome, job) {
                    (wassh_ssh::SftpOutcome::RealPath(path), _) => {
                        self.resolved = Some(path);
                        None
                    }
                    (
                        wassh_ssh::SftpOutcome::Content { data, truncated },
                        Some(FileJob::Read { path, max_bytes }),
                    ) => {
                        self.content = Some((
                            path,
                            max_bytes,
                            wassh_plugins::files::FileContent { data, truncated },
                        ));
                        None
                    }
                    (wassh_ssh::SftpOutcome::Transferred { .. }, Some(FileJob::Zip { dir, archive })) => {
                        // The tree is here; the archive is written on its own thread, so
                        // the bar sweeps again while it packs instead of the pane freezing.
                        let stop = self.stop.clone().unwrap_or_default();
                        let (answer, progress) = spawn_pack(dir, archive, stop);
                        self.pack = Some(answer);
                        self.progress = Some(progress);
                        self.moved = None;
                        None
                    }
                    (wassh_ssh::SftpOutcome::Done, job) => {
                        if let Some(FileJob::Action { done }) = job {
                            self.status = Some(done);
                        }
                        // A change landed: what was read of these files is stale, and the
                        // directory is shown as it is now.
                        self.forget_content();
                        self.requested.clone()
                    }
                    (wassh_ssh::SftpOutcome::Transferred { bytes }, job) => {
                        let done = match job {
                            Some(FileJob::Transfer { done }) | Some(FileJob::Action { done }) => {
                                done
                            }
                            _ => "Transfer finished".to_owned(),
                        };
                        self.status = Some(format!(
                            "{done} ({})",
                            wassh_plugins::files::format_size(bytes)
                        ));
                        // A transfer that landed leaves no row behind: the list is for what
                        // is still going and what went wrong.
                        self.active = None;
                        self.stop = None;
                        self.stop_requested = false;
                        self.request = None;
                        // A transfer landing can replace a file under a reader.
                        self.forget_content();
                        self.requested.clone()
                    }
                    // A stale answer for a request that was abandoned.
                    _ => None,
                }
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                if self.active.is_some() {
                    self.record();
                    // Keep the request so the pane can resume it.
                    if let Some(FileJob::Transfer { .. }) = self.job
                        && let Some(request) = self.request.clone()
                        && let Some(failed) = self.failed.last()
                    {
                        self.retryable = Some((request, failed.direction, failed.path.clone()));
                    }
                    self.active = None;
                }
                // A failed write may still have cut the file short.
                self.forget_content();
                self.request = None;
                self.job = None;
                self.stop = None;
                self.stop_requested = false;
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Still running.
                self.pending = Some(receiver);
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.error = Some("the file operation ended without a response".to_owned());
                self.job = None;
                None
            }
        }
    }

    /// Collect what the progress channel last reported: the value the bar shows.
    ///
    /// It is kept until the request answers, so the bar keeps moving meanwhile.
    fn take_progress(&mut self) {
        if let Some(progress) = &self.progress {
            for value in progress.try_iter() {
                self.moved = Some(value);
            }
        }
    }

    /// Take the answer of the thread packing an archive.
    fn poll_pack(&mut self) -> Option<String> {
        let answers = self.pack.take()?;
        let answer = answers.try_recv();
        // The last block may not have been read yet, and the row it closes shows it.
        self.take_progress();
        match answer {
            Ok(Ok(saved)) => {
                self.finish_pack(None);
                self.status = Some(saved);
                None
            }
            Ok(Err(reason)) => {
                if self.stop_requested {
                    self.finish_cancelled();
                } else {
                    self.finish_pack(Some(reason));
                }
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pack = Some(answers);
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if self.stop_requested {
                    self.finish_cancelled();
                } else {
                    self.finish_pack(Some("the archive was not written".to_owned()));
                }
                None
            }
        }
    }

    /// Close the archive's row and its bar, keeping a row when it failed.
    fn finish_pack(&mut self, failure: Option<String>) {
        if failure.is_some() {
            self.record();
        }
        self.active = None;
        self.progress = None;
        self.moved = None;
        self.stop = None;
        self.stop_requested = false;
        self.job = None;
        self.request = None;
        self.error = failure;
    }

    fn finish_cancelled(&mut self) {
        let direction = self.active.as_ref().map(|(direction, _)| *direction);
        if self.pack.is_none()
            && let Some(FileJob::Zip { dir, .. }) = &self.job
        {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.finish_pack(None);
        self.status = Some(
            match direction {
                Some(TransferDirection::Upload) => "Upload stopped",
                Some(TransferDirection::Download) => "Download stopped",
                Some(TransferDirection::Zip) => "ZIP download stopped",
                Some(TransferDirection::Copy) => "Copy stopped",
                None => "Transfer stopped",
            }
            .to_owned(),
        );
    }
}

impl wassh_plugins::SftpAccess for SessionEntry {
    fn request_listing(&mut self, path: &str) {
        if self.files.requested.as_deref() == Some(path) {
            return;
        }
        self.start_file_request(path);
    }

    fn refresh(&mut self, path: &str) {
        self.files.requested = None;
        // The pane asked to look again, so the file it read before is not the answer.
        self.files.forget_content();
        self.start_file_request(path);
    }

    fn listing(&self) -> Option<&wassh_plugins::SftpListing> {
        self.files.listing.as_ref()
    }

    fn listing_revision(&self) -> u64 {
        self.files.listing_revision
    }

    fn is_loading(&self) -> bool {
        self.files.listing_pending.is_some()
            || self.files.pending.is_some()
            || self.files.pack.is_some()
    }

    fn error(&self) -> Option<&str> {
        self.files
            .listing_error
            .as_deref()
            .or(self.files.error.as_deref())
    }

    fn upload(&mut self, local: &str, remote: &str) {
        if local.trim().is_empty() {
            self.files.error = Some("Type the path of a local file to upload.".to_owned());
            return;
        }
        self.enqueue_transfer(
            wassh_ssh::SftpRequest::Upload {
                local: std::path::PathBuf::from(local.trim()),
                remote: remote.to_owned(),
                resume: false,
            },
            TransferDirection::Upload,
            remote.to_owned(),
        );
    }

    fn write_file(&mut self, remote: &str, content: Vec<u8>) {
        self.enqueue_transfer(
            wassh_ssh::SftpRequest::Write {
                path: remote.to_owned(),
                content,
            },
            TransferDirection::Upload,
            remote.to_owned(),
        );
    }

    fn download(&mut self, remote: &str, local: &str) {
        if local.trim().is_empty() {
            self.files.error = Some("Type where the download should land.".to_owned());
            return;
        }
        self.enqueue_transfer(
            wassh_ssh::SftpRequest::Download {
                remote: remote.to_owned(),
                local: std::path::PathBuf::from(local.trim()),
                resume: false,
            },
            TransferDirection::Download,
            remote.to_owned(),
        );
    }

    fn is_transferring(&self) -> bool {
        // Copies, batches, archives and the packing of one move bytes too, so the bar
        // tracks them. A queued transfer has not landed yet, so it counts as well.
        if !self.files.queue.is_empty() || self.files.pack.is_some() {
            return true;
        }
        self.files.pending.is_some()
            && matches!(
                self.files.job,
                Some(FileJob::Transfer { .. } | FileJob::Action { .. } | FileJob::Zip { .. })
            )
    }

    fn transfer_status(&self) -> Option<&str> {
        self.files.status.as_deref()
    }

    fn report_problem(&mut self, message: &str) {
        self.files.error = Some(message.to_owned());
    }

    fn resolve(&mut self, path: &str) -> Option<String> {
        if let Some(resolved) = &self.files.resolved {
            return Some(resolved.clone());
        }
        if self.files.pending.is_some() {
            return None;
        }
        // A Local tab answers at once: this machine's paths need no round trip.
        if matches!(self.backend, Backend::Local(_)) {
            let resolved = if path == "." {
                // The browser opens in the user's home directory.
                match &self.connection.local {
                    LocalShell::Wsl { distro } => wsl_files::start_dir(distro.as_deref()),
                    _ => local_files::start_dir(),
                }
            } else {
                let resolved = match &self.connection.local {
                    LocalShell::Wsl { distro } => {
                        wsl_files::canonical(distro.as_deref(), path)
                    }
                    _ => local_files::canonical(path),
                };
                match resolved {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        self.files.error = Some(error);
                        return None;
                    }
                }
            };
            self.files.resolved = Some(resolved.clone());
            return Some(resolved);
        }
        let Some(job) = self.file_job(wassh_ssh::SftpRequest::RealPath {
            path: path.to_owned(),
        }) else {
            self.files.error = Some(FILES_TRANSPORT_ERROR.to_owned());
            return None;
        };
        self.adopt_job(job);
        self.files.job = Some(FileJob::Resolve);
        None
    }

    fn mkdir(&mut self, path: &str) {
        self.start_file_action(
            wassh_ssh::SftpRequest::Mkdir {
                path: path.to_owned(),
            },
            format!("Created {path}"),
        );
    }

    fn rename(&mut self, from: &str, to: &str) {
        self.start_file_action(
            wassh_ssh::SftpRequest::Rename {
                from: from.to_owned(),
                to: to.to_owned(),
            },
            format!("Renamed to {to}"),
        );
    }

    fn remove(&mut self, path: &str, is_dir: bool) {
        self.start_file_action(
            wassh_ssh::SftpRequest::Remove {
                path: path.to_owned(),
                is_dir,
            },
            format!("Deleted {path}"),
        );
    }

    fn chmod(&mut self, path: &str, mode: u32) {
        self.start_file_action(
            wassh_ssh::SftpRequest::Chmod {
                path: path.to_owned(),
                mode,
            },
            format!("Mode {mode:03o} on {path}"),
        );
    }

    fn read(&mut self, path: &str, max_bytes: u64) -> Option<wassh_plugins::files::FileContent> {
        // Answer only from a snapshot that holds at least as much as this read asks for.
        if let Some((cached, limit, content)) = &self.files.content
            && cached == path
            && *limit >= max_bytes
        {
            return Some(content.clone());
        }
        if self.files.pending.is_some() || self.files.pack.is_some() {
            return None;
        }
        let job = self.file_job(wassh_ssh::SftpRequest::Read {
            path: path.to_owned(),
            max_bytes,
        })?;
        self.adopt_job(job);
        self.files.job = Some(FileJob::Read {
            path: path.to_owned(),
            max_bytes,
        });
        None
    }

    fn upload_many(&mut self, items: Vec<(String, String)>) {
        // One transfer per file, so the pane can show a row for each.
        for (local, remote) in items {
            self.enqueue_transfer(
                wassh_ssh::SftpRequest::Upload {
                    local: std::path::PathBuf::from(local),
                    remote: remote.clone(),
                    resume: false,
                },
                TransferDirection::Upload,
                remote,
            );
        }
    }

    fn download_many(&mut self, items: Vec<(String, String)>) {
        for (remote, local) in items {
            self.enqueue_transfer(
                wassh_ssh::SftpRequest::Download {
                    remote: remote.clone(),
                    local: std::path::PathBuf::from(local),
                    resume: false,
                },
                TransferDirection::Download,
                remote,
            );
        }
    }

    fn progress(&self) -> Option<(u64, Option<u64>)> {
        self.files.moved.as_ref().map(|moved| (moved.done, moved.total))
    }

    fn transfers(&self) -> Vec<wassh_plugins::files::TransferRow> {
        self.files.rows()
    }

    fn stop_transfer(&mut self) {
        if self.files.active.is_some()
            && let Some(stop) = &self.files.stop
        {
            self.files.stop_requested = true;
            stop.stop();
        }
    }

    fn retry(&mut self) {
        let Some((mut request, direction, path)) = self.files.retryable.take() else {
            return;
        };
        // Continue where it stopped instead of starting over.
        match &mut request {
            wassh_ssh::SftpRequest::Upload { resume, .. }
            | wassh_ssh::SftpRequest::Download { resume, .. }
            | wassh_ssh::SftpRequest::UploadMany { resume, .. }
            | wassh_ssh::SftpRequest::DownloadMany { resume, .. } => *resume = true,
            _ => {}
        }
        self.enqueue_transfer(request, direction, path);
    }

    fn download_tree(&mut self, items: Vec<String>, dir: &str) {
        // A tree download is a transfer like any other, so the pane shows it in its list.
        self.enqueue_transfer(
            wassh_ssh::SftpRequest::DownloadTree {
                items,
                dir: std::path::PathBuf::from(dir),
            },
            TransferDirection::Download,
            dir.to_owned(),
        );
    }

    fn download_zip(&mut self, items: Vec<String>, archive: &str) {
        let dir = std::env::temp_dir().join(format!("wassh-zip-{}", items.len()));
        let _ = std::fs::remove_dir_all(&dir);
        self.start_zip(
            wassh_ssh::SftpRequest::DownloadTree {
                items,
                dir: dir.clone(),
            },
            dir,
            std::path::PathBuf::from(archive),
        );
    }

    fn copy_many(&mut self, items: Vec<(String, String)>, move_items: bool) {
        let count = items.len();
        self.enqueue_transfer_with_done(
            wassh_ssh::SftpRequest::CopyMany {
                items,
                // The copy is streamed through this machine: SFTP has no
                // server-side copy.
                scratch: std::env::temp_dir().join(format!("wassh-copy-{count}.tmp")),
                move_items,
            },
            TransferDirection::Copy,
            if move_items {
                format!("Moving {count} item(s)")
            } else {
                format!("Copying {count} item(s)")
            },
            if move_items {
                format!("Moved {count} item(s)")
            } else {
                format!("Copied {count} item(s)")
            },
        );
    }
}

impl wassh_plugins::MonitorAccess for SessionEntry {
    fn native_sampling(&self) -> bool {
        // A Local tab's host is this machine, so its statistics come from the operating
        // system itself wherever the platform has an API for them. A WSL tab's host is its
        // distribution instead, which the pane samples with a probe inside it.
        wassh_plugins::monitor::native_supported()
            && self.connection.kind == ConnectionType::Local
            && !matches!(self.connection.local, LocalShell::Wsl { .. })
    }

    fn native_details(&mut self, pid: u32) -> Option<Result<Vec<(String, String)>, String>> {
        if !self.native_sampling() {
            return None;
        }
        Some(
            wassh_plugins::monitor::native_details(pid)
                .ok_or_else(|| format!("process {pid} is gone")),
        )
    }

    fn native_stop(&mut self, pid: u32, signal: &str) -> Option<Result<(), String>> {
        if !self.native_sampling() {
            return None;
        }
        Some(wassh_plugins::monitor::native_stop(pid, signal))
    }
}

impl wassh_plugins::CommandAccess for SessionEntry {
    fn exec(&mut self, key: &str, command: &str) {
        self.start_command(key, command, None);
    }

    fn exec_input(&mut self, key: &str, command: &str, input: &str) {
        self.start_command(key, command, Some(input));
    }

    fn output(&self, key: &str) -> Option<&wassh_plugins::CommandOutput> {
        self.commands
            .results
            .get(key)
            .and_then(|result| result.output.as_ref())
    }

    fn failure(&self, key: &str) -> Option<&str> {
        self.commands
            .results
            .get(key)
            .and_then(|result| result.failure.as_deref())
    }

    fn is_running(&self, key: &str) -> bool {
        self.commands.is_running(key)
    }

    fn cancel(&mut self, key: &str) {
        self.commands.cancel(key);
    }

    fn platform(&self) -> wassh_core::Platform {
        self.connection.platform()
    }
}

impl wassh_plugins::TcpAccess for SessionEntry {
    fn open_forward(&mut self, key: &str, host: &str, port: u16) {
        self.forwards.poll();
        // Ask once per key: a second call while one is in flight, or after it
        // succeeded, would only bind another listener nobody uses.
        if self.forwards.addresses.contains_key(key)
            || self
                .forwards
                .pending
                .iter()
                .any(|(pending, _)| pending == key)
        {
            return;
        }
        self.forwards.errors.remove(key);
        let Backend::Ssh(session) = &self.backend else {
            self.forwards
                .errors
                .insert(key.to_owned(), "A broker tunnel needs an SSH session.".to_owned());
            return;
        };
        self.forwards
            .pending
            .push((key.to_owned(), session.forward_tcp(host.to_owned(), port)));
    }

    fn forward_address(&self, key: &str) -> Option<&str> {
        self.forwards.addresses.get(key).map(String::as_str)
    }

    fn forward_error(&self, key: &str) -> Option<&str> {
        self.forwards.errors.get(key).map(String::as_str)
    }

    fn forward_pending(&self, key: &str) -> bool {
        self.forwards
            .pending
            .iter()
            .any(|(pending, _)| pending == key)
    }
}

impl wassh_plugins::TerminalAccess for SessionEntry {
    fn screen_text(&self) -> String {
        // The emulator only knows the visible grid, which is what the AI agent tools
        // describe as "the screen"; trailing blanks and the empty rows below the last
        // output are dropped, so a tool reads only what was printed.
        let snapshot = self.snapshot();
        let mut lines: Vec<String> = grid_lines(&snapshot)
            .map(|line| line.trim_end().to_owned())
            .collect();
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    fn type_text(&mut self, text: &str) {
        self.send_paste(text);
    }

    fn is_closed(&self) -> bool {
        matches!(self.backend, Backend::Closed)
    }
}

/// Count the open sessions attached to each saved host, in first-seen order.
pub fn open_host_sessions<'a>(
    sessions: impl Iterator<Item = Option<&'a str>>,
) -> Vec<(String, usize)> {
    let mut open: Vec<(String, usize)> = Vec::new();
    for host in sessions.flatten() {
        match open.iter_mut().find(|(id, _)| id == host) {
            Some((_, count)) => *count += 1,
            None => open.push((host.to_owned(), 1)),
        }
    }
    open
}

/// Resolve the jump chain for `params`, outermost hop first.
///
/// Follows `jump_host_id` through the saved hosts, refusing loops. Credentials come
/// from `vault`: jump hosts never prompt, so a hop without usable credentials is an
/// error rather than a silent fallback.
pub fn jump_chain(
    hosts: &wassh_core::HostsDocument,
    params: &ConnectionParams,
    identities: &[wassh_core::Identity],
    vault: Option<&wassh_store::SecretsStore>,
) -> Result<Vec<wassh_ssh::JumpHop>, String> {
    let mut chain = Vec::new();
    let mut next = params.jump_host_id.clone();
    let mut seen = std::collections::HashSet::new();

    while let Some(id) = next {
        if !seen.insert(id.clone()) {
            return Err("jump hosts form a loop".to_owned());
        }
        if seen.len() > 8 {
            return Err("jump host chain is deeper than eight hops".to_owned());
        }
        let host = hosts
            .host(&id)
            .ok_or_else(|| format!("jump host '{id}' no longer exists"))?;
        let (connection, secrets) = resolve_auth(&host.connection, Some(&host.id), identities, vault)?;
        chain.insert(
            0,
            wassh_ssh::JumpHop {
                host: connection.host.clone(),
                port: connection.port,
                username: if connection.username.trim().is_empty() {
                    local_username()
                } else {
                    connection.username.clone()
                },
                auth: jump_auth(host, &connection, &secrets)?,
            },
        );
        next = connection.jump_host_id.clone();
    }

    Ok(chain)
}

/// Credentials one jump hop authenticates with, taken from the vault.
fn jump_auth(
    host: &wassh_core::HostProfile,
    connection: &ConnectionParams,
    secrets: &Secrets,
) -> Result<wassh_ssh::SshAuth, String> {
    let name = host.display_name();
    match connection.auth {
        AuthMethod::Password => {
            let password = secrets.password.clone();
            if password.is_empty() {
                return Err(format!(
                    "jump host '{name}' needs a saved password in its host settings"
                ));
            }
            Ok(wassh_ssh::SshAuth::Password(password))
        }
        AuthMethod::PrivateKey => {
            let path = connection.private_key_path.clone().unwrap_or_default();
            if path.trim().is_empty() {
                return Err(format!(
                    "jump host '{name}' has no private key configured"
                ));
            }
            let passphrase = (!secrets.key_passphrase.is_empty()).then(|| secrets.key_passphrase.clone());
            Ok(wassh_ssh::SshAuth::PrivateKey {
                path: path.into(),
                passphrase,
            })
        }
        AuthMethod::None => Ok(wassh_ssh::SshAuth::Agent),
    }
}

pub fn resolve_auth(
    params: &ConnectionParams,
    host_id: Option<&str>,
    identities: &[wassh_core::Identity],
    vault: Option<&wassh_store::SecretsStore>,
) -> Result<(ConnectionParams, Secrets), String> {
    let mut resolved = params.clone();
    let secrets = if let Some(id) = &params.identity_id {
        let identity = identities.iter().find(|identity| &identity.id == id)
            .ok_or_else(|| format!("identity '{id}' no longer exists"))?;
        resolved.username = identity.username.clone();
        resolved.auth = identity.auth;
        resolved.private_key_path = identity.private_key_path.clone();
        Secrets {
            password: vault.and_then(|vault| vault.identity_password(id)).unwrap_or_default(),
            key_passphrase: vault.and_then(|vault| vault.identity_passphrase(id)).unwrap_or_default(),
        }
    } else {
        Secrets {
            password: host_id.and_then(|id| vault.and_then(|vault| vault.password(id))).unwrap_or_default(),
            key_passphrase: host_id.and_then(|id| vault.and_then(|vault| vault.passphrase(id))).unwrap_or_default(),
        }
    };
    if resolved.username.trim().is_empty() {
        resolved.username = local_username();
    }
    Ok((resolved, secrets))
}

fn local_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_owned())
}

/// The character a snapshot cell shows, with the emulator's blank for an empty cell.
fn cell_char(cell: &CellSnapshot) -> char {
    if cell.ch == '\0' { ' ' } else { cell.ch }
}

/// One line per visible row, in reading order.
fn grid_lines(snapshot: &ScreenSnapshot) -> impl Iterator<Item = String> + '_ {
    let cols = usize::from(snapshot.size.cols);
    (0..usize::from(snapshot.size.rows)).map(move |row| {
        (0..cols)
            .map(move |col| snapshot.cell(row, col).map_or(' ', cell_char))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::SessionStyleDefaults;

    const ZIP_PROGRESS_TEST_BYTES: u64 = 7;
    const ZIP_PROGRESS_TEST_ARCHIVE_BYTES: u64 = 120;
    const TREE_SCAN_TEST_ITEMS: u64 = 128;
    const ZIP_SCAN_TEST_ITEMS: u64 = 2;

    #[test]
    fn a_local_tab_keeps_the_name_its_connection_gave_it() {
        let defaults = SessionStyleDefaults::default();
        let local = SessionEntry::prepare(
            "local",
            &Palette::dark(),
            &defaults,
            TerminalSize::new(80, 24),
            ConnectionParams {
                kind: ConnectionType::Local,
                ..ConnectionParams::default()
            },
            None,
        );
        assert!(
            !local.accepts_title(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
            "the console title is the executable's path"
        );
        assert!(!local.accepts_title("   "));

        let ssh = SessionEntry::prepare(
            "ssh",
            &Palette::dark(),
            &defaults,
            TerminalSize::new(80, 24),
            ConnectionParams::default(),
            None,
        );
        assert!(ssh.accepts_title("user@host: ~"));
    }

    #[test]
    fn a_wsl_tab_appends_its_environment_to_the_shell_title() {
        let wsl = SessionEntry::prepare(
            "wsl",
            &Palette::dark(),
            &SessionStyleDefaults::default(),
            TerminalSize::new(80, 24),
            ConnectionParams {
                kind: ConnectionType::Local,
                local: LocalShell::Wsl {
                    distro: Some("Ubuntu".to_owned()),
                },
                ..ConnectionParams::default()
            },
            None,
        );
        assert!(wsl.accepts_title("octopy@dev: ~"));
        assert_eq!(
            SessionEntry::titled(&wsl.connection, "octopy@dev: ~".to_owned()),
            "octopy@dev: ~ (Ubuntu)"
        );
    }

    #[test]
    fn a_local_shell_that_exits_ends_the_session_on_purpose() {
        // The shell is replaced with one that logs out immediately, like typing `exit`.
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe".to_owned(), vec!["/c".to_owned(), "exit".to_owned()]);
        #[cfg(not(windows))]
        let (shell, args) = ("/bin/sh".to_owned(), vec!["-c".to_owned(), "exit".to_owned()]);

        let size = TerminalSize::new(80, 24);
        let mut entry = SessionEntry::prepare(
            "t-local",
            &Palette::dark(),
            &SessionStyleDefaults::default(),
            size,
            ConnectionParams {
                kind: ConnectionType::Local,
                ..ConnectionParams::default()
            },
            None,
        );
        entry.backend = Backend::Local(
            PtySession::spawn(&shell, &args, size, "xterm-256color").expect("pty"),
        );
        entry.status = SessionStatus::Connected;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut closed = None;
        while closed.is_none() && std::time::Instant::now() < deadline {
            closed = entry.pump("xterm-256color").closed;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            closed,
            Some(Exit {
                reason: None,
                from_shell: true,
            }),
            "a shell that exits must be reported as ended on purpose"
        );
        assert_eq!(entry.status, SessionStatus::Closed);
    }

    #[test]
    fn the_transfer_queue_lists_what_runs_and_what_waits() {
        let mut files = RemoteFiles::default();
        assert!(files.rows().is_empty());

        files.queue.push_back(QueuedTransfer {
            request: wassh_ssh::SftpRequest::RealPath { path: ".".to_owned() },
            direction: TransferDirection::Upload,
            path: "/srv/second".to_owned(),
            done: "/srv/second".to_owned(),
        });
        files.queue.push_back(QueuedTransfer {
            request: wassh_ssh::SftpRequest::RealPath { path: ".".to_owned() },
            direction: TransferDirection::Upload,
            path: "/srv/third".to_owned(),
            done: "/srv/third".to_owned(),
        });
        let rows = files.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.state == TransferState::Queued));
        assert_eq!(rows[0].path, "/srv/second");

        // A failure worth resuming stays at the top, then the running one, then the
        // queue — the order the pane paints them in.
        files.active = Some((TransferDirection::Upload, "/srv/failed".to_owned()));
        files.moved = None;
        files.record();
        files.active = Some((TransferDirection::Upload, "/srv/running".to_owned()));
        let rows = files.rows();
        let paths: Vec<&str> = rows.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/srv/failed", "/srv/running", "/srv/second", "/srv/third"]
        );
        assert_eq!(rows[1].state, TransferState::Running);
        assert_eq!(rows[0].state, TransferState::Failed);
    }

    #[test]
    fn only_the_last_few_failures_are_kept() {
        let mut files = RemoteFiles::default();
        for index in 0..FAILED_TRANSFERS + 3 {
            files.active = Some((TransferDirection::Upload, format!("job {index}")));
            files.record();
        }
        files.active = None;
        let rows = files.rows();
        assert_eq!(rows.len(), FAILED_TRANSFERS);
        assert_eq!(rows[0].path, "job 3");
    }

    #[test]
    fn a_failed_transfer_is_worth_resuming() {
        let mut files = RemoteFiles::default();
        files.failed.push(wassh_plugins::files::TransferRow {
            direction: TransferDirection::Upload,
            path: "/srv/big.iso".to_owned(),
            done: 4096,
            total: Some(65_536),
            progress_text: None,
            state: TransferState::Failed,
        });
        assert!(
            files
                .rows()
                .iter()
                .any(|row| row.state == TransferState::Failed),
            "the pane needs a failure to offer Resume"
        );
    }

    #[test]
    fn local_sessions_start_connected() {
        let palette = Palette::dark();
        let entry = SessionEntry::new_local(
            "t1",
            &palette,
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            local_params(),
        );
        assert!(entry.is_live(), "status was {:?}", entry.status);
        assert_eq!(
            entry.title,
            if cfg!(windows) {
                "Local (PS)"
            } else {
                "Local terminal"
            }
        );
        assert_eq!(entry.host_id.as_deref(), Some(wassh_core::LOCAL_HOST_ID));
    }

    #[test]
    fn ssh_sessions_default_titles_use_the_endpoint() {
        let palette = Palette::dark();
        let params = ConnectionParams {
            kind: ConnectionType::Ssh,
            host: "example.com".to_owned(),
            port: 2222,
            username: "alice".to_owned(),
            ..ConnectionParams::default()
        };
        let entry = SessionEntry::new_ssh(
            "t2",
            &palette,
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            SshOpen {
                params,
                resolved: None,
                host_id: None,
                secrets: Secrets::default(),
                known_hosts: Arc::new(KnownHosts::in_memory()),
                jumps: Vec::new(),
                jump_error: None,
            },
        );
        assert_eq!(entry.title, "alice@example.com:2222");
    }

    fn password_entry() -> SessionEntry {
        let params = ConnectionParams {
            kind: ConnectionType::Ssh,
            host: "127.0.0.1".to_owned(),
            port: 1,
            username: "alice".to_owned(),
            auth: AuthMethod::Password,
            ..ConnectionParams::default()
        };
        SessionEntry::new_ssh_prompt(
            "t-prompt",
            &Palette::dark(),
            &SessionStyleDefaults::default(),
            TerminalSize::new(80, 24),
            SshOpen {
                params,
                resolved: None,
                host_id: None,
                secrets: Secrets::default(),
                known_hosts: Arc::new(KnownHosts::in_memory()),
                jumps: Vec::new(),
                jump_error: None,
            },
        )
    }

    #[test]
    fn password_sessions_prompt_inside_the_terminal() {
        let entry = password_entry();
        assert!(entry.awaiting_login());
        assert_eq!(entry.status, SessionStatus::Authenticating);
        assert!(matches!(entry.backend, Backend::Closed));
        assert!(
            entry.visible_text().contains("alice@127.0.0.1's password:"),
            "screen was {:?}",
            entry.visible_text()
        );
    }

    #[test]
    fn typing_a_password_stays_masked() {
        let mut entry = password_entry();
        let outcome = entry.login_input(
            &[wassh_ui::TerminalInput::Text("s3cret".to_owned())],
            "xterm-256color",
        );
        assert_eq!(outcome, LoginOutcome::Waiting);
        assert_eq!(
            entry.login.as_ref().map(|login| login.buffer.as_str()),
            Some("s3cret")
        );
        assert!(!entry.visible_text().contains("s3cret"));

        // Backspace edits the buffer without echoing.
        entry.login_input(
            &[wassh_ui::TerminalInput::Key(wassh_term::KeyStroke {
                key: Key::Backspace,
                ctrl: false,
                alt: false,
                shift: false,
            })],
            "xterm-256color",
        );
        assert_eq!(
            entry.login.as_ref().map(|login| login.buffer.as_str()),
            Some("s3cre")
        );
    }

    #[test]
    fn enter_hands_the_password_to_the_connection() {
        let mut entry = password_entry();
        let outcome = entry.login_input(
            &[
                wassh_ui::TerminalInput::Text("hunter2".to_owned()),
                wassh_ui::TerminalInput::Key(wassh_term::KeyStroke {
                    key: Key::Enter,
                    ctrl: false,
                    alt: false,
                    shift: false,
                }),
            ],
            "xterm-256color",
        );
        assert_eq!(outcome, LoginOutcome::Submitted);
        assert_eq!(entry.secrets.password, "hunter2");
        assert!(!entry.awaiting_login());
        assert!(entry.login.is_some(), "kept for a retry prompt");
        assert_eq!(
            entry.login.as_ref().map(|login| login.attempts),
            Some(1)
        );
    }

    #[test]
    fn identity_prompt_password_is_reused_until_a_saved_password_replaces_it() {
        let mut entry = password_entry();
        entry.connection.identity_id = Some("shared".to_owned());
        entry.login_input(
            &[
                wassh_ui::TerminalInput::Text("typed".to_owned()),
                wassh_ui::TerminalInput::Key(wassh_term::KeyStroke {
                    key: Key::Enter, ctrl: false, alt: false, shift: false,
                }),
            ],
            "xterm-256color",
        );
        let resolved = Some(entry.connection.clone());
        entry.set_resolved_auth(resolved.clone(), Secrets::default(), Vec::new(), None);
        assert_eq!(entry.secrets.password, "typed");
        entry.set_resolved_auth(resolved.clone(), Secrets { password: "saved".to_owned(), ..Default::default() }, Vec::new(), None);
        assert_eq!(entry.secrets.password, "saved");
        entry.set_resolved_auth(resolved, Secrets::default(), Vec::new(), None);
        assert!(entry.secrets.password.is_empty());
    }

    #[test]
    fn a_rejected_password_prints_the_denial_and_prompts_again() {
        let mut entry = password_entry();
        entry.login_input(
            &[
                wassh_ui::TerminalInput::Text("wrong".to_owned()),
                wassh_ui::TerminalInput::Key(wassh_term::KeyStroke {
                    key: Key::Enter,
                    ctrl: false,
                    alt: false,
                    shift: false,
                }),
            ],
            "xterm-256color",
        );
        assert!(entry.retry_login(), "the prompt flow survives a rejection");
        assert!(entry.awaiting_login());
        let screen = entry.visible_text();
        assert!(
            screen.contains("Permission denied, please try again."),
            "screen was {screen:?}"
        );
        assert!(screen.contains("alice@127.0.0.1's password:"));
    }

    #[test]
    fn status_changes_are_recorded_for_the_connection_log() {
        let mut entry = SessionEntry::new_local(
            "t-log",
            &Palette::dark(),
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            local_params(),
        );
        // A local shell starts up and stays connected.
        entry.pump("xterm-256color");
        let statuses: Vec<_> = entry
            .status_history
            .iter()
            .map(|event| event.status)
            .collect();
        assert_eq!(statuses, vec![SessionStatus::Connected]);

        // Pumping again does not duplicate the entry.
        entry.pump("xterm-256color");
        assert_eq!(entry.status_history.len(), 1);

        // A drop is recorded once, with the reason.
        entry.status = SessionStatus::Failed;
        entry.error = Some("connection reset".to_owned());
        entry.pump("xterm-256color");
        let last = entry.status_history.last().expect("a second event");
        assert_eq!(last.status, SessionStatus::Failed);
        assert_eq!(last.message.as_deref(), Some("connection reset"));
        assert_eq!(entry.status_history.len(), 2);

        // Intermediate states stay out of the log.
        entry.status = SessionStatus::Authenticating;
        entry.pump("xterm-256color");
        assert_eq!(entry.status_history.len(), 2);
    }

    #[test]
    fn pane_commands_need_a_transport_that_can_run_them() {
        // An SSH tab that never reached its host has no shell to ask.
        let mut entry = password_entry();
        use wassh_plugins::CommandAccess as _;
        entry.exec("docker.list", "docker ps");
        assert!(!entry.is_running("docker.list"));
        assert_eq!(entry.failure("docker.list"), Some(COMMAND_TRANSPORT_ERROR));
        assert!(entry.output("docker.list").is_none());
    }

    /// Prints `local` on this platform's shell.
    #[cfg(unix)]
    const ECHO_LOCAL: &str = "printf 'local'";
    #[cfg(windows)]
    const ECHO_LOCAL: &str = "'local'";

    /// Echoes its stdin back on this platform's shell.
    #[cfg(unix)]
    const ECHO_STDIN: &str = "cat";
    #[cfg(windows)]
    const ECHO_STDIN: &str = "[Console]::In.ReadToEnd()";

    /// Runs far longer than any test waits.
    #[cfg(unix)]
    const SLOW_LOCAL: &str = "sleep 30";
    #[cfg(windows)]
    const SLOW_LOCAL: &str = "Start-Sleep -Seconds 30";

    #[test]
    fn a_running_command_can_be_stopped() {
        use wassh_plugins::CommandAccess as _;
        let mut entry = local_entry("t-local-stop");

        entry.exec("slow", SLOW_LOCAL);
        entry.cancel("slow");
        settle_command(&mut entry, "slow");

        let failure = entry.failure("slow").expect("the command reports why");
        assert!(failure.contains("stopped"), "{failure}");
        // Stopping it again is harmless: nothing is left to stop.
        entry.cancel("slow");
    }

    #[test]
    fn commands_run_on_this_machine_for_a_local_session() {
        use wassh_plugins::CommandAccess as _;
        let mut entry = local_entry("t-local-command");

        entry.exec("probe", ECHO_LOCAL);
        settle_command(&mut entry, "probe");
        let output = entry.output("probe").expect("output");
        assert_eq!(output.stdout.trim(), "local");
        assert_eq!(output.code, Some(0));
        assert!(entry.failure("probe").is_none());

        // stdin travels too, which is how `sudo -S` gets its password.
        entry.exec_input("stdin", ECHO_STDIN, "s3cret");
        settle_command(&mut entry, "stdin");
        assert_eq!(
            entry
                .output("stdin")
                .map(|output| output.stdout.trim().to_owned()),
            Some("s3cret".to_owned())
        );
    }

    #[test]
    fn a_monitor_needs_a_transport_that_can_be_sampled() {
        // Statistics are the pane's own probe now, so the pane needs a shell to run it in.
        let mut entry = password_entry();
        use wassh_plugins::MonitorAccess as _;
        assert!(!entry.native_sampling());
        assert!(entry.native_details(1).is_none());
        assert!(entry.native_stop(1, "TERM").is_none());
    }

    #[test]
    fn a_finished_command_is_cached_under_its_key() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut commands = RemoteCommands {
            pending: vec![("docker.list".to_owned(), rx)],
            ..RemoteCommands::default()
        };
        assert!(commands.is_running("docker.list"));
        tx.send(Ok(wassh_ssh::ExecOutput {
            stdout: "8f2c|nginx|nginx:1.25|running|Up 3 hours\n".to_owned(),
            stderr: String::new(),
            exit_status: Some(0),
        }))
        .expect("send");

        commands.poll();
        assert!(!commands.is_running("docker.list"));
        let result = commands.results.get("docker.list").expect("a result");
        let output = result.output.as_ref().expect("output");
        assert_eq!(output.code, Some(0));
        assert!(output.stdout.contains("nginx"));
        assert!(result.failure.is_none());
    }

    /// Parameters a local tab opens with.
    fn local_params() -> ConnectionParams {
        ConnectionParams {
            kind: ConnectionType::Local,
            ..ConnectionParams::default()
        }
    }

    /// A local tab, with a shell of its own.
    fn local_entry(id: &str) -> SessionEntry {
        SessionEntry::new_local(
            id,
            &Palette::dark(),
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            local_params(),
        )
    }

    /// Fresh directory under the system temp dir.
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "wassh-session-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    /// Poll the session until no file request or transfer is left.
    fn settle(entry: &mut SessionEntry) {
        use wassh_plugins::SftpAccess as _;
        for _ in 0..2_000 {
            entry.poll_files();
            let busy = entry.transfers().iter().any(|row| {
                matches!(row.state, TransferState::Queued | TransferState::Running)
            });
            if !busy && !entry.is_loading() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the file request never finished: {:?}", entry.error());
    }

    /// How long a test waits for a command it started to come back.
    ///
    /// The command runs in the platform's shell (`powershell.exe` on Windows), which can
    /// take seconds to start while the whole suite runs in parallel.
    const COMMAND_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
    /// Poll interval while waiting for a command.
    const COMMAND_POLL: std::time::Duration = std::time::Duration::from_millis(2);

    /// Poll the session until the command asked for under `key` has finished.
    fn settle_command(entry: &mut SessionEntry, key: &str) {
        use wassh_plugins::CommandAccess as _;
        let deadline = std::time::Instant::now() + COMMAND_WAIT;
        loop {
            entry.poll_commands();
            if !entry.is_running(key) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the command {key} never finished"
            );
            std::thread::sleep(COMMAND_POLL);
        }
    }

    #[test]
    fn files_are_available_for_ssh_and_local_sessions() {
        let mut entry = local_entry("t-no-files");
        // Neither telnet, serial nor a dead tab has a filesystem for the pane.
        entry.backend = Backend::Closed;
        use wassh_plugins::SftpAccess as _;
        entry.request_listing("/");
        assert_eq!(entry.error(), Some(FILES_TRANSPORT_ERROR));
        assert!(entry.listing().is_none());
        assert!(!entry.is_loading());

        entry.upload("/tmp/report.txt", "/home/alice/report.txt");
        assert_eq!(entry.error(), Some(FILES_TRANSPORT_ERROR));
        assert!(!entry.is_transferring());
        assert!(entry.resolve(".").is_none());
    }

    #[test]
    fn a_tree_download_runs_as_a_transfer() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("tree-src");
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
        std::fs::write(dir.join("a.txt"), b"one").expect("write");
        std::fs::write(dir.join("sub").join("b.txt"), b"two").expect("write");
        let target = temp_dir("tree-out");
        let mut entry = local_entry("t-local-tree");

        entry.download_tree(
            vec![dir.to_string_lossy().into_owned()],
            &target.to_string_lossy(),
        );
        // The pane lists it like any other transfer, so the download is visible.
        assert!(
            entry
                .transfers()
                .iter()
                .any(|row| row.direction == TransferDirection::Download),
            "the tree download is not in the transfer list"
        );
        settle(&mut entry);

        assert!(entry.error().is_none(), "{:?}", entry.error());
        let copied = target.join(dir.file_name().expect("a folder name"));
        assert_eq!(std::fs::read(copied.join("a.txt")).expect("read"), b"one");
        assert_eq!(
            std::fs::read(copied.join("sub").join("b.txt")).expect("read"),
            b"two"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn a_local_session_zips_a_folder() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("zip-src");
        std::fs::create_dir_all(dir.join("sub")).expect("mkdir");
        std::fs::write(dir.join("a.txt"), b"one").expect("write");
        std::fs::write(dir.join("sub").join("b.txt"), b"two").expect("write");
        // A link inside the folder points back at it: packing must not follow it.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, dir.join("loop")).expect("link");
        let archive = temp_dir("zip-out").join("out.zip");
        let mut entry = local_entry("t-local-zip");

        // The pane hands over the folder's entries, so the archive holds them at its root.
        let items: Vec<String> = std::fs::read_dir(&dir)
            .expect("read")
            .flatten()
            .map(|child| child.path().to_string_lossy().into_owned())
            .collect();
        entry.download_zip(items, &archive.to_string_lossy());
        settle(&mut entry);

        assert!(entry.error().is_none(), "{:?}", entry.error());
        let bytes = std::fs::read(&archive).expect("the archive was written");
        let names: Vec<String> = wassh_plugins::files::zip_entries(&bytes)
            .expect("a readable archive")
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert!(names.contains(&"a.txt".to_owned()), "{names:?}");
        assert!(names.contains(&"sub/b.txt".to_owned()), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("loop")),
            "the link was packed: {names:?}"
        );

        // An empty folder still produces a readable archive.
        let empty = temp_dir("zip-empty");
        let empty_archive = temp_dir("zip-empty-out").join("empty.zip");
        entry.download_zip(Vec::new(), &empty_archive.to_string_lossy());
        settle(&mut entry);
        assert!(
            empty_archive.is_file(),
            "an empty folder wrote no archive: {:?}",
            entry.transfer_status()
        );

        for dir in [&dir, &empty, archive.parent().expect("a parent")] {
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = std::fs::remove_dir_all(empty_archive.parent().expect("a parent"));
    }

    #[test]
    fn a_local_session_browses_this_machine() {
        use wassh_plugins::SftpAccess as _;
        let mut entry = local_entry("t-local-browse");
        // A local tab starts in the user's home directory.
        let start = entry.resolve(".").expect("a starting directory");
        assert_eq!(start, local_files::start_dir());

        entry.request_listing(&start);
        settle(&mut entry);
        let listing = entry.listing().expect("a listing").clone();
        assert_eq!(listing.path, start);
        assert!(entry.error().is_none(), "{:?}", entry.error());
        // Rows carry the full path the pane navigates to.
        for row in &listing.entries {
            assert_eq!(row.path, wassh_plugins::files::join_remote(&start, &row.name));
        }
    }

    #[test]
    fn a_local_session_moves_files_on_this_machine() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("move");
        let mut entry = local_entry("t-local-move");
        let folder = dir.to_string_lossy().into_owned();

        let source = dir.join("source.txt");
        std::fs::write(&source, b"hello").expect("write");
        entry.upload(
            &source.to_string_lossy(),
            &dir.join("report.txt").to_string_lossy(),
        );
        settle(&mut entry);
        assert!(entry.error().is_none(), "{:?}", entry.error());
        assert_eq!(
            std::fs::read(dir.join("report.txt")).expect("read"),
            b"hello"
        );
        assert!(entry.transfer_status().is_some(), "the pane shows the result");

        let back = dir.join("back.txt");
        entry.download(&format!("{folder}/report.txt"), &back.to_string_lossy());
        settle(&mut entry);
        assert!(entry.error().is_none(), "{:?}", entry.error());
        assert_eq!(std::fs::read(&back).expect("read"), b"hello");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_after_a_write_sees_the_new_bytes() {
        // The agent edits a file by reading it, writing the change back and reading it
        // again for the next edit. A read answered from the snapshot taken before the
        // write would hand the next edit the old text, and its write would undo the
        // first one.
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("stale-read");
        let mut entry = local_entry("t-local-stale-read");
        let path = dir.join("note.txt");
        let remote = path.to_string_lossy().into_owned();
        std::fs::write(&path, b"one").expect("write");

        assert!(entry.read(&remote, 32).is_none());
        settle(&mut entry);
        let read = entry.read(&remote, 32).expect("the file is read");
        assert_eq!(read.data, b"one");
        // A frame of the viewer is answered from the snapshot instead of the host.
        assert!(entry.read(&remote, 16).is_some());
        // A bigger read is not answered by a smaller snapshot.
        assert!(entry.read(&remote, 4096).is_none());
        settle(&mut entry);

        entry.write_file(&remote, b"two".to_vec());
        settle(&mut entry);
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
        assert!(entry.read(&remote, 32).is_none());
        settle(&mut entry);
        let read = entry.read(&remote, 32).expect("the file is read again");
        assert_eq!(read.data, b"two", "the read answered from the old snapshot");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finished_transfer_forgets_the_file_it_read() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            job: Some(FileJob::Transfer {
                done: "Edited /home/alice/report.txt".to_owned(),
            }),
            pending: Some(rx),
            content: Some((
                "/home/alice/report.txt".to_owned(),
                32,
                wassh_plugins::files::FileContent {
                    data: b"before".to_vec(),
                    truncated: false,
                },
            )),
            ..RemoteFiles::default()
        };
        tx.send(Ok(wassh_ssh::SftpOutcome::Transferred { bytes: 6 }))
            .expect("send");

        assert_eq!(files.poll(), None);
        assert!(
            files.content.is_none(),
            "the snapshot survived the transfer that replaced the file"
        );
    }

    #[test]
    fn a_local_session_runs_queued_transfers_one_after_another() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("queue");
        let mut entry = local_entry("t-local-queue");

        let first = dir.join("one.txt");
        let second = dir.join("two.txt");
        std::fs::write(&first, b"one").expect("write");
        std::fs::write(&second, b"two").expect("write");
        entry.upload_many(vec![
            (
                first.to_string_lossy().into_owned(),
                dir.join("one-out.txt").to_string_lossy().into_owned(),
            ),
            (
                second.to_string_lossy().into_owned(),
                dir.join("two-out.txt").to_string_lossy().into_owned(),
            ),
        ]);
        settle(&mut entry);
        assert!(entry.error().is_none(), "{:?}", entry.error());
        assert_eq!(
            std::fs::read(dir.join("one-out.txt")).expect("read"),
            b"one",
            "the first queued transfer never ran"
        );
        assert_eq!(std::fs::read(dir.join("two-out.txt")).expect("read"), b"two");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_session_creates_renames_and_deletes() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("ops");
        let folder = dir.to_string_lossy().into_owned();
        let mut entry = local_entry("t-local-ops");

        entry.mkdir(&format!("{folder}/docs"));
        settle(&mut entry);
        assert!(dir.join("docs").is_dir(), "{:?}", entry.error());

        entry.rename(&format!("{folder}/docs"), &format!("{folder}/papers"));
        settle(&mut entry);
        assert!(dir.join("papers").is_dir() && !dir.join("docs").exists());

        entry.remove(&format!("{folder}/papers"), true);
        settle(&mut entry);
        assert!(!dir.join("papers").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zip_shows_the_step_it_is_in_while_it_runs() {
        let dir = temp_dir("zip-row");
        std::fs::write(dir.join("a.txt"), b"one").expect("write");
        let archive = temp_dir("zip-row-out").join("out.zip");
        let (tx, rx) = std::sync::mpsc::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            active: Some((TransferDirection::Zip, archive.display().to_string())),
            job: Some(FileJob::Zip {
                dir: dir.clone(),
                archive: archive.clone(),
            }),
            pending: Some(rx),
            progress: Some(progress_rx),
            ..RemoteFiles::default()
        };
        progress_tx
            .send(wassh_ssh::SftpProgress {
                done: 3,
                total: Some(3),
                label: Some("a.txt".to_owned()),
                phase: wassh_ssh::SftpProgressPhase::Transfer,
                items: None,
            })
            .expect("send");
        // While the files are fetched the row names the file being downloaded.
        assert_eq!(files.poll(), None);
        let rows = files.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].direction, TransferDirection::Zip);
        assert_eq!(rows[0].state, TransferState::Running);
        assert_eq!(rows[0].path, "Downloading a.txt");

        tx.send(Ok(wassh_ssh::SftpOutcome::Transferred { bytes: 3 }))
            .expect("send");
        assert_eq!(files.poll(), None);
        // Once the pack thread starts, it begins with packing status before switching
        // to the archive-write message.
        let rows = files.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].path.starts_with("Packing ") || rows[0].path == "Packing files");
        // The pane keeps polling; the pack thread answers on a later frame.
        for _ in 0..2_000 {
            let _ = files.poll();
            if files.pack.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        assert!(archive.is_file(), "no archive was written: {:?}", files.error);
        assert!(files.error.is_none(), "{:?}", files.error);
        assert!(
            files.status.as_deref().is_some_and(|line| line.contains("out.zip")),
            "{:?}",
            files.status
        );
        // A saved archive needs no row: the bar goes away with the job.
        assert!(files.rows().is_empty(), "{:?}", files.rows());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(archive.parent().expect("a parent"));
    }

    #[test]
    fn zip_row_names_the_packing_and_archive_write_phases() {
        let archive = std::path::PathBuf::from("C:/downloads/out.zip");
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            job: Some(FileJob::Zip {
                dir: std::path::PathBuf::from("C:/temp/staging"),
                archive: archive.clone(),
            }),
            pack: Some(receiver),
            active: Some((TransferDirection::Zip, archive.display().to_string())),
            moved: Some(wassh_ssh::SftpProgress {
                done: 3,
                total: Some(7),
                label: Some("a.txt".to_owned()),
                phase: wassh_ssh::SftpProgressPhase::Packing,
                items: None,
            }),
            ..RemoteFiles::default()
        };

        assert_eq!(files.rows()[0].path, "Packing a.txt");
        files.moved = Some(wassh_ssh::SftpProgress {
            done: 0,
            total: Some(ZIP_PROGRESS_TEST_ARCHIVE_BYTES),
            label: None,
            phase: wassh_ssh::SftpProgressPhase::Writing,
            items: None,
        });
        let row = &files.rows()[0];
        assert_eq!(row.path, "Downloading out.zip");
        assert_eq!(row.done, 0, "the second phase must start from zero");

        drop(sender);
    }

    #[test]
    fn a_tree_scan_names_the_work_and_shows_items_inspected() {
        let files = RemoteFiles {
            request: Some(wassh_ssh::SftpRequest::DownloadTree {
                items: vec!["/srv/docs".to_owned()],
                dir: std::path::PathBuf::from("C:/downloads"),
            }),
            active: Some((TransferDirection::Download, "/downloads/docs".to_owned())),
            moved: Some(wassh_ssh::SftpProgress {
                done: 0,
                total: None,
                label: None,
                phase: wassh_ssh::SftpProgressPhase::Scanning,
                items: Some(TREE_SCAN_TEST_ITEMS),
            }),
            ..RemoteFiles::default()
        };

        let row = files.rows().remove(0);
        assert_eq!(row.path, "Scanning files");
        assert_eq!(row.progress_text.as_deref(), Some("128 items inspected"));
        assert_eq!(row.total, None, "a scan must not invent a byte total");
    }

    #[test]
    fn navigating_during_a_zip_scan_keeps_the_transfer_count() {
        use wassh_plugins::SftpAccess as _;
        let mut entry = local_entry("t-local-zip-navigation");
        let (answer_tx, answer_rx) = std::sync::mpsc::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        progress_tx
            .send(wassh_ssh::SftpProgress {
                done: 0,
                total: None,
                label: None,
                phase: wassh_ssh::SftpProgressPhase::Scanning,
                items: Some(TREE_SCAN_TEST_ITEMS),
            })
            .expect("send scan progress");
        let staging = temp_dir("zip-navigation-staging");
        let browser_dir = temp_dir("zip-navigation-browser");
        entry.files.pending = Some(answer_rx);
        entry.files.progress = Some(progress_rx);
        entry.files.stop = Some(wassh_ssh::SftpStop::default());
        entry.files.request = Some(wassh_ssh::SftpRequest::DownloadTree {
            items: vec!["/srv/docs".to_owned()],
            dir: staging.clone(),
        });
        entry.files.job = Some(FileJob::Zip {
            dir: staging,
            archive: std::path::PathBuf::from("C:/downloads/docs.zip"),
        });
        entry.files.active = Some((TransferDirection::Zip, "C:/downloads/docs.zip".to_owned()));

        entry.request_listing(&browser_dir.display().to_string());
        entry.poll_files();

        let rows = entry.transfers();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "Scanning files");
        assert_eq!(rows[0].progress_text.as_deref(), Some("128 items inspected"));
        assert!(entry.files.pending.is_some(), "navigation replaced the transfer job");
        assert_eq!(entry.files.requested.as_deref(), Some(browser_dir.to_str().expect("path")));

        drop(answer_tx);
        for _ in 0..1_000 {
            entry.poll_files();
            if entry.files.listing_pending.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = std::fs::remove_dir_all(&browser_dir);
    }

    #[test]
    fn stopping_any_transfer_signals_its_worker_and_hides_its_row() {
        use wassh_plugins::SftpAccess as _;
        let mut entry = local_entry("t-local-stop-transfer");
        for direction in [
            TransferDirection::Upload,
            TransferDirection::Download,
            TransferDirection::Zip,
            TransferDirection::Copy,
        ] {
            let stop = wassh_ssh::SftpStop::default();
            entry.files.active = Some((direction, "/downloads/docs".to_owned()));
            entry.files.stop = Some(stop.clone());
            entry.files.stop_requested = false;

            entry.stop_transfer();

            assert!(stop.is_stopped(), "{direction:?} worker was not signalled");
            assert!(entry.files.stop_requested);
            assert!(entry.transfers().is_empty());
        }
    }

    #[test]
    fn a_copy_batch_has_a_stoppable_transfer_row() {
        use wassh_plugins::SftpAccess as _;
        let dir = temp_dir("copy-stop-row");
        let source = dir.join("source.txt");
        let target = dir.join("target.txt");
        std::fs::write(&source, b"content").expect("write");
        let mut entry = local_entry("t-local-copy-stop-row");

        entry.copy_many(
            vec![(source.display().to_string(), target.display().to_string())],
            false,
        );

        let rows = entry.transfers();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].direction, TransferDirection::Copy);
        assert_eq!(rows[0].path, format!("Copying {} item(s)", rows.len()));
        entry.stop_transfer();
        assert!(entry.files.stop_requested);
        assert!(entry.files.stop.as_ref().is_some_and(|stop| stop.is_stopped()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn packing_reports_each_file_then_restarts_for_the_archive() {
        let dir = temp_dir("zip-progress");
        std::fs::write(dir.join("a.txt"), b"one").expect("write");
        std::fs::write(dir.join("b.txt"), b"two!").expect("write");
        let archive = temp_dir("zip-progress-out").join("out.zip");
        let (sender, receiver) = std::sync::mpsc::channel();

        pack_zip(
            &dir,
            &archive,
            Some(&sender),
            &wassh_ssh::SftpStop::default(),
        )
        .expect("archive written");
        drop(sender);

        let reports: Vec<_> = receiver.try_iter().collect();
        let scanning: Vec<_> = reports
            .iter()
            .filter(|report| report.phase == wassh_ssh::SftpProgressPhase::Scanning)
            .collect();
        assert_eq!(scanning.last().and_then(|report| report.items), Some(ZIP_SCAN_TEST_ITEMS));
        assert!(scanning.iter().all(|report| report.total.is_none()));
        let packing: Vec<_> = reports
            .iter()
            .filter(|report| report.phase == wassh_ssh::SftpProgressPhase::Packing)
            .collect();
        assert!(
            reports.iter().position(|report| report.phase == wassh_ssh::SftpProgressPhase::Scanning)
                < reports.iter().position(|report| report.phase == wassh_ssh::SftpProgressPhase::Packing),
            "packing started before the scan finished: {reports:?}"
        );
        assert!(
            packing.iter().any(|report| report.label.as_deref() == Some("a.txt")),
            "a.txt was not reported: {reports:?}"
        );
        assert!(
            packing.iter().any(|report| report.label.as_deref() == Some("b.txt")),
            "b.txt was not reported: {reports:?}"
        );
        assert_eq!(
            packing.last().map(|report| report.done),
            Some(ZIP_PROGRESS_TEST_BYTES)
        );

        let writing: Vec<_> = reports
            .iter()
            .filter(|report| report.phase == wassh_ssh::SftpProgressPhase::Writing)
            .collect();
        assert_eq!(writing.first().map(|report| report.done), Some(0));
        let finished = writing.last().expect("archive write progress");
        assert_eq!(finished.done, finished.total.expect("known archive size"));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(archive.parent().expect("a parent"));
    }

    #[test]
    fn stopping_zip_packing_does_not_leave_an_archive() {
        let dir = temp_dir("zip-stopped");
        std::fs::write(dir.join("a.txt"), b"content").expect("write");
        let archive = temp_dir("zip-stopped-out").join("out.zip");
        let stop = wassh_ssh::SftpStop::default();
        stop.stop();

        let result = pack_zip(&dir, &archive, None, &stop);

        assert_eq!(result, Err(wassh_ssh::SFTP_STOPPED.to_owned()));
        assert!(!archive.exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(archive.parent().expect("a parent"));
    }

    #[test]
    fn progress_keeps_moving_while_a_transfer_runs() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            job: Some(FileJob::Transfer {
                done: "/home/alice/report.txt".to_owned(),
            }),
            pending: Some(rx),
            progress: Some(progress_rx),
            ..RemoteFiles::default()
        };

        progress_tx
            .send(wassh_ssh::SftpProgress {
                done: 512,
                total: Some(1024),
                label: None,
                phase: wassh_ssh::SftpProgressPhase::Transfer,
                items: None,
            })
            .expect("send");
        assert_eq!(files.poll(), None);
        assert_eq!(
            files.moved.as_ref().map(|moved| moved.done),
            Some(512),
            "the pane has nothing to draw the bar from"
        );

        tx.send(Ok(wassh_ssh::SftpOutcome::Transferred { bytes: 512 }))
            .expect("send");
        files.poll();
        assert!(files.moved.is_none(), "a finished transfer keeps progress");
    }

    #[test]
    fn a_finished_transfer_reports_its_size_and_reloads_the_directory() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            requested: Some("/home/alice".to_owned()),
            job: Some(FileJob::Transfer {
                done: "Uploaded /home/alice/report.txt".to_owned(),
            }),
            pending: Some(rx),
            ..RemoteFiles::default()
        };
        tx.send(Ok(wassh_ssh::SftpOutcome::Transferred { bytes: 2048 }))
            .expect("send");

        assert_eq!(files.poll().as_deref(), Some("/home/alice"));
        assert_eq!(
            files.status.as_deref(),
            Some("Uploaded /home/alice/report.txt (2.0 KB)")
        );
        assert!(files.error.is_none());
        assert!(files.job.is_none());
        // Nothing is left to poll.
        assert_eq!(files.poll(), None);
    }

    #[test]
    fn a_listing_answer_fills_the_browser() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut files = RemoteFiles {
            listing_pending: Some(("/etc".to_owned(), rx)),
            ..RemoteFiles::default()
        };
        tx.send(Ok(wassh_ssh::SftpOutcome::Listed(vec![
            wassh_ssh::SftpEntry {
                name: "hosts".to_owned(),
                path: "/etc/hosts".to_owned(),
                is_dir: false,
                is_link: false,
                size: 220,
                modified: Some(1_700_000_000),
                permissions: Some(0o644),
            },
        ])))
        .expect("send");

        files.poll_listing();
        let listing = files.listing.expect("a listing");
        assert_eq!(listing.path, "/etc");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "hosts");
        assert_eq!(files.requested.as_deref(), Some("/etc"));
    }

    #[test]
    fn a_failed_transfer_keeps_the_message() {
        let (tx, rx) = std::sync::mpsc::channel::<Result<wassh_ssh::SftpOutcome, String>>();
        let mut files = RemoteFiles {
            job: Some(FileJob::Transfer {
                done: "Saved /tmp/hosts".to_owned(),
            }),
            pending: Some(rx),
            ..RemoteFiles::default()
        };
        tx.send(Err("reading /etc/shadow: permission denied".to_owned()))
            .expect("send");

        assert_eq!(files.poll(), None);
        assert_eq!(
            files.error.as_deref(),
            Some("reading /etc/shadow: permission denied")
        );
        assert!(files.status.is_none());
        assert!(files.job.is_none());
    }

    #[test]
    fn plugin_data_is_scoped_to_the_saved_host() {
        let mut entry = password_entry();
        assert_eq!(entry.plugin_scope(), "tab:t-prompt");
        entry.host_id = Some("host-7".to_owned());
        assert_eq!(entry.plugin_scope(), "host-7");
    }

    #[test]
    fn legacy_shared_scratchpad_is_claimed_by_one_scope() {
        let dir = temp_dir("legacy-scratchpad");
        let config = wassh_store::Config::open(&dir).unwrap();
        config.save_plugin_data("scratchpad", None, &serde_json::json!({"text":"legacy notes"})).unwrap();
        let mut first = password_entry();
        first.host_id = Some("first".to_owned());
        let slot = first.ensure_pane(PluginId::Scratchpad);
        first.load_pane_data(slot, Some(&config), &PluginValues::default());
        assert_eq!(first.panes[slot].data.values["text"], "legacy notes");
        assert_eq!(config.plugin_data("scratchpad", Some("first")).unwrap().unwrap()["text"], "legacy notes");
        assert_eq!(config.plugin_data("scratchpad", None).unwrap(), Some(serde_json::Value::Null));
        let mut second = password_entry();
        second.host_id = Some("second".to_owned());
        let slot = second.ensure_pane(PluginId::Scratchpad);
        second.load_pane_data(slot, Some(&config), &PluginValues::default());
        assert!(!second.panes[slot].data.values.contains_key("text"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opening_a_plugin_docks_it_and_closing_removes_it() {
        let mut entry = password_entry();
        entry.open_plugin(wassh_plugins::PluginId::Scratchpad);
        assert!(entry.layout.contains("scratchpad"));
        // The manifest docks panes on the right until a user moves them.
        assert!(entry.layout.contains("scratchpad"));
        entry.close_plugin("scratchpad");
        assert!(!entry.layout.contains("scratchpad"));
        assert!(entry.layout.is_empty());
    }

    #[test]
    fn changed_plugin_data_is_reported_once() {
        let mut entry = password_entry();
        let slot = entry.ensure_pane(wassh_plugins::PluginId::Scratchpad);
        entry.load_pane_data(slot, None, &PluginValues::default());
        assert!(entry.take_dirty_plugins().is_empty(), "no edits yet");

        entry.panes[slot].data.values.insert(
            wassh_plugins::plugin::SCRATCHPAD_TEXT_KEY.to_owned(),
            serde_json::json!("keep me"),
        );
        let dirty = entry.take_dirty_plugins();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].0, wassh_plugins::PluginId::Scratchpad);
        assert_eq!(dirty[0].1, "tab:t-prompt");
        assert_eq!(
            dirty[0].2.get("text"),
            Some(&serde_json::json!("keep me"))
        );
        assert!(entry.take_dirty_plugins().is_empty(), "reported only once");
    }

    fn plugin_map(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn panes_start_from_the_plugin_values_the_dialogs_store() {
        let mut entry = password_entry();
        let mut values = PluginValues::default();
        values.host.insert(
            "connection-logger".to_owned(),
            plugin_map(&[
                ("enabled", serde_json::json!(false)),
                ("maxEntries", serde_json::json!(25)),
            ]),
        );
        let slot = entry.ensure_pane(wassh_plugins::PluginId::ConnectionLogger);
        entry.load_pane_data(slot, None, &values);
        let settings = &entry.panes[slot].data.values;
        assert_eq!(settings["enabled"], serde_json::json!(false));
        assert_eq!(settings["maxEntries"], serde_json::json!(25));
        // Those values belong to the dialog, so loading them is not an edit.
        assert!(entry.take_dirty_plugins().is_empty());
    }

    #[test]
    fn the_ai_agent_pane_sees_the_providers_configured_in_options() {
        // The provider list is edited in Options — with its own widget, so it has no
        // settings row — and used to be filtered out on the way to the pane, which then
        // kept saying no provider was configured.
        let mut entry = password_entry();
        let slot = entry.ensure_pane(wassh_plugins::PluginId::AiAgent);
        let mut values = PluginValues::default();
        values.app.insert(
            "ai-agent".to_owned(),
            plugin_map(&[(
                wassh_plugins::ai_agent::PROVIDERS_KEY,
                serde_json::json!([{ "id": "ollama", "name": "Ollama", "protocol": "openai",
                                     "baseUrl": "http://127.0.0.1:11434/v1",
                                     "models": ["llama3.1"] }]),
            )]),
        );
        entry.load_pane_data(slot, None, &values);
        let providers = entry.panes[slot].data.values
            [wassh_plugins::ai_agent::PROVIDERS_KEY]
            .as_array()
            .expect("the pane got the configured providers");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0]["id"], serde_json::json!("ollama"));
        // Loading what the dialog owns is not a pane edit.
        assert!(entry.take_dirty_plugins().is_empty());
    }

    #[test]
    fn an_adopted_app_setting_is_not_pane_data() {
        let mut entry = password_entry();
        let slot = entry.ensure_pane(wassh_plugins::PluginId::AiAgent);
        entry.load_pane_data(slot, None, &PluginValues::default());

        // A refreshed provider list comes back from the host as an app value.
        let providers = serde_json::json!([{ "id": "one", "models": ["a"] }]);
        entry.adopt_app_setting(wassh_plugins::PluginId::AiAgent, "providers", providers.clone());
        assert_eq!(entry.panes[slot].data.values["providers"], providers);
        assert!(
            entry.take_dirty_plugins().is_empty(),
            "adopting an app value is not an edit of the pane's own data"
        );
    }

    #[test]
    fn seeded_values_stay_out_of_the_plugins_own_data_file() {
        let mut entry = password_entry();
        let mut app = wassh_core::PluginSettings::new();
        app.insert(
            "docker-manager".to_owned(),
            plugin_map(&[("confirmDestructive", serde_json::json!(false))]),
        );
        let values = PluginValues {
            app,
            host: wassh_core::PluginSettings::new(),
            secrets: Default::default(),
        };
        let slot = entry.ensure_pane(wassh_plugins::PluginId::DockerManager);
        entry.load_pane_data(slot, None, &values);
        assert_eq!(
            entry.panes[slot].data.values["confirmDestructive"],
            serde_json::json!(false)
        );

        // The pane writes something of its own: only that is persisted.
        entry.panes[slot]
            .data
            .values
            .insert("note".to_owned(), serde_json::json!("hello"));
        let dirty = entry.take_dirty_plugins();
        assert_eq!(dirty.len(), 1);
        let payload = &dirty[0].2;
        assert_eq!(payload.get("note"), Some(&serde_json::json!("hello")));
        assert!(
            !payload.contains_key("confirmDestructive"),
            "dialog values are not copied into the plugin data file"
        );
    }

    #[test]
    fn reloading_pane_settings_picks_up_new_values() {
        let mut entry = password_entry();
        let slot = entry.ensure_pane(wassh_plugins::PluginId::ConnectionLogger);
        entry.load_pane_data(slot, None, &PluginValues::default());
        assert_eq!(entry.panes[slot].data.values["maxEntries"], serde_json::json!(500));

        let mut values = PluginValues::default();
        values.host.insert(
            "connection-logger".to_owned(),
            plugin_map(&[("maxEntries", serde_json::json!(10))]),
        );
        entry.reload_pane_settings();
        entry.load_pane_data(slot, None, &values);
        assert_eq!(entry.panes[slot].data.values["maxEntries"], serde_json::json!(10));
    }

    #[test]
    fn escape_cancels_the_prompt() {
        let mut entry = password_entry();
        let outcome = entry.login_input(
            &[wassh_ui::TerminalInput::Key(wassh_term::KeyStroke {
                key: Key::Escape,
                ctrl: false,
                alt: false,
                shift: false,
            })],
            "xterm-256color",
        );
        assert_eq!(outcome, LoginOutcome::Cancelled);
        assert!(!entry.awaiting_login());
        assert!(entry.login.is_none());
        assert_eq!(entry.error.as_deref(), Some("Login cancelled."));
    }

    #[test]
    fn serial_sessions_report_the_device_they_could_not_open() {
        let palette = Palette::dark();
        let mut entry = SessionEntry::new_local(
            "t3",
            &palette,
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            local_params(),
        );
        entry.connection.kind = ConnectionType::Serial;
        entry.connection.serial.path = "/dev/wassh-missing".to_owned();
        entry.restart(
            "xterm-256color",
            &Secrets::default(),
            Arc::new(KnownHosts::in_memory()),
        );
        assert_eq!(entry.status, SessionStatus::Failed);
        assert!(
            entry
                .error
                .as_deref()
                .is_some_and(|error| error.contains("/dev/wassh-missing")),
            "error was {:?}",
            entry.error
        );
    }

    fn jump_hosts_doc() -> wassh_core::HostsDocument {
        let make = |id: &str, host: &str, jump: Option<&str>, auth: AuthMethod| {
            wassh_core::HostProfile {
                id: id.to_owned(),
                name: id.to_owned(),
                connection: ConnectionParams {
                    kind: ConnectionType::Ssh,
                    host: host.to_owned(),
                    port: 22,
                    username: "alice".to_owned(),
                    auth,
                    private_key_path: (auth == AuthMethod::PrivateKey)
                        .then(|| "/home/alice/.ssh/id_ed25519".to_owned()),
                    jump_host_id: jump.map(str::to_owned),
                    ..ConnectionParams::default()
                },
                ..wassh_core::HostProfile::default()
            }
        };
        wassh_core::HostsDocument {
            hosts: vec![
                make("bastion", "bastion.example.com", None, AuthMethod::None),
                make("inner", "inner.example.com", Some("bastion"), AuthMethod::PrivateKey),
                make("target", "target.example.com", Some("inner"), AuthMethod::None),
                make("locked", "locked.example.com", None, AuthMethod::Password),
            ],
            ..wassh_core::HostsDocument::default()
        }
        .normalized()
    }

    #[test]
    fn shared_identity_resolves_current_credentials_for_hosts() {
        let dir = std::env::temp_dir().join(format!("wassh-identity-{}", std::process::id()));
        let mut vault = wassh_store::SecretsStore::open(&dir).unwrap();
        vault.set_identity_password("shared", Some("first")).unwrap();
        let mut identity = wassh_core::Identity {
            id: "shared".to_owned(), name: "Shared".to_owned(), username: "alice".to_owned(),
            ..Default::default()
        };
        let params = ConnectionParams {
            username: "individual".to_owned(), identity_id: Some("shared".to_owned()),
            ..Default::default()
        };
        for host_id in ["first-host", "second-host"] {
            let (resolved, secrets) = resolve_auth(&params, Some(host_id), &[identity.clone()], Some(&vault)).unwrap();
            assert_eq!(resolved.username, "alice");
            assert_eq!(secrets.password, "first");
            assert_eq!(params.username, "individual");
        }
        identity.username = "bob".to_owned();
        vault.set_identity_password("shared", Some("second")).unwrap();
        let (resolved, secrets) = resolve_auth(&params, None, &[identity.clone()], Some(&vault)).unwrap();
        assert_eq!(resolved.username, "bob");
        assert_eq!(secrets.password, "second");
        identity.auth = AuthMethod::PrivateKey;
        identity.private_key_path = Some("/key".to_owned());
        vault.set_identity_passphrase("shared", Some("unlock")).unwrap();
        let (resolved, secrets) = resolve_auth(&params, None, &[identity], Some(&vault)).unwrap();
        assert_eq!(resolved.auth, AuthMethod::PrivateKey);
        assert_eq!(resolved.private_key_path.as_deref(), Some("/key"));
        assert_eq!(secrets.key_passphrase, "unlock");
        let agent = wassh_core::Identity { id: "shared".to_owned(), auth: AuthMethod::None, ..Default::default() };
        let (resolved, _) = resolve_auth(&params, None, &[agent], Some(&vault)).unwrap();
        assert_eq!(resolved.auth, AuthMethod::None);
        assert!(resolve_auth(&params, None, &[], Some(&vault)).is_err());
        drop(vault);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_host_used_as_a_jump_uses_its_shared_identity() {
        let dir = std::env::temp_dir().join(format!("wassh-identity-jump-{}", std::process::id()));
        let mut vault = wassh_store::SecretsStore::open(&dir).unwrap();
        vault.set_identity_password("shared", Some("secret")).unwrap();
        let identity = wassh_core::Identity { id: "shared".to_owned(), username: "bob".to_owned(), ..Default::default() };
        let mut hosts = jump_hosts_doc();
        hosts.hosts.iter_mut().find(|host| host.id == "locked").unwrap().connection.identity_id = Some("shared".to_owned());
        let mut params = hosts.host("target").unwrap().connection.clone();
        params.jump_host_id = Some("locked".to_owned());
        let chain = jump_chain(&hosts, &params, &[identity], Some(&vault)).unwrap();
        assert_eq!(chain[0].username, "bob");
        assert!(matches!(&chain[0].auth, wassh_ssh::SshAuth::Password(value) if value == "secret"));
        drop(vault);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn jump_chains_are_resolved_outermost_first() {
        let hosts = jump_hosts_doc();
        let params = hosts.host("target").unwrap().connection.clone();
        let chain = jump_chain(&hosts, &params, &[], None).expect("chain");
        assert_eq!(
            chain.iter().map(|hop| hop.host.as_str()).collect::<Vec<_>>(),
            vec!["bastion.example.com", "inner.example.com"]
        );
        assert!(matches!(chain[0].auth, wassh_ssh::SshAuth::Agent));
        assert!(matches!(chain[1].auth, wassh_ssh::SshAuth::PrivateKey { .. }));
    }

    #[test]
    fn hosts_without_a_jump_host_have_an_empty_chain() {
        let hosts = jump_hosts_doc();
        let params = hosts.host("bastion").unwrap().connection.clone();
        assert!(jump_chain(&hosts, &params, &[], None).unwrap().is_empty());
    }

    #[test]
    fn password_jump_hosts_use_the_saved_password() {
        let hosts = jump_hosts_doc();
        let mut params = hosts.host("target").unwrap().connection.clone();
        params.jump_host_id = Some("locked".to_owned());

        let dir = std::env::temp_dir().join(format!("wassh-jump-{}", std::process::id()));
        let mut vault = wassh_store::SecretsStore::open(&dir).unwrap();
        vault.set_password("locked", Some("hunter2")).unwrap();

        let chain = jump_chain(&hosts, &params, &[], Some(&vault)).expect("chain");
        assert!(matches!(
            &chain[0].auth,
            wassh_ssh::SshAuth::Password(password) if password == "hunter2"
        ));
        drop(vault);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn password_jump_hosts_need_a_saved_password() {
        let hosts = jump_hosts_doc();
        let mut params = hosts.host("target").unwrap().connection.clone();
        params.jump_host_id = Some("locked".to_owned());
        let error = jump_chain(&hosts, &params, &[], None).unwrap_err();
        assert!(error.contains("locked"), "error was {error}");
        assert!(error.contains("password"));
    }

    #[test]
    fn jump_loops_are_detected() {
        let mut hosts = jump_hosts_doc();
        let first = hosts.hosts.iter_mut().find(|host| host.id == "bastion").unwrap();
        first.connection.jump_host_id = Some("inner".to_owned());
        let params = hosts.host("target").unwrap().connection.clone();
        let error = jump_chain(&hosts, &params, &[], None).unwrap_err();
        assert!(error.contains("loop"), "error was {error}");
    }

    #[test]
    fn missing_jump_hosts_are_reported() {
        let hosts = jump_hosts_doc();
        let mut params = hosts.host("target").unwrap().connection.clone();
        params.jump_host_id = Some("ghost".to_owned());
        let error = jump_chain(&hosts, &params, &[], None).unwrap_err();
        assert!(error.contains("ghost"), "error was {error}");
    }

    #[test]
    fn telnet_sessions_start_connecting() {
        let palette = Palette::dark();
        let params = ConnectionParams {
            kind: ConnectionType::Telnet,
            host: "127.0.0.1".to_owned(),
            port: 23,
            ..ConnectionParams::default()
        };
        let entry = SessionEntry::new_telnet(
            "t4",
            &palette,
            &SessionStyleDefaults::default(),
            "xterm-256color",
            TerminalSize::new(80, 24),
            params,
            None,
        );
        assert_eq!(entry.title, "telnet 127.0.0.1:23");
        assert!(matches!(
            entry.status,
            SessionStatus::Connecting | SessionStatus::Connected
        ));
    }

    #[test]
    fn open_sessions_are_counted_per_saved_host() {
        let sessions = [Some("alpha"), None, Some("beta"), Some("alpha")];
        assert_eq!(
            open_host_sessions(sessions.into_iter()),
            vec![("alpha".to_owned(), 2), ("beta".to_owned(), 1)]
        );
        assert!(open_host_sessions(std::iter::empty()).is_empty());
    }
}
