//! The Docker Manager plugin: containers, images, volumes, networks and the
//! engine, over the session's command channel.
//!
//! Port of the reference's `docker-manager` plugin. The reference talked to the
//! Engine API over the daemon socket; this pane asks the `docker` CLI the same
//! questions through [`CommandAccess`], which already runs commands on the host
//! the pane is attached to. [`model`] holds the commands and parsers, [`engine`]
//! the engine status and the privileged install/upgrade plans.
//!
//! Anything that needs root is tried with `sudo -n` first. When that asks for a
//! password the pane shows a password field and the user types it there; nothing
//! is stored, so there is no saved sudo password anywhere.

mod engine;
mod model;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use egui::Ui;
use serde_json::Value;
use wassh_core::Palette;
use wassh_ui::dock::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, bar_meter, checkbox, line_chart, password_input, search_input, select_str,
    small_button_enabled, text_input,
};

use crate::color::to_color32;
use crate::command::CommandAccess;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

pub use model::{
    Action, Container, Detail, Image, Mount, Network, Port, RunRequest, Stats, Volume,
};

/// Key the container list is requested under.
pub const LIST_KEY: &str = "docker.list";
/// Key container actions are requested under.
pub const ACTION_KEY: &str = "docker.action";
/// Key log output is requested under.
pub const LOGS_KEY: &str = "docker.logs";
/// Key stats samples are requested under.
pub const STATS_KEY: &str = "docker.stats";
/// Key the inspect document is requested under.
pub const INSPECT_KEY: &str = "docker.inspect";
/// Key the image list is requested under.
pub const IMAGES_KEY: &str = "docker.images";
/// Key the volume list is requested under.
pub const VOLUMES_KEY: &str = "docker.volumes";
/// Key the network list is requested under.
pub const NETWORKS_KEY: &str = "docker.networks";
/// Key the engine probe is requested under.
pub const ENGINE_KEY: &str = "docker.engine";
/// Key the package-manager probe is requested under.
pub const DETECT_KEY: &str = "docker.detect";
/// Key privileged (sudo) work is requested under.
pub const ROOT_KEY: &str = "docker.root";
/// Key image pulls are requested under.
pub const PULL_KEY: &str = "docker.pull";
/// Key one-off changes (remove image, create volume, …) are requested under.
pub const CHANGE_KEY: &str = "docker.change";
/// Key container updates (inspect + pull + recreate) are requested under.
pub const UPDATE_KEY: &str = "docker.update";
/// Key the inspect that feeds an update is requested under.
pub const UPDATE_INSPECT_KEY: &str = "docker.updateinspect";

/// Refresh interval used when the host does not say otherwise.
const DEFAULT_REFRESH_MS: u64 = 5_000;
/// Stats interval used when the host does not say otherwise.
const DEFAULT_STATS_MS: u64 = 2_000;
/// Slowest and fastest list refresh the settings may ask for.
const MIN_REFRESH_MS: u64 = 1_000;
const MAX_REFRESH_MS: u64 = 60_000;
/// Slowest and fastest stats sampling the settings may ask for.
const MIN_STATS_MS: u64 = 500;
const MAX_STATS_MS: u64 = 30_000;
/// Log lines fetched when the host does not say otherwise.
const DEFAULT_LOG_TAIL: usize = model::DEFAULT_LOG_TAIL;
/// Bounds on the log tail setting.
const MIN_LOG_TAIL: usize = 10;
const MAX_LOG_TAIL: usize = 5_000;
/// Samples kept for the CPU chart.
const STATS_HISTORY: usize = 60;
/// How long a status line stays up.
const NOTICE_LIFETIME: Duration = Duration::from_secs(6);
/// Height of the log, stats, detail and engine-output panels.
const PANEL_HEIGHT: f32 = 160.0;
/// Docker's wording for "the tag moved".
const PULLED_NEWER: &str = "Downloaded newer image";

/// Which list the pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Containers,
    Images,
    Volumes,
    Networks,
}

impl Tab {
    const ALL: [Self; 4] = [
        Self::Containers,
        Self::Images,
        Self::Volumes,
        Self::Networks,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Images => "Images",
            Self::Volumes => "Volumes",
            Self::Networks => "Networks",
        }
    }
}

/// Which part of the inspect document the detail panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Section {
    #[default]
    Overview,
    Env,
    Mounts,
    Ports,
    Networks,
    Raw,
}

impl Section {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::Env,
        Self::Mounts,
        Self::Ports,
        Self::Networks,
        Self::Raw,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Env => "Env",
            Self::Mounts => "Mounts",
            Self::Ports => "Ports",
            Self::Networks => "Networks",
            Self::Raw => "Raw",
        }
    }
}

/// Host settings the pane reads.
#[derive(Clone, Debug)]
struct HostSettings {
    socket: String,
    show_stopped: bool,
    refresh: Duration,
    stats: Duration,
    log_tail: usize,
    confirm_destructive: bool,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            socket: model::DEFAULT_SOCKET.to_owned(),
            show_stopped: false,
            refresh: Duration::from_millis(DEFAULT_REFRESH_MS),
            stats: Duration::from_millis(DEFAULT_STATS_MS),
            log_tail: DEFAULT_LOG_TAIL,
            confirm_destructive: true,
        }
    }
}

/// Read the pane's settings, with the reference's defaults and bounds.
fn read_settings(settings: &serde_json::Map<String, Value>) -> HostSettings {
    let text = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let number = |key: &str| settings.get(key).and_then(Value::as_u64);
    let flag = |key: &str, fallback: bool| {
        settings
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(fallback)
    };
    let socket = text("socketPath");
    let refresh = number("refreshIntervalMs")
        .unwrap_or(DEFAULT_REFRESH_MS)
        .clamp(MIN_REFRESH_MS, MAX_REFRESH_MS);
    let stats = number("statsIntervalMs")
        .unwrap_or(DEFAULT_STATS_MS)
        .clamp(MIN_STATS_MS, MAX_STATS_MS);
    let log_tail = number("logTailLines")
        .map(|lines| (lines as usize).clamp(MIN_LOG_TAIL, MAX_LOG_TAIL))
        .unwrap_or(DEFAULT_LOG_TAIL);
    HostSettings {
        socket: if socket.is_empty() {
            model::DEFAULT_SOCKET.to_owned()
        } else {
            socket
        },
        show_stopped: flag("showStopped", false),
        refresh: Duration::from_millis(refresh),
        stats: Duration::from_millis(stats),
        log_tail,
        confirm_destructive: flag("confirmDestructive", true),
    }
}

/// What one finished command produced.
struct Outcome {
    stdout: String,
    error: Option<String>,
}

/// A status line above the list.
struct Notice {
    text: String,
    failed: bool,
    at: Instant,
}

/// A destructive action waiting for a yes.
struct Confirm {
    action: Action,
    id: String,
    name: String,
}

/// A command that wants the user's sudo password.
struct Sudo {
    command: String,
    password: String,
}

/// A container update in flight.
struct Update {
    id: String,
    name: String,
    /// Whether the inspect that feeds the recreate script has been asked for.
    inspecting: bool,
}

/// What a row in the container list asks for.
enum RowEvent {
    Select(String),
    Act(Action, String, String),
    Logs(String),
    Stats(String),
    Shell(String),
    Update(String),
}

