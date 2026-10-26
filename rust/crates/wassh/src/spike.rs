//! Phase 0 terminal spike: a native window running a local shell.
//!
//! This exercises the same three layers the finished app uses — PTY transport, the
//! `alacritty_terminal` emulator and the egui terminal widget — so VT behaviour can be
//! exercised with `vim`, `htop`, `mc`, `tmux` and `less`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use wassh_core::{
    APP_NAME, DEFAULT_SCROLLBACK_LINES, DEFAULT_TERM_TYPE, Palette, SessionStatus, ThemeMode,
};
use wassh_ssh::{
    AuthPromptField, HostKeyPolicy, KnownHosts, SshAuth, SshEvent, SshOptions, SshSession,
};
use wassh_term::{PtySession, Terminal, TerminalEvent, TerminalSize, encode};
use wassh_ui::terminal_view::to_color32;
use wassh_ui::{TerminalInput, TerminalView};

use crate::gallery::Gallery;

/// Open the spike window. Blocks until the window closes.
pub fn run_app() -> eframe::Result<()> {
    let (target, auth_mode) = ssh_target_from_args();
    let widgets = std::env::args().any(|argument| argument == "--widgets");
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title(format!("{APP_NAME} — terminal spike"))
        .with_app_id("wassh");
    if !cfg!(target_os = "macos") {
        viewport = viewport.with_decorations(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "wassh",
        options,
        Box::new(move |cc| Ok(Box::new(SpikeApp::new(cc, target, auth_mode, widgets)))),
    )
}

/// Destination parsed from `--ssh user@host[:port]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SshTarget {
    host: String,
    port: u16,
    user: String,
}

/// Which authentication method the spike should use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum AuthMode {
    #[default]
    Password,
    Agent,
    Keyboard,
}

fn ssh_target_from_args() -> (Option<SshTarget>, AuthMode) {
    let mut args = std::env::args().skip(1);
    let mut target = None;
    let mut mode = AuthMode::Password;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ssh" => target = args.next().map(|spec| parse_target(&spec)),
            "--auth" => {
                mode = match args.next().as_deref() {
                    Some("agent") => AuthMode::Agent,
                    Some("keyboard") => AuthMode::Keyboard,
                    _ => AuthMode::Password,
                }
            }
            _ => {}
        }
    }
    (target, mode)
}

fn parse_target(spec: &str) -> SshTarget {
    let (user, host_port) = spec.split_once('@').unwrap_or(("root", spec));
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => (host.to_owned(), port),
            Err(_) => (host_port.to_owned(), 22),
        },
        None => (host_port.to_owned(), 22),
    };
    SshTarget {
        host,
        port,
        user: user.to_owned(),
    }
}

