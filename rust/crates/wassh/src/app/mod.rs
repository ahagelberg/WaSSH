//! The application shell.
//!
//! Owns the window chrome, the sidebar, the tab strip and the terminal area, plus every
//! dialog and the session list. State that survives a restart lives in `wassh-store`;
//! everything else is rebuilt on launch.

pub mod dialogs;
pub mod quick_connect;
pub mod session;

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use egui::{Key, Modifiers, Pos2, Vec2};

use wassh_core::{
    APP_NAME, AppSettings, AuthMethod, ConnectionParams, ConnectionType, HostProfile,
    HostsDocument, Palette, SessionStatus, WindowBounds, colors,
};
use wassh_plugins::PluginId;
use wassh_ssh::KnownHosts;
use wassh_store::Config;
use wassh_term::TerminalSize;
use wassh_ui::widgets::{Button, MenuItem, MenuState, ToastKind, Toasts};

use self::dialogs::{
    Command, DialogAction, Dialogs, FindCommand, FindState, GroupEditState, HostSave, HostState,
    HostTarget, OptionsState, PaletteState, PluginSecretDrafts, SecretEdit, tab_menu_command,
    tab_menu_items,
};
use self::quick_connect::{QuickForm, quick_connect};
use self::session::{PluginValues, Secrets, SessionEntry, SshOpen, jump_chain};

/// How long the window must stay put before its geometry is written to `settings.json`.
const WINDOW_BOUNDS_SETTLE: Duration = Duration::from_millis(700);

/// Menus shown in the titlebar.
/// Toolbar labels of the Plugins menu entries, in registry order.
const PLUGIN_MENU_LABELS: &[&str] = &[
    "Monitor",
    "Scratch",
    "Macros",
    "MQTT",
    "Files",
    "AI Agent",
    "Conn Log",
    "Docker",
];

const MENUS: &[wassh_ui::titlebar::Menu] = &[
    (
        "File",
        &[
            "New tab",
            "New host…",
            "Reopen closed tab",
            "",
            "Options…",
            "Quit",
        ],
    ),
    ("Edit", &["Copy", "Paste", "", "Find…"]),
    (
        "Session",
        &[
            "Reconnect",
            "Reconnect all",
            "",
            "Session settings…",
            "Tunnels…",
        ],
    ),
    (
        "View",
        &[
            "Zoom In",
            "Zoom Out",
            "Reset zoom",
            "",
            "Toggle sidebar",
            "Toggle Full Screen",
        ],
    ),
    ("Plugins", PLUGIN_MENU_LABELS),
    ("Help", &["About WaSSH"]),
];

/// A tab that was closed and can be reopened.
#[derive(Clone, Debug)]
struct ClosedTab {
    connection: ConnectionParams,
    host_id: Option<String>,
}

/// The whole application.
pub struct Shell {
    config: Option<Config>,
    /// Encrypted per-host passwords and key passphrases.
    secrets: Option<wassh_store::SecretsStore>,
    hosts: HostsDocument,
    settings: AppSettings,
    palette: Palette,
    sessions: Vec<SessionEntry>,
    active: Option<usize>,
    sidebar: wassh_ui::SidebarState,
    tabs: wassh_ui::TabBarState,
    menu: MenuState,
    menu_target: Option<String>,
    toasts: Toasts,
    dialogs: Dialogs,
    quick: QuickForm,
    closed: Vec<ClosedTab>,
    known_hosts: Arc<KnownHosts>,
    fullscreen: bool,
    fullscreen_request: Option<bool>,
    zoom: f32,
    next_id: u64,
    find_matches: usize,
    find_index: usize,
    last_terminal_size: TerminalSize,
    quit_requested: bool,
    /// Plugin being dragged to a dock zone, from the toolbar or a pane header.
    dock_drag: Option<wassh_ui::DockDrag>,
    /// When the process asked for the window, for the cold-start measurement.
    started_at: std::time::Instant,
    /// Time from start to the first finished frame, once measured.
    first_frame_ms: Option<f64>,
    /// Frames drawn so far; `--perf` stops after the warm-up.
    frames: u32,
    /// Close the window and print the numbers once the warm-up is over.
    perf_probe: bool,
    /// Tabs from the last run are reopened after the first frame, so the window shows up
    /// immediately however many shells that means spawning.
    restore_pending: bool,
    /// Local shells `--perf` opens after the first frame, to measure memory with tabs.
    perf_tabs: usize,
    /// Last desktop light/dark preference seen, so `ThemeMode::System` can follow it.
    system_dark: Option<bool>,
    /// Palette egui's own visuals were last built from.
    styled_palette: Option<Palette>,
    /// Normal (non-maximized) window geometry from the last frame, kept while maximized so
    /// unmaximizing and the next launch still land on the same rectangle.
    last_bounds: Option<WindowBounds>,
    /// Saved position to apply once the real window exists.
    restore_bounds: Option<WindowBounds>,
    /// When the window geometry last changed, so a drag is written once it settles.
    bounds_changed_at: Option<std::time::Instant>,
}

/// Whether the desktop is currently in its dark variant.
///
/// The window system reports this through egui; when it says nothing (some bare X11 setups)
/// dark is assumed, matching the app's default look.
fn system_prefers_dark(ctx: &egui::Context) -> bool {
    ctx.system_theme()
        .map(|theme| theme == egui::Theme::Dark)
        .unwrap_or(true)
}

/// When `main` started, for the cold-start measurement.
static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Record that the process is starting, so `--perf` measures from here.
pub fn mark_process_start() {
    let _ = PROCESS_START.set(std::time::Instant::now());
}

/// The user's home directory, or the working directory when `HOME` is unset.
fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Expand a leading `~/` in a typed path.
fn expand_home(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => std::path::PathBuf::from(path),
    }
}

/// Run the application.
pub fn run() -> eframe::Result<()> {
    run_app(false, 0)
}

/// Run the application, print the cold-start and memory numbers, then close.
///
/// `tabs` opens that many local shells so memory can be measured with panes on screen; they
/// live in a throwaway configuration and are never saved.
pub fn run_perf(tabs: usize) -> eframe::Result<()> {
    run_app(true, tabs)
}

fn run_app(perf: bool, perf_tabs: usize) -> eframe::Result<()> {
    let config = if perf {
        None
    } else {
        Config::open_data_dir().ok()
    };
    let bounds = config
        .as_ref()
        .and_then(|config| config.settings.get().window_bounds);

    // The desktop draws the window: title bar, caption buttons, resize border, shadow and
    // rounded corners all come from the theme, so the app only puts its menu strip inside.
    let mut viewport = egui::ViewportBuilder::default()
        .with_min_inner_size([800.0, 500.0])
        .with_title(APP_NAME)
        .with_app_id("wassh");
    viewport = match bounds {
        // `width`/`height` are points, so the window keeps its apparent size on a monitor
        // with different scaling. The position is applied on the first frame, in pixels.
        Some(bounds) => viewport
            .with_inner_size([bounds.width as f32, bounds.height as f32])
            .with_maximized(bounds.maximized),
        None => viewport.with_inner_size([1280.0, 800.0]),
    };

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "wassh",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Shell::new_with_probe(
                cc, config, perf, perf_tabs, bounds,
            )))
        }),
    )
}

impl Shell {
    /// Build the shell for a window.
    ///
    /// A `--perf` probe works on an empty, in-memory configuration: a measurement must never
    /// read or write the real hosts, settings or saved tabs.
    fn new_with_probe(
        cc: &eframe::CreationContext<'_>,
        config: Option<Config>,
        perf: bool,
        perf_tabs: usize,
        bounds: Option<WindowBounds>,
    ) -> Self {
        let mut shell = Self::new_for_test(config, perf, perf_tabs);
        shell.last_bounds = bounds.map(|bounds| WindowBounds {
            maximized: false,
            ..bounds
        });
        shell.restore_bounds = bounds.filter(|bounds| !bounds.maximized);
        let system_dark = system_prefers_dark(&cc.egui_ctx);
        shell.system_dark = Some(system_dark);
        if shell.settings.theme == wassh_core::ThemeMode::System {
            shell.palette = Palette::for_mode(shell.settings.theme.resolve(system_dark));
        }
        shell
    }

    /// Build the shell against `config`, without touching the editor context.
    #[cfg(test)]
    fn new_with(config: Option<Config>, perf: bool) -> Self {
        Self::new_for_test(config, perf, 0)
    }

    /// The constructor every other one funnels through.
    fn new_for_test(config: Option<Config>, perf: bool, perf_tabs: usize) -> Self {
        let settings = config
            .as_ref()
            .map(|config| config.settings.get().clone())
            .unwrap_or_default();
        let hosts = config
            .as_ref()
            .map(|config| config.hosts.get().clone())
            .unwrap_or_default();
        // Provisional: `Shell::new` re-resolves this against the desktop's preference.
        let palette = Palette::for_mode(settings.theme.resolve(true));

        let known_hosts = match wassh_store::data_dir() {
            Ok(dir) if !perf => KnownHosts::load(wassh_store::known_hosts_file(dir)),
            _ => KnownHosts::in_memory(),
        };

        let mut sidebar = wassh_ui::SidebarState::new();
        sidebar.collapsed = settings.sidebar_collapsed;

        let secrets = config
            .as_ref()
            .and_then(|config| wassh_store::SecretsStore::open(&config.dir).ok());
        let mut shell = Self {
            config,
            secrets,
            hosts,
            settings,
            palette,
            sessions: Vec::new(),
            active: None,
            sidebar,
            tabs: wassh_ui::TabBarState::new(),
            menu: MenuState::new(),
            menu_target: None,
            toasts: Toasts::new(),
            dialogs: Dialogs::default(),
            quick: QuickForm::default(),
            closed: Vec::new(),
            known_hosts: Arc::new(known_hosts),
            fullscreen: false,
            fullscreen_request: None,
            zoom: 0.0,
            next_id: 1,
            find_matches: 0,
            find_index: 0,
            last_terminal_size: TerminalSize::new(80, 24),
            quit_requested: false,
            dock_drag: None,
            started_at: PROCESS_START
                .get()
                .copied()
                .unwrap_or_else(std::time::Instant::now),
            first_frame_ms: None,
            frames: 0,
            perf_probe: perf,
            restore_pending: true,
            system_dark: None,
            styled_palette: None,
            last_bounds: None,
            restore_bounds: None,
            bounds_changed_at: None,
            perf_tabs,
        };
        for problem in shell
            .config
            .as_ref()
            .map(|config| config.problems.clone())
            .unwrap_or_default()
        {
            shell.toasts.push(
                ToastKind::Error,
                format!("could not read configuration: {problem}"),
            );
        }
        shell.migrate_plugin_secrets();
        shell
    }

    // --- tab persistence ---------------------------------------------------

    fn tab_snapshots(&self) -> Vec<wassh_core::TabSnapshot> {
        self.sessions
            .iter()
            .enumerate()
            .map(|(index, session)| wassh_core::TabSnapshot {
                id: session.id.clone(),
                connection: session.connection.clone(),
                host_id: session.host_id.clone(),
                active: self.active == Some(index),
                active_plugin_ids: session.layout.plugin_ids(),
                plugin_layout: (!session.layout.is_empty()).then(|| session.layout.clone()),
            })
            .collect()
    }

    fn persist_tabs(&mut self) {
        let snapshots = self.tab_snapshots();
        if let Some(config) = self.config.as_mut() {
            if let Err(error) = config.tabs.set(snapshots) {
                self.toasts
                    .push(ToastKind::Error, format!("could not stage tabs: {error}"));
                return;
            }
            if let Err(error) = config.tabs.save() {
                self.toasts
                    .push(ToastKind::Error, format!("could not save tabs: {error}"));
            }
        }
    }