/// The Docker Manager pane.
#[derive(Default)]
pub struct DockerManager {
    settings: HostSettings,
    /// The toolbar's "show stopped" toggle, which starts from the host setting.
    show_stopped: Option<bool>,
    tab: Tab,
    filter: String,
    selected: Option<String>,
    section: Section,
    shell: usize,
    containers: Vec<Container>,
    images: Vec<Image>,
    volumes: Vec<Volume>,
    networks: Vec<Network>,
    detail: Option<Detail>,
    logs_for: Option<String>,
    logs: String,
    stats_for: Option<String>,
    stats: Option<Stats>,
    cpu_history: Vec<f32>,
    notice: Option<Notice>,
    confirm: Option<Confirm>,
    sudo: Option<Sudo>,
    engine: engine::Status,
    engine_open: bool,
    manager: Option<engine::Manager>,
    pull: String,
    new_volume: String,
    new_network: String,
    run: RunForm,
    run_open: bool,
    updating: Vec<Update>,
    stale: Vec<String>,
    pull_queue: Vec<String>,
    /// How often each key has been asked, so a cached answer is read once.
    attempts: BTreeMap<&'static str, u32>,
    handled: BTreeMap<&'static str, u32>,
    /// The command behind each key, kept for a sudo retry.
    commands: BTreeMap<&'static str, String>,
    last_poll: Option<Instant>,
    last_stats: Option<Instant>,
    pending: Option<(Action, String, String)>,
}

/// The create-container form.
#[derive(Default)]
struct RunForm {
    image: String,
    tag: String,
    name: String,
    command: String,
    ports: String,
    env: String,
    volumes: String,
    restart: usize,
    network: String,
}

/// The `--restart` choices the form offers.
const RESTART_CHOICES: [&str; 4] = ["no", "always", "unless-stopped", "on-failure"];

impl RunForm {
    fn request(&self) -> RunRequest {
        let index = self.restart.min(RESTART_CHOICES.len() - 1);
        RunRequest {
            image: self.image.trim().to_owned(),
            tag: self.tag.trim().to_owned(),
            name: self.name.trim().to_owned(),
            command: self.command.trim().to_owned(),
            ports: lines(&self.ports),
            env: lines(&self.env),
            volumes: lines(&self.volumes),
            restart_policy: RESTART_CHOICES[index].to_owned(),
            network: self.network.trim().to_owned(),
            detach: true,
            auto_remove: false,
        }
    }
}

/// One entry per non-empty line.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Plugin for DockerManager {
    fn manifest(&self) -> PluginManifest {
        PluginId::DockerManager.manifest()
    }

    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        if self.engine.state.is_none() || self.engine.state == Some(engine::State::Checking) {
            return Some(("checking".to_owned(), HeaderHealth::Warn));
        }
        if !self.engine.is_ready() {
            return Some(("no daemon".to_owned(), HeaderHealth::Bad));
        }
        let running = self
            .containers
            .iter()
            .filter(|item| item.is_running())
            .count();
        Some((format!("{running} up"), HeaderHealth::Ok))
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        self.settings = read_settings(context.settings);
        let muted = to_color32(context.palette.text_muted);
        let host_label = context.host_label.clone();
        let Some(access) = context.session.as_deref_mut() else {
            ui.label(
                egui::RichText::new("Containers need an SSH or local session.")
                    .size(12.0)
                    .color(muted),
            );
            return;
        };
        let typed = self.draw(ui, &context.palette, &host_label, access);
        if let Some(text) = typed {
            context.send_text(&text);
        }
    }
}

impl DockerManager {
    /// Draw the pane and return anything to type into the session's terminal.
    fn draw(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        host_label: &str,
        access: &mut dyn CommandAccess,
    ) -> Option<String> {
        self.poll(access);
        self.engine_banner(ui, palette, access);
        self.tab_bar(ui, palette);
        let mut typed = None;
        match self.tab {
            Tab::Containers => typed = self.draw_containers(ui, palette, access),
            Tab::Images => self.draw_images(ui, palette, access),
            Tab::Volumes => self.draw_volumes(ui, palette, access),
            Tab::Networks => self.draw_networks(ui, palette, access),
        }
        self.notice_row(ui, palette);
        self.confirm_card(ui, palette, access);
        self.sudo_card(ui, palette, access);
        if self.engine_open {
            self.engine_panel(ui, palette, access);
        }
        ui.add_space(2.0);
        let running = self
            .containers
            .iter()
            .filter(|item| item.is_running())
            .count();
        ui.label(
            egui::RichText::new(format!(
                "{host_label} · {running} up of {} container{}",
                self.containers.len(),
                if self.containers.len() == 1 { "" } else { "s" }
            ))
            .size(10.0)
            .color(to_color32(palette.text_muted)),
        );
        typed
    }

    // --- commands -------------------------------------------------------

    /// Run `command` under `key` and remember it for a possible sudo retry.
    fn run(&mut self, access: &mut dyn CommandAccess, key: &'static str, command: &str) {
        *self.attempts.entry(key).or_insert(0) += 1;
        self.commands.insert(key, command.to_owned());
        access.exec(key, command);
    }

    /// Run a privileged command, with the password on stdin when one is given.
    fn run_root(&mut self, access: &mut dyn CommandAccess, command: &str, password: Option<&str>) {
        *self.attempts.entry(ROOT_KEY).or_insert(0) += 1;
        self.commands.insert(ROOT_KEY, command.to_owned());
        let full = model::privileged(command, password);
        match password {
            Some(password) => access.exec_input(ROOT_KEY, &full, &format!("{password}\n")),
            None => access.exec(ROOT_KEY, &full),
        }
    }

