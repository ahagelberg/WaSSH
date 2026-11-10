//! The application shell.
//!
//! Owns the window chrome, the sidebar, the tab strip and the terminal area, plus every
//! dialog and the session list. State that survives a restart lives in `wassh-store`;
//! everything else is rebuilt on launch.

pub mod dialogs;
pub mod local_exec;
pub mod local_files;
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
    HostTarget, IdentityDraft, OptionsState, PaletteState, PluginSecretDrafts, SecretEdit, tab_menu_command,
    tab_menu_items,
};
use self::quick_connect::{QuickForm, quick_connect};
use self::session::{
    PluginValues, Secrets, SessionEntry, SshOpen, jump_chain, open_host_sessions, resolve_auth,
};

/// How long the window must stay put before its geometry is written to `settings.json`.
const WINDOW_BOUNDS_SETTLE: Duration = Duration::from_millis(700);

/// How long after the last terminal activity the app keeps repainting at the active rate.
///
/// Typing and a command's output both fit inside one window: the keystroke extends it, the
/// echo arrives a few milliseconds later and extends it again.
const ACTIVE_LINGER: Duration = Duration::from_millis(400);

/// Repaint interval while output arrives or the user is typing (about 60 fps).
const ACTIVE_INTERVAL: Duration = Duration::from_millis(16);

/// Largest window-frame offset (borders plus title bar, in points) that is believed.
///
/// A window manager that ignored the restore request reports the distance from the old
/// position instead, which is far larger than a decoration; that is not learned.
const MAX_FRAME_OFFSET: f32 = 200.0;

/// Frames to wait after asking the window manager to move the window, before reading back
/// where it landed.
const RESTORE_FRAMES: u8 = 2;

/// Repaint interval while waiting for a requested window move to land.
const RESTORE_POLL: Duration = Duration::from_millis(30);

/// How long a requested clipboard paste may take to arrive.
///
/// A `Paste` event after this belongs to the platform's Ctrl+V and goes to the shell.
const PASTE_WAIT: Duration = Duration::from_secs(1);

/// The program icon shown in the task bar and in Alt-Tab.
const ICON_PNG: &[u8] = include_bytes!("../../../../res/WaSSH program icon.png");

/// Repaint interval when nothing is happening.
///
/// The transports and the worker threads wake the window as soon as they have something
/// to show (`wassh_core::wake`), so this only has to cover what cannot: a slow idle
/// cadence keeps a static window from repainting its whole terminal every few frames.
const IDLE_INTERVAL: Duration = Duration::from_secs(1);

/// Menus shown in the titlebar.
///
/// The Plugins rows are the registry's toolbar labels, so a built-in appears in the menu
/// without a second list naming it.
fn menus() -> &'static [wassh_ui::titlebar::Menu] {
    static MENUS: std::sync::OnceLock<Vec<wassh_ui::titlebar::Menu>> = std::sync::OnceLock::new();
    MENUS.get_or_init(|| {
        let plugins: &'static [&'static str] = Box::leak(
            wassh_plugins::toolbar_plugins()
                .map(|manifest| manifest.toolbar_label)
                .collect(),
        );
        vec![
            ("File", &["Options…", "", "Quit"]),
            ("Edit", &["Copy", "Paste", "", "Find…"]),
            (
                "Session",
                &[
                    "New terminal",
                    "",
                    "Reconnect",
                    "Reconnect all",
                    "Reopen closed tab",
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
            ("Plugins", plugins),
            ("Help", &["About WaSSH"]),
        ]
    })
}

/// Keys that run a menu row, for the rows that have a binding.
///
/// A table of its own rather than a field per row, so the menus stay plain label lists and
/// only the rows that really are wired up appear here: an accelerator is a promise.
const MENU_SHORTCUTS: &[(&str, &str)] = &[
    ("Options…", "Ctrl+,"),
    ("Copy", "Ctrl+Shift+C"),
    ("Paste", "Ctrl+Shift+V"),
    ("Find…", "Ctrl+Shift+F"),
    ("New terminal", "Ctrl+Shift+N"),
    ("Reconnect", "Ctrl+Shift+R"),
    ("Reopen closed tab", "Ctrl+Shift+T"),
];

/// A tab that was closed and can be reopened.
#[derive(Clone, Debug)]
struct ClosedTab {
    connection: ConnectionParams,
    host_id: Option<String>,
}

/// What a cached [`PluginValues`] was built from.
struct PluginValuesKey {
    session: String,
    host: Option<String>,
    generation: u64,
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
    /// Session, grid revision and query the match count was computed for.
    find_computed: Option<(String, u64, String)>,
    /// Plugin settings and decrypted secrets for the tab being drawn; moved out for the
    /// frame, and rebuilt only when what they derive from changed.
    plugin_values: Option<PluginValues>,
    plugin_values_key: Option<PluginValuesKey>,
    /// Bumped whenever the settings, the hosts or the credential vault change.
    data_generation: u64,
    /// Window title handed to the header bar and the platform, and the tab title it was
    /// last built from.
    title: String,
    title_tab: String,
    /// Plugin labels the Plugins menu ticks and shows a gear for, reused per frame.
    checked_labels: Vec<&'static str>,
    gear_labels: Vec<&'static str>,
    /// Buttons of the plugin strip, reused per frame.
    toolbar_buttons: Vec<wassh_ui::ToolbarButton<'static>>,
    /// Reconnect countdown text, with the attempt and second it was built for.
    retry_label: Option<(u32, u64, String)>,
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
    /// Where the window is, and how it gets put back there next launch.
    window: WindowGeometry,
    /// When a clipboard paste was asked for, until which a `Paste` event is ours.
    ///
    /// The platform's Ctrl+V produces the same event, and that key belongs to the shell.
    paste_deadline: Option<std::time::Instant>,
    /// When the fast repaint window ends; `None` (or a past instant) means the app is idle.
    active_until: Option<std::time::Instant>,
}

/// Where the window is, and how it gets put back there next launch.
///
/// Restoring a position is a small protocol with the window manager: the move is requested,
/// the window reports where it actually landed a couple of frames later, and the difference
/// is the frame's decoration offset — which is subtracted again when saving, or the window
/// would walk a decoration's width across the screen on every launch.
#[derive(Default)]
struct WindowGeometry {
    /// Normal (non-maximized) rectangle from the last frame, kept while maximized so
    /// unmaximizing and the next launch still land on the same rectangle.
    normal: Option<WindowBounds>,
    /// Rectangle the next launch should restore.
    saved: Option<WindowBounds>,
    /// Position the window was asked to take, in points, until it has landed.
    requested: Option<egui::Pos2>,
    /// Frames to wait before looking at where the window actually landed.
    wait: u8,
    /// Measured distance between a requested position and the client position the platform
    /// reports for it — the frame's borders and title bar.
    ///
    /// X11 lets the window manager add the decoration to the requested position but reports
    /// the *client* position back, so saving that report verbatim would walk the window a
    /// decoration's width further across the screen on every launch. It is measured once
    /// from the restore move and subtracted again when saving.
    offset: Option<egui::Vec2>,
    /// When the rectangle last changed, so a drag is written once it settles.
    changed_at: Option<std::time::Instant>,
}

impl WindowGeometry {
    /// Take up the rectangle the last run saved.
    fn new(saved: Option<WindowBounds>) -> Self {
        Self {
            normal: saved.map(|bounds| WindowBounds {
                maximized: false,
                ..bounds
            }),
            // A maximized window is restored maximized on the normal rectangle.
            saved: saved.filter(|bounds| !bounds.maximized),
            ..Self::default()
        }
    }

    /// Whether a restore move is still on its way.
    fn restoring(&self) -> bool {
        self.requested.is_some()
    }

    /// Whether the rectangle stopped changing long enough to be written.
    fn take_settled(&mut self, force: bool) -> bool {
        let Some(changed_at) = self.changed_at else {
            return false;
        };
        if !force && changed_at.elapsed() < WINDOW_BOUNDS_SETTLE {
            return false;
        }
        self.changed_at = None;
        true
    }

    /// Put the window back where it was saved, in physical pixels.
    ///
    /// `OuterPosition` takes points, so the saved pixel position is divided by the window's
    /// current scale: the conversion happens against the same scale it is multiplied by, and
    /// the window lands on the monitor, and at the pixel, it was saved from. The size is
    /// already applied in points when the window is created.
    fn restore(&mut self, ctx: &egui::Context) -> bool {
        let Some(bounds) = self.saved.take() else {
            return false;
        };
        let pixels_per_point = ctx.pixels_per_point();
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return false;
        }
        let target = egui::pos2(
            bounds.x as f32 / pixels_per_point,
            bounds.y as f32 / pixels_per_point,
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target));
        // Where the window lands tells us the decoration offset; see `learn_offset`.
        self.requested = Some(target);
        self.wait = RESTORE_FRAMES;
        ctx.request_repaint();
        true
    }

    /// Measure the window frame's offset from the position asked for, once per session.
    ///
    /// The move is applied asynchronously, so this runs a couple of frames after the
    /// request. A window manager that ignored the request would report a large difference;
    /// that is discarded rather than baked into every later save.
    fn learn_offset(&mut self, ctx: &egui::Context) {
        let Some(requested) = self.requested else {
            return;
        };
        if self.wait > 0 {
            self.wait -= 1;
            ctx.request_repaint_after(RESTORE_POLL);
            return;
        }
        self.requested = None;
        let Some(inner) = ctx.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let offset = inner.min - requested;
        if offset.length() <= MAX_FRAME_OFFSET {
            self.offset = Some(offset);
        }
        ctx.request_repaint();
    }

    /// The rectangle the window has now, if it can be saved at all.
    ///
    /// While maximized the normal rectangle is kept, so unmaximizing and the next launch
    /// still use it.
    fn current(&mut self, ctx: &egui::Context) -> Option<WindowBounds> {
        let viewport = ctx.input(|input| input.viewport().clone());
        if viewport.fullscreen.unwrap_or(false) || viewport.minimized.unwrap_or(false) {
            return None;
        }
        let inner = viewport.inner_rect?;
        let pixels_per_point = ctx.pixels_per_point();
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return None;
        }
        let maximized = viewport.maximized.unwrap_or(false);
        if !maximized {
            // The frame position is what gets restored, so the measured decoration offset
            // is removed from the client position the platform reports. Wayland reports no
            // outer position at all, and there a known offset lets the position survive.
            let position = match self.offset {
                Some(offset) => inner.min - offset,
                None => viewport.outer_rect.map_or(inner.min, |outer| outer.min),
            };
            self.normal = Some(WindowBounds {
                x: (position.x * pixels_per_point).round() as i32,
                y: (position.y * pixels_per_point).round() as i32,
                width: inner.width().round() as u32,
                height: inner.height().round() as u32,
                maximized: false,
            });
        }
        let normal = self.normal?;
        Some(WindowBounds { maximized, ..normal })
    }
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

/// Decode the bundled program icon for the window.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(ICON_PNG).expect("the bundled program icon is a valid PNG")
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

    // Windows and Linux get a title bar the app draws, so the window chrome is painted from
    // the same palette as everything else instead of from the desktop's theme. macOS keeps
    // the desktop's title bar and only borrows the menu strip.
    let mut viewport = egui::ViewportBuilder::default()
        .with_min_inner_size([800.0, 500.0])
        .with_title(APP_NAME)
        .with_app_id("wassh")
        .with_icon(window_icon());
    if !cfg!(target_os = "macos") {
        viewport = viewport.with_decorations(false);
    }
    viewport = match bounds {
        // `width`/`height` are points, so the window keeps its apparent size on a monitor
        // with different scaling. The position is applied on the first frame, in pixels.
        Some(bounds) => viewport
            .with_inner_size([bounds.width as f32, bounds.height as f32])
            .with_maximized(bounds.maximized),
        None => viewport.with_inner_size([1280.0, 800.0]),
    };

    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    prefer_x11_backend(&mut options);
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