    fn restore_tabs(&mut self) {
        if !self.settings.reconnect_on_startup || !self.sessions.is_empty() {
            // Tabs are only ever restored into an empty window.
            return;
        }
        let tabs = self
            .config
            .as_ref()
            .map(|config| config.tabs.get().clone())
            .unwrap_or_default();
        if tabs.is_empty() {
            return;
        }
        let active_id = tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.id.clone());
        for tab in tabs {
            // Keep generated ids clear of the ones coming back from disk.
            if let Some(number) = tab
                .id
                .strip_prefix("tab-")
                .and_then(|suffix| suffix.parse::<u64>().ok())
            {
                self.next_id = self.next_id.max(number + 1);
            }
            self.restore_session(tab.id, tab.connection, tab.host_id, tab.plugin_layout);
        }
        if let Some(active) = active_id
            && let Some(index) = self.sessions.iter().position(|session| session.id == active)
        {
            self.active = Some(index);
        }

    }

    fn restore_session(
        &mut self,
        id: String,
        connection: ConnectionParams,
        host_id: Option<String>,
        layout: Option<wassh_core::TabPluginLayout>,
    ) {
        match connection.kind {
            ConnectionType::Local => {
                let entry = SessionEntry::new_local(
                    id,
                    &self.palette,
                    &self.settings.session_style_defaults,
                    &self.settings.term_type,
                    self.last_terminal_size,
                );
                self.sessions.push(entry);
                if let (Some(session), Some(layout)) =
                    (self.sessions.last_mut(), layout.clone())
                {
                    session.restore_layout(layout);
                }
                self.active = Some(self.sessions.len() - 1);
            }
            ConnectionType::Ssh => {
                let jumps = jump_chain(&self.hosts, &connection).unwrap_or_default();
                let mut secrets = Secrets::default();
                if connection.auth == AuthMethod::Password {
                    secrets.password = self
                        .stored_password(host_id.as_deref())
                        .unwrap_or_default();
                }
                let open = SshOpen {
                    params: connection,
                    host_id,
                    secrets,
                    known_hosts: self.known_hosts.clone(),
                    jumps,
                };
                // With no stored password the tab reopens and asks in the terminal.
                let entry = if open.params.auth == AuthMethod::Password
                    && open.secrets.password.is_empty()
                {
                    SessionEntry::new_ssh_prompt(
                        id,
                        &self.palette,
                        &self.settings.session_style_defaults,
                        self.last_terminal_size,
                        open,
                    )
                } else {
                    SessionEntry::new_ssh(
                        id,
                        &self.palette,
                        &self.settings.session_style_defaults,
                        &self.settings.term_type,
                        self.last_terminal_size,
                        open,
                    )
                };
                self.sessions.push(entry);
                if let (Some(session), Some(layout)) =
                    (self.sessions.last_mut(), layout.clone())
                {
                    session.restore_layout(layout);
                }
                self.active = Some(self.sessions.len() - 1);
            }
            ConnectionType::Telnet => {
                let entry = SessionEntry::new_telnet(
                    id,
                    &self.palette,
                    &self.settings.session_style_defaults,
                    &self.settings.term_type,
                    self.last_terminal_size,
                    connection,
                    host_id,
                );
                self.sessions.push(entry);
                if let (Some(session), Some(layout)) =
                    (self.sessions.last_mut(), layout.clone())
                {
                    session.restore_layout(layout);
                }
                self.active = Some(self.sessions.len() - 1);
            }
            ConnectionType::Serial => {
                let entry = SessionEntry::new_serial(
                    id,
                    &self.palette,
                    &self.settings.session_style_defaults,
                    self.last_terminal_size,
                    connection,
                    host_id,
                );
                self.sessions.push(entry);
                if let (Some(session), Some(layout)) =
                    (self.sessions.last_mut(), layout.clone())
                {
                    session.restore_layout(layout);
                }
                self.active = Some(self.sessions.len() - 1);
            }
        }
    }

    // --- session lifecycle -------------------------------------------------

    fn next_session_id(&mut self) -> String {
        let id = format!("tab-{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn open(&mut self, connection: ConnectionParams, host_id: Option<String>) {
        match connection.kind {
            ConnectionType::Local => self.open_local(),
            ConnectionType::Telnet => self.open_telnet(connection, host_id),
            ConnectionType::Serial => self.open_serial(connection, host_id),
            ConnectionType::Ssh => {
                if connection.auth != AuthMethod::Password {
                    self.start_ssh(connection, host_id, Secrets::default());
                    return;
                }
                // A saved password connects straight away; otherwise the terminal asks.
                match self.stored_password(host_id.as_deref()) {
                    Some(password) => self.start_ssh(
                        connection,
                        host_id,
                        Secrets {
                            password,
                            ..Secrets::default()
                        },
                    ),
                    None => self.open_ssh_prompt(connection, host_id),
                }
            }
        }
    }

    fn open_telnet(&mut self, connection: ConnectionParams, host_id: Option<String>) {
        let id = self.next_session_id();
        let entry = SessionEntry::new_telnet(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            &self.settings.term_type,
            self.last_terminal_size,
            connection,
            host_id,
        );
        self.push_session(entry);
    }

    fn open_serial(&mut self, connection: ConnectionParams, host_id: Option<String>) {
        let id = self.next_session_id();
        let entry = SessionEntry::new_serial(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            self.last_terminal_size,
            connection,
            host_id,
        );
        self.push_session(entry);
    }

    fn open_local(&mut self) {
        let id = self.next_session_id();
        let entry = SessionEntry::new_local(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            &self.settings.term_type,
            self.last_terminal_size,
        );
        self.push_session(entry);
    }

    /// Open an SSH tab whose password is typed into the terminal itself.
    ///
    /// The tab appears immediately with the `user@host's password:` prompt, matching the
    /// terminal itself, like PuTTY; a rejected password re-prompts in place.
    fn open_ssh_prompt(&mut self, connection: ConnectionParams, host_id: Option<String>) {
        let jumps = match jump_chain(&self.hosts, &connection) {
            Ok(jumps) => jumps,
            Err(error) => {
                self.toasts.push(ToastKind::Error, error);
                return;
            }
        };
        let id = self.next_session_id();
        let entry = SessionEntry::new_ssh_prompt(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            self.last_terminal_size,
            SshOpen {
                params: connection,
                host_id,
                secrets: Secrets::default(),
                known_hosts: self.known_hosts.clone(),
                jumps,
            },
        );
        self.push_session(entry);
    }

    fn start_ssh(
        &mut self,
        connection: ConnectionParams,
        host_id: Option<String>,
        secrets: Secrets,
    ) {
        let jumps = match jump_chain(&self.hosts, &connection) {
            Ok(jumps) => jumps,
            Err(error) => {
                self.toasts.push(ToastKind::Error, error);
                return;
            }
        };
        let id = self.next_session_id();
        let entry = SessionEntry::new_ssh(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            &self.settings.term_type,
            self.last_terminal_size,
            SshOpen {
                params: connection,
                host_id,
                secrets,
                known_hosts: self.known_hosts.clone(),
                jumps,
            },
        );
        self.push_session(entry);
    }

    fn push_session(&mut self, entry: SessionEntry) {
        self.sessions.push(entry);
        self.active = Some(self.sessions.len() - 1);
        self.zoom = 0.0;
        self.apply_zoom();
        self.persist_tabs();
    }

    fn close(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        let mut entry = self.sessions.remove(index);
        entry.disconnect();
        self.closed.push(ClosedTab {
            connection: entry.connection.clone(),
            host_id: entry.host_id.clone(),
        });
        if self.closed.len() > 20 {
            self.closed.remove(0);
        }
        if self.sessions.is_empty() {
            self.active = None;
        } else if let Some(active) = self.active {
            self.active = Some(active.min(self.sessions.len() - 1));
        }
        self.persist_tabs();
    }

    fn reopen_closed(&mut self) {
        let Some(tab) = self.closed.pop() else {
            self.toasts.push(ToastKind::Info, "No recently closed tabs.");
            return;
        };
        self.open(tab.connection, tab.host_id);
    }

    fn active(&self) -> Option<&SessionEntry> {
        self.active.and_then(|index| self.sessions.get(index))
    }

    fn cycle_tab(&mut self, forward: bool) {
        if self.sessions.len() < 2 {
            return;
        }
        let current = self.active.unwrap_or(0);
        let count = self.sessions.len();
        self.active = Some(if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        });
    }

    fn restart(&mut self, index: usize, secrets: Secrets) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.restart(&self.settings.term_type, &secrets, self.known_hosts.clone());
        }
    }

    // --- window geometry ---------------------------------------------------

    /// Write `settings.json` without re-applying defaults to the open tabs.
    fn write_settings(&mut self) {
        if let Some(config) = self.config.as_mut() {
            if let Err(error) = config.settings.set(self.settings.clone()) {
                self.toasts.push(
                    ToastKind::Error,
                    format!("could not stage settings: {error}"),
                );
            }
            if let Err(error) = config.settings.save() {
                self.toasts
                    .push(ToastKind::Error, format!("could not save settings: {error}"));
            }
        }
    }

    /// Put the window back where it was saved, in physical pixels.
    ///
    /// `OuterPosition` takes points, so the saved pixel position is divided by the window's
    /// current scale: the conversion happens against the same scale it is multiplied by, and
    /// the window lands on the monitor, and at the pixel, it was saved from. The size is
    /// already applied in points when the window is created.
    fn restore_window_position(&mut self, ctx: &egui::Context) -> bool {
        let Some(bounds) = self.restore_bounds.take() else {
            return false;
        };
        let pixels_per_point = ctx.pixels_per_point();
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            bounds.x as f32 / pixels_per_point,
            bounds.y as f32 / pixels_per_point,
        )));
        ctx.request_repaint();
        true
    }

    /// Remember the window rectangle so the next launch can restore it.
    fn track_window_bounds(&mut self, ctx: &egui::Context) {
        let viewport = ctx.input(|input| input.viewport().clone());
        if viewport.fullscreen.unwrap_or(false) || viewport.minimized.unwrap_or(false) {
            return;
        }
        let (Some(outer), Some(inner)) = (viewport.outer_rect, viewport.inner_rect) else {
            return;
        };
        let pixels_per_point = ctx.pixels_per_point();
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return;
        }
        let maximized = viewport.maximized.unwrap_or(false);
        if !maximized {
            self.last_bounds = Some(WindowBounds {
                x: (outer.left() * pixels_per_point).round() as i32,
                y: (outer.top() * pixels_per_point).round() as i32,
                width: inner.width().round() as u32,
                height: inner.height().round() as u32,
                maximized: false,
            });
        }
        // While maximized the normal rectangle is kept, so unmaximizing and the next launch
        // still use it.
        let Some(normal) = self.last_bounds else {
            return;
        };
        let current = WindowBounds { maximized, ..normal };
        if self.settings.window_bounds == Some(current) {
            return;
        }
        self.settings.window_bounds = Some(current);
        self.bounds_changed_at = Some(std::time::Instant::now());
        ctx.request_repaint_after(WINDOW_BOUNDS_SETTLE);
    }

    /// Write the window rectangle once it has stopped changing, or at once when the app exits.
    fn persist_window_bounds(&mut self, force: bool) {
        let Some(changed_at) = self.bounds_changed_at else {
            return;
        };
        if !force && changed_at.elapsed() < WINDOW_BOUNDS_SETTLE {
            return;
        }
        self.bounds_changed_at = None;
        self.write_settings();
    }

    // --- persistence -------------------------------------------------------

    fn save_settings(&mut self) {
        self.settings.sidebar_collapsed = self.sidebar.collapsed;
        // `System` resolves against the desktop preference seen this session.
        let system_dark = self.system_dark.unwrap_or(true);
        self.palette = Palette::for_mode(self.settings.theme.resolve(system_dark));
        self.write_settings();
        // New defaults and the (possibly new) theme apply to the sessions already open.
        let palette = self.palette;
        let defaults = self.settings.session_style_defaults.clone();
        for session in &mut self.sessions {
            session.palette = palette;
            session.apply_style(&defaults);
            // Panes pick up the plugin values that just changed.
            session.reload_pane_settings();
        }
    }

    /// Write the credential edits a settings dialog returned.
    fn apply_secret_edits(&mut self, save: &HostSave) {
        let Some(store) = self.secrets.as_mut() else {
            if save.password != SecretEdit::Unchanged
                || save.passphrase != SecretEdit::Unchanged
            {
                self.toasts.push(
                    ToastKind::Error,
                    "Secret storage is unavailable; the password was not saved.",
                );
            }
            return;
        };
        let id = save.host.id.clone();
        let results = [
            secret_value(&save.password).map(|value| store.set_password(&id, value)),
            secret_value(&save.passphrase).map(|value| store.set_passphrase(&id, value)),
        ];
        for result in results.into_iter().flatten() {
            if let Err(error) = result {
                self.toasts
                    .push(ToastKind::Error, format!("could not save secret: {error}"));
            }
        }
    }

    /// Push a saved host's settings onto the sessions already open for it.
    ///
    /// Returns how many sessions were updated. Styles apply immediately; everything else
    /// takes effect on the next connect, exactly like the original dialog's button.
    fn apply_host_to_sessions(&mut self, host: &HostProfile) -> usize {
        let defaults = self.settings.session_style_defaults.clone();
        let mut applied = 0usize;
        for session in &mut self.sessions {
            if session.host_id.as_deref() != Some(host.id.as_str()) {
                continue;
            }
            session.apply_connection(host.connection.clone(), &defaults);
            session.reload_pane_settings();
            applied += 1;
        }
        applied
    }

    /// Password stored for a saved host, if any.
    fn stored_password(&self, host_id: Option<&str>) -> Option<String> {
        let host_id = host_id?;
        self.secrets.as_ref()?.password(host_id)
    }

    /// Plugin `secret: true` fields the vault holds for `scope`.
    fn stored_plugin_secrets(&self, scope: &str) -> Vec<(&'static str, &'static str)> {
        let Some(store) = self.secrets.as_ref() else {
            return Vec::new();
        };
        let mut stored = Vec::new();
        for id in PluginId::ALL {
            for (key, _) in wassh_plugins::settings::secret_fields(id) {
                if store.has_plugin_secret(scope, id.as_str(), key) {
                    stored.push((id.as_str(), key));
                }
            }
        }
        stored
    }

    /// Decrypted plugin secrets for one host, per plugin id and field key.
    ///
    /// App-wide fields are read from the `app` scope, host fields from the host
    /// id, so a pane sees one map regardless of where the schema declared it.
    fn plugin_secrets_for(
        &self,
        host_id: Option<&str>,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
        let mut out: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
            std::collections::BTreeMap::new();
        let Some(store) = self.secrets.as_ref() else {
            return out;
        };
        for id in PluginId::ALL {
            for (key, app_wide) in wassh_plugins::settings::secret_fields(id) {
                let scope = if app_wide {
                    wassh_store::secrets::APP_SCOPE
                } else {
                    match host_id {
                        Some(host_id) => host_id,
                        None => continue,
                    }
                };
                if let Some(value) = store.plugin_secret(scope, id.as_str(), key) {
                    out.entry(id.as_str().to_owned())
                        .or_default()
                        .insert(key.to_owned(), value);
                }
            }
        }
        out
    }

    /// Seal the plugin secrets a dialog or pane collected under `scope`.
    fn apply_plugin_secrets(&mut self, scope: &str, drafts: &PluginSecretDrafts) {
        if drafts.is_empty() {
            return;
        }
        let Some(store) = self.secrets.as_mut() else {
            self.toasts.push(
                ToastKind::Error,
                "Secret storage is unavailable; the plugin password was not saved.",
            );
            return;
        };
        for (key, draft) in drafts {
            let Some((plugin, field)) = key.split_once('|') else {
                continue;
            };
            let result = match draft.edit() {
                SecretEdit::Unchanged => continue,
                SecretEdit::Set(value) => {
                    store.set_plugin_secret(scope, plugin, field, Some(&value))
                }
                SecretEdit::Clear => store.set_plugin_secret(scope, plugin, field, None),
            };
            if let Err(error) = result {
                self.toasts.push(
                    ToastKind::Error,
                    format!("could not save {plugin} password: {error}"),
                );
            }
        }
    }

    /// Move plain-text plugin secrets out of the settings files into the vault.
    ///
    /// Earlier builds kept a `secret: true` field in `settings.json` or
    /// `hosts.json` like any other setting. Anything still there is sealed on
    /// startup and removed, so an upgrade never leaves a password readable.
    fn migrate_plugin_secrets(&mut self) {
        let mut pending: Vec<(String, String, String, String)> = Vec::new();
        for (plugin, values) in &self.settings.plugin_settings {
            let Some(id) = PluginId::parse(plugin) else {
                continue;
            };
            for (key, app_wide) in wassh_plugins::settings::secret_fields(id) {
                if !app_wide {
                    continue;
                }
                if let Some(text) = values
                    .get(key)
                    .and_then(|value| value.as_str())
                    .filter(|text| !text.is_empty())
                {
                    pending.push((
                        wassh_store::secrets::APP_SCOPE.to_owned(),
                        plugin.clone(),
                        key.to_owned(),
                        text.to_owned(),
                    ));
                }
            }
        }
        for host in &self.hosts.hosts {
            for (plugin, values) in &host.plugin_settings {
                let Some(id) = PluginId::parse(plugin) else {
                    continue;
                };
                for (key, app_wide) in wassh_plugins::settings::secret_fields(id) {
                    if app_wide {
                        continue;
                    }
                    if let Some(text) = values
                        .get(key)
                        .and_then(|value| value.as_str())
                        .filter(|text| !text.is_empty())
                    {
                        pending.push((
                            host.id.clone(),
                            plugin.clone(),
                            key.to_owned(),
                            text.to_owned(),
                        ));
                    }
                }
            }
        }
        if pending.is_empty() {
            return;
        }
        let from_hosts = pending
            .iter()
            .any(|(scope, ..)| scope != wassh_store::secrets::APP_SCOPE);
        let Some(store) = self.secrets.as_mut() else {
            self.toasts.push(
                ToastKind::Error,
                "Secret storage is unavailable; plugin passwords are still in the settings file.",
            );
            return;
        };
        let mut failed = false;
        for (scope, plugin, field, value) in pending {
            if let Err(error) = store.set_plugin_secret(&scope, &plugin, &field, Some(&value)) {
                failed = true;
                self.toasts.push(
                    ToastKind::Error,
                    format!("could not move the {plugin} password: {error}"),
                );
            }
        }
        if failed {
            return;
        }
        strip_plugin_secrets(&mut self.settings.plugin_settings);
        self.save_settings();
        if from_hosts {
            for host in &mut self.hosts.hosts {
                strip_plugin_secrets(&mut host.plugin_settings);
            }
            self.persist_hosts();
        }
    }

    /// Build a host dialog state with the app defaults and any stored secrets.
    fn host_state(&self, host: HostProfile, is_new: bool) -> HostState {
        // Ask the configuration rather than trusting the caller's flag: a draft
        // for an id that is not in `hosts.json` must never offer "Apply to
        // sessions" or "Delete".
        let saved = self.hosts.host(&host.id).is_some();
        let (password, passphrase) = self.stored_secrets(&host.id);
        let open = self
            .sessions
            .iter()
            .any(|session| session.host_id.as_deref() == Some(host.id.as_str()));
        let stored_plugin_secrets = self.stored_plugin_secrets(&host.id);
        HostState::draft(host, is_new)
            .with_defaults(&self.settings.session_style_defaults)
            .with_open_sessions(open)
            .with_saved(saved)
            .with_stored_secrets(password, passphrase)
            .with_plugin_secrets(stored_plugin_secrets)
    }

    /// Whether a password / passphrase is already stored for `host_id`.
    fn stored_secrets(&self, host_id: &str) -> (bool, bool) {
        match self.secrets.as_ref() {
            Some(store) => (store.has_password(host_id), store.has_passphrase(host_id)),
            None => (false, false),
        }
    }

    /// Options dialog state, with the app-wide plugin secrets already noted.
    fn options_state(&self) -> OptionsState {
        let mut state = OptionsState::new(&self.settings);
        state.plugin_secrets = self
            .stored_plugin_secrets(wassh_store::secrets::APP_SCOPE)
            .into_iter()
            .map(|(plugin, field)| {
                (
                    self::dialogs::secret_field_key(plugin, field),
                    self::dialogs::SecretDraft::stored(),
                )
            })
            .collect();
        state
    }

    /// A host id that is not in use yet.
    fn next_host_id(&mut self) -> String {
        loop {
            let id = format!("host-{}", self.next_id);
            self.next_id += 1;
            if self.hosts.host(&id).is_none() {
                return id;
            }
        }
    }

    /// Copy a host: same connection, tags, group and plugin settings under a new
    /// id, named "‹name› (copy)" and placed right after the original. Stored
    /// passwords are not copied, exactly like the original app.
    fn duplicate_host(&mut self, id: &str) {
        let Some(original) = self.hosts.host(id).cloned() else {
            return;
        };
        let mut copy = original.clone();
        copy.id = self.next_host_id();
        copy.name = format!("{} (copy)", original.display_name());
        // Keep the copy beside its original so lists stay in a predictable order.
        let position = self
            .hosts
            .hosts
            .iter()
            .position(|host| host.id == id)
            .map(|index| index + 1);
        match position {
            Some(index) => self.hosts.hosts.insert(index, copy.clone()),
            None => self.hosts.hosts.push(copy.clone()),
        }
        if copy.group_id.is_none() {
            let after = self
                .hosts
                .ungrouped_host_ids
                .iter()
                .position(|existing| existing == id)
                .map(|index| index + 1)
                .unwrap_or(self.hosts.ungrouped_host_ids.len());
            self.hosts.ungrouped_host_ids.insert(after, copy.id.clone());
        }
        self.hosts = std::mem::take(&mut self.hosts).normalized();
        let name = copy.name.clone();
        self.save_host(copy);
        self.toasts
            .push(ToastKind::Info, format!("Duplicated as “{name}”."));
    }

    fn save_host(&mut self, mut host: HostProfile) {
        // A plugin password belongs in the vault, never in `hosts.json`.
        strip_plugin_secrets(&mut host.plugin_settings);
        let host_id = host.id.clone();
        self.hosts.upsert(host.clone());
        self.hosts = std::mem::take(&mut self.hosts).normalized();
        // Panes of this host pick up its plugin settings on the next draw.
        for session in &mut self.sessions {
            if session.host_id.as_deref() == Some(host_id.as_str()) {
                session.reload_pane_settings();
            }
        }
        if let Some(config) = self.config.as_mut()
            && let Err(error) = config.save_host(host)
        {
            self.toasts
                .push(ToastKind::Error, format!("could not save host: {error}"));
        }
    }

    fn delete_host(&mut self, id: &str) {
        self.hosts.remove(id);
        if let Some(store) = self.secrets.as_mut() {
            let _ = store.set_password(id, None);
            let _ = store.set_passphrase(id, None);
            let _ = store.forget_scope(id);
        }
        if let Some(config) = self.config.as_mut()
            && let Err(error) = config.delete_host(id)
        {
            self.toasts
                .push(ToastKind::Error, format!("could not delete host: {error}"));
        }
    }

    fn persist_hosts(&mut self) {
        if let Some(config) = self.config.as_mut() {
            if let Err(error) = config.hosts.set(self.hosts.clone()) {
                self.toasts
                    .push(ToastKind::Error, format!("could not stage hosts: {error}"));
            }
            if let Err(error) = config.hosts.save() {
                self.toasts
                    .push(ToastKind::Error, format!("could not save hosts: {error}"));
            }
        }
    }

    // --- commands ----------------------------------------------------------

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::NewLocalTab => self.open_local(),
            Command::NewHost => {
                let mut draft = HostProfile {
                    id: format!("host-{}", self.next_id),
                    ..HostProfile::default()
                };
                draft.connection = draft.connection.normalized();
                self.dialogs.host = Some(self.host_state(draft, true));
            }
            Command::EditHost => {
                let saved = self
                    .active()
                    .and_then(|session| session.host_id.clone())
                    .and_then(|id| self.hosts.host(&id).cloned());
                let state = match saved {
                    Some(host) => self.host_state(host, false),
                    None => {
                        let mut draft = self
                            .active()
                            .map(|session| HostProfile {
                                id: format!("host-{}", self.next_id),
                                name: session.title.clone(),
                                connection: session.connection.clone(),
                                ..HostProfile::default()
                            })
                            .unwrap_or_default();
                        draft.connection = draft.connection.normalized();
                        self.host_state(draft, true)
                    }
                };
                self.dialogs.host = Some(state);
            }
            Command::OpenOptions => {
                let home = home_dir();
                let now = wassh_plugins::connection_logger::now_ms();
                self.dialogs.options = Some(self.options_state().with_backup_default(&home, now));
            }
            Command::OpenTunnels => {
                let saved = self
                    .active()
                    .and_then(|session| session.host_id.clone())
                    .and_then(|id| self.hosts.host(&id).cloned());
                if let Some(host) = saved {
                    let mut state = self.host_state(host, false);
                    let sections = dialogs::host_sections(state.draft.connection.kind);
                    if let Some(index) = sections.iter().position(|name| *name == "Tunnels") {
                        state.nav.jump_to(index);
                    }
                    self.dialogs.host = Some(state);
                } else {
                    self.toasts
                        .push(ToastKind::Info, "Save this session as a host first.");
                }
            }
            Command::ToggleSidebar => {
                self.sidebar.collapsed = !self.sidebar.collapsed;
                self.save_settings();
            }
            Command::Reconnect => {
                if let Some(index) = self.active {
                    self.restart(index, Secrets::default());
                }
            }
            Command::ReconnectAll => {
                for index in 0..self.sessions.len() {
                    self.restart(index, Secrets::default());
                }
            }
            Command::CloseTab => {
                if let Some(index) = self.active {
                    self.close(index);
                }
            }
            Command::NextTab => self.cycle_tab(true),
            Command::PreviousTab => self.cycle_tab(false),
            Command::Find => {
                self.dialogs.find = Some(FindState::new());
                self.find_matches = 0;
                self.find_index = 0;
            }
            Command::About => self.dialogs.about = true,
            Command::Quit => self.quit_requested = true,
        }
    }

    fn menu_command(&mut self, ctx: &egui::Context, item: &str) {
        match item {
            "Copy" => self.copy_selection(ctx),
            "Paste" => self.paste_clipboard(ctx),
            "New tab" => self.handle_command(Command::NewLocalTab),
            "New host…" => self.handle_command(Command::NewHost),
            "Reopen closed tab" => self.reopen_closed(),
            "Options…" => self.handle_command(Command::OpenOptions),
            "Quit" => self.handle_command(Command::Quit),
            "Find…" => self.handle_command(Command::Find),
            "Reconnect" => self.handle_command(Command::Reconnect),
            "Reconnect all" => self.handle_command(Command::ReconnectAll),
            "Session settings…" => self.handle_command(Command::EditHost),
            "Tunnels…" => self.handle_command(Command::OpenTunnels),
            "Zoom In" => {
                self.zoom = (self.zoom + 1.0).min(30.0);
                self.apply_zoom();
            }
            "Zoom Out" => {
                self.zoom = (self.zoom - 1.0).max(-6.0);
                self.apply_zoom();
            }
            "Reset zoom" => {
                self.zoom = 0.0;
                self.apply_zoom();
            }
            "Toggle sidebar" => self.handle_command(Command::ToggleSidebar),
            "Toggle Full Screen" => {
                self.fullscreen = !self.fullscreen;
                self.fullscreen_request = Some(self.fullscreen);
            }
            "About WaSSH" => self.handle_command(Command::About),
            other => {
                if let Some(id) = PluginId::UI_ORDER
                    .into_iter()
                    .find(|id| id.manifest().toolbar_label == other)
                {
                    self.toggle_plugin(id);
                }
            }
        }
    }

    /// Show a plugin pane in the active tab, or explain why it cannot be shown.
    /// Open Options on the plugin's own section, from the gear in its menu entry.
    fn open_plugin_options(&mut self, label: &str) {
        let Some(id) = wassh_plugins::PluginId::UI_ORDER
            .into_iter()
            .find(|id| id.manifest().toolbar_label == label)
        else {
            return;
        };
        let home = home_dir();
        let now = wassh_plugins::connection_logger::now_ms();
        self.dialogs.options = Some(
            self.options_state()
                .with_backup_default(&home, now)
                .focusing_plugin(id),
        );
    }

    /// Toolbar labels of the plugins open in the active tab, so the Plugins menu
    /// can tick them.
    fn active_plugin_labels(&self) -> Vec<&'static str> {
        let Some(session) = self.active.and_then(|index| self.sessions.get(index)) else {
            return Vec::new();
        };
        wassh_plugins::PluginId::UI_ORDER
            .into_iter()
            .filter(|id| session.layout.contains(id.as_str()))
            .map(|id| id.manifest().toolbar_label)
            .collect()
    }

    /// Open the plugin's pane in the active tab, or close it when it is already
    /// open — what the toolbar button and the Plugins menu both do.
    fn toggle_plugin(&mut self, id: wassh_plugins::PluginId) {
        let Some(index) = self.active else {
            self.toasts
                .push(ToastKind::Info, "Open a session first.".to_owned());
            return;
        };
        if let Some(session) = self.sessions.get_mut(index) {
            if session.layout.contains(id.as_str()) {
                session.close_plugin(id.as_str());
            } else {
                session.open_plugin(id);
            }
            self.persist_tabs();
        }
    }

    fn apply_zoom(&mut self) {
        let base = f32::from(self.settings.session_style_defaults.font_size);
        let default_size = (base + self.zoom).clamp(8.0, 48.0);
        for session in &mut self.sessions {
            session.view.font_size = session
                .connection
                .style
                .font_size
                .map_or(default_size, f32::from);
        }
    }

    /// Copy the terminal's selection to the system clipboard.
    fn copy_selection(&mut self, ctx: &egui::Context) {
        let text = self
            .active()
            .and_then(|session| session.terminal.selection_text());
        match text {
            Some(text) => ctx.copy_text(text),
            None => self
                .toasts
                .push(ToastKind::Info, "Select some text to copy."),
        }
    }

    /// Ask the integration for the clipboard contents; the text arrives as a paste event and
    /// is typed into the terminal from there.
    fn paste_clipboard(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialogs.modal_open() {
            return;
        }
        let mut commands = Vec::new();
        let mut palette = false;
        let mut copy = false;
        let mut paste = false;
        ctx.input_mut(|input| {
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::P)
                || input.consume_key(Modifiers::NONE, Key::F1)
            {
                palette = true;
            }
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::T) {
                commands.push(Command::NewLocalTab);
            }
            // Ctrl+W is the tab convention everywhere; Ctrl+Shift+W and Ctrl+F4 keep working
            // for people used to the older terminal bindings.
            if input.consume_key(Modifiers::CTRL, Key::W)
                || input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::W)
                || input.consume_key(Modifiers::CTRL, Key::F4)
            {
                commands.push(Command::CloseTab);
            }
            // Ctrl+Shift+C / Ctrl+Insert copy the selection, Ctrl+Shift+V / Shift+Insert paste,
            // so the plain Ctrl+C / Ctrl+V still reach the shell as ^C / ^V.
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::C)
                || input.consume_key(Modifiers::CTRL, Key::Insert)
            {
                copy = true;
            }
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::V)
                || input.consume_key(Modifiers::SHIFT, Key::Insert)
            {
                paste = true;
            }
            if input.consume_key(Modifiers::CTRL, Key::Tab) {
                commands.push(Command::NextTab);
            }
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
                commands.push(Command::PreviousTab);
            }
            if input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::F) {
                commands.push(Command::Find);
            }
        });
        if palette {
            self.dialogs.palette = Some(PaletteState::new());
        }
        if copy {
            self.copy_selection(ctx);
        }
        if paste {
            self.paste_clipboard(ctx);
        }
        for command in commands {
            self.handle_command(command);
        }
    }

    // --- per-frame work ----------------------------------------------------

    fn pump(&mut self, ctx: &egui::Context) {
        if self.sessions.is_empty() {
            return;
        }
        let term_type = self.settings.term_type.clone();
        let mut results = Vec::with_capacity(self.sessions.len());
        for (index, session) in self.sessions.iter_mut().enumerate() {
            results.push((index, session.pump(&term_type)));
            // Collect any finished remote-file or monitoring requests.
            session.poll_files();
            session.poll_monitor();
            session.poll_commands();
            if session.requests_pending() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        for (index, pumped) in results {
            if let Some(title) = pumped.title
                && self.active == Some(index)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{APP_NAME} — {title}"
                )));
            }
            if pumped.bell {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
            }
            if let Some(reason) = pumped.closed
                && let Some(session) = self.sessions.get_mut(index)
                && !session.exit_reported
            {
                session.exit_reported = true;
                if let Some(reason) = reason {
                    session.error = Some(reason);
                }
            }
        }

        self.schedule_reconnects(ctx);
    }

    /// Queue and run automatic reconnects according to each session's policy.
    fn schedule_reconnects(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|input| input.focused);
        let now = std::time::Instant::now();
        let term_type = self.settings.term_type.clone();
        let mut retries = Vec::new();

        for (index, session) in self.sessions.iter_mut().enumerate() {
            // A pending retry that has come due.
            if session.retry_at.is_some_and(|at| at <= now)
                && matches!(session.status, SessionStatus::Disconnected | SessionStatus::Failed)
                && session.host_key_prompt.is_none()
            {
                session.retry_at = None;
                retries.push(index);
                continue;
            }
            if !matches!(session.status, SessionStatus::Disconnected | SessionStatus::Failed) {
                session.retry_at = None;
                continue;
            }
            if session.retry_at.is_some() || session.host_key_prompt.is_some() {
                continue;
            }
            if !session.connection.reconnect.allows_retry(focused) {
                continue;
            }
            let attempt = session.reconnect_attempt + 1;
            let delay = wassh_core::backoff_delay(attempt);
            session.reconnect_attempt = attempt;
            session.retry_at = Some(now + delay);
            session.status = SessionStatus::Reconnecting;
        }

        for index in retries {
            if let Some(session) = self.sessions.get_mut(index) {
                let secrets = session.secrets.clone();
                session.restart(&term_type, &secrets, self.known_hosts.clone());
            }
        }

        // Wake up in time for the next scheduled retry.
        if let Some(next) = self
            .sessions
            .iter()
            .filter_map(|session| session.retry_at)
            .min()
        {
            let wait = next.saturating_duration_since(now);
            ctx.request_repaint_after(wait.min(Duration::from_secs(1)));
        }
    }

    fn forward_input(&mut self, ctx: &egui::Context) {
        if self.dialogs.modal_open() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let Some(index) = self.active else {
            return;
        };
        let inputs = wassh_ui::collect_terminal_input(ctx);
        if inputs.is_empty() {
            return;
        }
        let Some(session) = self.sessions.get_mut(index) else {
            return;
        };
        if session.awaiting_login() {
            // The password is typed straight into the terminal, never into a dialog.
            session.login_input(&inputs, &self.settings.term_type);
            return;
        }
        if !session.is_live() {
            return;
        }
        for input in inputs {
            match input {
                wassh_ui::TerminalInput::Text(text) => session.send(text.as_bytes()),
                wassh_ui::TerminalInput::Key(stroke) => session.send_key(&stroke),
                wassh_ui::TerminalInput::Paste(text) => session.send_paste(&text),
            }
        }
    }

    fn update_find_matches(&mut self) {
        let query = self
            .dialogs
            .find
            .as_ref()
            .map(|find| find.query.trim().to_lowercase())
            .unwrap_or_default();
        if query.is_empty() {
            self.find_matches = 0;
            return;
        }
        let text = self
            .active()
            .map(|session| session.visible_text().to_lowercase())
            .unwrap_or_default();
        self.find_matches = text.matches(&query).count();
    }

    /// Write or restore the configuration archive, reporting the outcome as a toast.
    fn run_backup(&mut self, import: bool, path: &str) {
        let path = path.trim();
        let Some(config) = self.config.as_ref() else {
            self.toasts
                .push(ToastKind::Error, "The configuration folder is unavailable.");
            return;
        };
        let archive = expand_home(path);
        if import {
            match wassh_store::import_backup(&archive, &config.dir) {
                Ok(summary) => self.toasts.push(
                    ToastKind::Info,
                    format!(
                        "Restored {} file{} from {}. Restart WaSSH to load it.",
                        summary.files,
                        if summary.files == 1 { "" } else { "s" },
                        summary.path.display()
                    ),
                ),
                Err(error) => self.toasts.push(ToastKind::Error, error.to_string()),
            }
        } else {
            match wassh_store::export_backup(&config.dir, &archive) {
                Ok(summary) => self.toasts.push(
                    ToastKind::Info,
                    format!(
                        "Backed up {} file{} to {}.",
                        summary.files,
                        if summary.files == 1 { "" } else { "s" },
                        summary.path.display()
                    ),
                ),
                Err(error) => self.toasts.push(ToastKind::Error, error.to_string()),
            }
        }
    }

    fn handle_dialog_actions(&mut self, actions: Vec<DialogAction>) {
        for action in actions {
            match action {
                DialogAction::SaveSettings(mut settings, secrets) => {
                    strip_plugin_secrets(&mut settings.plugin_settings);
                    self.settings = settings;
                    self.apply_plugin_secrets(wassh_store::secrets::APP_SCOPE, &secrets);
                    self.save_settings();
                    self.dialogs.options = None;
                    self.toasts.push(ToastKind::Info, "Settings saved.");
                }
                DialogAction::SaveHost(save) => {
                    self.apply_secret_edits(&save);
                    self.apply_plugin_secrets(&save.host.id.clone(), &save.plugin_secrets);
                    self.save_host(save.host);
                    self.dialogs.host = None;
                    self.toasts.push(ToastKind::Info, "Host saved.");
                }
                DialogAction::ApplyHost(save) => {
                    self.apply_secret_edits(&save);
                    self.apply_plugin_secrets(&save.host.id.clone(), &save.plugin_secrets);
                    // Remember the host's id before the box is consumed.
                    let applied = self.apply_host_to_sessions(&save.host);
                    self.save_host(save.host);
                    self.dialogs.host = None;
                    self.toasts.push(
                        ToastKind::Info,
                        format!(
                            "Host saved and applied to {applied} open session{}.",
                            if applied == 1 { "" } else { "s" }
                        ),
                    );
                }
                DialogAction::DeleteHost(id) => {
                    self.delete_host(&id);
                    self.dialogs.host = None;
                    self.toasts.push(ToastKind::Info, "Host deleted.");
                }
                DialogAction::Backup { import, path } => self.run_backup(import, &path),
                DialogAction::Command(command) => {
                    self.close_dialogs();
                    self.handle_command(command);
                }
                DialogAction::Find(command) => match command {
                    FindCommand::Close => self.dialogs.find = None,
                    FindCommand::Next => {
                        if self.find_matches > 0 {
                            self.find_index = (self.find_index + 1) % self.find_matches;
                        }
                    }
                    FindCommand::Previous => {
                        if self.find_matches > 0 {
                            self.find_index =
                                (self.find_index + self.find_matches - 1) % self.find_matches;
                        }
                    }
                },
                DialogAction::SaveGroup {
                    group_id,
                    name,
                    color,
                } => {
                    if let Some(group) = self.hosts.groups.iter_mut().find(|group| group.id == group_id)
                    {
                        group.name = name;
                        group.color = color;
                    }
                    self.hosts = std::mem::take(&mut self.hosts).normalized();
                    self.persist_hosts();
                    self.dialogs.group_edit = None;
                }
                DialogAction::Close => self.close_dialogs(),
            }
        }
    }

    fn close_dialogs(&mut self) {
        self.dialogs.options = None;
        self.dialogs.host = None;
        self.dialogs.about = false;
        self.dialogs.group_edit = None;
        self.dialogs.palette = None;
    }

    // --- drawing -----------------------------------------------------------

    /// Write any plugin data that changed while the tab was drawn.
    fn persist_plugin_data(&mut self, index: usize) {
        let Some(session) = self.sessions.get_mut(index) else {
            return;
        };
        let host_id = session.host_id.clone();
        let dirty = session.take_dirty_plugins();
        let secret_writes = session.take_dirty_plugin_secrets();
        for (id, field, value) in secret_writes {
            let scope = host_id.clone().unwrap_or_else(|| {
                wassh_store::secrets::APP_SCOPE.to_owned()
            });
            let Some(store) = self.secrets.as_mut() else {
                self.toasts.push(
                    ToastKind::Error,
                    "Secret storage is unavailable; the plugin password was not saved.",
                );
                continue;
            };
            if let Err(error) = store.set_plugin_secret(&scope, id.as_str(), &field, value.as_deref())
            {
                self.toasts.push(
                    ToastKind::Error,
                    format!("could not save {} password: {error}", id.manifest().name),
                );
            }
        }
        if dirty.is_empty() {
            return;
        }
        let mut failures = Vec::new();
        let mut app_changed = false;
        let mut host_changed = false;
        if let Some(store) = self.config.as_ref() {
            for (id, scope, data) in dirty {
                // A key the settings dialogs also own is mirrored back, so the
                // panel and the dialog cannot disagree about the same value.
                for (key, value) in &data {
                    // A secret field is sealed in the vault, never mirrored.
                    if wassh_plugins::settings::secret_scope(id, key).is_some() {
                        continue;
                    }
                    if wassh_plugins::settings::app_schema(id)
                        .iter()
                        .any(|field| field.key == key)
                    {
                        self.settings
                            .plugin_settings
                            .entry(id.as_str().to_owned())
                            .or_default()
                            .insert(key.clone(), value.clone());
                        app_changed = true;
                    } else if wassh_plugins::settings::host_schema(id)
                        .iter()
                        .any(|field| field.key == key)
                        && let Some(host) = host_id
                            .as_deref()
                            .and_then(|host_id| self.hosts.host_mut(host_id))
                    {
                        host.plugin_settings
                            .entry(id.as_str().to_owned())
                            .or_default()
                            .insert(key.clone(), value.clone());
                        host_changed = true;
                    }
                }
                let value = serde_json::Value::Object(data);
                if let Err(error) = store.save_plugin_data(id.as_str(), Some(&scope), &value) {
                    failures.push(format!("{}: {error}", id.manifest().name));
                }
            }
        }
        for failure in failures {
            self.toasts
                .push(ToastKind::Error, format!("could not save plugin data ({failure})"));
        }
        if app_changed {
            self.save_settings();
        }
        if host_changed {
            self.persist_hosts();
        }
    }

    /// Draw the plugin strip and toggle the clicked plugin's pane.
    ///
    /// One button per built-in plugin, highlighted
    /// while that plugin is open in the active tab.
    fn show_plugin_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.active else {
            return;
        };
        if index >= self.sessions.len() {
            return;
        }
        let palette = self.palette;
        let session = &self.sessions[index];
        let buttons: Vec<wassh_ui::ToolbarButton<'_>> = wassh_plugins::PluginId::UI_ORDER
            .into_iter()
            // Disabled plugins stay out of the strip, exactly like the original build.
            .filter(|id| self.settings.plugin_enabled(id.as_str()))
            .map(|id| {
                let manifest = id.manifest();
                wassh_ui::ToolbarButton {
                    id: id.as_str(),
                    label: manifest.toolbar_label,
                    active: session.layout.contains(id.as_str()),
                    tooltip: manifest.name,
                }
            })
            .collect();

        let mut output = wassh_ui::plugin_toolbar::ToolbarOutput::default();
        egui::Panel::top("wassh-plugin-toolbar")
            .exact_size(wassh_ui::plugin_toolbar::TOOLBAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                output = wassh_ui::plugin_toolbar(ui, &palette, &buttons);
            });

        if let Some(plugin_id) = output.dragging {
            // Dragging a button carries the plugin to wherever it is dropped.
            self.dock_drag = Some(wassh_ui::DockDrag {
                plugin_id,
                from_pane: false,
            });
            return;
        }
        let Some(plugin_id) = output.clicked else {
            return;
        };
        let Some(id) = wassh_plugins::PluginId::parse(&plugin_id) else {
            return;
        };
        self.toggle_plugin(id);
    }

    fn show_tab_bar(&mut self, ui: &mut egui::Ui) {
        if self.sessions.is_empty() {
            return;
        }
        let palette = self.palette;
        let active = self.active;
        let views: Vec<wassh_ui::TabView<'_>> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| wassh_ui::TabView {
                id: &session.id,
                title: &session.title,
                status: session.status,
                active: active == Some(index),
                accent: session.accent(),
            })
            .collect();

        let mut actions = Vec::new();
        egui::Panel::top("wassh-tabbar")
            .frame(
                egui::Frame::NONE
                    .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel_2))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                actions = wassh_ui::tab_bar(ui, &palette, &views, &mut self.tabs);
            });

        for action in actions {
            match action {
                wassh_ui::TabAction::Select(id) => {
                    self.active = self.sessions.iter().position(|session| session.id == id);
                    self.persist_tabs();
                }
                wassh_ui::TabAction::Close(id) => {
                    if let Some(index) = self.sessions.iter().position(|session| session.id == id) {
                        self.close(index);
                    }
                }
                wassh_ui::TabAction::NewTab => self.open_local(),
                wassh_ui::TabAction::Context(id, (x, y)) => {
                    self.menu
                        .open_at(Pos2::new(x as f32, y as f32), tab_menu_items());
                    self.menu_target = Some(id);
                }
                wassh_ui::TabAction::Reorder { from, to } => {
                    if from < self.sessions.len() && to <= self.sessions.len() {
                        let session = self.sessions.remove(from);
                        let id = session.id.clone();
                        self.sessions.insert(to.min(self.sessions.len()), session);
                        self.active = self.sessions.iter().position(|entry| entry.id == id);
                        self.persist_tabs();
                    }
                }
            }
        }
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = self.palette;
        let collapsed = self.sidebar.collapsed;
        let width = if collapsed {
            wassh_ui::sidebar::COLLAPSED_WIDTH
        } else {
            wassh_ui::sidebar::SIDEBAR_WIDTH
        };
        let mut action = None;
        let mut submitted = None;
        let hosts = &self.hosts;
        let sidebar = &mut self.sidebar;
        let quick = &mut self.quick;

        egui::Panel::left("wassh-sidebar")
            .exact_size(width)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                let quick_height = if collapsed { 0.0 } else { 236.0 };
                let list_height = (ui.available_height() - quick_height).max(80.0);
                ui.allocate_ui(Vec2::new(width, list_height), |ui| {
                    action = wassh_ui::sidebar(ui, &palette, hosts, sidebar);
                });
                if !collapsed {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.set_width(width - 20.0);
                            submitted = quick_connect(ui, &palette, quick);
                        });
                    });
                }
            });

        if let Some(connection) = submitted {
            self.open(connection, None);
        }
        if let Some(action) = action {
            self.handle_sidebar_action(action);
        }
    }

    fn handle_sidebar_action(&mut self, action: wassh_ui::SidebarAction) {
        match action {
            wassh_ui::SidebarAction::OpenHost(id) => {
                if let Some(host) = self.hosts.host(&id).cloned() {
                    self.open(host.connection, Some(host.id));
                }
            }
            wassh_ui::SidebarAction::NewHost => self.handle_command(Command::NewHost),
            wassh_ui::SidebarAction::NewGroup => {
                let id = format!("group-{}", self.next_id);
                self.next_id += 1;
                self.hosts.groups.push(wassh_core::HostGroup {
                    id: id.clone(),
                    name: "New group".to_owned(),
                    color: Some(colors::GROUP_COLOR_PRESETS[4]),
                    collapsed: false,
                });
                if let Some(group) = self.hosts.groups.iter().find(|group| group.id == id).cloned()
                {
                    self.sidebar.begin_rename(&group);
                }
                self.persist_hosts();
            }
            wassh_ui::SidebarAction::ToggleCollapsed => {
                self.sidebar.collapsed = !self.sidebar.collapsed;
                self.save_settings();
            }
            wassh_ui::SidebarAction::ToggleGroup(id) => {
                if let Some(group) = self.hosts.groups.iter_mut().find(|group| group.id == id) {
                    group.collapsed = !group.collapsed;
                }
                self.persist_hosts();
            }
            wassh_ui::SidebarAction::RenameGroup(id, name) => {
                if let Some(group) = self.hosts.groups.iter_mut().find(|group| group.id == id) {
                    group.name = name;
                }
                self.persist_hosts();
            }
            wassh_ui::SidebarAction::HostContext(id, (x, y)) => {
                let items = vec![
                    MenuItem::new("Connect"),
                    MenuItem::new("Edit host…"),
                    MenuItem::new("Duplicate host"),
                    MenuItem::separator(),
                    MenuItem::danger("Delete host"),
                ];
                self.menu.open_at(Pos2::new(x as f32, y as f32), items);
                self.menu_target = Some(format!("host:{id}"));
            }
            wassh_ui::SidebarAction::GroupContext(id, (x, y)) => {
                self.menu.open_at(
                    Pos2::new(x as f32, y as f32),
                    vec![
                        MenuItem::new("Edit group…"),
                        MenuItem::separator(),
                        MenuItem::danger("Delete group"),
                    ],
                );
                self.menu_target = Some(format!("group:{id}"));
            }
            wassh_ui::SidebarAction::MoveHost { host, group, index } => {
                self.move_host(&host, group.as_deref(), index);
            }
        }
    }

    fn move_host(&mut self, host_id: &str, group: Option<&str>, index: usize) {
        if let Some(host) = self.hosts.host_mut(host_id) {
            host.group_id = group.map(str::to_owned);
        }
        self.hosts.ungrouped_host_ids.retain(|id| id != host_id);
        if group.is_none() {
            let position = index.min(self.hosts.ungrouped_host_ids.len());
            self.hosts
                .ungrouped_host_ids
                .insert(position, host_id.to_owned());
        }
        self.hosts = std::mem::take(&mut self.hosts).normalized();
        self.persist_hosts();
    }

    fn show_main(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.palette;
        let Some(index) = self.active else {
            self.show_empty_state(ui);
            return;
        };
        if index >= self.sessions.len() {
            self.active = None;
            self.show_empty_state(ui);
            return;
        }

        self.show_session_banners(ui, index);

        // The drag lives on the shell, but the dock renderer needs it while the session is
        // borrowed, so it is taken out and put back.
        let mut drag = self.dock_drag.take();
        let mut dropped = false;
        let plugin_values = self.plugin_values_for(index);
        let size_after = {
            let session = &mut self.sessions[index];
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                session.terminal.scroll(-(scroll / 20.0).round() as i32);
            }

            // Take the layout out so the pane callbacks can borrow the session.
            let mut layout = std::mem::take(&mut session.layout);
            let measured = session.view.size_for(ui, ui.available_size());
            let mut content = SessionPanes {
                session,
                palette,
                store: self.config.as_ref(),
                plugin_values: &plugin_values,
                measured,
            };
            let actions = wassh_ui::dock_layout(
                ui,
                &palette,
                &mut layout,
                &mut content,
                &mut drag,
            );
            let SessionPanes {
                session,
                measured: content_measured,
                ..
            } = content;
            session.layout = layout;
            for action in actions {
                match action {
                    wassh_ui::DockAction::Close(plugin_id) => session.close_plugin(&plugin_id),
                    wassh_ui::DockAction::HeaderAction(plugin_id) => {
                        session.click_plugin_header_action(&plugin_id);
                    }
                    wassh_ui::DockAction::Resized => {}
                    wassh_ui::DockAction::Drop { plugin_id, target } => {
                        wassh_ui::apply_drop(&mut session.layout, &plugin_id, target);
                        dropped = true;
                    }
                }
            }
            content_measured
        };
        self.dock_drag = drag;
        self.last_terminal_size = size_after;
        self.persist_plugin_data(index);
        if dropped {
            self.persist_tabs();
        }

        self.forward_input(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn show_session_banners(&mut self, ui: &mut egui::Ui, index: usize) {
        let palette = self.palette;
        let host_key = self.sessions[index].host_key_prompt.clone();
        if let Some(prompt) = host_key {
            let mut answer = None;
            egui::Panel::top("wassh-host-key")
                .frame(
                    egui::Frame::NONE
                        .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    wassh_ui::widgets::banner_with(
                        ui,
                        &palette,
                        wassh_ui::widgets::BannerKind::Warn,
                        |ui| {
                            let message = if prompt.mismatch {
                                "WARNING: remote host key does not match the cached key!"
                            } else {
                                "The server host key is not cached."
                            };
                            ui.label(
                                egui::RichText::new(message)
                                    .size(12.0)
                                    .color(wassh_ui::terminal_view::to_color32(palette.warn)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} — {}:{}",
                                    prompt.fingerprint, prompt.host, prompt.port
                                ))
                                .size(11.0)
                                .color(wassh_ui::terminal_view::to_color32(palette.text)),
                            );
                            ui.horizontal(|ui| {
                                if Button::new("Accept").primary().show(ui, &palette).clicked() {
                                    answer = Some(true);
                                }
                                if Button::new("Reject").danger().show(ui, &palette).clicked() {
                                    answer = Some(false);
                                }
                            });
                        },
                    );
                    ui.add_space(6.0);
                });
            if let Some(answer) = answer {
                self.sessions[index].respond_host_key(answer);
            }
        }

        let mut challenge = self.sessions[index].auth_prompt.take();
        if let Some(prompt) = challenge.as_mut() {
            let mut submit = false;
            egui::Panel::top("wassh-auth-prompt")
                .frame(
                    egui::Frame::NONE
                        .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    if !prompt.instructions.is_empty() {
                        ui.label(
                            egui::RichText::new(&prompt.instructions)
                                .size(12.0)
                                .color(wassh_ui::terminal_view::to_color32(palette.warn)),
                        );
                    }
                    if !prompt.name.is_empty() {
                        ui.label(
                            egui::RichText::new(&prompt.name)
                                .size(12.0)
                                .strong()
                                .color(wassh_ui::terminal_view::to_color32(palette.text)),
                        );
                    }
                    for (field_index, field) in prompt.fields.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&field.label)
                                    .size(11.0)
                                    .color(wassh_ui::terminal_view::to_color32(
                                        palette.text_muted,
                                    )),
                            );
                            let edit =
                                egui::TextEdit::singleline(&mut prompt.answers[field_index]);
                            let edit = if field.echo { edit } else { edit.password(true) };
                            ui.add(edit);
                        });
                    }
                    submit = Button::new("Continue")
                        .primary()
                        .show(ui, &palette)
                        .clicked();
                    ui.add_space(6.0);
                });
            if submit && let Some(prompt) = challenge.take() {
                self.sessions[index].respond_auth(prompt.answers);
            } else {
                self.sessions[index].auth_prompt = challenge;
            }
        }

        let session = &self.sessions[index];
        // While the user is typing a password the prompt is the status, like PuTTY.
        if !session.is_live() && !session.awaiting_login() {
            let status = session.status;
            let error = session.error.clone();
            let note = session.status_message.clone();
            let countdown = session.retry_at.map(|at| {
                let remaining = at.saturating_duration_since(std::time::Instant::now());
                let seconds = remaining.as_secs().max(1);
                format!("Attempt {} (in {}s)", session.reconnect_attempt, seconds)
            });
            let can_retry = status.is_retryable() || status == SessionStatus::Closed;
            let mut retry = false;
            egui::Panel::top("wassh-status")
                .frame(
                    egui::Frame::NONE
                        .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                        .inner_margin(egui::Margin::ZERO),
                )
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let text = match status {
                            SessionStatus::Connecting => "Connecting…",
                            SessionStatus::Authenticating => "Authenticating…",
                            SessionStatus::Reconnecting => "Reconnecting…",
                            SessionStatus::AwaitingHostKey => {
                                "Waiting for host-key confirmation…"
                            }
                            SessionStatus::Closed => "Session closed",
                            SessionStatus::Disconnected => "Disconnected",
                            SessionStatus::Failed => "Connection failed",
                            _ => "Not connected",
                        };
                        ui.label(
                            egui::RichText::new(text)
                                .size(12.0)
                                .color(wassh_ui::terminal_view::to_color32(palette.text)),
                        );
                        if let Some(note) = &note {
                            ui.label(
                                egui::RichText::new(note).size(11.0).color(
                                    wassh_ui::terminal_view::to_color32(palette.text_muted),
                                ),
                            );
                        }
                        if let Some(countdown) = &countdown {
                            ui.label(
                                egui::RichText::new(countdown).size(11.0).color(
                                    wassh_ui::terminal_view::to_color32(palette.warn),
                                ),
                            );
                        }
                        if let Some(error) = &error {
                            ui.label(
                                egui::RichText::new(error)
                                    .size(11.0)
                                    .color(wassh_ui::terminal_view::to_color32(palette.danger)),
                            );
                        }
                        if can_retry
                            && Button::new("Reconnect")
                                .compact()
                                .show(ui, &palette)
                                .clicked()
                        {
                            retry = true;
                        }
                    });
                    ui.add_space(6.0);
                });
            if retry {
                self.restart(index, Secrets::default());
            }
        }
    }

    /// Plugin settings that apply to the session at `index`: the app-wide values
    /// plus the ones Host settings stores for its host.
    fn plugin_values_for(&self, index: usize) -> PluginValues {
        let host_id = self.sessions.get(index).and_then(|session| session.host_id.clone());
        let host = host_id
            .as_deref()
            .and_then(|host_id| self.hosts.host(host_id));
        PluginValues {
            app: self.settings.plugin_settings.clone(),
            host: host
                .map(|host| host.plugin_settings.clone())
                .unwrap_or_default(),
            secrets: self.plugin_secrets_for(host_id.as_deref()),
        }
    }

    fn show_empty_state(&mut self, ui: &mut egui::Ui) {
        let palette = self.palette;
        let mut open_local = false;
        let mut new_host = false;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(20.0));
            ui.label(
                egui::RichText::new(APP_NAME)
                    .size(28.0)
                    .strong()
                    .color(wassh_ui::terminal_view::to_color32(palette.text)),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Quick-connect from the sidebar or open a saved host.")
                    .size(13.0)
                    .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let width = ui.available_width();
                ui.add_space((width - 260.0).max(0.0) / 2.0);
                if Button::new("New local terminal")
                    .primary()
                    .show(ui, &palette)
                    .clicked()
                {
                    open_local = true;
                }
                if Button::new("New host…").show(ui, &palette).clicked() {
                    new_host = true;
                }
            });
        });
        if open_local {
            self.open_local();
        }
        if new_host {
            self.handle_command(Command::NewHost);
        }
    }

    fn show_dialogs(&mut self, ctx: &egui::Context) {
        let palette = self.palette;
        let matches = self.find_matches;
        let hosts = self.hosts.hosts.clone();
        self.dialogs.enabled_plugins = self.settings.enabled_plugins.clone();
        let actions = self.dialogs.show(ctx, &palette, &hosts, matches);
        self.handle_dialog_actions(actions);
    }

    fn show_context_menu(&mut self, ctx: &egui::Context) {
        let palette = self.palette;
        let Some(index) = self.menu.show(ctx, &palette) else {
            return;
        };
        let Some(target) = self.menu_target.take() else {
            return;
        };

        if let Some(id) = target.strip_prefix("host:") {
            let id = id.to_owned();
            match index {
                0 => {
                    if let Some(host) = self.hosts.host(&id).cloned() {
                        self.open(host.connection, Some(host.id));
                    }
                }
                1 => {
                    if let Some(host) = self.hosts.host(&id).cloned() {
                        self.dialogs.host = Some(self.host_state(host, false));
                    }
                }
                2 => self.duplicate_host(&id),
                4 => {
                    self.delete_host(&id);
                    self.toasts.push(ToastKind::Info, "Host deleted.");
                }
                _ => {}
            }
            return;
        }
        if let Some(id) = target.strip_prefix("group:") {
            let id = id.to_owned();
            match index {
                0 => {
                    if let Some(group) = self.hosts.groups.iter().find(|group| group.id == id) {
                        self.dialogs.group_edit = Some(GroupEditState::new(group));
                    }
                }
                2 => {
                    self.hosts.groups.retain(|group| group.id != id);
                    for host in &mut self.hosts.hosts {
                        if host.group_id.as_deref() == Some(id.as_str()) {
                            host.group_id = None;
                        }
                    }
                    self.hosts = std::mem::take(&mut self.hosts).normalized();
                    self.persist_hosts();
                }
                _ => {}
            }
            return;
        }
        if let Some(position) = self
            .sessions
            .iter()
            .position(|session| session.id == target)
            && let Some(command) = tab_menu_command(index)
        {
            self.active = Some(position);
            self.handle_command(command);
        }
    }
}