    /// The outcome of `key`, exactly once per request.
    fn take(&mut self, access: &dyn CommandAccess, key: &'static str) -> Option<Outcome> {
        if access.is_running(key) {
            return None;
        }
        let attempt = self.attempts.get(key).copied().unwrap_or(0);
        if attempt == 0 || self.handled.get(key).copied().unwrap_or(0) == attempt {
            return None;
        }
        let output = access.output(key).cloned();
        let failure = access.failure(key).map(str::to_owned);
        // Nothing has come back yet: the answer is not an empty one.
        if output.is_none() && failure.is_none() {
            return None;
        }
        self.handled.insert(key, attempt);
        let Some(output) = output else {
            return Some(Outcome {
                stdout: String::new(),
                error: failure,
            });
        };
        let error = failure.or_else(|| {
            if output.succeeded() {
                None
            } else {
                Some(
                    output
                        .error_line()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("docker exited with {:?}", output.code)),
                )
            }
        });
        Some(Outcome {
            stdout: output.stdout,
            error,
        })
    }

    /// Whether stopped containers are listed.
    fn show_stopped(&self) -> bool {
        self.show_stopped.unwrap_or(self.settings.show_stopped)
    }

    /// Fold every finished command into the pane's state.
    fn poll(&mut self, access: &mut dyn CommandAccess) {
        self.poll_lists(access);
        self.poll_action(access);
        self.poll_logs(access);
        self.poll_stats(access);
        self.poll_inspect(access);
        self.poll_engine(access);
        self.poll_change(access);
        self.poll_root(access);
        self.poll_pull(access);
        self.poll_update(access);
        self.request_lists(access);
    }

    /// Ask for the list the current tab shows, when it is due.
    fn request_lists(&mut self, access: &mut dyn CommandAccess) {
        let due = self
            .last_poll
            .is_none_or(|polled| polled.elapsed() >= self.settings.refresh);
        if !due {
            return;
        }
        let busy = access.is_running(LIST_KEY)
            || access.is_running(IMAGES_KEY)
            || access.is_running(VOLUMES_KEY)
            || access.is_running(NETWORKS_KEY);
        if busy {
            return;
        }
        self.last_poll = Some(Instant::now());
        let socket = self.settings.socket.clone();
        let (key, command) = match self.tab {
            Tab::Containers => (LIST_KEY, model::ps_command(&socket, self.show_stopped())),
            Tab::Images => (IMAGES_KEY, model::images_command(&socket)),
            Tab::Volumes => (VOLUMES_KEY, model::volumes_command(&socket)),
            Tab::Networks => (NETWORKS_KEY, model::networks_command(&socket)),
        };
        self.run(access, key, &command);
    }

    fn poll_lists(&mut self, access: &mut dyn CommandAccess) {
        if let Some(outcome) = self.take(access, LIST_KEY) {
            match outcome.error {
                Some(error) => self.failed(LIST_KEY, error),
                None => self.containers = model::parse_containers(&outcome.stdout),
            }
            if let Some(selected) = &self.selected
                && !self.containers.iter().any(|item| &item.id == selected)
            {
                self.selected = None;
                self.detail = None;
                self.logs_for = None;
                self.stats_for = None;
            }
        }
        if let Some(outcome) = self.take(access, IMAGES_KEY) {
            match outcome.error {
                Some(error) => self.failed(IMAGES_KEY, error),
                None => self.images = model::parse_images(&outcome.stdout, &self.containers),
            }
        }
        if let Some(outcome) = self.take(access, VOLUMES_KEY) {
            match outcome.error {
                Some(error) => self.failed(VOLUMES_KEY, error),
                None => self.volumes = model::parse_volumes(&outcome.stdout),
            }
        }
        if let Some(outcome) = self.take(access, NETWORKS_KEY) {
            match outcome.error {
                Some(error) => self.failed(NETWORKS_KEY, error),
                None => self.networks = model::parse_networks(&outcome.stdout),
            }
        }
    }

    fn poll_action(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, ACTION_KEY) else {
            return;
        };
        let Some((action, name, id)) = self.pending.take() else {
            return;
        };
        match outcome.error {
            Some(error) => self.failed(ACTION_KEY, error),
            None => {
                self.notify(format!("{} {name}", action.done()));
                if action == Action::Remove && self.selected.as_deref() == Some(id.as_str()) {
                    self.selected = None;
                    self.detail = None;
                }
            }
        }
        // The container's state changed, so ask again straight away.
        self.last_poll = None;
    }

    fn poll_logs(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, LOGS_KEY) else {
            return;
        };
        match outcome.error {
            Some(error) => self.failed(LOGS_KEY, error),
            None => self.logs = outcome.stdout,
        }
    }

    fn poll_stats(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, STATS_KEY) else {
            return;
        };
        if let Some(error) = outcome.error {
            self.failed(STATS_KEY, error);
            return;
        }
        let Some(stats) = model::parse_stats(&outcome.stdout) else {
            return;
        };
        self.stats = Some(stats);
        self.cpu_history.push(stats.cpu_percent as f32);
        if self.cpu_history.len() > STATS_HISTORY {
            self.cpu_history.remove(0);
        }
    }

    fn poll_inspect(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, INSPECT_KEY) else {
            return;
        };
        match outcome.error {
            Some(error) => self.failed(INSPECT_KEY, error),
            None => self.detail = model::parse_detail(&outcome.stdout),
        }
    }

    fn poll_engine(&mut self, access: &mut dyn CommandAccess) {
        if self.engine.state.is_none() && !access.is_running(ENGINE_KEY) {
            let command = model::engine_command(&self.settings.socket);
            self.run(access, ENGINE_KEY, &command);
        }
        if let Some(outcome) = self.take(access, ENGINE_KEY) {
            self.engine = match outcome.error {
                Some(error) => engine::status_from_failure(&error),
                None => engine::parse_info(&outcome.stdout).unwrap_or(engine::Status {
                    state: Some(engine::State::Error),
                    message: "docker info printed something unexpected".to_owned(),
                    ..engine::Status::default()
                }),
            };
        }
        if let Some(outcome) = self.take(access, DETECT_KEY) {
            match outcome.error {
                Some(error) => {
                    self.manager = None;
                    self.failed(DETECT_KEY, error);
                }
                None => self.manager = Some(engine::manager_from_probe(&outcome.stdout)),
            }
        }
    }

    /// Changes that are not tied to one panel: removing images, creating
    /// volumes and networks, pruning.
    fn poll_change(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, CHANGE_KEY) else {
            return;
        };
        match outcome.error {
            Some(error) => self.failed(CHANGE_KEY, error),
            None => {
                let summary = outcome
                    .stdout
                    .lines()
                    .map(str::trim)
                    .rfind(|line| !line.is_empty())
                    .unwrap_or("Done")
                    .to_owned();
                self.notify(summary);
                self.last_poll = None;
            }
        }
    }

    /// The result of privileged work (install, upgrade, or a retried command).
    fn poll_root(&mut self, access: &mut dyn CommandAccess) {
        let Some(outcome) = self.take(access, ROOT_KEY) else {
            return;
        };
        match outcome.error {
            Some(error) => self.failed(ROOT_KEY, error),
            None => {
                self.notify("Done".to_owned());
                // The daemon may have changed underneath us.
                self.engine.state = None;
                self.manager = None;
                self.last_poll = None;
            }
        }
    }

    fn poll_pull(&mut self, access: &mut dyn CommandAccess) {
        if let Some(outcome) = self.take(access, PULL_KEY) {
            let image = self.pull_queue.first().cloned();
            match outcome.error {
                Some(error) => {
                    self.failed(PULL_KEY, error);
                    self.pull_queue.clear();
                }
                None => {
                    if let Some(image) = image {
                        if outcome.stdout.contains(PULLED_NEWER) {
                            self.mark_stale(&image);
                        }
                        self.notify(format!("Pulled {image}"));
                        self.pull_queue.remove(0);
                    }
                    self.last_poll = None;
                }
            }
        }
        // One image at a time, so a slow registry cannot pile requests up.
        if self.pull_queue.is_empty() || access.is_running(PULL_KEY) {
            return;
        }
        let image = self.pull_queue[0].clone();
        let command = model::pull_command(&self.settings.socket, &image);
        self.run(access, PULL_KEY, &command);
    }

    fn poll_update(&mut self, access: &mut dyn CommandAccess) {
        // The recreate script, once it has run.
        if let Some(outcome) = self.take(access, UPDATE_KEY)
            && let Some(update) = self.updating.first()
        {
            let update = Update {
                id: update.id.clone(),
                name: update.name.clone(),
                inspecting: false,
            };
            match outcome.error {
                Some(error) => {
                    self.failed(UPDATE_KEY, error);
                    self.updating.remove(0);
                }
                None => {
                    self.notify(format!("Updated {}", update.name));
                    self.stale.retain(|id| *id != update.id);
                    self.updating.remove(0);
                    self.last_poll = None;
                }
            }
        }
        // The inspect that feeds it: build the script and start it.
        if let Some(outcome) = self.take(access, UPDATE_INSPECT_KEY)
            && let Some(update) = self.updating.first().map(|update| update.id.clone())
        {
            match outcome.error {
                Some(error) => {
                    self.failed(UPDATE_INSPECT_KEY, error);
                    self.updating.remove(0);
                }
                None => {
                    let document = serde_json::from_str::<Value>(&outcome.stdout)
                        .ok()
                        .and_then(|value| {
                            value.as_array().and_then(|items| items.first().cloned())
                        });
                    match document.and_then(|document| {
                        model::recreate_script(&self.settings.socket, &document)
                    }) {
                        Some(script) => {
                            if let Some(entry) = self.updating.first_mut() {
                                entry.inspecting = false;
                            }
                            self.run(access, UPDATE_KEY, &script);
                        }
                        None => {
                            self.failed(
                                UPDATE_INSPECT_KEY,
                                format!(
                                    "Container {update} cannot be recreated from its inspect data."
                                ),
                            );
                            self.updating.remove(0);
                        }
                    }
                }
            }
        }
        // Start the next container's inspect.
        let idle = !access.is_running(UPDATE_INSPECT_KEY) && !access.is_running(UPDATE_KEY);
        if !idle {
            return;
        }
        let Some(next) = self.updating.iter_mut().find(|update| !update.inspecting) else {
            return;
        };
        let id = next.id.clone();
        next.inspecting = true;
        match model::inspect_command(&self.settings.socket, &id) {
            Some(command) => self.run(access, UPDATE_INSPECT_KEY, &command),
            None => self.updating.retain(|update| update.id != id),
        }
    }

    /// Note which containers run `image`, so they show as outdated.
    fn mark_stale(&mut self, image: &str) {
        for container in &self.containers {
            if container.image == image && !self.stale.contains(&container.id) {
                self.stale.push(container.id.clone());
            }
        }
    }

    /// Show `error`, and offer the password when it says root was needed.
    fn failed(&mut self, key: &'static str, error: String) {
        if model::needs_root(&error)
            && self.sudo.is_none()
            && let Some(command) = self.commands.get(key).cloned()
        {
            self.sudo = Some(Sudo {
                command,
                password: String::new(),
            });
        }
        self.notify_failure(error);
    }

    fn notify(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            failed: false,
            at: Instant::now(),
        });
    }

    fn notify_failure(&mut self, text: String) {
        self.notice = Some(Notice {
            text,
            failed: true,
            at: Instant::now(),
        });
    }

    // --- containers -----------------------------------------------------

    fn draw_containers(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        access: &mut dyn CommandAccess,
    ) -> Option<String> {
        ui.horizontal_wrapped(|ui| {
            search_input(ui, palette, &mut self.filter);
            let mut stopped = self.show_stopped();
            checkbox(ui, palette, &mut stopped, "Show stopped");
            if stopped != self.show_stopped() {
                self.show_stopped = Some(stopped);
                self.last_poll = None;
            }
            if small_button_enabled(ui, palette, "Refresh", !access.is_running(LIST_KEY)) {
                self.last_poll = None;
            }
            if small_button_enabled(ui, palette, "New…", true) {
                self.run_open = !self.run_open;
            }
            let can_check = self.pull_queue.is_empty()
                && self
                    .containers
                    .iter()
                    .any(|container| container.is_running());
            if small_button_enabled(ui, palette, "Check updates", can_check) {
                let mut images: Vec<String> = self
                    .containers
                    .iter()
                    .filter(|container| container.is_running())
                    .map(|container| container.image.clone())
                    .collect();
                images.dedup();
                self.pull_queue = images;
                self.notify("Checking the registry for newer images…".to_owned());
            }
            let stale = self.stale.clone();
            let label = format!("Update all ({})", stale.len());
            if small_button_enabled(
                ui,
                palette,
                &label,
                !stale.is_empty() && self.updating.is_empty(),
            ) {
                self.queue_updates(&stale);
            }
        });

        if self.run_open {
            self.run_form(ui, palette, access);
        }

        let needle = self.filter.trim().to_lowercase();
        let mut rows: Vec<Container> = self
            .containers
            .iter()
            .filter(|container| {
                needle.is_empty()
                    || container.name.to_lowercase().contains(&needle)
                    || container.image.to_lowercase().contains(&needle)
            })
            .cloned()
            .collect();
        // Running containers first, then by name, as the reference listed them.
        rows.sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| a.name.cmp(&b.name)));

        if self.containers.is_empty() && access.is_running(LIST_KEY) {
            ui.label(
                egui::RichText::new("Listing containers…")
                    .size(11.0)
                    .color(to_color32(palette.text_muted)),
            );
        } else if self.containers.is_empty() {
            ui.label(
                egui::RichText::new(if self.engine.is_ready() {
                    "No containers on this host."
                } else {
                    "No containers. Is Docker installed and running on this host?"
                })
                .size(11.0)
                .color(to_color32(palette.text_muted)),
            );
        }

        let panelled = self.logs_for.is_some() || self.stats_for.is_some();
        let list_height = if panelled {
            (ui.available_height() * 0.5).max(80.0)
        } else {
            ui.available_height()
        };
        let selected = self.selected.clone();
        let stale = self.stale.clone();
        let mut events = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("docker-containers")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for container in &rows {
                    if let Some(event) = container_row(ui, palette, container, &selected, &stale) {
                        events.push(event);
                    }
                }
            });

        let mut typed = None;
        for event in events {
            match event {
                RowEvent::Select(id) => self.select(access, &id),
                RowEvent::Act(action, id, name) => {
                    if self.pending.is_some() {
                        continue;
                    }
                    if action.is_destructive() && self.settings.confirm_destructive {
                        self.confirm = Some(Confirm { action, id, name });
                    } else {
                        self.start_action(access, action, &id, &name);
                    }
                }
                RowEvent::Logs(id) => {
                    self.logs_for = Some(id);
                    self.logs.clear();
                }
                RowEvent::Stats(id) => {
                    self.stats_for = Some(id);
                    self.stats = None;
                    self.cpu_history.clear();
                    self.last_stats = None;
                }
                RowEvent::Shell(id) => {
                    typed = Some(format!(
                        "docker exec -it {} {}\n",
                        model::shell_quote(&id),
                        model::SHELL_CHOICES[self.shell.min(model::SHELL_CHOICES.len() - 1)]
                    ));
                }
                RowEvent::Update(id) => self.queue_updates(&[id]),
            }
        }

        if self.detail.is_some() && typed.is_none() {
            typed = self.draw_detail(ui, palette);
        }
        if let Some(id) = self.logs_for.clone() {
            self.draw_logs(ui, palette, access, &id);
        }
        if self.stats_for.is_some() {
            self.draw_stats(ui, palette, access);
        }
        typed
    }

    fn queue_updates(&mut self, ids: &[String]) {
        self.updating = ids
            .iter()
            .filter_map(|id| {
                self.containers
                    .iter()
                    .find(|container| &container.id == id)
                    .map(|container| Update {
                        id: container.id.clone(),
                        name: container.name.clone(),
                        inspecting: false,
                    })
            })
            .collect();
    }

    fn select(&mut self, access: &mut dyn CommandAccess, id: &str) {
        self.selected = Some(id.to_owned());
        self.detail = None;
        self.logs_for = None;
        self.stats_for = None;
        self.section = Section::default();
        if let Some(command) = model::inspect_command(&self.settings.socket, id) {
            self.run(access, INSPECT_KEY, &command);
        }
    }

    fn start_action(
        &mut self,
        access: &mut dyn CommandAccess,
        action: Action,
        id: &str,
        name: &str,
    ) {
        match model::action_command(&self.settings.socket, id, action) {
            Some(command) => {
                self.pending = Some((action, name.to_owned(), id.to_owned()));
                self.run(access, ACTION_KEY, &command);
            }
            None => {
                self.notify_failure("That container id is not safe to pass to a shell.".to_owned());
            }
        }
    }

    fn request_logs(&mut self, access: &mut dyn CommandAccess) {
        let Some(id) = self.logs_for.clone() else {
            return;
        };
        if access.is_running(LOGS_KEY) || !self.logs.is_empty() {
            return;
        }
        if let Some(command) =
            model::logs_command(&self.settings.socket, &id, self.settings.log_tail)
        {
            self.run(access, LOGS_KEY, &command);
        }
    }

    fn request_stats(&mut self, access: &mut dyn CommandAccess) {
        let Some(id) = self.stats_for.clone() else {
            return;
        };
        if access.is_running(STATS_KEY) {
            return;
        }
        let due = self
            .last_stats
            .is_none_or(|sampled| sampled.elapsed() >= self.settings.stats);
        if !due {
            return;
        }
        self.last_stats = Some(Instant::now());
        if let Some(command) = model::stats_command(&self.settings.socket, &id) {
            self.run(access, STATS_KEY, &command);
        }
    }

    fn draw_logs(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        access: &mut dyn CommandAccess,
        id: &str,
    ) {
        self.request_logs(access);
        ui.horizontal(|ui| {
            if small_button_enabled(ui, palette, "Close logs", true) {
                self.logs_for = None;
            }
            ui.label(
                egui::RichText::new(format!("logs · {}", model::short_id(id)))
                    .size(10.0)
                    .color(to_color32(palette.text_muted)),
            );
        });
        egui::ScrollArea::vertical()
            .id_salt("docker-logs")
            .max_height(PANEL_HEIGHT)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if self.logs.trim().is_empty() {
                    let waiting = if access.is_running(LOGS_KEY) {
                        "Fetching logs…"
                    } else {
                        "No output."
                    };
                    ui.label(
                        egui::RichText::new(waiting)
                            .size(11.0)
                            .color(to_color32(palette.text_muted)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(&self.logs)
                            .font(egui::FontId::monospace(10.0))
                            .color(to_color32(palette.text)),
                    );
                }
            });
    }

    fn draw_stats(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        self.request_stats(access);
        ui.horizontal(|ui| {
            if small_button_enabled(ui, palette, "Close stats", true) {
                self.stats_for = None;
            }
            ui.label(
                egui::RichText::new("live stats")
                    .size(10.0)
                    .color(to_color32(palette.text_muted)),
            );
        });
        let Some(stats) = self.stats else {
            ui.label(
                egui::RichText::new("Sampling…")
                    .size(11.0)
                    .color(to_color32(palette.text_muted)),
            );
            return;
        };
        line_chart(
            ui,
            palette,
            &self.cpu_history,
            100.0,
            to_color32(palette.accent),
        );
        ui.label(
            egui::RichText::new(format!(
                "CPU {:.2}% · mem {} of {} ({:.1}%) · {} pids",
                stats.cpu_percent,
                model::format_bytes(stats.memory_used),
                model::format_bytes(stats.memory_limit),
                stats.memory_percent,
                stats.pids
            ))
            .size(10.5)
            .color(to_color32(palette.text)),
        );
        bar_meter(
            ui,
            palette,
            Some((stats.memory_percent / 100.0) as f32),
            to_color32(palette.accent),
        );
        ui.label(
            egui::RichText::new(format!(
                "net ↓{} ↑{} · block read {} write {}",
                model::format_bytes(stats.net_rx),
                model::format_bytes(stats.net_tx),
                model::format_bytes(stats.block_read),
                model::format_bytes(stats.block_write)
            ))
            .size(10.0)
            .color(to_color32(palette.text_muted)),
        );
    }

    /// The inspect panel under the list.
    fn draw_detail(&mut self, ui: &mut Ui, palette: &Palette) -> Option<String> {
        let detail = self.detail.clone()?;
        let mut typed = None;
        ui.horizontal_wrapped(|ui| {
            if small_button_enabled(ui, palette, "Close details", true) {
                self.detail = None;
                self.selected = None;
            }
            ui.label(
                egui::RichText::new(format!("{} · {}", detail.name, detail.status))
                    .size(11.5)
                    .strong()
                    .color(to_color32(palette.text)),
            );
            for section in Section::ALL {
                let active = section == self.section;
                if Button::new(section.label())
                    .compact()
                    .active(active)
                    .show(ui, palette)
                    .clicked()
                    && !active
                {
                    self.section = section;
                }
            }
            if detail.status == "running" {
                let mut shell = self.shell.min(model::SHELL_CHOICES.len() - 1);
                if let Some(picked) = select_str(
                    ui,
                    ("docker-shell", "pick"),
                    model::SHELL_CHOICES[shell],
                    &model::SHELL_CHOICES,
                ) && let Some(index) = model::SHELL_CHOICES
                    .iter()
                    .position(|choice| *choice == picked)
                {
                    shell = index;
                }
                self.shell = shell;
                if small_button_enabled(ui, palette, "Open shell", true) {
                    typed = Some(format!(
                        "docker exec -it {} {}\n",
                        model::shell_quote(&detail.id),
                        model::SHELL_CHOICES[shell]
                    ));
                }
            }
        });
        let text = match self.section {
            Section::Overview => format!(
                "id         {}\nimage      {}\ncommand    {}\ncreated    {}\nstarted    {}\nfinished   {}\n\
                 exit code  {}\npid        {}\nrestarts   {}\nplatform   {}\nuser       {}\nworkdir    {}\n\
                 tty        {}\nrestart    {}\nnetwork    {}\nprivileged {}\nauto-remove {}",
                detail.id,
                detail.image,
                empty_as(&detail.command, "-"),
                detail.created,
                detail.started_at,
                detail.finished_at,
                detail
                    .exit_code
                    .map_or_else(|| "-".to_owned(), |code| code.to_string()),
                detail
                    .pid
                    .map_or_else(|| "-".to_owned(), |pid| pid.to_string()),
                detail.restart_count,
                empty_as(&detail.platform, "-"),
                empty_as(&detail.user, "-"),
                empty_as(&detail.workdir, "/"),
                detail.tty,
                empty_as(&detail.restart_policy, "no"),
                empty_as(&detail.network_mode, "-"),
                detail.privileged,
                detail.auto_remove
            ),
            Section::Env => detail.env.join("\n"),
            Section::Mounts => detail.mounts.join("\n"),
            Section::Ports => detail.ports.join("\n"),
            Section::Networks => detail.networks.join("\n"),
            Section::Raw => detail.raw.clone(),
        };
        egui::ScrollArea::vertical()
            .id_salt("docker-detail")
            .max_height(PANEL_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(if text.trim().is_empty() {
                        "Nothing to show."
                    } else {
                        text.as_str()
                    })
                    .font(egui::FontId::monospace(10.0))
                    .color(to_color32(palette.text)),
                );
            });
        ui.separator();
        typed
    }

    fn draw_images(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.horizontal_wrapped(|ui| {
            text_input(ui, palette, &mut self.pull, "nginx:latest");
            let ready = !self.pull.trim().is_empty();
            if small_button_enabled(ui, palette, "Pull", ready) {
                self.pull_queue = vec![self.pull.trim().to_owned()];
                self.notify("Pulling…".to_owned());
            }
            if small_button_enabled(ui, palette, "Refresh", !access.is_running(IMAGES_KEY)) {
                self.last_poll = None;
            }
        });
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("docker-images")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for image in &self.images {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(image.references.join(", "))
                                .size(11.5)
                                .color(to_color32(palette.text)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} · {} container{}",
                                model::format_bytes(image.size),
                                image.created,
                                image.containers,
                                if image.containers == 1 { "" } else { "s" }
                            ))
                            .size(10.0)
                            .color(to_color32(palette.text_muted)),
                        );
                        if small_button_enabled(ui, palette, "Remove", true) {
                            remove = Some(image.id.clone());
                        }
                    });
                    ui.separator();
                }
            });
        if let Some(id) = remove
            && let Some(command) = model::remove_image_command(&self.settings.socket, &id, true)
        {
            self.run(access, CHANGE_KEY, &command);
        }
    }

    fn draw_volumes(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.horizontal_wrapped(|ui| {
            text_input(ui, palette, &mut self.new_volume, "volume name");
            let name = self.new_volume.trim().to_owned();
            if small_button_enabled(ui, palette, "Create", !name.is_empty()) {
                if let Some(command) = model::create_volume_command(&self.settings.socket, &name) {
                    self.run(access, CHANGE_KEY, &command);
                }
                self.new_volume.clear();
            }
            if small_button_enabled(ui, palette, "Prune", true) {
                let command = model::prune_volumes_command(&self.settings.socket);
                self.run(access, CHANGE_KEY, &command);
            }
            if small_button_enabled(ui, palette, "Refresh", !access.is_running(VOLUMES_KEY)) {
                self.last_poll = None;
            }
        });
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("docker-volumes")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for volume in &self.volumes {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&volume.name)
                                .size(11.5)
                                .color(to_color32(palette.text)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                volume.driver,
                                empty_as(&volume.mountpoint, "-")
                            ))
                            .size(10.0)
                            .color(to_color32(palette.text_muted)),
                        );
                        if small_button_enabled(ui, palette, "Remove", true) {
                            remove = Some(volume.name.clone());
                        }
                    });
                    ui.separator();
                }
            });
        if let Some(name) = remove
            && let Some(command) = model::remove_volume_command(&self.settings.socket, &name, false)
        {
            self.run(access, CHANGE_KEY, &command);
        }
    }

    fn draw_networks(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.horizontal_wrapped(|ui| {
            text_input(ui, palette, &mut self.new_network, "network name");
            let name = self.new_network.trim().to_owned();
            if small_button_enabled(ui, palette, "Create", !name.is_empty()) {
                if let Some(command) =
                    model::create_network_command(&self.settings.socket, &name, "bridge")
                {
                    self.run(access, CHANGE_KEY, &command);
                }
                self.new_network.clear();
            }
            if small_button_enabled(ui, palette, "Refresh", !access.is_running(NETWORKS_KEY)) {
                self.last_poll = None;
            }
        });
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("docker-networks")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for network in &self.networks {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&network.name)
                                .size(11.5)
                                .color(to_color32(palette.text)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} · {}{}",
                                network.short_id(),
                                network.driver,
                                network.scope,
                                if network.internal { " · internal" } else { "" }
                            ))
                            .size(10.0)
                            .color(to_color32(palette.text_muted)),
                        );
                        if small_button_enabled(ui, palette, "Remove", true) {
                            remove = Some(network.id.clone());
                        }
                    });
                    ui.separator();
                }
            });
        if let Some(id) = remove
            && let Some(command) = model::remove_network_command(&self.settings.socket, &id)
        {
            self.run(access, CHANGE_KEY, &command);
        }
    }

    // --- chrome ---------------------------------------------------------

    fn engine_banner(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.horizontal_wrapped(|ui| {
            let colour = if self.engine.is_ready() {
                palette.ok
            } else if self.engine.state.is_none() {
                palette.text_muted
            } else {
                palette.danger
            };
            ui.label(
                egui::RichText::new(self.engine.summary())
                    .size(11.0)
                    .color(to_color32(colour)),
            );
            if small_button_enabled(ui, palette, "Engine…", true) {
                self.engine_open = !self.engine_open;
            }
            if small_button_enabled(ui, palette, "Recheck", !access.is_running(ENGINE_KEY)) {
                self.engine.state = None;
            }
        });
    }

    fn tab_bar(&mut self, ui: &mut Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                let active = tab == self.tab;
                if Button::new(tab.label())
                    .compact()
                    .active(active)
                    .show(ui, palette)
                    .clicked()
                    && !active
                {
                    self.tab = tab;
                    self.last_poll = None;
                }
            }
        });
    }

    fn notice_row(&mut self, ui: &mut Ui, palette: &Palette) {
        let Some(notice) = &self.notice else {
            return;
        };
        if notice.at.elapsed() > NOTICE_LIFETIME {
            self.notice = None;
            return;
        }
        ui.label(
            egui::RichText::new(&notice.text)
                .size(10.5)
                .color(to_color32(if notice.failed {
                    palette.danger
                } else {
                    palette.accent
                })),
        );
    }

    fn confirm_card(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "{} {}?",
                if confirm.action == Action::Remove {
                    "Remove"
                } else {
                    "Kill"
                },
                confirm.name
            ))
            .size(12.0)
            .strong()
            .color(to_color32(palette.text)),
        );
        ui.label(
            egui::RichText::new("This cannot be undone.")
                .size(10.5)
                .color(to_color32(palette.text_muted)),
        );
        let (action, id, name) = (confirm.action, confirm.id.clone(), confirm.name.clone());
        let mut decision = None;
        ui.horizontal(|ui| {
            if Button::new("Yes")
                .compact()
                .kind(ButtonKind::Danger)
                .show(ui, palette)
                .clicked()
            {
                decision = Some(true);
            }
            if Button::new("Cancel").compact().show(ui, palette).clicked() {
                decision = Some(false);
            }
        });
        match decision {
            Some(true) => {
                self.confirm = None;
                self.start_action(access, action, &id, &name);
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// The password card, shown when a command needed root.
    fn sudo_card(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        let Some(mut sudo) = self.sudo.take() else {
            return;
        };
        ui.separator();
        ui.label(
            egui::RichText::new("Root privileges needed")
                .size(12.0)
                .strong()
                .color(to_color32(palette.text)),
        );
        ui.label(
            egui::RichText::new("The password is used once and never stored.")
                .size(10.5)
                .color(to_color32(palette.text_muted)),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(&sudo.command)
                    .size(10.5)
                    .monospace()
                    .color(to_color32(palette.accent)),
            )
            .wrap(),
        );
        let response = password_input(ui, palette, &mut sudo.password, "sudo password");
        let submitted =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let mut decision = None;
        ui.horizontal(|ui| {
            if Button::new("Run with sudo")
                .compact()
                .primary()
                .show(ui, palette)
                .clicked()
                || submitted
            {
                decision = Some(true);
            }
            if Button::new("Cancel").compact().show(ui, palette).clicked() {
                decision = Some(false);
            }
        });
        match decision {
            Some(true) => {
                let password = (!sudo.password.is_empty()).then_some(sudo.password.as_str());
                self.run_root(access, &sudo.command, password);
                self.notify("Running with sudo…".to_owned());
            }
            Some(false) => {}
            None => self.sudo = Some(sudo),
        }
    }

    fn engine_panel(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.separator();
        ui.label(
            egui::RichText::new("Docker engine")
                .size(12.0)
                .strong()
                .color(to_color32(palette.text)),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} · {} · {} · {} running, {} paused, {} stopped · {} image{}",
                empty_as(&self.engine.version, "version unknown"),
                empty_as(&self.engine.os, "os unknown"),
                empty_as(&self.engine.arch, "arch unknown"),
                self.engine.running,
                self.engine.paused,
                self.engine.stopped,
                self.engine.images,
                if self.engine.images == 1 { "" } else { "s" }
            ))
            .size(10.5)
            .color(to_color32(palette.text_muted)),
        );
        let busy = access.is_running(ROOT_KEY);
        let mut detect = false;
        let mut root = None;
        ui.horizontal_wrapped(|ui| {
            if small_button_enabled(
                ui,
                palette,
                "Detect install method",
                !access.is_running(DETECT_KEY),
            ) {
                detect = true;
            }
            if let Some(manager) = self.manager {
                ui.label(
                    egui::RichText::new(manager.label())
                        .size(10.5)
                        .color(to_color32(palette.text)),
                );
                if !self.engine.is_ready()
                    && let Some(command) = manager.install_command()
                    && small_button_enabled(ui, palette, "Install engine", !busy)
                {
                    root = Some(command);
                }
                if self.engine.is_ready()
                    && let Some(command) = manager.upgrade_command()
                    && small_button_enabled(ui, palette, "Upgrade engine", !busy)
                {
                    root = Some(command);
                }
            }
        });
        if detect {
            self.run(access, DETECT_KEY, engine::DETECT_COMMAND);
        }
        if let Some(command) = root {
            // Passwordless sudo first; the password card appears when that fails.
            self.run_root(access, command, None);
            self.notify("Running the privileged command…".to_owned());
        }
        if let Some(output) = access.output(ROOT_KEY).map(|output| output.stdout.clone())
            && !output.trim().is_empty()
        {
            egui::ScrollArea::vertical()
                .id_salt("docker-engine-output")
                .max_height(PANEL_HEIGHT)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(output)
                            .font(egui::FontId::monospace(9.5))
                            .color(to_color32(palette.text)),
                    );
                });
        }
    }

    /// The create-container form.
    fn run_form(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            text_input(ui, palette, &mut self.run.image, "image (nginx)");
            text_input(ui, palette, &mut self.run.tag, "tag (latest)");
            text_input(ui, palette, &mut self.run.name, "name (optional)");
            text_input(ui, palette, &mut self.run.network, "network (optional)");
        });
        ui.horizontal_wrapped(|ui| {
            let mut restart = self.run.restart.min(RESTART_CHOICES.len() - 1);
            if let Some(picked) = select_str(
                ui,
                ("docker-restart", "pick"),
                RESTART_CHOICES[restart],
                &RESTART_CHOICES,
            ) && let Some(index) = RESTART_CHOICES.iter().position(|choice| *choice == picked)
            {
                restart = index;
            }
            self.run.restart = restart;
            text_input(
                ui,
                palette,
                &mut self.run.command,
                "command override (optional)",
            );
        });
        ui.horizontal_wrapped(|ui| {
            text_input(ui, palette, &mut self.run.ports, "ports 8080:80");
            text_input(ui, palette, &mut self.run.env, "env KEY=VALUE");
            text_input(ui, palette, &mut self.run.volumes, "volumes site:/data");
        });
        let mut create = false;
        ui.horizontal(|ui| {
            let ready = !self.run.image.trim().is_empty();
            if small_button_enabled(ui, palette, "Create", ready) {
                create = true;
            }
            if small_button_enabled(ui, palette, "Close form", true) {
                self.run_open = false;
            }
        });
        if create {
            let command = model::run_command(&self.settings.socket, &self.run.request());
            self.run(access, CHANGE_KEY, &command);
            self.run_open = false;
            self.tab = Tab::Containers;
            self.notify("Creating the container…".to_owned());
        }
        ui.separator();
    }
}