/// Menu bar shown inside the custom titlebar.
const TITLEBAR_MENUS: &[wassh_ui::titlebar::Menu] = &[
    ("File", &["Preferences…", "Command Palette", "", "Quit"]),
    ("Edit", &["Copy", "Paste", "", "Find"]),
    ("Session", &["Reconnect", "Reconnect All", "", "Session settings"]),
    ("View", &["Zoom In", "Zoom Out", "", "Toggle Full Screen"]),
    ("Help", &["Check for Updates…", "", "About WaSSH"]),
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct HostKeyPrompt {
    host: String,
    port: u16,
    fingerprint: String,
    mismatch: bool,
}

struct AuthPrompt {
    name: String,
    instructions: String,
    fields: Vec<AuthPromptField>,
    answers: Vec<String>,
}

struct SpikeApp {
    terminal: Terminal,
    events: Receiver<TerminalEvent>,
    view: TerminalView,
    palette: Palette,
    local: Option<PtySession>,
    ssh: Option<SshSession>,
    target: Option<SshTarget>,
    known_hosts: Arc<KnownHosts>,
    host_key_prompt: Option<HostKeyPrompt>,
    auth_prompt: Option<AuthPrompt>,
    auth_mode: AuthMode,
    password: String,
    status: SessionStatus,
    error: Option<String>,
    fullscreen: bool,
    gallery: Option<Gallery>,
}

impl SpikeApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        target: Option<SshTarget>,
        auth_mode: AuthMode,
        widgets: bool,
    ) -> Self {
        let palette = Palette::for_mode(ThemeMode::Dark);
        apply_visuals(&cc.egui_ctx, palette);

        let (tx, events) = channel();
        let size = TerminalSize::new(80, 24);
        let terminal = Terminal::new(size, DEFAULT_SCROLLBACK_LINES as usize, tx, &palette);
        let known_hosts = match wassh_store::data_dir() {
            Ok(dir) => KnownHosts::load(wassh_store::known_hosts_file(dir)),
            Err(_) => KnownHosts::in_memory(),
        };

        let mut app = Self {
            terminal,
            events,
            view: TerminalView::default(),
            palette,
            local: None,
            ssh: None,
            target,
            known_hosts: Arc::new(known_hosts),
            host_key_prompt: None,
            auth_prompt: None,
            auth_mode,
            password: String::new(),
            status: SessionStatus::Idle,
            error: None,
            fullscreen: false,
            gallery: widgets.then(Gallery::default),
        };

        if app.target.is_none() && app.gallery.is_none() {
            app.start_local();
        }
        app
    }

    fn start_local(&mut self) {
        let (shell, shell_args) = PtySession::default_shell();
        match PtySession::spawn(
            &shell,
            &shell_args,
            self.terminal.size(),
            DEFAULT_TERM_TYPE,
        ) {
            Ok(pty) => {
                self.local = Some(pty);
                self.status = SessionStatus::Connected;
            }
            Err(error) => {
                self.error = Some(format!("failed to start {shell}: {error}"));
                self.status = SessionStatus::Failed;
            }
        }
    }

    fn start_ssh(&mut self) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let mut options = SshOptions::new(
            target.host.clone(),
            target.user.clone(),
            match self.auth_mode {
                AuthMode::Password => SshAuth::Password(self.password.clone()),
                AuthMode::Agent => SshAuth::Agent,
                AuthMode::Keyboard => SshAuth::KeyboardInteractive,
            },
        );
        options.port = target.port;
        options.cols = self.terminal.size().cols;
        options.rows = self.terminal.size().rows;
        options.term = DEFAULT_TERM_TYPE.to_owned();
        options.x11 = wassh_ssh::x11::local_config();
        options.host_key_policy = HostKeyPolicy::Ask(self.known_hosts.clone());

        self.error = None;
        self.host_key_prompt = None;
        self.status = SessionStatus::Connecting;
        match SshSession::connect(options) {
            Ok(session) => self.ssh = Some(session),
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.status = SessionStatus::Failed;
            }
        }
    }

    fn pump_output(&mut self, ctx: &egui::Context) {
        if let Some(local) = &self.local {
            for chunk in local.read_pending() {
                self.terminal.feed(&chunk);
            }
        }

        let ssh_events = self
            .ssh
            .as_ref()
            .map(SshSession::events)
            .unwrap_or_default();
        for event in ssh_events {
            match event {
                SshEvent::Data(bytes) => self.terminal.feed(&bytes),
                SshEvent::Status { status, .. } => self.status = status,
                SshEvent::HostKeyPrompt {
                    host,
                    port,
                    fingerprint,
                    mismatch,
                } => {
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
                    self.auth_prompt = Some(AuthPrompt {
                        name,
                        instructions,
                        answers: vec![String::new(); prompts.len()],
                        fields: prompts,
                    });
                }
                SshEvent::AuthFailed { detail } => {
                    // The spike has no prompt, so report it like any other failure.
                    self.error = Some(detail);
                    self.status = SessionStatus::Failed;
                }
                SshEvent::Closed { reason, end } => {
                    if end.by_shell() {
                        self.status = SessionStatus::Closed;
                    }
                    self.host_key_prompt = None;
                    self.auth_prompt = None;
                    if let Some(reason) = reason {
                        self.error = Some(reason);
                    }
                }
                SshEvent::Error(error) => {
                    self.error = Some(error);
                    self.status = SessionStatus::Failed;
                }
            }
        }

        let mut replies = Vec::new();
        for event in self.events.try_iter() {
            match event {
                TerminalEvent::Title(title) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                        "{APP_NAME} — {title}"
                    )));
                }
                TerminalEvent::Bell => {}
                TerminalEvent::PtyWrite(text) => replies.push(text),
                TerminalEvent::Wakeup => {}
            }
        }
        for reply in replies {
            self.write_backend(reply.as_bytes());
        }
    }

    fn forward_input(&mut self, ctx: &egui::Context) {
        if self.status != SessionStatus::Connected || ctx.egui_wants_keyboard_input() {
            return;
        }
        let modes = self.terminal.modes();
        for input in wassh_ui::collect_terminal_input(ctx, false) {
            let bytes = match input {
                TerminalInput::Text(text) => text.into_bytes(),
                TerminalInput::Key(stroke) => encode(&stroke, modes),
                TerminalInput::Paste(text) => {
                    if modes.bracketed_paste {
                        let mut wrapped = Vec::with_capacity(text.len() + 12);
                        wrapped.extend_from_slice(b"\x1b[200~");
                        wrapped.extend_from_slice(text.as_bytes());
                        wrapped.extend_from_slice(b"\x1b[201~");
                        wrapped
                    } else {
                        text.into_bytes()
                    }
                }
            };
            if bytes.is_empty() {
                continue;
            }
            self.write_backend(&bytes);
        }
    }

    /// Write raw bytes to whichever transport is connected.
    fn write_backend(&mut self, bytes: &[u8]) {
        if let Some(ssh) = &mut self.ssh {
            ssh.write(bytes);
        } else if let Some(local) = &mut self.local {
            let _ = local.write(bytes);
        }
    }

    fn resize_backend(&mut self, size: TerminalSize) {
        if let Some(ssh) = &mut self.ssh {
            ssh.resize(size.cols, size.rows);
        }
        if let Some(local) = &mut self.local {
            let _ = local.resize(size);
        }
    }

    fn handle_titlebar_action(&mut self, ctx: &egui::Context, action: wassh_ui::TitlebarAction) {
        match action {
            wassh_ui::TitlebarAction::Minimize => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            wassh_ui::TitlebarAction::ToggleMaximize => {
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            // The spike has no plugin settings to open.
            wassh_ui::TitlebarAction::MenuGear(_) => {}
            wassh_ui::TitlebarAction::Close => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            wassh_ui::TitlebarAction::MenuItem(item) => match item {
                "Quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                "Zoom In" => {
                    self.view.font_size = (self.view.font_size + 1.0).min(48.0);
                }
                "Zoom Out" => {
                    self.view.font_size = (self.view.font_size - 1.0).max(8.0);
                }
                "Toggle Full Screen" => {
                    self.fullscreen = !self.fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                }
                // The remaining entries belong to the shell UI (Phase 1).
                _ => {}
            },
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_output(&ctx);

        if let Some(prompt) = self.host_key_prompt.clone() {
            let mut answer = None;
            let warning = to_color32(self.palette.warn);
            let text = to_color32(self.palette.text);
            egui::Panel::top("host-key-prompt").show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let message = if prompt.mismatch {
                        "WARNING: remote host key does not match the cached key!"
                    } else {
                        "The server host key is not cached."
                    };
                    ui.colored_label(warning, message);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} — {}:{}",
                            prompt.fingerprint, prompt.host, prompt.port
                        ))
                        .color(text),
                    );
                    if ui.button("Accept").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Reject").clicked() {
                        answer = Some(false);
                    }
                });
                ui.add_space(6.0);
            });
            if let Some(accept) = answer {
                if let Some(ssh) = &self.ssh {
                    ssh.respond_host_key(accept);
                }
                self.host_key_prompt = None;
            }
        }

        if self.auth_prompt.is_some() {
            let palette = self.palette;
            let mut submit = false;
            {
                let prompt = self.auth_prompt.as_mut().expect("checked above");
                egui::Panel::top("auth-prompt").show(ui, |ui| {
                    ui.add_space(6.0);
                    if !prompt.instructions.is_empty() {
                        ui.colored_label(to_color32(palette.warn), &prompt.instructions);
                    }
                    if !prompt.name.is_empty() {
                        ui.label(egui::RichText::new(&prompt.name).strong());
                    }
                    for (index, field) in prompt.fields.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(&field.label);
                            let edit = egui::TextEdit::singleline(&mut prompt.answers[index]);
                            let edit = if field.echo { edit } else { edit.password(true) };
                            ui.add(edit);
                        });
                    }
                    submit = ui.button("Continue").clicked();
                    ui.add_space(6.0);
                });
            }
            if submit
                && let Some(prompt) = self.auth_prompt.take()
                && let Some(ssh) = &self.ssh
            {
                ssh.respond_auth(prompt.answers);
            }
        }

        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let action = if cfg!(target_os = "macos") {
            wassh_ui::show_menu_bar(
                ui,
                &self.palette,
                TITLEBAR_MENUS,
                wassh_ui::titlebar::MenuState::default(),
            )
        } else {
            wassh_ui::resize_edges(&ctx, maximized);
            wassh_ui::show_header_bar(ui, &self.palette, APP_NAME, maximized, TITLEBAR_MENUS)
        };
        if let Some(action) = action {
            self.handle_titlebar_action(&ctx, action);
        }

        if let Some(gallery) = &mut self.gallery {
            gallery.show(ui, &self.palette);
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }

        let needs_login = self.target.is_some() && self.ssh.is_none();
        if needs_login {
            let target = self.target.clone().expect("checked above");
            let error = self.error.clone();
            let mut connect = false;
            egui::Panel::top("ssh-login").show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}@{}:{}",
                        target.user, target.host, target.port
                    ));
                    let mut submitted = false;
                    if self.auth_mode == AuthMode::Password {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.password)
                                .password(true)
                                .hint_text("Password"),
                        );
                        submitted = response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    }
                    connect = ui.button("Connect").clicked() || submitted;
                });
                if let Some(error) = &error {
                    ui.colored_label(to_color32(self.palette.danger), error);
                }
                ui.add_space(6.0);
            });
            if connect {
                self.start_ssh();
            }
        } else if self.status != SessionStatus::Connected {
            let error = self.error.clone();
            egui::Panel::top("ssh-status").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let text = match self.status {
                        SessionStatus::Closed => "Session closed",
                        SessionStatus::Failed => "Connection failed",
                        SessionStatus::Disconnected => "Disconnected",
                        SessionStatus::Connecting => "Connecting…",
                        SessionStatus::Authenticating => "Authenticating…",
                        SessionStatus::Reconnecting => "Reconnecting…",
                        _ => "Connected",
                    };
                    ui.label(text);
                    if let Some(error) = &error {
                        ui.colored_label(to_color32(self.palette.danger), error);
                    }
                });
                ui.add_space(4.0);
            });
        }

        self.forward_input(&ctx);

        let available = ui.available_size();
        let size = self.view.size_for(ui, available);
        if size != self.terminal.size() {
            self.terminal.resize(size);
            self.resize_backend(size);
        }

        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            self.terminal.scroll(-(scroll / 20.0).round() as i32);
        }

        let snapshot = self.terminal.snapshot();
        self.view.show(ui, &snapshot, &self.palette);

        // Placeholder until output wakes the UI directly.
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn apply_visuals(ctx: &egui::Context, palette: Palette) {
    wassh_ui::apply_visuals(ctx, &palette);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_host_and_port() {
        assert_eq!(
            parse_target("alice@example.com:2222"),
            SshTarget {
                host: "example.com".to_owned(),
                port: 2222,
                user: "alice".to_owned()
            }
        );
        assert_eq!(
            parse_target("example.com"),
            SshTarget {
                host: "example.com".to_owned(),
                port: 22,
                user: "root".to_owned()
            }
        );
    }

    #[test]
    fn keeps_non_numeric_suffix_as_part_of_the_host() {
        let target = parse_target("bob@host:notaport");
        assert_eq!(target.host, "host:notaport");
        assert_eq!(target.port, 22);
    }
}