impl eframe::App for Shell {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        wassh_ui::terminal_view::to_color32(self.palette.bg_app).to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
        if self.perf_probe {
            self.frames += 1;
            // Give the renderer a moment to settle so the memory figure is representative.
            if self.frames == 30 {
                self.perf_probe = false;
                let start = self.first_frame_ms.unwrap_or_default();
                let rss = resident_bytes()
                    .map(|bytes| format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)))
                    .unwrap_or_else(|| "unknown".to_owned());
                println!("cold start: {start:.0} ms (process start to first frame)");
                println!("resident: {rss} after {} frames", self.frames);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self) {
        self.persist_window_bounds(true);
    }
}

/// Resident memory of this process, in bytes, where the platform reports it.
pub fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(resident_pages * 4096)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

impl Shell {
    /// One frame of the whole application.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.follow_system_theme(&ctx);
        // egui's own widgets (combo boxes, scrollbars) draw from its visuals, so they are
        // rebuilt whenever the palette changes, including a theme switch in Options.
        if self.styled_palette != Some(self.palette) {
            self.styled_palette = Some(self.palette);
            apply_visuals(&ctx, self.palette);
        }
        if self.first_frame_ms.is_none() {
            self.first_frame_ms = Some(self.started_at.elapsed().as_secs_f64() * 1000.0);
        }
        if let Some(fullscreen) = self.fullscreen_request.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
        }
        if self.quit_requested {
            self.persist_window_bounds(true);
            if let Some(config) = self.config.as_mut() {
                let _ = config.flush();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // The saved position is applied to the real window on the first frame; until then
        // the reported geometry is not the user's.
        let restoring = self.restore_window_position(&ctx);
        self.frame_body(ui);
        if !restoring {
            self.track_window_bounds(&ctx);
        }
        self.persist_window_bounds(false);
    }

    /// Everything inside the window frame.
    fn frame_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        self.pump(&ctx);
        self.shortcuts(&ctx);
        self.update_find_matches();

        // The desktop owns the title bar, the window buttons, the resize border and the
        // shadow, so the app only adds its menu strip underneath.
        let checked = self.active_plugin_labels();
        let state = wassh_ui::titlebar::MenuState {
            checked: &checked,
            gears: PLUGIN_MENU_LABELS,
        };
        let action = wassh_ui::show_menu_bar(ui, &self.palette, MENUS, state);
        match action {
            Some(wassh_ui::TitlebarAction::MenuItem(item)) => self.menu_command(&ctx, item),
            Some(wassh_ui::TitlebarAction::MenuGear(item)) => self.open_plugin_options(item),
            _ => {}
        }

        // The sidebar claims the left edge first, so the tab strip starts at the sidebar's
        // edge instead of running to the window border.
        self.show_sidebar(ui);
        self.show_tab_bar(ui);
        self.show_plugin_toolbar(ui);
        self.show_main(ui, &ctx);
        self.show_dialogs(&ctx);
        self.show_context_menu(&ctx);

        self.toasts.show(ui, &self.palette);
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        if self.sessions.is_empty() && self.active.is_some() {
            self.active = None;
        }

        // The window is up, so the tabs from the last run can be spawned now; they appear on
        // the next frame instead of delaying this one.
        if self.restore_pending {
            self.finish_startup();
            ctx.request_repaint();
        }
    }

    /// Re-colour everything when the desktop switches between light and dark.
    ///
    /// Only matters while the setting is `ThemeMode::System`; an explicit choice is left
    /// alone.
    fn follow_system_theme(&mut self, ctx: &egui::Context) {
        let system_dark = system_prefers_dark(ctx);
        if self.system_dark == Some(system_dark) {
            return;
        }
        self.system_dark = Some(system_dark);
        if self.settings.theme != wassh_core::ThemeMode::System {
            return;
        }
        self.palette = Palette::for_mode(self.settings.theme.resolve(system_dark));
        let palette = self.palette;
        let defaults = self.settings.session_style_defaults.clone();
        for session in &mut self.sessions {
            session.palette = palette;
            session.apply_style(&defaults);
        }
        ctx.request_repaint();
    }

    /// Reopen the tabs from the last run, once the window has been drawn.
    fn finish_startup(&mut self) {
        if !self.restore_pending {
            return;
        }
        self.restore_pending = false;
        self.restore_tabs();
        for _ in 0..self.perf_tabs {
            self.open_local();
        }
        self.perf_tabs = 0;
    }
}