/// One row in the container list.
fn container_row(
    ui: &mut Ui,
    palette: &Palette,
    container: &Container,
    selected: &Option<String>,
    stale: &[String],
) -> Option<RowEvent> {
    let is_selected = selected.as_deref() == Some(container.id.as_str());
    let mut event = None;
    ui.horizontal_wrapped(|ui| {
        let name = egui::RichText::new(&container.name)
            .size(12.0)
            .strong()
            .color(to_color32(if is_selected {
                palette.accent
            } else {
                palette.text
            }));
        if ui
            .add(egui::Label::new(name).sense(egui::Sense::click()))
            .clicked()
        {
            event = Some(RowEvent::Select(container.id.clone()));
        }
        ui.label(
            egui::RichText::new(format!("· {}", container.image))
                .size(11.0)
                .color(to_color32(palette.text_muted)),
        );
        if stale.contains(&container.id) {
            ui.label(
                egui::RichText::new("update")
                    .size(10.0)
                    .color(to_color32(palette.warn)),
            );
        }
        ui.label(
            egui::RichText::new(&container.status)
                .size(11.0)
                .color(to_color32(if container.is_running() {
                    palette.ok
                } else {
                    palette.text_muted
                })),
        );
    });
    ui.horizontal_wrapped(|ui| {
        for action in container.actions() {
            let label = match action {
                Action::Start => "Start",
                Action::Stop => "Stop",
                Action::Restart => "Restart",
                Action::Pause => "Pause",
                Action::Unpause => "Resume",
                Action::Kill => "Kill",
                Action::Remove => "Remove",
            };
            if small_button_enabled(ui, palette, label, true) {
                event = Some(RowEvent::Act(
                    action,
                    container.id.clone(),
                    container.name.clone(),
                ));
            }
        }
        if small_button_enabled(ui, palette, "Logs", true) {
            event = Some(RowEvent::Logs(container.id.clone()));
        }
        if container.is_running() && small_button_enabled(ui, palette, "Stats", true) {
            event = Some(RowEvent::Stats(container.id.clone()));
        }
        if container.is_running() && small_button_enabled(ui, palette, "Shell", true) {
            event = Some(RowEvent::Shell(container.id.clone()));
        }
        if stale.contains(&container.id) && small_button_enabled(ui, palette, "Update", true) {
            event = Some(RowEvent::Update(container.id.clone()));
        }
        if small_button_enabled(ui, palette, "Details", true) {
            event = Some(RowEvent::Select(container.id.clone()));
        }
    });
    let ports: Vec<String> = container.ports.iter().map(Port::label).collect();
    if !ports.is_empty() {
        ui.label(
            egui::RichText::new(ports.join("  "))
                .size(10.0)
                .monospace()
                .color(to_color32(palette.text_muted)),
        );
    }
    ui.separator();
    event
}