/// Run through XWayland when one is available.
///
/// Wayland gives a client no way to place its own window, so a saved position could never
/// be restored there (and the compositor reports no outer position to save either).
/// XWayland, which every Wayland desktop with X support provides, supports both. A session
/// without XWayland keeps the Wayland backend and simply cannot restore a position;
/// `WASSH_WAYLAND=1` asks for native Wayland even when XWayland exists.
fn prefer_x11_backend(options: &mut eframe::NativeOptions) {
    #[cfg(target_os = "linux")]
    {
        let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_x11 = std::env::var_os("DISPLAY").is_some();
        let wants_native_wayland = std::env::var_os("WASSH_WAYLAND").is_some();
        if has_wayland && has_x11 && !wants_native_wayland {
            options.event_loop_builder = Some(Box::new(|builder| {
                use winit::platform::x11::EventLoopBuilderExtX11 as _;
                builder.with_x11();
            }));
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
    }
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
        shell.window = WindowGeometry::new(bounds);
        let system_dark = system_prefers_dark(&cc.egui_ctx);
        shell.system_dark = Some(system_dark);
        if shell.settings.theme == wassh_core::ThemeMode::System {
            shell.palette = Palette::for_setting(shell.settings.theme, system_dark);
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
        let palette = Palette::for_setting(settings.theme, true);

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
            find_computed: None,
            plugin_values: None,
            plugin_values_key: None,
            data_generation: 0,
            title: APP_NAME.to_owned(),
            title_tab: String::new(),
            checked_labels: Vec::new(),
            gear_labels: Vec::new(),
            toolbar_buttons: Vec::new(),
            retry_label: None,
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
            window: WindowGeometry::default(),
            paste_deadline: None,
            active_until: None,
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
        for notice in shell
            .config
            .as_ref()
            .map(|config| config.notices.clone())
            .unwrap_or_default()
        {
            shell.toasts.push(ToastKind::Info, notice);
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
                    self.local_params(),
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
                let (jumps, jump_error) = match jump_chain(
                    &self.hosts,
                    &connection,
                    self.identities(),
                    self.secrets.as_ref(),
                ) {
                    Ok(jumps) => (jumps, None),
                    // Keep the tab, but report why it cannot connect instead of
                    // falling back to a direct connection to the host.
                    Err(error) => (Vec::new(), Some(error)),
                };
                let (resolved, secrets, jump_error) = match resolve_auth(&connection, host_id.as_deref(), self.identities(), self.secrets.as_ref()) {
                    Ok((resolved, secrets)) => (Some(resolved), secrets, jump_error),
                    Err(error) => (None, Secrets::default(), Some(error)),
                };
                let open = SshOpen {
                    params: connection,
                    resolved,
                    host_id,
                    secrets,
                    known_hosts: self.known_hosts.clone(),
                    jumps,
                    jump_error,
                };
                // With no stored password the tab reopens and asks in the terminal.
                let entry = if open.jump_error.is_none()
                    && open.resolved.as_ref().unwrap_or(&open.params).auth == AuthMethod::Password
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
                match resolve_auth(&connection, host_id.as_deref(), self.identities(), self.secrets.as_ref()) {
                    Ok((resolved, secrets)) if resolved.auth == AuthMethod::Password && secrets.password.is_empty() => self.open_ssh_prompt(connection, host_id),
                    Ok((_, secrets)) => self.start_ssh(connection, host_id, secrets),
                    Err(error) => self.toasts.push(ToastKind::Error, error),
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
            self.local_params(),
        );
        self.push_session(entry);
    }

    /// Parameters a new local tab opens with: the built-in host's style, as a local shell.
    fn local_params(&self) -> ConnectionParams {
        let mut params = self
            .hosts
            .host(wassh_core::LOCAL_HOST_ID)
            .map(|host| host.connection.clone())
            .unwrap_or_default();
        params.kind = ConnectionType::Local;
        params.normalized()
    }

    /// Open an SSH tab whose password is typed into the terminal itself.
    ///
    /// The tab appears immediately with the `user@host's password:` prompt, matching the
    /// terminal itself, like PuTTY; a rejected password re-prompts in place.
    fn open_ssh_prompt(&mut self, connection: ConnectionParams, host_id: Option<String>) {
        let jumps = match jump_chain(&self.hosts, &connection, self.identities(), self.secrets.as_ref()) {
            Ok(jumps) => jumps,
            Err(error) => {
                self.toasts.push(ToastKind::Error, error);
                return;
            }
        };
        let (resolved, secrets) = match resolve_auth(&connection, host_id.as_deref(), self.identities(), self.secrets.as_ref()) {
            Ok(result) => result,
            Err(error) => { self.toasts.push(ToastKind::Error, error); return; }
        };
        let id = self.next_session_id();
        let entry = SessionEntry::new_ssh_prompt(
            id,
            &self.palette,
            &self.settings.session_style_defaults,
            self.last_terminal_size,
            SshOpen {
                params: connection,
                resolved: Some(resolved),
                host_id,
                secrets,
                known_hosts: self.known_hosts.clone(),
                jumps,
                jump_error: None,
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
        let jumps = match jump_chain(&self.hosts, &connection, self.identities(), self.secrets.as_ref()) {
            Ok(jumps) => jumps,
            Err(error) => {
                self.toasts.push(ToastKind::Error, error);
                return;
            }
        };
        let resolved = match resolve_auth(&connection, host_id.as_deref(), self.identities(), self.secrets.as_ref()) {
            Ok((resolved, _)) => resolved,
            Err(error) => { self.toasts.push(ToastKind::Error, error); return; }
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
                resolved: Some(resolved),
                host_id,
                secrets,
                known_hosts: self.known_hosts.clone(),
                jumps,
                jump_error: None,
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
        if let Some(session) = self.sessions.get(index) && session.connection.kind == ConnectionType::Ssh {
            let connection = session.connection.clone();
            let host_id = session.host_id.clone();
            let previous = session.secrets.clone();
            let resolution = resolve_auth(&connection, host_id.as_deref(), self.identities(), self.secrets.as_ref());
            let jumps = jump_chain(&self.hosts, &connection, self.identities(), self.secrets.as_ref());
            let (resolved, mut credentials, mut error) = match resolution {
                Ok((resolved, credentials)) => (Some(resolved), credentials, None),
                Err(error) => (None, Secrets::default(), Some(error)),
            };
            if connection.identity_id.is_none() {
                if !previous.password.is_empty() { credentials.password = previous.password; }
                if !previous.key_passphrase.is_empty() { credentials.key_passphrase = previous.key_passphrase; }
            }
            if !secrets.password.is_empty() { credentials = secrets.clone(); }
            let jumps = match jumps { Ok(jumps) => jumps, Err(reason) => { error = Some(reason); Vec::new() } };
            self.sessions[index].set_resolved_auth(resolved, credentials, jumps, error);
        }
        if let Some(session) = self.sessions.get_mut(index) {
            session.restart(&self.settings.term_type, &secrets, self.known_hosts.clone());
        }
    }

    fn identities(&self) -> &[wassh_core::Identity] {
        self.config.as_ref().map(|config| config.identities.get().as_slice()).unwrap_or_default()
    }

    // --- window geometry ---------------------------------------------------

    /// Write `settings.json` without re-applying defaults to the open tabs.
    fn write_settings(&mut self) {
        self.touch_plugin_data();
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

    /// Remember the window rectangle so the next launch can restore it.
    fn track_window_bounds(&mut self, ctx: &egui::Context) {
        let Some(current) = self.window.current(ctx) else {
            return;
        };
        if self.settings.window_bounds == Some(current) {
            return;
        }
        self.settings.window_bounds = Some(current);
        self.window.changed_at = Some(std::time::Instant::now());
        ctx.request_repaint_after(WINDOW_BOUNDS_SETTLE);
    }

    /// Write the window rectangle once it has stopped changing, or at once when the app exits.
    fn persist_window_bounds(&mut self, force: bool) {
        if !self.window.take_settled(force) {
            return;
        }
        self.write_settings();
    }

    // --- persistence -------------------------------------------------------

    fn save_settings(&mut self) {
        self.settings.sidebar_collapsed = self.sidebar.collapsed;
        // `System` resolves against the desktop preference seen this session.
        let system_dark = self.system_dark.unwrap_or(true);
        self.palette = Palette::for_setting(self.settings.theme, system_dark);
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
        self.touch_plugin_data();
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
            // "Apply to sessions" makes them match the host, so a tab's own plugin
            // values give way to the ones being pushed.
            session.clear_plugin_overrides();
            session.reload_pane_settings();
            applied += 1;
        }
        self.touch_plugin_data();
        applied
    }

    /// Apply a session dialog's settings to the one tab it was opened from.
    ///
    /// Returns false when the tab is gone. Credentials stay in the tab: nothing is
    /// written to `hosts.json` or to the vault, so the saved host and its other
    /// sessions keep their own settings. Plugin values the dialog changed are kept in
    /// the tab the same way, layered over the host's.
    fn apply_to_session(&mut self, session_id: &str, save: &HostSave) -> bool {
        let defaults = self.settings.session_style_defaults.clone();
        let stored_plugins = self
            .hosts
            .host(&save.host.id)
            .map(|host| host.plugin_settings.clone())
            .unwrap_or_default();
        let overrides = plugin_overrides(&stored_plugins, &save.host.plugin_settings);
        let secret_overrides = session_secret_overrides(&save.plugin_secrets);
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return false;
        };
        session.apply_connection(save.host.connection.clone(), &defaults);
        session.set_plugin_settings(overrides, secret_overrides);
        if let Some(value) = secret_value(&save.password) {
            session.secrets.password = value.unwrap_or_default().to_owned();
        }
        if let Some(value) = secret_value(&save.passphrase) {
            session.secrets.key_passphrase = value.unwrap_or_default().to_owned();
        }
        self.touch_plugin_data();
        true
    }

    /// Plugin `secret: true` fields the vault holds for `scope`.
    fn stored_plugin_secrets(&self, scope: &str) -> Vec<(String, String)> {
        let Some(store) = self.secrets.as_ref() else {
            return Vec::new();
        };
        let mut stored = Vec::new();
        for id in PluginId::ALL {
            let fields = wassh_plugins::settings::secret_fields_for(id, self.plugin_settings_of(id));
            for (key, _) in fields {
                if store.has_plugin_secret(scope, id.as_str(), &key) {
                    stored.push((id.as_str().to_owned(), key));
                }
            }
        }
        stored
    }

    /// The app-wide settings a plugin owns, which also name any secret it declares at
    /// runtime (the AI agent's provider keys).
    fn plugin_settings_of(
        &self,
        id: PluginId,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.settings.plugin_settings.get(id.as_str())
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
            let fields = wassh_plugins::settings::secret_fields_for(id, self.plugin_settings_of(id));
            for (key, app_wide) in fields {
                let scope = if app_wide {
                    wassh_store::secrets::APP_SCOPE
                } else {
                    match host_id {
                        Some(host_id) => host_id,
                        None => continue,
                    }
                };
                if let Some(value) = store.plugin_secret(scope, id.as_str(), &key) {
                    out.entry(id.as_str().to_owned())
                        .or_default()
                        .insert(key, value);
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
        self.touch_plugin_data();
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
            let Some(value) = dialogs::plugin_secret_value(draft) else {
                continue;
            };
            let result = store.set_plugin_secret(scope, plugin, field, value);
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
        self.touch_plugin_data();
        let mut pending: Vec<(String, String, String, String)> = Vec::new();
        for (plugin, values) in &self.settings.plugin_settings {
            let Some(id) = PluginId::parse(plugin) else {
                pending.extend(unknown_plugin_secrets(
                    wassh_store::secrets::APP_SCOPE,
                    plugin,
                    values,
                ));
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
            for (field, text) in wassh_plugins::settings::legacy_secrets(id, values) {
                pending.push((
                    wassh_store::secrets::APP_SCOPE.to_owned(),
                    plugin.clone(),
                    field,
                    text,
                ));
            }
        }
        for host in &self.hosts.hosts {
            for (plugin, values) in &host.plugin_settings {
                let Some(id) = PluginId::parse(plugin) else {
                    pending.extend(unknown_plugin_secrets(&host.id, plugin, values));
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
        let mut docs: Vec<(String, Option<String>, serde_json::Value)> = Vec::new();
        if let Some(config) = self.config.as_ref() {
            for id in PluginId::ALL {
                let scopes = config.plugin_data_scopes(id.as_str()).unwrap_or_default();
                for scope in scopes {
                    let Ok(Some(mut data)) = config.plugin_data(id.as_str(), scope.as_deref()) else {
                        continue;
                    };
                    let found = take_document_secrets(id, scope.as_deref(), &mut data);
                    if found.is_empty() {
                        continue;
                    }
                    pending.extend(found);
                    docs.push((id.as_str().to_owned(), scope, data));
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
        if let Some(config) = self.config.as_ref() {
            for (plugin, scope, data) in docs {
                if let Err(error) = config.save_plugin_data(&plugin, scope.as_deref(), &data) {
                    self.toasts.push(
                        ToastKind::Error,
                        format!("could not clear the {plugin} password from its data: {error}"),
                    );
                }
            }
        }
    }

    /// Build a host dialog state with the app defaults and any stored secrets.
    fn host_state(&self, host: HostProfile, is_new: bool) -> HostState {
        self.dialog_state(host, is_new, HostTarget::Host)
    }

    /// Build the dialog for the session opened from a tab: Save stays on that tab.
    fn session_state(&self, host: HostProfile, session: &str) -> HostState {
        let mut state = self.dialog_state(host, false, HostTarget::Session(session.to_owned()));
        // A secret this tab set shows as stored: the tab holds it, not the vault.
        if let Some(entry) = self.sessions.iter().find(|entry| entry.id == session) {
            for (id, fields) in entry.plugin_secret_overrides() {
                for field in fields.keys() {
                    state
                        .plugin_secrets
                        .entry(self::dialogs::secret_field_key(id, field))
                        .or_insert_with(self::dialogs::SecretDraft::stored);
                }
            }
        }
        state
    }

    fn dialog_state(&self, host: HostProfile, is_new: bool, target: HostTarget) -> HostState {
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
        let mut state = HostState::draft(host, is_new)
            .with_defaults(&self.settings.session_style_defaults)
            .with_open_sessions(open)
            .with_saved(saved)
            .with_stored_secrets(password, passphrase)
            .with_plugin_secrets(stored_plugin_secrets)
            .with_target(target);
        if let Some(config) = &self.config {
            state.identities = config.identities.get().clone();
        }
        state
    }

    /// Whether a password / passphrase is already stored for `host_id`.
    fn stored_secrets(&self, host_id: &str) -> (bool, bool) {
        match self.secrets.as_ref() {
            Some(store) => (store.has_password(host_id), store.has_passphrase(host_id)),
            None => (false, false),
        }
    }

    #[cfg(test)]
    fn stored_password(&self, host_id: Option<&str>) -> Option<String> {
        self.secrets.as_ref()?.password(host_id?)
    }

    /// Options dialog state, with the app-wide plugin secrets already noted.
    fn options_state(&self) -> OptionsState {
        let mut state = OptionsState::new(&self.settings);
        if let Some(config) = &self.config {
            state.identities = config.identities.get().iter().map(|identity| IdentityDraft {
                identity: identity.clone(),
                host_count: self.hosts.hosts.iter().filter(|host| host.connection.identity_id.as_deref() == Some(identity.id.as_str())).count(),
                stored_password: self.secrets.as_ref().and_then(|store| store.identity_password(&identity.id)).is_some(),
                stored_passphrase: self.secrets.as_ref().and_then(|store| store.identity_passphrase(&identity.id)).is_some(),
                ..Default::default()
            }).collect();
            state.identity_ids = state.identities.iter().map(|draft| draft.identity.id.clone()).collect();
        }
        state.plugin_secrets = self
            .stored_plugin_secrets(wassh_store::secrets::APP_SCOPE)
            .into_iter()
            .map(|(plugin, field)| {
                (
                    self::dialogs::secret_field_key(&plugin, &field),
                    self::dialogs::SecretDraft::stored(),
                )
            })
            .collect();
        state
    }

    fn save_identities(&mut self, drafts: Vec<IdentityDraft>) -> bool {
        let Some(config) = self.config.as_ref() else { return drafts.is_empty(); };
        if drafts.iter().any(|draft| draft.identity.name.trim().is_empty()) {
            self.toasts.push(ToastKind::Error, "Identity names cannot be empty.");
            return false;
        }
        let removed: Vec<String> = config.identities.get().iter()
            .filter(|identity| !drafts.iter().any(|draft| draft.identity.id == identity.id))
            .map(|identity| identity.id.clone()).collect();
        for id in &removed {
            let used = self.hosts.hosts.iter().any(|host| host.connection.identity_id.as_ref() == Some(id))
                || self.sessions.iter().any(|session| session.connection.identity_id.as_ref() == Some(id))
                || config.tabs.get().iter().any(|tab| tab.connection.identity_id.as_ref() == Some(id));
            if used {
                self.toasts.push(ToastKind::Error, "Cannot delete an identity used by a host or session.");
                return false;
            }
        }
        let changes_secrets = !removed.is_empty() || drafts.iter().any(|draft| draft.clear_password || draft.clear_passphrase || !draft.password.is_empty() || !draft.passphrase.is_empty());
        if changes_secrets && self.secrets.is_none() {
            self.toasts.push(ToastKind::Error, "The secrets store is unavailable.");
            return false;
        }
        if let Some(store) = self.secrets.as_mut() {
            for draft in &drafts {
                let id = &draft.identity.id;
                let password = if draft.clear_password { Some(None) } else if draft.password.is_empty() { None } else { Some(Some(draft.password.as_str())) };
                let passphrase = if draft.clear_passphrase { Some(None) } else if draft.passphrase.is_empty() { None } else { Some(Some(draft.passphrase.as_str())) };
                if let Some(value) = password && let Err(error) = store.set_identity_password(id, value) {
                    self.toasts.push(ToastKind::Error, error.to_string());
                    return false;
                }
                if let Some(value) = passphrase && let Err(error) = store.set_identity_passphrase(id, value) {
                    self.toasts.push(ToastKind::Error, error.to_string());
                    return false;
                }
            }
        }
        let identities = drafts.into_iter().map(|draft| draft.identity).collect();
        if let Some(config) = self.config.as_mut() && let Err(error) = config.identities.set(identities) {
            self.toasts.push(ToastKind::Error, error.to_string());
            return false;
        }
        if let Some(store) = self.secrets.as_mut() {
            for id in removed {
                if let Err(error) = store.forget_identity(&id) {
                    self.toasts.push(ToastKind::Error, error.to_string());
                }
            }
        }
        true
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
        self.touch_plugin_data();
        self.hosts.upsert(host.clone());
        self.hosts = std::mem::take(&mut self.hosts).normalized();
        // Open sessions keep the values they started with; only "Apply to sessions"
        // pushes the change to them.
        if let Some(config) = self.config.as_mut()
            && let Err(error) = config.save_host(host)
        {
            self.toasts
                .push(ToastKind::Error, format!("could not save host: {error}"));
        }
    }

    fn delete_host(&mut self, id: &str) {
        // The built-in local terminal host is never removed by `hosts.remove`.
        if self.hosts.remove(id).is_none() {
            return;
        }
        self.touch_plugin_data();
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
        self.touch_plugin_data();
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
                // The dialog edits the live session: its Save must not touch the saved
                // host or the host's other tabs.
                let Some((session_id, host_id, title, connection)) = self.active().map(|session| {
                    (
                        session.id.clone(),
                        session.host_id.clone(),
                        session.title.clone(),
                        session.connection.clone(),
                    )
                }) else {
                    return;
                };
                let mut draft = host_id
                    .as_deref()
                    .and_then(|id| self.hosts.host(id).cloned())
                    .unwrap_or_else(|| HostProfile {
                        id: format!("host-{}", self.next_id),
                        name: title,
                        ..HostProfile::default()
                    });
                draft.connection = connection;
                // Show what this tab uses, not just what the host stores.
                if let Some(session) = self.sessions.iter().find(|session| session.id == session_id)
                {
                    session.with_plugin_overrides(&mut draft.plugin_settings);
                }
                self.dialogs.host = Some(self.session_state(draft, &session_id));
            }
            Command::SaveAsHost => {
                // The tab menu's entry: a new host built from the active session.
                let Some(session) = self.active() else {
                    return;
                };
                let mut draft = HostProfile {
                    id: format!("host-{}", self.next_id),
                    name: session.title.clone(),
                    connection: session.connection.clone(),
                    ..HostProfile::default()
                };
                draft.connection = draft.connection.normalized();
                self.dialogs.host = Some(self.host_state(draft, true));
            }
            Command::OpenOptions => {
                let home = home_dir();
                let now = wassh_core::time::now_ms();
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
            Command::ReopenTab => self.reopen_closed(),
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
            "New terminal" => self.handle_command(Command::NewLocalTab),
            "Reopen closed tab" => self.handle_command(Command::ReopenTab),
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
        let now = wassh_core::time::now_ms();
        self.dialogs.options = Some(
            self.options_state()
                .with_backup_default(&home, now)
                .focusing_plugin(id),
        );
    }

    /// Toolbar labels the Plugins menu ticks and shows a gear for.
    ///
    /// One pass over the registry into reused buffers: the menus are drawn every frame and
    /// the labels never change.
    fn refresh_plugin_labels(&mut self) {
        self.checked_labels.clear();
        self.gear_labels.clear();
        for id in wassh_plugins::PluginId::UI_ORDER {
            let label = id.manifest().toolbar_label;
            if self.settings.plugin_enabled(id.as_str())
                && !wassh_plugins::settings::app_schema(id).is_empty()
            {
                self.gear_labels.push(label);
            }
            let open = self
                .active()
                .is_some_and(|session| session.layout.contains(id.as_str()));
            if open {
                self.checked_labels.push(label);
            }
        }
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
    fn paste_clipboard(&mut self, ctx: &egui::Context) {
        // A plain Ctrl+V produces the same event, so remember that this one is ours.
        self.paste_deadline = Some(std::time::Instant::now() + PASTE_WAIT);
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
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::P)
                || consume_chord(input, Modifiers::NONE, Key::F1)
            {
                palette = true;
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::T) {
                commands.push(Command::ReopenTab);
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::N) {
                commands.push(Command::NewLocalTab);
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::R) {
                commands.push(Command::Reconnect);
            }
            // Ctrl+W belongs to the shell (Emacs and readline use it to delete a word),
            // so only the shifted form and Ctrl+F4 close a tab.
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::W)
                || consume_chord(input, Modifiers::CTRL, Key::F4)
            {
                commands.push(Command::CloseTab);
            }
            // Ctrl+Shift+C / Ctrl+Insert copy the selection; Ctrl+Shift+V / Shift+Insert /
            // Ctrl+Shift+Insert paste. egui-winit leaves Insert to the application (see
            // vendor/egui-winit/PATCHED.md), so the Insert chords are ordinary key events;
            // every other Ctrl combination — Ctrl+C and Ctrl+Z above all — reaches the
            // shell, so nothing else here may claim one.
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::C)
                || consume_chord(input, Modifiers::CTRL, Key::Insert)
            {
                copy = true;
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::V)
                || consume_chord(input, Modifiers::SHIFT, Key::Insert)
                || consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::Insert)
            {
                paste = true;
            }
            if consume_chord(input, Modifiers::CTRL, Key::Tab) {
                commands.push(Command::NextTab);
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
                commands.push(Command::PreviousTab);
            }
            if consume_chord(input, Modifiers::CTRL | Modifiers::SHIFT, Key::F) {
                commands.push(Command::Find);
            }
            // Ctrl+, opens Options, as it does in every other desktop application.
            if consume_chord(input, Modifiers::CTRL, Key::Comma) {
                commands.push(Command::OpenOptions);
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
            session.poll_commands();
            if session.requests_pending() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        let mut exited = Vec::new();
        for (index, pumped) in results {
            // New output or a state change: keep repainting at the active rate until the
            // session settles, so the echo and the rest of a burst arrive without a wait.
            if pumped.wake {
                self.active_until = Some(std::time::Instant::now() + ACTIVE_LINGER);
            }
            if pumped.bell {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
            }
            if let Some(exit) = pumped.closed {
                // The shell was told to exit (`exit`, `logout`), so the tab is done. A
                // failed transport keeps its tab: the reconnect policy takes over below.
                if exit.from_shell {
                    exited.push(index);
                } else if let Some(session) = self.sessions.get_mut(index)
                    && !session.exit_reported
                {
                    session.exit_reported = true;
                    if let Some(reason) = exit.reason {
                        session.error = Some(reason);
                    }
                }
            }
        }
        for index in exited.into_iter().rev() {
            self.close(index);
        }

        self.schedule_reconnects(ctx);
    }

    /// Queue and run automatic reconnects according to each session's policy.
    fn schedule_reconnects(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|input| input.focused);
        let now = std::time::Instant::now();
        let mut retries = Vec::new();

        for (index, session) in self.sessions.iter_mut().enumerate() {
            // A pending retry that has come due. Booking one moves the session to
            // `Reconnecting`, so that state has to count as retryable here as well.
            if session.retry_at.is_some_and(|at| at <= now)
                && session.status.is_retryable()
                && session.host_key_prompt.is_none()
            {
                session.retry_at = None;
                retries.push(index);
                continue;
            }
            // Only a failed transport keeps a retry timer; a session that connected,
            // closed or was never up holds none.
            if !session.status.is_retryable() {
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
            self.restart(index, Secrets::default());
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
        // A widget holding egui's focus (a button reached by Tab, say) consumes nothing,
        // so only a text field being edited may keep the keys from the terminal.
        if self.dialogs.modal_open() || ctx.text_edit_focused() {
            return;
        }
        let Some(index) = self.active else {
            return;
        };
        let paste_pending = self
            .paste_deadline
            .is_some_and(|deadline| deadline > std::time::Instant::now());
        let inputs = wassh_ui::collect_terminal_input(ctx, paste_pending);
        if inputs
            .iter()
            .any(|input| matches!(input, wassh_ui::TerminalInput::Paste(_)))
        {
            self.paste_deadline = None;
        }
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
            self.find_computed = None;
            return;
        }
        let Some(session) = self.active() else {
            self.find_matches = 0;
            return;
        };
        // Searching the whole visible grid is only worth doing when the text it was
        // computed for is still the text on screen.
        let revision = session.revision();
        let current = self.find_computed.as_ref().is_some_and(|(id, at, cached)| {
            id == &session.id && *at == revision && cached == &query
        });
        if current {
            return;
        }
        let matches = session.visible_text().to_lowercase().matches(&query).count();
        let session_id = session.id.clone();
        self.find_matches = matches;
        self.find_computed = Some((session_id, revision, query));
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
                DialogAction::SaveSettings(mut settings, secrets, identities) => {
                    if !self.save_identities(identities) {
                        continue;
                    }
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
                DialogAction::SaveSession { session, save } => {
                    let applied = self.apply_to_session(&session, &save);
                    self.dialogs.host = None;
                    if applied {
                        self.toasts
                            .push(ToastKind::Info, "Session settings applied.");
                    } else {
                        self.toasts
                            .push(ToastKind::Error, "The session is no longer open.");
                    }
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
        let app_writes = session.take_dirty_app_settings();
        if !app_writes.is_empty() {
            for (id, key, value) in app_writes {
                self.settings
                    .plugin_settings
                    .entry(id.as_str().to_owned())
                    .or_default()
                    .insert(key.clone(), value.clone());
                // The pane reads settings from a copy, so it takes the value back.
                if let Some(session) = self.sessions.get_mut(index) {
                    session.adopt_app_setting(id, &key, value);
                }
            }
            self.save_settings();
        }
        let wrote_secrets = !secret_writes.is_empty();
        for (id, field, value) in secret_writes {
            let scope = plugin_secret_scope(id, &field, host_id.as_deref());
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
        if wrote_secrets {
            self.touch_plugin_data();
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
                    {
                        // A tab with its own value keeps it: the pane's change lands in
                        // the tab, not in the host the tab happens to share.
                        if let Some(session) = self
                            .sessions
                            .get_mut(index)
                            .filter(|session| session.has_plugin_override(id.as_str(), key))
                        {
                            session.set_plugin_override(id.as_str(), key, value.clone());
                            continue;
                        }
                        let Some(host) = host_id
                            .as_deref()
                            .and_then(|host_id| self.hosts.host_mut(host_id))
                        else {
                            continue;
                        };
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
        // The strip is the same buttons every frame, so the buffer is reused.
        self.toolbar_buttons.clear();
        for id in wassh_plugins::PluginId::UI_ORDER {
            if !self.settings.plugin_enabled(id.as_str()) {
                continue;
            }
            let manifest = id.manifest();
            let open = self.sessions[index].layout.contains(id.as_str());
            self.toolbar_buttons.push(wassh_ui::ToolbarButton {
                id: id.as_str(),
                label: manifest.toolbar_label,
                active: open,
                tooltip: manifest.name,
            });
        }

        let mut output = wassh_ui::plugin_toolbar::ToolbarOutput::default();
        egui::Panel::top("wassh-plugin-toolbar")
            .exact_size(wassh_ui::plugin_toolbar::TOOLBAR_HEIGHT)
            .frame(
                egui::Frame::NONE
                    .fill(wassh_ui::terminal_view::to_color32(palette.bg_panel))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                output = wassh_ui::plugin_toolbar(ui, &palette, &self.toolbar_buttons);
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
        let open =
            open_host_sessions(self.sessions.iter().map(|entry| entry.host_id.as_deref()));
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
                    action = wassh_ui::sidebar(ui, &palette, hosts, &open, sidebar);
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
                let mut items = vec![
                    MenuItem::new("Connect"),
                    MenuItem::new("Edit host…"),
                    MenuItem::new("Duplicate host"),
                ];
                // The built-in local terminal host cannot be deleted.
                if !self.hosts.host(&id).is_some_and(HostProfile::is_local) {
                    items.push(MenuItem::separator());
                    items.push(MenuItem::danger("Delete host"));
                }
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
        let plugin_values = self.take_plugin_values(index);
        let size_after = {
            let session = &mut self.sessions[index];

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
        self.plugin_values = Some(plugin_values);
        self.persist_plugin_data(index);
        if dropped {
            self.persist_tabs();
        }

        self.forward_input(ctx);
    }

    /// Ask for another frame: soon while the window is busy, later when it is idle.
    ///
    /// egui only repaints when it is asked to. Output and finished work wake the window on
    /// their own (`wassh_core::wake`), so the idle cadence is only a safety net for what
    /// cannot; typing and fresh output switch to the active rate.
    fn schedule_repaint(&mut self, ctx: &egui::Context) {
        // Working with the window means the echo is about to arrive.
        if ctx.input(|input| input_started_work(&input.events)) {
            self.active_until = Some(std::time::Instant::now() + ACTIVE_LINGER);
        }
        let active = self
            .active_until
            .is_some_and(|until| until > std::time::Instant::now());
        ctx.request_repaint_after(if active { ACTIVE_INTERVAL } else { IDLE_INTERVAL });
    }

    /// Keep the reconnect countdown current.
    ///
    /// The banner shows the seconds left, which ticks while the tab waits; the text is
    /// rebuilt when the second or the attempt changes, not once per frame.
    fn refresh_retry_label(&mut self, index: usize) {
        let Some(session) = self.sessions.get(index) else {
            self.retry_label = None;
            return;
        };
        let Some(at) = session.retry_at else {
            self.retry_label = None;
            return;
        };
        let remaining = at.saturating_duration_since(std::time::Instant::now());
        let seconds = remaining.as_secs().max(1);
        let attempt = session.reconnect_attempt;
        let current = self
            .retry_label
            .as_ref()
            .is_some_and(|(cached_attempt, cached_seconds, _)| {
                *cached_attempt == attempt && *cached_seconds == seconds
            });
        if !current {
            self.retry_label = Some((
                attempt,
                seconds,
                wassh_core::reconnect_label(attempt, remaining),
            ));
        }
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

        self.refresh_retry_label(index);
        let countdown = self.retry_label.as_ref().map(|(_, _, text)| text.as_str());
        let session = &self.sessions[index];
        // While the user is typing a password the prompt is the status, like PuTTY.
        if !session.is_live() && !session.awaiting_login() {
            let status = session.status;
            let error = session.error.clone();
            let note = session.status_message.clone();
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
                        if let Some(countdown) = countdown {
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
    ///
    /// Panes read this every frame, and decrypting the vault or copying the settings
    /// maps every frame would be wasted work, so the result is kept until the settings,
    /// the hosts or the vault change. It is moved out for the frame because the dock
    /// needs it while the session itself is borrowed mutably; the caller puts it back.
    fn take_plugin_values(&mut self, index: usize) -> PluginValues {
        let Some(session) = self.sessions.get(index) else {
            return PluginValues::default();
        };
        let host_id = session.host_id.as_deref();
        let fresh = self.plugin_values_key.as_ref().is_some_and(|key| {
            key.generation == self.data_generation
                && key.session == session.id
                && key.host.as_deref() == host_id
        });
        if !fresh {
            let key = PluginValuesKey {
                session: session.id.clone(),
                host: host_id.map(str::to_owned),
                generation: self.data_generation,
            };
            let mut host = host_id
                .and_then(|host_id| self.hosts.host(host_id))
                .map(|host| host.plugin_settings.clone())
                .unwrap_or_default();
            // What this tab's own Session settings changed wins over the host's values.
            session.with_plugin_overrides(&mut host);
            let mut secrets = self.plugin_secrets_for(host_id);
            for (id, typed) in session.plugin_secret_overrides() {
                secrets
                    .entry(id.clone())
                    .or_default()
                    .extend(typed.iter().map(|(key, value)| (key.clone(), value.clone())));
            }
            let values = PluginValues {
                app: self.settings.plugin_settings.clone(),
                host,
                secrets,
            };
            self.plugin_values_key = Some(key);
            self.plugin_values = Some(values);
        }
        self.plugin_values.take().unwrap_or_default()
    }

    /// Mark everything derived from the settings, the hosts and the vault as stale.
    fn touch_plugin_data(&mut self) {
        self.data_generation = self.data_generation.wrapping_add(1);
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
        if !self.dialogs.any_open() {
            return;
        }
        let palette = self.palette;
        let matches = self.find_matches;
        // Only Host settings reads the host list, so the other dialogs do not clone it.
        let hosts = if self.dialogs.host.is_some() {
            self.hosts.hosts.clone()
        } else {
            Vec::new()
        };
        let actions = self.dialogs.show(
            ctx,
            &palette,
            &hosts,
            &self.settings.enabled_plugins,
            matches,
        );
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
            // The probe measures frames, not idle time: keep them coming.
            ui.ctx().request_repaint();
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
        let restoring = self.window.restore(&ctx);
        self.frame_body(ui);
        self.window.learn_offset(&ctx);
        // Until a restore has landed, the position is not the user's choice yet, so it is
        // not saved over the one being restored.
        if !restoring && !self.window.restoring() {
            self.track_window_bounds(&ctx);
        }
        self.persist_window_bounds(false);
    }

    /// The text the window's title bar shows: the app name and the active tab.
    ///
    /// The header bar and the platform both read it, so it is rebuilt only when the active
    /// tab's title changed rather than once per frame.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let tab = self
            .active
            .and_then(|index| self.sessions.get(index))
            .map(|session| session.title.as_str())
            .unwrap_or_default();
        if self.title_tab == tab {
            return;
        }
        self.title_tab.clear();
        self.title_tab.push_str(tab);
        self.title.clear();
        self.title.push_str(APP_NAME);
        if !tab.is_empty() {
            self.title.push_str(" — ");
            self.title.push_str(tab);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title.clone()));
    }

    /// Everything inside the window frame.
    fn frame_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // Background threads ask for a frame through this once it is installed.
        if !wassh_core::wake::installed() {
            let repaint = ctx.clone();
            wassh_core::wake::install(std::sync::Arc::new(move || repaint.request_repaint()));
        }

        // egui reads Tab, Shift+Tab and the arrow keys as navigation between focusable
        // widgets, and the first widget that wants focus takes it. Outside a dialog the
        // terminal is the only keyboard client here, so that would hand the keys to a
        // chrome widget and leave the terminal deaf — Tab has to reach the shell. A dialog
        // is the opposite case: there the keyboard walks its fields.
        if !ctx.text_edit_focused() && !self.dialogs.modal_open() {
            ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
        }

        self.pump(&ctx);
        self.shortcuts(&ctx);
        self.update_find_matches();
        self.sync_window_title(&ctx);
        self.refresh_plugin_labels();

        // The window chrome: the app draws its own title bar on Windows and Linux, so it is
        // painted from the palette and follows the app's theme. macOS keeps the desktop's
        // title bar, and the app only adds the menu strip under it.
        let state = wassh_ui::titlebar::MenuState {
            shortcuts: MENU_SHORTCUTS,
            checked: &self.checked_labels,
            gears: &self.gear_labels,
        };
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let action = if cfg!(target_os = "macos") {
            wassh_ui::show_menu_bar(ui, &self.palette, menus(), state)
        } else {
            wassh_ui::resize_edges(&ctx, maximized);
            wassh_ui::show_header_bar(ui, &self.palette, &self.title, maximized, menus(), state)
        };
        match action {
            Some(wassh_ui::TitlebarAction::MenuItem(item)) => self.menu_command(&ctx, item),
            Some(wassh_ui::TitlebarAction::MenuGear(item)) => self.open_plugin_options(item),
            Some(wassh_ui::TitlebarAction::Minimize) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            Some(wassh_ui::TitlebarAction::ToggleMaximize) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            Some(wassh_ui::TitlebarAction::Close) => self.quit_requested = true,
            None => {}
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
        self.schedule_repaint(&ctx);
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
        self.palette = Palette::for_setting(self.settings.theme, system_dark);
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

/// Vault scope a plugin secret field belongs to.
///
/// App-wide fields are stored under the app scope even when the write comes
/// from a host's pane, so a provider key is never filed per host.
fn plugin_secret_scope(id: PluginId, field: &str, host_id: Option<&str>) -> String {
    let app_wide = wassh_plugins::settings::secret_scope(id, field) == Some(true)
        || wassh_plugins::settings::is_runtime_secret(id, field);
    match (app_wide, host_id) {
        (true, _) | (false, None) => wassh_store::secrets::APP_SCOPE.to_owned(),
        (false, Some(host_id)) => host_id.to_owned(),
    }
}

/// Drop plain-text copies of a plugin's `secret: true` fields.
///
/// Those fields live in the credential vault; a settings map must never carry
/// one, or `settings.json` and `hosts.json` would hold a readable password.
fn strip_plugin_secrets(settings: &mut wassh_core::PluginSettings) {
    settings.retain(|plugin, values| {
        match PluginId::parse(plugin) {
            Some(id) => {
                for (key, _) in wassh_plugins::settings::secret_fields_for(id, Some(values)) {
                    values.remove(&key);
                }
                wassh_plugins::settings::take_legacy_secrets(id, values);
            }
            None => values.retain(|key, _| !names_a_secret(key)),
        }
        !values.is_empty()
    });
}

/// Plain-text secrets a plugin's own stored document still holds.
///
/// A pane used to write `secret: true` fields — and the AI agent its provider
/// API keys — into its own data file. They are taken out here, with the vault
/// scope each one belongs to, so the caller can seal them.
fn take_document_secrets(
    id: PluginId,
    scope: Option<&str>,
    data: &mut serde_json::Value,
) -> Vec<(String, String, String, String)> {
    let Some(values) = data.as_object_mut() else {
        return Vec::new();
    };
    let host_scope = scope.unwrap_or(wassh_store::secrets::APP_SCOPE);
    let mut found = Vec::new();
    for (key, app_wide) in wassh_plugins::settings::secret_fields(id) {
        let Some(text) = values
            .remove(key)
            .and_then(|value| value.as_str().map(str::to_owned))
        else {
            continue;
        };
        let vault_scope = if app_wide {
            wassh_store::secrets::APP_SCOPE
        } else {
            host_scope
        };
        found.push((
            vault_scope.to_owned(),
            id.as_str().to_owned(),
            key.to_owned(),
            text,
        ));
    }
    if let Some(keys) = data.as_object_mut() {
        for (field, text) in wassh_plugins::settings::take_legacy_secrets(id, keys) {
            found.push((
                wassh_store::secrets::APP_SCOPE.to_owned(),
                id.as_str().to_owned(),
                field,
                text,
            ));
        }
    }
    found
}

/// Secret-named fields of a plugin id this build does not know.
///
/// The reference app could load external plugins, so `hosts.json` can carry
/// settings of a plugin that no longer exists here. Its schema is unknown, so
/// a field is taken as a credential when its name says so.
fn unknown_plugin_secrets(
    scope: &str,
    plugin: &str,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(String, String, String, String)> {
    values
        .iter()
        .filter_map(|(key, value)| {
            let text = value.as_str().filter(|text| !text.is_empty())?;
            names_a_secret(key).then(|| {
                (
                    scope.to_owned(),
                    plugin.to_owned(),
                    key.to_owned(),
                    text.to_owned(),
                )
            })
        })
        .collect()
}

/// Whether a field name holds a credential whatever plugin wrote it.
fn names_a_secret(key: &str) -> bool {
    const SUFFIXES: [&str; 5] = ["password", "passphrase", "apikey", "token", "secret"];
    let lower = key.to_ascii_lowercase();
    SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
}

/// The value a [`SecretEdit`] stands for: `None` keeps what is stored.
fn secret_value(edit: &SecretEdit) -> Option<Option<&str>> {
    match edit {
        SecretEdit::Unchanged => None,
        SecretEdit::Set(value) => Some(Some(value.as_str())),
        SecretEdit::Clear => Some(None),
    }
}

/// The plugin values `draft` changes relative to what the host already stores.
///
/// A session dialog edits a copy of the host's values, so what it leaves alone must not
/// become a tab-only value: only the fields the user changed go into the tab.
fn plugin_overrides(
    stored: &wassh_core::PluginSettings,
    draft: &wassh_core::PluginSettings,
) -> wassh_core::PluginSettings {
    let mut overrides = wassh_core::PluginSettings::new();
    for (id, values) in draft {
        for (key, value) in values {
            if stored.get(id).and_then(|stored| stored.get(key)) != Some(value) {
                overrides
                    .entry(id.clone())
                    .or_default()
                    .insert(key.clone(), value.clone());
            }
        }
    }
    overrides
}

/// Plugin secrets a session dialog typed, ready to keep in the tab.
///
/// Keys are `"<plugin>|<field>"`, the shape [`HostSave::plugin_secrets`] uses.
fn session_secret_overrides(
    drafts: &PluginSecretDrafts,
) -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    let mut overrides: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    > = std::collections::BTreeMap::new();
    for (key, draft) in drafts {
        let Some((plugin, field)) = key.split_once('|') else {
            continue;
        };
        if let Some(Some(value)) = dialogs::plugin_secret_value(draft) {
            overrides
                .entry(plugin.to_owned())
                .or_default()
                .insert(field.to_owned(), value.to_owned());
        }
    }
    overrides
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
        drop(snapshot);
        self.session.track_selection(ui, &response, size);
        self.session.scroll_from_wheel(ui, &response);
    }

    fn pane(&mut self, ui: &mut egui::Ui, plugin_id: &str) {
        self.session
            .show_plugin(ui, plugin_id, self.store, self.plugin_values);
    }

    fn pane_title(&self, plugin_id: &str) -> String {
        self.session.plugin_title(plugin_id)
    }

    fn pane_status(&self, plugin_id: &str) -> Option<(&str, wassh_ui::dock::HeaderHealth)> {
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

/// Whether this frame's input means the user is working with the window.
///
/// Pointer *movement* deliberately does not count: the desktop keeps sending motion while
/// the cursor rests over the window, and treating that as activity would pin the window at
/// the active repaint rate for as long as the pointer is on it.
fn input_started_work(events: &[egui::Event]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key { pressed: true, .. }
                | egui::Event::Text(_)
                | egui::Event::PointerButton { pressed: true, .. }
                | egui::Event::MouseWheel { .. }
                | egui::Event::Touch { .. }
                | egui::Event::Paste(_)
                | egui::Event::PasteImage(_)
                | egui::Event::MiddleClickPaste { .. }
                | egui::Event::Copy
                | egui::Event::Cut
                | egui::Event::Zoom(_)
        )
    })
}

fn apply_visuals(ctx: &egui::Context, palette: Palette) {
    wassh_ui::apply_visuals(ctx, &palette);
}

/// Consume a key press whose modifiers match `pattern` exactly.
///
/// `InputState::consume_key` matches *logically*, i.e. it ignores extra Shift and Alt, so a
/// plain-Ctrl binding would also swallow Ctrl+Shift+<key>. The terminal wants every one of
/// those (`^C`, `^Z`, `^W` … and their shifted variants), so only an exact chord is claimed.
///
/// `Modifiers::matches_exact` still treats Ctrl and Cmd as the same modifier, which is what
/// a real event needs: egui-winit sets `command` from Ctrl on this platform.
fn consume_chord(
    input: &mut egui::InputState,
    pattern: egui::Modifiers,
    key: egui::Key,
) -> bool {
    let mut consumed = false;
    input.events.retain(|event| {
        let claimed = matches!(
            event,
            egui::Event::Key { key: pressed, pressed: true, modifiers, .. }
                if *pressed == key && modifiers.matches_exact(pattern)
        );
        consumed |= claimed;
        !claimed
    });
    consumed
}#[cfg(test)]
mod tests {
    /// A command that keeps running until it is interrupted, in the shell a local tab
    /// starts on this platform, and the line it prints as soon as it is running.
    ///
    /// The marker matters: an interrupt that arrives while the shell is still starting the
    /// command is swallowed (`cmd.exe` ignores it before `ping` is up), so the test waits
    /// for it before interrupting. `sleep` itself prints nothing, so the shell does.
    #[cfg(unix)]
    const BLOCKING_COMMAND: &str = "printf 'go-%s\\n' 1; sleep 60\r";
    #[cfg(unix)]
    const BLOCKING_MARKER: &str = "go-1";
    /// `cmd.exe` has no `sleep`; pinging the loopback address blocks for a minute, never
    /// reads its stdin, and prints this line once it is running.
    #[cfg(windows)]
    const BLOCKING_COMMAND: &str = "ping -n 61 127.0.0.1\r";
    #[cfg(windows)]
    const BLOCKING_MARKER: &str = "Pinging";

    /// How long a test waits for its local shell to paint something.
    const SHELL_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
    /// Poll interval while waiting for the shell.
    const SHELL_POLL: std::time::Duration = std::time::Duration::from_millis(10);
    /// Frames egui needs to deliver a wheel delta it smooths out over several frames.
    const WHEEL_FRAMES: usize = 20;

    /// Turn the wheel at `pos` and let egui finish delivering the smoothed delta.
    fn wheel_at(shell: &mut Shell, ctx: &egui::Context, pos: egui::Pos2, delta: egui::Vec2) {
        let screen = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(1280.0, 800.0),
        ));
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: screen,
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| shell.frame(ui));
            output.drop_without_applying_deltas();
        };
        frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Wheel,
                phase: egui::TouchPhase::Move,
            },
        ]);
        for _ in 0..WHEEL_FRAMES {
            frame(vec![egui::Event::PointerMoved(pos)]);
        }
    }

    /// Run frames until the first session's screen holds `needle`.
    fn wait_for_text(shell: &mut Shell, ctx: &egui::Context, needle: &str, what: &str) {
        let deadline = std::time::Instant::now() + SHELL_WAIT;
        while !shell
            .sessions
            .first()
            .is_some_and(|session| session.visible_text().contains(needle))
        {
            assert!(std::time::Instant::now() < deadline, "{what}");
            run_shell(shell, ctx, 1);
            std::thread::sleep(SHELL_POLL);
        }
    }

    /// Wait until the local shell has painted its prompt.
    ///
    /// A `ConPTY` shell asks the terminal for a cursor position before it starts, so
    /// anything typed before that is dropped — and an interrupt that arrives while the
    /// shell is still starting ends it (`STATUS_CONTROL_C_EXIT`) instead of the command.
    fn wait_for_the_prompt(shell: &mut Shell, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + SHELL_WAIT;
        loop {
            let painted = shell
                .sessions
                .first()
                .is_some_and(|session| !session.visible_text().trim().is_empty());
            run_shell(shell, ctx, 1);
            if painted {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the local shell never painted its prompt"
            );
            std::thread::sleep(SHELL_POLL);
        }
    }

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
        // egui-winit fills `command` from Ctrl (Cmd on macOS) on every real event, so a
        // shortcut test that leaves it unset would not exercise what the app actually sees.
        let modifiers = egui::Modifiers {
            command: modifiers.ctrl || modifiers.mac_cmd,
            ..modifiers
        };
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Whether the frame asked the integration for the clipboard contents.
    fn requested_paste(output: &egui::FullOutput) -> bool {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::RequestPaste))
            })
    }

    /// The delay one frame asked the platform to wait before painting again.
    ///
    /// Needs a context that outlives the call: a fresh context asks for an immediate
    /// repaint to upload its fonts, which would mask the cadence.
    fn repaint_delay(
        ctx: &egui::Context,
        shell: &mut Shell,
        events: Vec<egui::Event>,
    ) -> Duration {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| shell.frame(ui));
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::ZERO, |viewport| viewport.repaint_delay);
        output.drop_without_applying_deltas();
        delay
    }

    #[test]
    fn a_quiet_window_repaints_slowly_and_activity_speeds_it_up() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        let ctx = egui::Context::default();
        // Let fonts, textures and the restored tab settle first.
        for _ in 0..3 {
            repaint_delay(&ctx, &mut shell, Vec::new());
        }

        // A settled window is only polled: a static pane (the monitor, say) cannot
        // burn the CPU by being redrawn sixty times a second.
        shell.active_until = None;
        assert_eq!(
            repaint_delay(&ctx, &mut shell, Vec::new()),
            IDLE_INTERVAL
        );

        // A keystroke opens the active window, so the echo is painted as soon as it
        // lands instead of waiting for the idle poll. (The delay reported by the key
        // frame itself is always zero: egui repaints immediately after input.)
        repaint_delay(
            &ctx,
            &mut shell,
            vec![key_event(egui::Key::A, egui::Modifiers::NONE)],
        );
        assert!(
            shell
                .active_until
                .is_some_and(|until| until > std::time::Instant::now()),
            "a keystroke must open the active repaint window"
        );

        // Output from the shell does the same.
        shell.active_until = None;
        shell.sessions[0].send(b"echo hello\r");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while shell.active_until.is_none() && std::time::Instant::now() < deadline {
            repaint_delay(&ctx, &mut shell, Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            shell.active_until.is_some(),
            "the shell's answer never asked for a repaint"
        );
    }

    #[test]
    fn ctrl_w_belongs_to_the_shell() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        shell.open_local();
        assert_eq!(shell.sessions.len(), 2);

        // Emacs and readline delete a word with Ctrl+W, so the tab stays open.
        frame_with_events(&mut shell, vec![key_event(egui::Key::W, egui::Modifiers::CTRL)]);
        assert_eq!(shell.sessions.len(), 2, "Ctrl+W closed a tab");

        // The shifted form and Ctrl+F4 still close it.
        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::W,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(shell.sessions.len(), 1, "Ctrl+Shift+W closes one tab");
        frame_with_events(&mut shell, vec![key_event(egui::Key::F4, egui::Modifiers::CTRL)]);
        assert!(shell.sessions.is_empty(), "Ctrl+F4 closes the last tab");
    }

    #[test]
    fn a_shifted_key_is_not_swallowed_by_the_plain_binding() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        shell.open_local();
        shell.open_local();
        shell.active = Some(1);

        frame_with_events(&mut shell, vec![key_event(egui::Key::Tab, egui::Modifiers::CTRL)]);
        assert_eq!(shell.active, Some(2), "Ctrl+Tab cycles forward");

        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::Tab,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(
            shell.active,
            Some(1),
            "Ctrl+Shift+Tab goes back instead of reusing the Ctrl+Tab binding"
        );

        // Ctrl+F1 is the program's, not the command palette's.
        frame_with_events(&mut shell, vec![key_event(egui::Key::F1, egui::Modifiers::CTRL)]);
        assert!(
            shell.dialogs.palette.is_none(),
            "Ctrl+F1 must reach the terminal"
        );
    }

    /// A plain Tab is terminal input, never egui's focus navigation.
    ///
    /// egui gives a Tab to the first widget that wants focus, and the widget then held the
    /// keyboard: every later keystroke was kept from the shell.
    #[test]
    fn tab_stays_with_the_terminal_instead_of_moving_focus() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        let ctx = egui::Context::default();
        // The widgets must have been drawn before one of them can take focus.
        run_shell(&mut shell, &ctx, 2);
        assert_eq!(ctx.memory(|memory| memory.focused()), None);

        for modifiers in [egui::Modifiers::NONE, egui::Modifiers::SHIFT] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1280.0, 800.0),
                )),
                events: vec![key_event(egui::Key::Tab, modifiers)],
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| shell.frame(ui));
            output.drop_without_applying_deltas();
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                None,
                "Tab moved the focus out of the terminal"
            );
            assert!(!ctx.egui_wants_keyboard_input());
        }
    }

    #[test]
    fn ctrl_c_interrupts_a_running_command() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        let ctx = egui::Context::default();
        wait_for_the_prompt(&mut shell, &ctx);
        // Start something that will not finish on its own.
        shell.sessions[0].send(BLOCKING_COMMAND.as_bytes());
        wait_for_text(
            &mut shell,
            &ctx,
            BLOCKING_MARKER,
            "the command never started",
        );

        // Ctrl+C arrives from egui as a clipboard event.
        frame_with_events(&mut shell, vec![egui::Event::Copy]);

        // Only a shell that got `^C` reads the next line; the blocked command ignores its
        // stdin, so without the interrupt the answer would not come for a minute.
        shell.sessions[0].send(b"echo ready-two\r");
        wait_for_text(&mut shell, &ctx, "ready-two", "Ctrl+C never reached the shell");
    }

    /// Job control is a terminal-line-discipline feature: `cmd.exe` has none, so Ctrl+Z
    /// cannot suspend a command there. Nothing else claims the key on any platform, which
    /// `the_menu_accelerators_are_the_bindings_that_exist` covers.
    #[cfg(unix)]
    #[test]
    fn ctrl_z_suspends_a_running_command() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        let ctx = egui::Context::default();
        wait_for_the_prompt(&mut shell, &ctx);
        shell.sessions[0].send(BLOCKING_COMMAND.as_bytes());
        wait_for_text(
            &mut shell,
            &ctx,
            BLOCKING_MARKER,
            "the command never started",
        );

        // Ctrl+Z is an ordinary key event; it must not be claimed by any app shortcut.
        frame_with_events(&mut shell, vec![key_event(egui::Key::Z, egui::Modifiers::CTRL)]);

        shell.sessions[0].send(b"echo ready-four\r");
        wait_for_text(&mut shell, &ctx, "ready-four", "Ctrl+Z never reached the shell");
    }

    #[test]
    fn ctrl_f4_closes_the_active_session() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        shell.open_local();
        shell.open_local();
        let ids: Vec<String> = shell.sessions.iter().map(|entry| entry.id.clone()).collect();
        shell.active = Some(1);

        frame_with_events(&mut shell, vec![key_event(egui::Key::F4, egui::Modifiers::CTRL)]);

        let remaining: Vec<String> = shell.sessions.iter().map(|entry| entry.id.clone()).collect();
        assert_eq!(
            remaining,
            vec![ids[0].clone(), ids[2].clone()],
            "Ctrl+F4 closes the session in front of the user, not just the last tab"
        );
        assert_eq!(shell.active, Some(1), "the neighbour takes over");
        assert_eq!(shell.closed.len(), 1, "and it can be reopened");
    }

    #[test]
    fn ctrl_c_is_not_a_copy_shortcut() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();

        // egui reports the platform's Ctrl+C as a clipboard event; the terminal must get
        // `^C` (SIGINT) instead of the app copying an empty selection.
        let output = frame_with_events(&mut shell, vec![egui::Event::Copy]);
        let copied = output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(_))
        });
        assert!(!copied, "Ctrl+C was treated as copy");
        assert!(shell.toasts.is_empty(), "Ctrl+C must not warn about a selection");
    }

    #[test]
    fn a_tab_closes_when_its_shell_exits() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let ctx = egui::Context::default();
        shell.open_local();
        assert_eq!(shell.sessions.len(), 1);

        // Type `exit` until the shell has taken it, then let the shell pump.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut typed = 0;
        while !shell.sessions.is_empty() && std::time::Instant::now() < deadline {
            if typed < 5 {
                shell.sessions[0].send(b"exit\r\n");
                typed += 1;
            }
            run_shell(&mut shell, &ctx, 1);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert!(
            shell.sessions.is_empty(),
            "a shell that exits must take its tab with it"
        );
        assert_eq!(shell.closed.len(), 1, "and stay reopenable");
    }

    /// Send `events`, let egui apply the focus change on the next pass, and read it back.
    fn focus_step(
        shell: &mut Shell,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Option<egui::Id> {
        shell_rows(shell, ctx, events);
        shell_rows(shell, ctx, Vec::new());
        ctx.memory(|memory| memory.focused())
    }

    /// One shell frame, returning every painted text and its position.
    fn shell_rows(
        shell: &mut Shell,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(i32, i32, String)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| shell.frame(ui));
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut rows: Vec<_> = shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.pos.y as i32,
                    text.pos.x as i32,
                    text.galley.text().to_owned(),
                )),
                _ => None,
            })
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn a_prompt_in_the_ai_agent_pane_reaches_the_provider() {
        use std::io::{Read, Write};
        // A provider that lists one model and answers one chat call, reporting what it was
        // asked.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().expect("the bound address").port();
        let (sender, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(read @ 1..) = stream.read(&mut chunk) {
                    request.extend_from_slice(&chunk[..read]);
                    let Some(head) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head_text = String::from_utf8_lossy(&request[..head]).to_lowercase();
                    let length: usize = head_text
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    if request.len() >= head + 4 + length {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let listed = text.starts_with("GET ");
                let body = if listed {
                    "{\"data\":[{\"id\":\"test-model\"}]}"
                } else {
                    "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\"hello back\"}}]}\n\n\
                     data: [DONE]\n\n"
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
                if !listed {
                    let _ = sender.send(text);
                    break;
                }
            }
        });

        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.settings.enabled_plugins.push("ai-agent".to_owned());
        shell
            .settings
            .plugin_settings
            .entry("ai-agent".to_owned())
            .or_default()
            .insert(
                wassh_plugins::ai_agent::PROVIDERS_KEY.to_owned(),
                serde_json::json!([{ "id": "local", "name": "Local", "protocol": "openai",
                                     "baseUrl": format!("http://127.0.0.1:{port}"),
                                     "models": ["test-model"] }]),
            );
        shell.open_local();
        shell.sessions[0].open_plugin(wassh_plugins::PluginId::AiAgent);

        let ctx = egui::Context::default();
        let mut rows = Vec::new();
        for _ in 0..3 {
            rows = shell_rows(&mut shell, &ctx, Vec::new());
        }
        // The empty prompt shows its placeholder; clicking it takes the focus.
        let prompt = rows
            .iter()
            .find(|(_, _, text)| text == "Ask the agent…")
            .cloned()
            .expect("the AI agent composer shows its prompt");
        let composer = egui::Pos2::new(prompt.1 as f32 + 8.0, prompt.0 as f32 + 4.0);
        let click = [
            egui::Event::PointerMoved(composer),
            egui::Event::PointerButton {
                pos: composer,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: composer,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ];
        shell_rows(&mut shell, &ctx, click.to_vec());
        shell_rows(&mut shell, &ctx, vec![egui::Event::Text("hello".to_owned())]);
        shell_rows(
            &mut shell,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );

        let request = requests
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the provider was never asked");
        assert!(request.contains("\"content\": \"hello\""), "{request}");
    }

    #[test]
    fn a_failed_connection_keeps_its_tab_and_retries() {
        use wassh_core::ReconnectMode;

        let mut shell = Shell::new_with(Some(scratch_config()), false);
        // Nothing listens on port 1, so the connect fails immediately.
        let connection = ConnectionParams {
            kind: ConnectionType::Ssh,
            host: "127.0.0.1".to_owned(),
            port: 1,
            username: "nobody".to_owned(),
            auth: AuthMethod::None,
            reconnect: ReconnectMode::Always,
            ..ConnectionParams::default()
        };
        shell.start_ssh(connection, None, Secrets::default());
        assert_eq!(shell.sessions.len(), 1);

        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while shell.sessions[0].status != SessionStatus::Reconnecting
            && std::time::Instant::now() < deadline
        {
            run_shell(&mut shell, &ctx, 1);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert_eq!(shell.sessions.len(), 1, "a failed connection keeps its tab");
        assert_eq!(
            shell.sessions[0].status,
            SessionStatus::Reconnecting,
            "and the reconnect policy takes over"
        );

        // The booked wait must end in an attempt on its own; a scheduled retry that is
        // cancelled leaves the tab in `Reconnecting` forever.
        assert_eq!(shell.sessions[0].reconnect_attempt, 1);
        shell.sessions[0].retry_at = Some(std::time::Instant::now());
        run_shell(&mut shell, &ctx, 1);
        assert_ne!(
            shell.sessions[0].status,
            SessionStatus::Reconnecting,
            "the scheduled retry never ran"
        );
        assert!(
            shell.sessions[0].retry_at.is_none(),
            "the wait outlived the retry it belonged to"
        );
        assert_eq!(
            shell.sessions[0].reconnect_attempt, 1,
            "one attempt was due, not two"
        );
    }

    #[test]
    fn ctrl_shift_t_reopens_the_closed_tab() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let reopen = || {
            vec![key_event(
                egui::Key::T,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )]
        };

        // Nothing closed yet: the shortcut must not open a fresh local tab.
        frame_with_events(&mut shell, reopen());
        assert!(shell.sessions.is_empty(), "Ctrl+Shift+T opened a tab");

        shell.open_local();
        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::W,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert!(shell.sessions.is_empty());
        assert_eq!(shell.closed.len(), 1);

        frame_with_events(&mut shell, reopen());
        assert_eq!(shell.sessions.len(), 1, "Ctrl+Shift+T reopened the tab");
        assert!(shell.closed.is_empty());
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
        assert!(
            requested_paste(&output),
            "Shift+Insert requests a paste"
        );

        // Ctrl+Shift+Insert does the same.
        let output = frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::Insert,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert!(
            requested_paste(&output),
            "Ctrl+Shift+Insert requests a paste"
        );

        // Ctrl+Shift+V does the same, with the `command` flag a real Ctrl press carries.
        let output = frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::V,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert!(
            requested_paste(&output),
            "Ctrl+Shift+V requests a paste"
        );
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

    #[test]
    fn identity_deletion_is_blocked_by_hosts_and_tabs() {
        let mut config = scratch_config();
        let identity = wassh_core::Identity { id: "shared".to_owned(), name: "Shared".to_owned(), ..Default::default() };
        config.identities.set(vec![identity]).unwrap();
        let mut shell = Shell::new_with(Some(config), false);
        let connection = ConnectionParams { host: "example.com".to_owned(), identity_id: Some("shared".to_owned()), ..Default::default() };
        let host = HostProfile { id: "host-identity".to_owned(), name: "Host".to_owned(), connection: connection.clone(), ..Default::default() };
        shell.hosts.upsert(host);
        assert!(!shell.save_identities(Vec::new()));
        shell.hosts.remove("host-identity");
        shell.config.as_mut().unwrap().tabs.set(vec![wassh_core::TabSnapshot { id: "saved-tab".to_owned(), connection: connection.clone(), ..Default::default() }]).unwrap();
        assert!(!shell.save_identities(Vec::new()));
        shell.config.as_mut().unwrap().tabs.set(Vec::new()).unwrap();
        shell.open(connection, None);
        assert!(!shell.save_identities(Vec::new()));
        shell.close(0);
        assert!(shell.save_identities(Vec::new()));
        assert!(shell.identities().is_empty());
        let dir = shell.config.as_ref().unwrap().dir.clone();
        drop(shell);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn identity_options_count_saved_hosts_only() {
        let mut config = scratch_config();
        config.identities.set(vec![
            wassh_core::Identity { id: "shared".to_owned(), name: "Shared".to_owned(), ..Default::default() },
            wassh_core::Identity { id: "unused".to_owned(), name: "Unused".to_owned(), ..Default::default() },
        ]).unwrap();
        let mut shell = Shell::new_with(Some(config), false);
        for id in ["first", "second"] {
            shell.hosts.upsert(HostProfile {
                id: id.to_owned(), name: id.to_owned(),
                connection: ConnectionParams { identity_id: Some("shared".to_owned()), ..Default::default() },
                ..Default::default()
            });
        }
        shell.hosts.upsert(HostProfile { id: "other".to_owned(), name: "Other".to_owned(), ..Default::default() });
        shell.config.as_mut().unwrap().tabs.set(vec![wassh_core::TabSnapshot {
            connection: ConnectionParams { identity_id: Some("unused".to_owned()), ..Default::default() },
            ..Default::default()
        }]).unwrap();
        let state = shell.options_state();
        assert_eq!(state.identities[0].host_count, 2);
        assert_eq!(state.identities[1].host_count, 0);
        shell.hosts.remove("first");
        assert_eq!(shell.options_state().identities[0].host_count, 1);
        let dir = shell.config.as_ref().unwrap().dir.clone();
        drop(shell);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn identity_credentials_are_reloaded_on_reconnect() {
        let mut config = scratch_config();
        config.identities.set(vec![wassh_core::Identity { id: "shared".to_owned(), name: "Shared".to_owned(), username: "alice".to_owned(), ..Default::default() }]).unwrap();
        let mut shell = Shell::new_with(Some(config), false);
        let connection = ConnectionParams { host: "127.0.0.1".to_owned(), port: 1, identity_id: Some("shared".to_owned()), ..Default::default() };
        shell.open(connection, None);
        assert!(shell.sessions[0].awaiting_login());
        shell.secrets.as_mut().unwrap().set_identity_password("shared", Some("updated")).unwrap();
        shell.restart(0, Secrets::default());
        assert_eq!(shell.sessions[0].secrets.password, "updated");
        assert!(!shell.sessions[0].awaiting_login());
        let dir = shell.config.as_ref().unwrap().dir.clone();
        shell.close(0);
        drop(shell);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn identity_drafts_save_secrets_and_restore_tabs() {
        let mut config = scratch_config();
        config.settings.get_mut().reconnect_on_startup = true;
        let mut tab = password_tab(None);
        tab.connection.identity_id = Some("shared".to_owned());
        config.tabs.set(vec![tab]).unwrap();
        let mut shell = Shell::new_with(Some(config), false);
        let draft = IdentityDraft {
            identity: wassh_core::Identity {
                id: "shared".to_owned(), name: "Shared".to_owned(), username: "bob".to_owned(),
                ..Default::default()
            },
            password: "saved-secret".to_owned(),
            ..Default::default()
        };
        assert!(shell.save_identities(vec![draft]));
        let dir = shell.config.as_ref().unwrap().dir.clone();
        let metadata = std::fs::read_to_string(wassh_store::paths::identities_file(&dir)).unwrap();
        assert!(!metadata.contains("saved-secret"));
        shell.finish_startup();
        assert_eq!(shell.sessions.len(), 1);
        assert_eq!(shell.sessions[0].secrets.password, "saved-secret");
        assert!(!shell.sessions[0].awaiting_login());
        assert_eq!(shell.sessions[0].connection.identity_id.as_deref(), Some("shared"));
        shell.close(0);
        drop(shell);
        let _ = std::fs::remove_dir_all(dir);
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
    fn a_plain_text_provider_key_is_sealed_on_startup() {
        let mut config = scratch_config();
        let mut values = serde_json::Map::new();
        values.insert(
            wassh_plugins::ai_agent::API_KEYS_KEY.to_owned(),
            serde_json::json!({ "deepseek": "sk-secret" }),
        );
        config
            .settings
            .get_mut()
            .plugin_settings
            .insert(PluginId::AiAgent.as_str().to_owned(), values);

        let shell = Shell::new_with(Some(config), false);
        assert!(
            !shell
                .settings
                .plugin_settings
                .get(PluginId::AiAgent.as_str())
                .is_some_and(|values| values.contains_key(wassh_plugins::ai_agent::API_KEYS_KEY)),
            "settings must not keep a readable provider key"
        );
        let dir = shell.config.as_ref().expect("config").dir.clone();
        let store = wassh_store::SecretsStore::open(&dir).expect("vault");
        let field = wassh_plugins::ai_agent::provider_secret_field("deepseek");
        assert_eq!(
            store
                .plugin_secret(
                    wassh_store::secrets::APP_SCOPE,
                    PluginId::AiAgent.as_str(),
                    &field
                )
                .as_deref(),
            Some("sk-secret")
        );
    }

    #[test]
    fn a_provider_key_in_a_pane_document_is_sealed_on_startup() {
        let config = scratch_config();
        config
            .save_plugin_data(
                PluginId::AiAgent.as_str(),
                Some("tab-abc"),
                &serde_json::json!({
                    "apiKeys": { "deepseek": "sk-doc" },
                    "providers": [],
                }),
            )
            .expect("pane document");

        let shell = Shell::new_with(Some(config), false);
        let config = shell.config.as_ref().expect("config");
        let store = wassh_store::SecretsStore::open(&config.dir).expect("vault");
        let field = wassh_plugins::ai_agent::provider_secret_field("deepseek");
        assert_eq!(
            store
                .plugin_secret(
                    wassh_store::secrets::APP_SCOPE,
                    PluginId::AiAgent.as_str(),
                    &field
                )
                .as_deref(),
            Some("sk-doc")
        );
        let document = config
            .plugin_data(PluginId::AiAgent.as_str(), Some("tab-abc"))
            .expect("read back")
            .expect("document");
        assert!(
            document.get(wassh_plugins::ai_agent::API_KEYS_KEY).is_none(),
            "the pane document still holds keys: {document:?}"
        );
        assert!(document.get("providers").is_some(), "providers stay");
    }

    #[test]
    fn a_password_of_a_plugin_this_build_lacks_is_still_sealed() {
        let mut config = scratch_config();
        let mut values = serde_json::Map::new();
        values.insert("password".to_owned(), serde_json::json!("broker-pw"));
        values.insert("host".to_owned(), serde_json::json!("broker.local"));
        let mut host = HostProfile {
            id: "h1".to_owned(),
            name: "Dev".to_owned(),
            ..HostProfile::default()
        };
        host.plugin_settings.insert("mqtt-explorer".to_owned(), values);
        config.save_host(host).expect("host");

        let shell = Shell::new_with(Some(config), false);
        let config = shell.config.as_ref().expect("config");
        let store = wassh_store::SecretsStore::open(&config.dir).expect("vault");
        assert_eq!(
            store
                .plugin_secret("h1", "mqtt-explorer", "password")
                .as_deref(),
            Some("broker-pw")
        );
        let saved = shell.hosts.host("h1").expect("host");
        assert!(
            !saved.plugin_settings["mqtt-explorer"].contains_key("password"),
            "hosts.json still holds the password: {:?}",
            saved.plugin_settings
        );
        assert_eq!(
            saved.plugin_settings["mqtt-explorer"]["host"],
            serde_json::json!("broker.local"),
            "other fields stay"
        );
        let text = std::fs::read_to_string(config.dir.join("hosts.json")).expect("read");
        assert!(!text.contains("broker-pw"), "the file kept the password");
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
            shell.local_params(),
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
    fn a_restored_window_remembers_the_frame_offset() {
        // The compositor places the window frame where it is asked to and reports the
        // client position back. Saving that report verbatim would walk the window down and
        // right by the decoration on every launch; the measured offset cancels it.
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let ctx = egui::Context::default();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        // The restore asked for frame (500,300); the client landed at (504,332).
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(504.0, 332.0),
                    egui::vec2(900.0, 700.0),
                )),
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(504.0, 332.0),
                    egui::vec2(900.0, 700.0),
                )),
                maximized: Some(false),
                ..Default::default()
            },
        );
        shell.window.requested = Some(egui::pos2(500.0, 300.0));
        shell.window.wait = 0;
        let output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            shell.window.learn_offset(&ctx);
            shell.track_window_bounds(&ctx);
        });
        output.drop_without_applying_deltas();

        assert_eq!(shell.window.offset, Some(egui::vec2(4.0, 32.0)));
        assert_eq!(
            shell.settings.window_bounds,
            Some(WindowBounds {
                x: 500,
                y: 300,
                width: 900,
                height: 700,
                maximized: false,
            }),
            "the saved value is the frame position the restore asked for"
        );
        assert!(shell.window.requested.is_none(), "the offset is learnt once");
    }

    #[test]
    fn a_window_that_ignored_the_restore_does_not_learn_a_bogus_offset() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let ctx = egui::Context::default();
        // Asked for (500,300), the window stayed at (1200,700): the request was refused.
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(1200.0, 700.0),
                    egui::vec2(900.0, 700.0),
                )),
                outer_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(1200.0, 700.0),
                    egui::vec2(900.0, 700.0),
                )),
                maximized: Some(false),
                ..Default::default()
            },
        );
        shell.window.requested = Some(egui::pos2(500.0, 300.0));
        shell.window.wait = 0;
        let output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            shell.window.learn_offset(&ctx);
        });
        output.drop_without_applying_deltas();
        assert_eq!(shell.window.offset, None, "a refused move teaches nothing");
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
                shell.local_params(),
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
    fn a_session_dialog_save_keeps_plugin_values_in_its_tab() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let mut host = HostProfile {
            id: "h1".to_owned(),
            name: "Dev".to_owned(),
            ..HostProfile::default()
        };
        host.plugin_settings.insert(
            "server-monitor".to_owned(),
            [("showGauges".to_owned(), serde_json::json!(true))]
                .into_iter()
                .collect(),
        );
        shell.save_host(host.clone());

        let open = |id: &str, shell: &mut Shell| {
            let mut entry = crate::app::session::SessionEntry::new_local(
                id,
                &shell.palette,
                &shell.settings.session_style_defaults,
                "xterm-256color",
                TerminalSize::new(80, 24),
                shell.local_params(),
            );
            entry.host_id = Some("h1".to_owned());
            shell.sessions.push(entry);
        };
        open("tab-1", &mut shell);
        open("tab-2", &mut shell);

        // The tab's dialog changed one existing value and added a broker of its own.
        let mut edited = host.plugin_settings.clone();
        edited
            .get_mut("server-monitor")
            .expect("the host's values")
            .insert("showGauges".to_owned(), serde_json::json!(false));
        edited.insert(
            "mqtt-analyser".to_owned(),
            [("host".to_owned(), serde_json::json!("broker.internal"))]
                .into_iter()
                .collect(),
        );
        let save = HostSave {
            host: HostProfile {
                id: "h1".to_owned(),
                plugin_settings: edited,
                ..HostProfile::default()
            },
            password: SecretEdit::Unchanged,
            passphrase: SecretEdit::Unchanged,
            plugin_secrets: [(
                dialogs::secret_field_key("mqtt-analyser", "password"),
                dialogs::SecretDraft {
                    draft: "s3cret".to_owned(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        };
        assert!(shell.apply_to_session("tab-1", &save));

        // The tab reads what its dialog set, secrets included.
        let values = shell.take_plugin_values(0);
        assert_eq!(
            values.host["server-monitor"]["showGauges"],
            serde_json::json!(false)
        );
        assert_eq!(
            values.host["mqtt-analyser"]["host"],
            serde_json::json!("broker.internal")
        );
        assert_eq!(values.secrets["mqtt-analyser"]["password"], "s3cret");

        // Only what the dialog changed is the tab's own; the host keeps its values and
        // its other tab reads them.
        let overrides = shell.sessions[0].plugin_overrides();
        assert_eq!(overrides.len(), 2, "{overrides:?}");
        assert_eq!(
            overrides["server-monitor"].len(),
            1,
            "an untouched field is not a tab-only value"
        );
        assert_eq!(
            shell.hosts.host("h1").expect("saved host").plugin_settings["server-monitor"]
                ["showGauges"],
            serde_json::json!(true)
        );
        let other = shell.take_plugin_values(1);
        assert_eq!(
            other.host["server-monitor"]["showGauges"],
            serde_json::json!(true)
        );
        assert!(!other.host.contains_key("mqtt-analyser"));
        assert!(other.secrets.is_empty(), "{:?}", other.secrets);
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

        let copy_id = shell
            .hosts
            .hosts
            .iter()
            .find(|host| host.name == "Beta (copy)")
            .map(|host| host.id.clone())
            .expect("copy saved");
        let names: Vec<_> = shell
            .hosts
            .hosts
            .iter()
            .filter(|host| !host.is_local())
            .map(|host| host.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Beta", "Beta (copy)"]);
        // The ungrouped order matches the document order, after the built-in host.
        assert_eq!(
            shell.hosts.ungrouped_host_ids,
            [
                wassh_core::LOCAL_HOST_ID.to_owned(),
                "host-1".to_owned(),
                "host-2".to_owned(),
                copy_id.clone(),
            ]
        );

        let copy = shell.hosts.host(&copy_id).expect("copy saved");
        assert_ne!(copy.id, "host-2", "the copy gets a fresh id");
        assert_eq!(copy.connection.host, "host-2.example.com");
        assert_eq!(copy.connection.port, 2222);
        assert_eq!(copy.connection.username, "root");
        assert_eq!(copy.group_id, None);
        // It is on disk too.
        let stored = shell.config.as_ref().expect("config").hosts.get();
        assert_eq!(stored.hosts.len(), 4);
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
        assert_eq!(
            shell.hosts.ungrouped_host_ids,
            [wassh_core::LOCAL_HOST_ID.to_owned()]
        );
    }

    #[test]
    fn duplicating_an_unknown_host_does_nothing() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.save_host(host("host-1", "Alpha", None));
        shell.duplicate_host("nope");
        assert_eq!(shell.hosts.hosts.len(), 2);
        assert!(shell.hosts.host("nope").is_none());
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
    fn only_plugins_with_an_options_section_get_a_config_gear() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.settings.enabled_plugins = wassh_plugins::PluginId::UI_ORDER
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        // Every plugin that declares app-wide settings gets a gear: the monitor, the
        // scratchpad and the files pane declare none, so their rows have nothing to open.
        shell.refresh_plugin_labels();
        assert_eq!(
            shell.gear_labels,
            ["Macros", "MQTT", "AI Agent", "Conn Log", "Docker"]
        );

        // A disabled plugin has no section in Options, so it loses its gear too.
        shell.settings.enabled_plugins = vec!["docker-manager".to_owned()];
        shell.refresh_plugin_labels();
        assert_eq!(shell.gear_labels, ["Docker"]);
        shell.settings.enabled_plugins.clear();
        shell.refresh_plugin_labels();
        assert!(shell.gear_labels.is_empty());
    }

    #[test]
    fn the_gear_in_the_plugins_menu_opens_the_plugin_options() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.settings.enabled_plugins = vec!["macro-pad".to_owned()];
        // "Macros" is the macro pad's toolbar label.
        shell.open_plugin_options("Macros");
        let state = shell.dialogs.options.as_ref().expect("options opened");
        assert_eq!(
            state.nav.active, dialogs::OPTION_SECTIONS.len(),
            "the macro pad section follows the built-in ones"
        );

        // A label that is not a plugin does nothing.
        shell.dialogs.options = None;
        shell.open_plugin_options("Nonexistent");
        assert!(shell.dialogs.options.is_none());
    }

    /// The labels of one menu, as the title bar offers them.
    fn menu_items(name: &str) -> &'static [&'static str] {
        menus()
            .iter()
            .find(|(menu, _)| *menu == name)
            .map(|(_, items)| *items)
            .expect("the menu exists")
    }

    #[test]
    fn the_file_menu_keeps_only_the_application_commands() {
        // New tabs and hosts have their own places (the tab strip and the sidebar), so the
        // File menu carries the program's own commands only.
        let items = menu_items("File");
        assert_eq!(items, ["Options…", "", "Quit"]);
        assert!(!items.contains(&"New tab"));
        assert!(!items.contains(&"New host…"));
    }

    #[test]
    fn the_menu_accelerators_are_the_bindings_that_exist() {
        // Only chords that really are wired are advertised, and nothing the terminal keeps
        // for the shell (Ctrl+C, Ctrl+W …) ever appears next to a menu row.
        let shortcuts: Vec<(&str, &str)> = MENU_SHORTCUTS.to_vec();
        // Every advertised accelerator belongs to a row that exists, in one menu only.
        for (label, keys) in &shortcuts {
            assert!(!keys.is_empty(), "{label:?} has no accelerator");
            let rows: Vec<&str> = menus()
                .iter()
                .flat_map(|(_, items)| items.iter().copied())
                .filter(|item| item == label)
                .collect();
            assert_eq!(rows.len(), 1, "{label:?} names one menu row: {rows:?}");
        }
        assert_eq!(
            shortcuts,
            [
                ("Options…", "Ctrl+,"),
                ("Copy", "Ctrl+Shift+C"),
                ("Paste", "Ctrl+Shift+V"),
                ("Find…", "Ctrl+Shift+F"),
                ("New terminal", "Ctrl+Shift+N"),
                ("Reconnect", "Ctrl+Shift+R"),
                ("Reopen closed tab", "Ctrl+Shift+T"),
            ]
        );
    }

    #[test]
    fn tab_walks_the_focus_through_a_dialog() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.handle_command(Command::OpenOptions);
        let ctx = egui::Context::default();
        let _ = run_shell(&mut shell, &ctx, 3);
        assert!(
            ctx.memory(|memory| memory.focused()).is_none(),
            "nothing is focused until the user asks"
        );

        // Tab keeps moving: every stop is a different control, and it stays on screen —
        // the dialog scrolls along with the focus instead of leaving it below the fold.
        let screen = ctx.content_rect();
        let mut walked = Vec::new();
        for step in 0..24 {
            let focused = focus_step(
                &mut shell,
                &ctx,
                vec![key_event(egui::Key::Tab, egui::Modifiers::NONE)],
            )
            .unwrap_or_else(|| panic!("Tab dropped the focus at step {step}"));
            if let Some(before) = walked.last() {
                assert_ne!(&focused, before, "Tab did not move the focus");
            }
            let rect = ctx.read_response(focused).expect("the focused control").rect;
            assert!(
                screen.contains_rect(rect),
                "the focused control is off screen: {rect:?}"
            );
            walked.push(focused);
        }

        // Shift+Tab walks back through the same stops.
        for step in 0..4 {
            let before = *walked.last().expect("a focus");
            let focused = focus_step(
                &mut shell,
                &ctx,
                vec![key_event(egui::Key::Tab, egui::Modifiers::SHIFT)],
            )
            .unwrap_or_else(|| panic!("Shift+Tab dropped the focus at step {step}"));
            assert_ne!(focused, before, "Shift+Tab did not move the focus");
            assert!(
                walked.contains(&focused),
                "Shift+Tab left the dialog's controls"
            );
            walked.push(focused);
        }
    }

    #[test]
    fn the_session_menu_opens_a_local_terminal() {
        let items = menu_items("Session");
        assert_eq!(items.first(), Some(&"New terminal"));
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        let ctx = egui::Context::default();
        shell.menu_command(&ctx, "New terminal");
        assert_eq!(shell.sessions.len(), 1, "New terminal opened a tab");
        assert_eq!(shell.active, Some(0));
        assert!(
            matches!(
                shell.sessions[0].connection.kind,
                wassh_core::ConnectionType::Local
            ),
            "the new tab is a local terminal"
        );
    }

    #[test]
    fn ctrl_shift_n_opens_a_local_terminal() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::N,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert_eq!(shell.sessions.len(), 1, "Ctrl+Shift+N opened a tab");
        assert!(matches!(
            shell.sessions[0].connection.kind,
            wassh_core::ConnectionType::Local
        ));
    }

    #[test]
    fn ctrl_shift_r_restarts_the_active_session() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        let ctx = egui::Context::default();
        run_shell(&mut shell, &ctx, 2);

        // A session that failed keeps its tab and shows the reason there; reconnect is what
        // clears it and starts the transport again.
        shell.sessions[0].error = Some("connection lost".to_owned());
        shell.sessions[0].status_message = Some("stale".to_owned());
        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::R,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )],
        );
        assert!(
            shell.sessions[0].error.is_none(),
            "Ctrl+Shift+R did not restart the session"
        );
        assert!(shell.sessions[0].status_message.is_none());

        // And the chord never reaches the shell, where Ctrl+R is readline's reverse search.
        run_shell(&mut shell, &ctx, 2);
        assert!(
            !shell.sessions[0].visible_text().contains("reverse-i-search"),
            "Ctrl+Shift+R was typed into the terminal"
        );
    }

    #[test]
    fn ctrl_comma_opens_options() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        frame_with_events(
            &mut shell,
            vec![key_event(
                egui::Key::Comma,
                egui::Modifiers::CTRL,
            )],
        );
        assert!(shell.dialogs.options.is_some(), "Ctrl+, opens Options");
    }

    #[test]
    fn the_wheel_scrolls_only_the_terminal_under_the_pointer() {
        let mut shell = Shell::new_with(Some(scratch_config()), false);
        shell.open_local();
        shell.sessions[0].open_plugin(PluginId::Scratchpad);
        // Scrollback without waiting on a shell: the lines go straight into the emulator.
        let printed: String = (0..80).map(|n| format!("line-{n}\r\n")).collect();
        shell.sessions[0].terminal.feed(printed.as_bytes());
        let ctx = egui::Context::default();
        let _ = run_shell(&mut shell, &ctx, 2);
        let text = shell.sessions[0].visible_text();
        assert!(text.contains("line-79"), "the tail is on screen");
        assert!(!text.contains("line-30"), "the head is in the scrollback");

        // A wheel turn over the plugin pane leaves the terminal where it is.
        wheel_at(&mut shell, &ctx, egui::pos2(1000.0, 400.0), egui::vec2(0.0, 400.0));
        assert!(
            shell.sessions[0].visible_text().contains("line-79"),
            "scrolling the pane moved the terminal"
        );

        // Over the terminal it scrolls back through the history.
        wheel_at(&mut shell, &ctx, egui::pos2(400.0, 400.0), egui::vec2(0.0, 400.0));
        let text = shell.sessions[0].visible_text();
        assert!(
            text.contains("line-30") && !text.contains("line-79"),
            "the wheel did not scroll back: {text}"
        );
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