/// Drop plain-text copies of a plugin's `secret: true` fields.
///
/// Those fields live in the credential vault; a settings map must never carry
/// one, or `settings.json` and `hosts.json` would hold a readable password.
fn strip_plugin_secrets(settings: &mut wassh_core::PluginSettings) {
    settings.retain(|plugin, values| {
        if let Some(id) = PluginId::parse(plugin) {
            for (key, _) in wassh_plugins::settings::secret_fields(id) {
                values.remove(key);
            }
        }
        !values.is_empty()
    });
}

/// The value a [`SecretEdit`] stands for: `None` keeps what is stored.
fn secret_value(edit: &SecretEdit) -> Option<Option<&str>> {
    match edit {
        SecretEdit::Unchanged => None,
        SecretEdit::Set(value) => Some(Some(value.as_str())),
        SecretEdit::Clear => Some(None),
    }
}

/// Borrowed session state handed to the dock renderer.
///
/// The renderer draws the terminal and every plugin pane through this one value, so the
/// dock never has to know how a session stores its transport or its emulator.
struct SessionPanes<'a> {
    session: &'a mut SessionEntry,
    palette: Palette,
    /// Configuration store, used to load and save plugin data.
    store: Option<&'a wassh_store::Config>,
    /// Plugin settings the dialogs hold for this session.
    plugin_values: &'a PluginValues,
    /// Emulator size measured for the space the docks left for the terminal.
    measured: TerminalSize,
}