/// `text`, or `fallback` when it is empty.
fn empty_as(text: &str, fallback: &str) -> String {
    if text.trim().is_empty() {
        fallback.to_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandOutput;

    #[derive(Default)]
    struct Stub {
        outputs: BTreeMap<String, CommandOutput>,
        commands: Vec<(String, String)>,
        inputs: Vec<String>,
        running: Vec<String>,
    }

    impl CommandAccess for Stub {
        fn exec(&mut self, key: &str, command: &str) {
            self.commands.push((key.to_owned(), command.to_owned()));
            self.running.push(key.to_owned());
        }

        fn exec_input(&mut self, key: &str, command: &str, input: &str) {
            self.inputs.push(input.to_owned());
            self.exec(key, command);
        }

        fn output(&self, key: &str) -> Option<&CommandOutput> {
            self.outputs.get(key)
        }

        fn failure(&self, key: &str) -> Option<&str> {
            self.outputs
                .get(key)
                .filter(|output| !output.succeeded())
                .and_then(CommandOutput::error_line)
        }

        fn is_running(&self, key: &str) -> bool {
            self.running.iter().any(|running| running == key)
        }
    }

    fn output(stdout: &str, code: Option<u32>) -> CommandOutput {
        CommandOutput {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            code,
        }
    }

    impl Stub {
        /// The command finished and left `answer` behind.
        fn finish(&mut self, key: &str, answer: CommandOutput) {
            self.running.retain(|running| running != key);
            self.outputs.insert(key.to_owned(), answer);
        }
    }

    const PS: &str = r#"{"ID":"abc","Names":"web","Image":"nginx:latest","State":"running","Status":"Up 1 hour","Ports":"","Labels":"","Mounts":"","Networks":"bridge"}"#;

    fn pane() -> DockerManager {
        DockerManager {
            settings: read_settings(&serde_json::Map::new()),
            ..DockerManager::default()
        }
    }

    #[test]
    fn the_list_is_asked_for_once_and_read_once() {
        let mut pane = pane();
        let mut access = Stub::default();
        pane.poll(&mut access);
        let list = access
            .commands
            .iter()
            .find(|(key, _)| key == LIST_KEY)
            .map(|(_, command)| command.clone())
            .expect("the list is asked for");
        assert!(list.starts_with("docker ps"), "{list}");

        access.finish(LIST_KEY, output(PS, Some(0)));
        pane.poll(&mut access);
        assert_eq!(pane.containers.len(), 1);
        let asked = access.commands.len();
        // The refresh interval has not passed, so nothing new goes out.
        pane.poll(&mut access);
        assert_eq!(
            access.commands.len(),
            asked,
            "a cached answer is not re-read"
        );
    }

    #[test]
    fn a_privilege_failure_asks_for_the_password() {
        let mut pane = pane();
        let mut access = Stub::default();
        pane.poll(&mut access);
        access.outputs.insert(
            LIST_KEY.to_owned(),
            CommandOutput {
                stdout: String::new(),
                stderr: "permission denied while trying to connect to the Docker daemon socket"
                    .to_owned(),
                code: Some(1),
            },
        );
        access.running.clear();
        pane.poll(&mut access);
        let sudo = pane.sudo.as_ref().expect("the password card is up");
        assert!(sudo.command.starts_with("docker ps"), "{}", sudo.command);
        assert!(
            pane.notice.as_ref().is_some_and(|notice| notice.failed),
            "and the failure is reported"
        );
    }

    #[test]
    fn the_sudo_retry_feeds_the_password_on_stdin() {
        let mut pane = pane();
        let mut access = Stub::default();
        pane.run_root(&mut access, "docker ps", Some("hunter2"));
        assert_eq!(access.commands[0].0, ROOT_KEY);
        assert_eq!(access.commands[0].1, "sudo -S -p '' sh -c 'docker ps'");
        assert_eq!(access.inputs, ["hunter2\n"]);
        // The unprivileged command is remembered, so a retry can be offered again.
        assert_eq!(
            pane.commands.get(ROOT_KEY).map(String::as_str),
            Some("docker ps")
        );
    }

    #[test]
    fn a_privileged_install_starts_passwordless() {
        let mut pane = pane();
        let mut access = Stub::default();
        pane.run_root(&mut access, "apt-get install -y docker.io", None);
        assert_eq!(
            access.commands[0].1,
            "sudo -n sh -c 'apt-get install -y docker.io'"
        );
        assert!(access.inputs.is_empty());
    }

    #[test]
    fn a_destructive_action_asks_first_when_the_setting_is_on() {
        let mut pane = pane();
        let mut access = Stub::default();
        let container = Container {
            id: "abc123".to_owned(),
            name: "web".to_owned(),
            state: "running".to_owned(),
            ..Container::default()
        };
        // What the row does, then what the pane does with it.
        if Action::Remove.is_destructive() && pane.settings.confirm_destructive {
            pane.confirm = Some(Confirm {
                action: Action::Remove,
                id: container.id.clone(),
                name: container.name.clone(),
            });
        }
        assert!(pane.confirm.is_some(), "remove asks first");
        assert!(access.commands.is_empty(), "and nothing ran yet");

        pane.settings.confirm_destructive = false;
        pane.confirm = None;
        pane.start_action(&mut access, Action::Remove, &container.id, &container.name);
        assert_eq!(access.commands[0].0, ACTION_KEY);
        assert!(
            access.commands[0].1.contains("rm -f 'abc123'"),
            "{:?}",
            access.commands[0]
        );
    }

    #[test]
    fn a_start_waits_for_the_pane_to_report_it() {
        let mut pane = pane();
        let mut access = Stub::default();
        pane.start_action(&mut access, Action::Start, "abc123", "web");
        assert_eq!(
            pane.pending
                .as_ref()
                .map(|(action, name, _)| (*action, name.as_str())),
            Some((Action::Start, "web"))
        );
        access
            .outputs
            .insert(ACTION_KEY.to_owned(), output("", Some(0)));
        access.running.clear();
        pane.poll(&mut access);
        assert!(pane.pending.is_none());
        assert_eq!(
            pane.notice.as_ref().map(|notice| notice.text.as_str()),
            Some("Started web")
        );
    }

    #[test]
    fn a_pull_that_downloads_marks_the_image_outdated() {
        let mut pane = pane();
        pane.containers = vec![Container {
            id: "abc".to_owned(),
            name: "web".to_owned(),
            image: "nginx:latest".to_owned(),
            ..Container::default()
        }];
        pane.pull_queue = vec!["nginx:latest".to_owned()];
        let mut access = Stub::default();
        pane.poll(&mut access);
        assert!(
            access.commands.iter().any(|(key, _)| key == PULL_KEY),
            "the image is pulled: {:?}",
            access.commands
        );
        access.finish(
            PULL_KEY,
            output("Status: Downloaded newer image for nginx:latest", Some(0)),
        );
        pane.poll(&mut access);
        assert_eq!(pane.stale, ["abc"]);
        assert!(pane.pull_queue.is_empty());
    }

    #[test]
    fn the_host_settings_keep_the_reference_defaults_and_bounds() {
        let defaults = read_settings(&serde_json::Map::new());
        assert_eq!(defaults.socket, model::DEFAULT_SOCKET);
        assert!(!defaults.show_stopped);
        assert_eq!(defaults.refresh, Duration::from_millis(DEFAULT_REFRESH_MS));
        assert_eq!(defaults.log_tail, DEFAULT_LOG_TAIL);
        assert!(defaults.confirm_destructive);

        let mut settings = serde_json::Map::new();
        settings.insert("refreshIntervalMs".to_owned(), Value::from(1u64));
        settings.insert("statsIntervalMs".to_owned(), Value::from(999_999u64));
        settings.insert("logTailLines".to_owned(), Value::from(5u64));
        let clamped = read_settings(&settings);
        assert_eq!(clamped.refresh, Duration::from_millis(MIN_REFRESH_MS));
        assert_eq!(clamped.stats, Duration::from_millis(MAX_STATS_MS));
        assert_eq!(clamped.log_tail, MIN_LOG_TAIL);
    }

    #[test]
    fn the_create_form_builds_a_run_request() {
        let mut form = RunForm {
            image: " nginx ".to_owned(),
            tag: "alpine".to_owned(),
            ports: "8080:80\n".to_owned(),
            env: "TZ=UTC\n\nLANG=C".to_owned(),
            restart: 2,
            ..RunForm::default()
        };
        form.volumes = "site:/data:ro".to_owned();
        let request = form.request();
        assert_eq!(request.reference(), "nginx:alpine");
        assert_eq!(request.ports, ["8080:80"]);
        assert_eq!(request.env, ["TZ=UTC", "LANG=C"]);
        assert_eq!(request.restart_policy, "unless-stopped");
        assert!(request.detach);
    }
}