impl wassh_ui::DockContent for SessionPanes<'_> {
    fn terminal(&mut self, ui: &mut egui::Ui) {
        let size = self.session.view.size_for(ui, ui.available_size());
        self.session.resize(size);
        self.measured = size;
        let snapshot = self.session.snapshot();
        let response = self.session.view.show(ui, &snapshot, &self.palette);
        self.session.track_selection(ui, &response, &snapshot);
    }

    fn pane(&mut self, ui: &mut egui::Ui, plugin_id: &str) {
        self.session
            .show_plugin(ui, plugin_id, self.store, self.plugin_values);
    }

    fn pane_title(&self, plugin_id: &str) -> String {
        self.session.plugin_title(plugin_id)
    }

    fn pane_status(&self, plugin_id: &str) -> Option<(String, wassh_ui::dock::HeaderHealth)> {
        self.session.plugin_status(plugin_id)
    }

    fn pane_header_action(&self, plugin_id: &str) -> Option<wassh_ui::dock::HeaderAction> {
        self.session
            .plugin_header_action(plugin_id)
            .map(|button| wassh_ui::dock::HeaderAction {
                label: button.label,
                enabled: button.enabled,
            })
    }
}

fn apply_visuals(ctx: &egui::Context, palette: Palette) {
    wassh_ui::apply_visuals(ctx, &palette);
}

#[cfg(test)]
mod tests {
    /// Run one frame with `events` as input and return what the platform was asked to do.
    fn frame_with_events(
        shell: &mut Shell,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| shell.frame(ui));
        // Keep the output for inspection but drop the texture deltas egui insists on.
        let cleaned = egui::FullOutput {
            platform_output: std::mem::take(&mut output.platform_output),
            textures_delta: Default::default(),
            shapes: Vec::new(),
            pixels_per_point: output.pixels_per_point,
            viewport_output: std::mem::take(&mut output.viewport_output),
        };
        output.drop_without_applying_deltas();
        cleaned
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn ctrl_w_closes_the_active_tab() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        shell.open_local();
        assert_eq!(shell.sessions.len(), 2);

        frame_with_events(&mut shell, vec![key_event(egui::Key::W, egui::Modifiers::CTRL)]);
        assert_eq!(shell.sessions.len(), 1, "Ctrl+W closed one tab");
    }

    #[test]
    fn ctrl_shift_c_copies_the_terminal_selection() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        {
            let session = &mut shell.sessions[0];
            session.terminal.feed(b"selected text");
            session.terminal.begin_selection(0, 0);
            session.terminal.extend_selection(0, 12);
        }

        let output = frame_with_events(
            &mut shell,
            vec![key_event(egui::Key::C, egui::Modifiers::CTRL | egui::Modifiers::SHIFT)],
        );
        let copied = output.platform_output.commands.iter().find_map(|command| {
            match command {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            }
        });
        assert_eq!(copied.as_deref(), Some("selected text"));
    }

    #[test]
    fn classic_insert_bindings_copy_and_paste() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        {
            let session = &mut shell.sessions[0];
            session.terminal.feed(b"abc");
            session.terminal.begin_selection(0, 0);
            session.terminal.extend_selection(0, 2);
        }

        // Ctrl+Insert copies.
        let output = frame_with_events(
            &mut shell,
            vec![key_event(egui::Key::Insert, egui::Modifiers::CTRL)],
        );
        let copied = output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "abc")
        });
        assert!(copied, "Ctrl+Insert copies the selection");

        // Shift+Insert asks the integration for the clipboard.
        let output = frame_with_events(
            &mut shell,
            vec![key_event(egui::Key::Insert, egui::Modifiers::SHIFT)],
        );
        let requested = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::RequestPaste))
            });
        assert!(requested, "Shift+Insert requests a paste");
    }

    #[test]
    fn typed_archive_paths_expand_the_home_shortcut() {
        let home = home_dir();
        assert_eq!(expand_home("~/backup.zip"), home.join("backup.zip"));
        assert_eq!(
            expand_home("/tmp/backup.zip"),
            std::path::PathBuf::from("/tmp/backup.zip")
        );
        assert_eq!(expand_home("backup.zip"), std::path::PathBuf::from("backup.zip"));
    }

    #[test]
    fn a_backup_round_trips_through_the_store() {
        let dir = std::env::temp_dir().join(format!("wassh-ui-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("data dir");
        std::fs::write(dir.join("hosts.json"), b"{\"hosts\":[]}").expect("write");

        let archive = dir.join("backup.zip");
        let written = wassh_store::export_backup(&dir, &archive).expect("export");
        assert_eq!(written.files, 1);

        let restored = dir.join("restored");
        let read = wassh_store::import_backup(&archive, &restored).expect("import");
        assert_eq!(read.files, 1);
        assert!(restored.join("hosts.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_config() -> Config {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wassh-shell-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut config = Config::open(&dir).expect("config");
        config
            .settings
            .update(|settings| settings.reconnect_on_startup = false)
            .expect("settings");
        config
    }

    /// Run `frames` headless frames of the whole shell.
    fn run_shell(shell: &mut Shell, ctx: &egui::Context, frames: usize) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let mut shapes = 0;
        for _ in 0..frames {
            let output = ctx.run_ui(input.clone(), |ui| shell.frame(ui));
            shapes = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        shapes
    }

    #[test]
    fn the_new_host_dialog_opens_and_keeps_painting() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.handle_command(Command::NewHost);
        assert!(shell.dialogs.host.is_some(), "the command opened the dialog");

        let ctx = egui::Context::default();
        let _ = run_shell(&mut shell, &ctx, 3);
        assert!(
            shell.dialogs.host.is_some(),
            "the dialog stays open while the app draws"
        );
        let id = egui::Id::unique(("wassh-settings", "host-settings"));
        let rect = ctx.memory(|memory| memory.area_rect(id));
        let rect = rect.expect("the dialog area is on screen");
        assert!(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1280.0, 800.0))
                .contains_rect(rect),
            "dialog rect {rect:?} is outside the window"
        );
    }

    fn password_tab(host_id: Option<&str>) -> wassh_core::TabSnapshot {
        wassh_core::TabSnapshot {
            id: "tab-1".to_owned(),
            host_id: host_id.map(str::to_owned),
            active: true,
            connection: ConnectionParams {
                kind: ConnectionType::Ssh,
                host: "127.0.0.1".to_owned(),
                port: 1,
                username: "alice".to_owned(),
                auth: AuthMethod::Password,
                ..ConnectionParams::default()
            },
            ..wassh_core::TabSnapshot::default()
        }
    }

    #[test]
    fn saved_tabs_reopen_and_ask_for_the_password_in_the_terminal() {
        let mut config = scratch_config();
        config.settings.get_mut().reconnect_on_startup = true;
        config.tabs.set(vec![password_tab(Some("h1"))]).unwrap();

        let mut shell = Shell::new_with(Some(config), false);
        assert!(shell.sessions.is_empty(), "restoring waits for the first frame");
        shell.finish_startup();
        assert_eq!(shell.sessions.len(), 1, "the tab was reopened");
        assert_eq!(shell.active, Some(0));
        assert_eq!(shell.sessions[0].host_id.as_deref(), Some("h1"));
        assert!(
            shell.sessions[0].awaiting_login(),
            "a password tab must ask in the terminal"
        );
        // New tabs must not reuse a restored id.
        assert!(shell.next_id >= 2, "next_id was {}", shell.next_id);

        // The window state is written back, keeping the host link.
        let raw = std::fs::read_to_string(
            wassh_store::paths::tabs_file(&shell.config.as_ref().unwrap().dir),
        )
        .unwrap();
        assert!(raw.contains("\"hostId\": \"h1\""), "tabs.json was {raw}");
    }

    #[test]
    fn a_perf_probe_uses_an_empty_configuration() {
        let mut shell = Shell::new_for_test(None, true, 3);
        assert!(shell.config.is_none(), "the probe must not touch real config");
        shell.finish_startup();
        // Only the shells it was asked for; nothing restored, nothing saved.
        assert_eq!(shell.sessions.len(), 3);
        shell.persist_tabs();
        assert!(shell.config.is_none());
    }

    #[test]
    fn restoring_into_a_window_that_already_has_tabs_does_nothing() {
        let mut config = scratch_config();
        config.settings.get_mut().reconnect_on_startup = true;
        config.tabs.set(vec![password_tab(None)]).unwrap();

        let mut shell = Shell::new_with(Some(config), false);
        shell.finish_startup();
        assert_eq!(shell.sessions.len(), 1);

        // A second restore would otherwise duplicate every saved tab.
        shell.restore_tabs();
        assert_eq!(shell.sessions.len(), 1);
    }

    #[test]
    fn startup_restores_each_saved_tab_exactly_once() {
        let mut config = scratch_config();
        config.settings.get_mut().reconnect_on_startup = true;
        let mut tabs = vec![
            wassh_core::TabSnapshot {
                id: "tab-4".to_owned(),
                connection: ConnectionParams {
                    kind: ConnectionType::Local,
                    ..ConnectionParams::default()
                },
                ..wassh_core::TabSnapshot::default()
            },
            password_tab(Some("h1")),
        ];
        tabs[0].active = true;
        config.tabs.set(tabs).unwrap();

        let mut shell = Shell::new_with(Some(config), false);
        shell.finish_startup();
        assert_eq!(shell.sessions.len(), 2);

        // Frames keep the tab set stable; a restore that runs more than once shows up here.
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        for _ in 0..4 {
            let output = ctx.run_ui(input.clone(), |ui| shell.frame(ui));
            output.drop_without_applying_deltas();
        }
        assert_eq!(shell.sessions.len(), 2, "frames must not open more tabs");
    }

    #[test]
    fn a_saved_password_connects_without_prompting_on_restore() {
        let mut config = scratch_config();
        config.settings.get_mut().reconnect_on_startup = true;
        config.tabs.set(vec![password_tab(Some("h1"))]).unwrap();
        {
            let dir = config.dir.clone();
            let mut store = wassh_store::SecretsStore::open(&dir).unwrap();
            store.set_password("h1", Some("hunter2")).unwrap();
        }

        let mut shell = Shell::new_with(Some(config), false);
        shell.finish_startup();
        assert_eq!(shell.sessions.len(), 1);
        assert!(!shell.sessions[0].awaiting_login());
        assert_eq!(shell.sessions[0].secrets.password, "hunter2");
    }

    #[test]
    fn changing_default_settings_updates_open_sessions() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        assert_eq!(shell.sessions.len(), 1);
        assert_eq!(shell.sessions[0].view.font_size, 14.0);

        shell.settings.session_style_defaults.font_size = 22;
        shell.save_settings();
        assert_eq!(
            shell.sessions[0].view.font_size, 22.0,
            "the new default applied to the open tab"
        );

        // A session with its own override keeps it.
        shell.sessions[0].connection.style.font_size = Some(9);
        shell.settings.session_style_defaults.font_size = 30;
        shell.save_settings();
        assert_eq!(shell.sessions[0].view.font_size, 9.0);
    }

    #[test]
    fn applying_a_host_updates_its_open_sessions() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let mut entry = crate::app::session::SessionEntry::new_local(
            "tab-x",
            &shell.palette,
            &shell.settings.session_style_defaults,
            "xterm-256color",
            TerminalSize::new(80, 24),
        );
        entry.host_id = Some("h1".to_owned());
        shell.sessions.push(entry);

        let mut host = HostProfile {
            id: "h1".to_owned(),
            name: "Dev".to_owned(),
            ..HostProfile::default()
        };
        host.connection.kind = ConnectionType::Ssh;
        host.connection.host = "192.168.42.7".to_owned();
        host.connection.style.font_size = Some(20);

        assert_eq!(shell.apply_host_to_sessions(&host), 1);
        assert_eq!(shell.sessions[0].view.font_size, 20.0);
        assert_eq!(shell.sessions[0].connection.host, "192.168.42.7");

        // A host with no open session is a no-op.
        assert_eq!(shell.apply_host_to_sessions(&HostProfile::default()), 0);
    }

    #[test]
    fn the_window_rectangle_is_remembered_in_physical_pixels() {
        let dir = std::env::temp_dir().join(format!(
            "wassh-window-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config = Config::open(&dir).expect("config");
        let mut shell = Shell::new_with(Some(config), false);

        let ctx = egui::Context::default();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let normal = egui::ViewportInfo {
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(10.0, 20.0),
                egui::vec2(800.0, 610.0),
            )),
            inner_rect: Some(egui::Rect::from_min_size(
                egui::pos2(12.0, 50.0),
                egui::vec2(796.0, 580.0),
            )),
            maximized: Some(false),
            ..Default::default()
        };
        input.viewports.insert(egui::ViewportId::ROOT, normal);
        let output = ctx.run_ui(input.clone(), |ui| {
            let ctx = ui.ctx().clone();
            shell.track_window_bounds(&ctx);
        });
        output.drop_without_applying_deltas();

        // The outer rect is physical, the inner size stays in points.
        assert_eq!(
            shell.settings.window_bounds,
            Some(WindowBounds {
                x: 10,
                y: 20,
                width: 796,
                height: 580,
                maximized: false,
            })
        );

        // Maximizing keeps the normal rectangle next to the flag.
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(2560.0, 1440.0),
                )),
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(2560.0, 1400.0),
                )),
                maximized: Some(true),
                ..Default::default()
            },
        );
        let output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            shell.track_window_bounds(&ctx);
        });
        output.drop_without_applying_deltas();
        assert_eq!(
            shell.settings.window_bounds,
            Some(WindowBounds {
                x: 10,
                y: 20,
                width: 796,
                height: 580,
                maximized: true,
            })
        );

        shell.persist_window_bounds(true);
        let reopened = Config::open(&dir).expect("reopen");
        assert_eq!(
            reopened.settings.get().window_bounds,
            shell.settings.window_bounds
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_dialog_save_stays_on_its_tab() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let mut host = HostProfile {
            id: "h1".to_owned(),
            name: "Dev".to_owned(),
            ..HostProfile::default()
        };
        host.connection.kind = ConnectionType::Ssh;
        host.connection.host = "192.0.2.1".to_owned();
        shell.save_host(host.clone());

        let open = |id: &str, shell: &mut Shell| {
            let mut entry = crate::app::session::SessionEntry::new_local(
                id,
                &shell.palette,
                &shell.settings.session_style_defaults,
                "xterm-256color",
                TerminalSize::new(80, 24),
            );
            entry.host_id = Some("h1".to_owned());
            entry.connection = host.connection.clone();
            shell.sessions.push(entry);
        };
        open("tab-1", &mut shell);
        open("tab-2", &mut shell);
        shell.active = Some(0);

        // Only the host dialog of a saved host offers Apply to sessions.
        assert!(shell.host_state(host.clone(), false).can_apply_to_sessions());
        let session_dialog = shell.session_state(host.clone(), "tab-1");
        assert!(!session_dialog.can_apply_to_sessions());
        assert!(!session_dialog.can_delete());

        let mut edited = host.connection.clone();
        edited.host = "10.0.0.5".to_owned();
        edited.style.font_size = Some(21);
        let save = HostSave {
            host: HostProfile {
                id: "h1".to_owned(),
                connection: edited,
                ..HostProfile::default()
            },
            password: SecretEdit::Set("hunter2".to_owned()),
            passphrase: SecretEdit::Unchanged,
            plugin_secrets: PluginSecretDrafts::new(),
        };
        assert!(shell.apply_to_session("tab-1", &save));
        assert!(!shell.apply_to_session("gone", &save));

        assert_eq!(shell.sessions[0].connection.host, "10.0.0.5");
        assert_eq!(shell.sessions[0].view.font_size, 21.0);
        assert_eq!(shell.sessions[0].secrets.password, "hunter2");
        // The host's other tab and the saved host keep their own settings, and the
        // password never reaches the vault.
        assert_eq!(shell.sessions[1].connection.host, "192.0.2.1");
        assert_eq!(
            shell.hosts.host("h1").expect("saved host").connection.host,
            "192.0.2.1"
        );
        assert_eq!(shell.stored_password(Some("h1")), None);
    }

    #[test]
    fn a_saved_password_is_stored_encrypted_and_reused() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        assert_eq!(shell.stored_password(Some("host-1")), None);

        let save = HostSave {
            host: HostProfile {
                id: "host-1".to_owned(),
                name: "Example".to_owned(),
                connection: ConnectionParams {
                    kind: ConnectionType::Ssh,
                    host: "example.com".to_owned(),
                    username: "alice".to_owned(),
                    auth: AuthMethod::Password,
                    ..ConnectionParams::default()
                },
                ..HostProfile::default()
            },
            password: SecretEdit::Set("hunter2".to_owned()),
            passphrase: SecretEdit::Unchanged,
            plugin_secrets: PluginSecretDrafts::new(),
        };
        shell.apply_secret_edits(&save);
        shell.save_host(save.host.clone());

        assert_eq!(
            shell.stored_password(Some("host-1")).as_deref(),
            Some("hunter2")
        );
        assert_eq!(shell.stored_secrets("host-1"), (true, false));

        // The password is not in hosts.json.
        let hosts = std::fs::read_to_string(
            wassh_store::paths::hosts_file(&shell.config.as_ref().unwrap().dir),
        )
        .unwrap();
        assert!(!hosts.contains("hunter2"), "hosts.json was {hosts}");

        // Clearing it removes the secret.
        shell.apply_secret_edits(&HostSave {
            host: save.host.clone(),
            password: SecretEdit::Clear,
            passphrase: SecretEdit::Unchanged,
            plugin_secrets: PluginSecretDrafts::new(),
        });
        assert_eq!(shell.stored_password(Some("host-1")), None);
    }

    fn host(id: &str, name: &str, group: Option<&str>) -> HostProfile {
        HostProfile {
            id: id.to_owned(),
            name: name.to_owned(),
            group_id: group.map(str::to_owned),
            connection: wassh_core::ConnectionParams {
                host: format!("{id}.example.com"),
                port: 2222,
                username: "root".to_owned(),
                ..Default::default()
            },
            ..HostProfile::default()
        }
    }

    #[test]
    fn duplicating_a_host_places_the_copy_next_to_it() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.save_host(host("host-1", "Alpha", None));
        shell.save_host(host("host-2", "Beta", None));

        shell.duplicate_host("host-2");

        let ids: Vec<_> = shell.hosts.hosts.iter().map(|host| host.id.as_str()).collect();
        let names: Vec<_> = shell
            .hosts
            .hosts
            .iter()
            .map(|host| host.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Beta", "Beta (copy)"]);
        // The ungrouped order matches the document order.
        assert_eq!(
            shell.hosts.ungrouped_host_ids,
            ["host-1", "host-2", ids[2]]
        );

        let copy = &shell.hosts.hosts[2];
        assert_ne!(copy.id, "host-2", "the copy gets a fresh id");
        assert_eq!(copy.connection.host, "host-2.example.com");
        assert_eq!(copy.connection.port, 2222);
        assert_eq!(copy.connection.username, "root");
        assert_eq!(copy.group_id, None);
        // It is on disk too.
        let stored = shell.config.as_ref().expect("config").hosts.get();
        assert_eq!(stored.hosts.len(), 3);
    }

    #[test]
    fn duplicating_a_grouped_host_keeps_its_group_and_position() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.hosts.groups.push(wassh_core::HostGroup {
            id: "g1".to_owned(),
            name: "Ops".to_owned(),
            color: None,
            collapsed: false,
        });
        shell.save_host(host("host-1", "Alpha", Some("g1")));
        shell.save_host(host("host-2", "Beta", Some("g1")));

        shell.duplicate_host("host-1");

        let members: Vec<_> = shell
            .hosts
            .hosts_in_group("g1")
            .iter()
            .map(|host| host.name.as_str())
            .collect();
        assert_eq!(members, ["Alpha", "Alpha (copy)", "Beta"]);
        assert!(shell.hosts.ungrouped_host_ids.is_empty());
    }

    #[test]
    fn duplicating_an_unknown_host_does_nothing() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.save_host(host("host-1", "Alpha", None));
        shell.duplicate_host("nope");
        assert_eq!(shell.hosts.hosts.len(), 1);
    }

    #[test]
    fn deleting_a_host_forgets_its_password() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let host = HostProfile {
            id: "host-9".to_owned(),
            name: "Gone".to_owned(),
            ..HostProfile::default()
        };
        shell.save_host(host.clone());
        shell.apply_secret_edits(&HostSave {
            host: host.clone(),
            password: SecretEdit::Set("hunter2".to_owned()),
            passphrase: SecretEdit::Set("key-pass".to_owned()),
            plugin_secrets: PluginSecretDrafts::new(),
        });
        assert_eq!(shell.stored_secrets("host-9"), (true, true));
        shell.delete_host("host-9");
        assert_eq!(shell.stored_secrets("host-9"), (false, false));
    }


    /// A draft set with one plugin secret typed by the user.
    fn secret_drafts(plugin: &str, field: &str, value: &str) -> PluginSecretDrafts {
        [(
            self::dialogs::secret_field_key(plugin, field),
            self::dialogs::SecretDraft {
                draft: value.to_owned(),
                stored: false,
                clear: false,
            },
        )]
        .into_iter()
        .collect()
    }

    #[test]
    fn a_plugin_password_is_sealed_in_the_vault_and_reaches_the_pane() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.save_host(HostProfile {
            id: "host-1".to_owned(),
            name: "Broker".to_owned(),
            ..HostProfile::default()
        });
        shell.apply_plugin_secrets("host-1", &secret_drafts("mqtt-analyser", "password", "hunter2"));
        shell.apply_plugin_secrets(
            wassh_store::secrets::APP_SCOPE,
            &secret_drafts("ai-agent", "webSearchApiKey", "web-key"),
        );

        // Sealed, exactly like a host password.
        let store = shell.secrets.as_ref().expect("vault");
        assert_eq!(
            store
                .plugin_secret("host-1", "mqtt-analyser", "password")
                .as_deref(),
            Some("hunter2")
        );
        let config = shell.config.as_ref().expect("config");
        let hosts = std::fs::read_to_string(wassh_store::paths::hosts_file(&config.dir)).unwrap();
        assert!(!hosts.contains("hunter2"), "hosts.json was {hosts}");
        let settings =
            std::fs::read_to_string(wassh_store::paths::settings_file(&config.dir)).unwrap();
        assert!(!settings.contains("web-key"), "settings.json was {settings}");

        // A pane of that host receives the decrypted values, host and app alike.
        let secrets = shell.plugin_secrets_for(Some("host-1"));
        assert_eq!(secrets["mqtt-analyser"]["password"], "hunter2");
        assert_eq!(secrets["ai-agent"]["webSearchApiKey"], "web-key");
        // Another host must not see the host-scoped one.
        assert!(!shell.plugin_secrets_for(Some("host-2")).contains_key("mqtt-analyser"));

        // Clearing it forgets the value.
        let mut clear: PluginSecretDrafts = secret_drafts("mqtt-analyser", "password", "");
        if let Some(draft) = clear.values_mut().next() {
            draft.clear = true;
        }
        shell.apply_plugin_secrets("host-1", &clear);
        assert!(
            !shell
                .secrets
                .as_ref()
                .unwrap()
                .has_plugin_secret("host-1", "mqtt-analyser", "password")
        );
    }

    #[test]
    fn a_plain_text_plugin_password_is_moved_into_the_vault_on_startup() {
        let mut config = scratch_config();
        let mut host = HostProfile {
            id: "host-1".to_owned(),
            name: "Broker".to_owned(),
            ..HostProfile::default()
        };
        host.plugin_settings.insert(
            "mqtt-analyser".to_owned(),
            [("password".to_owned(), serde_json::json!("hunter2"))]
                .into_iter()
                .collect(),
        );
        config
            .hosts
            .update(|hosts| {
                hosts.hosts.push(host.clone());
                hosts.ungrouped_host_ids.push("host-1".to_owned());
            })
            .expect("hosts");
        let file = wassh_store::paths::hosts_file(&config.dir);
        assert!(std::fs::read_to_string(&file).unwrap().contains("hunter2"));

        let shell = Shell::new_with(Some(config), false);
        assert_eq!(
            shell
                .secrets
                .as_ref()
                .expect("vault")
                .plugin_secret("host-1", "mqtt-analyser", "password")
                .as_deref(),
            Some("hunter2")
        );
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(!written.contains("hunter2"), "hosts.json was {written}");
        // And the pane still finds it.
        assert_eq!(
            shell.plugin_secrets_for(Some("host-1"))["mqtt-analyser"]["password"],
            "hunter2"
        );
    }

    #[test]
    fn the_gear_in_the_plugins_menu_opens_the_plugin_options() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.settings.enabled_plugins = vec!["macro-pad".to_owned()];
        // "Macros" is the macro pad's toolbar label.
        shell.open_plugin_options("Macros");
        let state = shell.dialogs.options.as_ref().expect("options opened");
        assert_eq!(
            state.nav.active, 6,
            "the macro pad section follows the six built-in ones"
        );

        // A label that is not a plugin does nothing.
        shell.dialogs.options = None;
        shell.open_plugin_options("Nonexistent");
        assert!(shell.dialogs.options.is_none());
    }

    #[test]
    fn the_options_dialog_opens_too() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.handle_command(Command::OpenOptions);
        assert!(shell.dialogs.options.is_some());
        let ctx = egui::Context::default();
        let _ = run_shell(&mut shell, &ctx, 3);
        assert!(shell.dialogs.options.is_some());
    }
}

