//! The Docker Manager plugin: container list, lifecycle buttons and logs, over SSH.

use std::time::{Duration, Instant};

use egui::Ui;
use wassh_core::Palette;

use crate::color::to_color32;
use crate::command::{CommandAccess, CommandOutput};
use wassh_ui::widgets::small_button_enabled;

use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the container list is requested under.
pub const LIST_KEY: &str = "docker.list";
/// Key lifecycle commands are requested under.
pub const ACTION_KEY: &str = "docker.action";
/// Key log output is requested under.
pub const LOGS_KEY: &str = "docker.logs";

/// How often the list refreshes itself while the pane is open.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Lines of log fetched for a container.
pub const LOG_LINES: usize = 200;

/// The command that lists containers. `|` cannot appear in a docker name, image or status, so
/// it is a safe separator.
pub const LIST_COMMAND: &str =
    "docker ps -a --format '{{.ID}}|{{.Names}}|{{.Image}}|{{.State}}|{{.Status}}'";

/// One container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    /// Short id.
    pub id: String,
    /// Container name.
    pub name: String,
    /// Image it was started from.
    pub image: String,
    /// `running`, `exited`, `created`, `paused`, `restarting`…
    pub state: String,
    /// Human status line, such as `Up 3 hours` or `Exited (1) 2 days ago`.
    pub status: String,
}

impl Container {
    /// Whether the container is up.
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

/// What the buttons can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockerAction {
    Start,
    Stop,
    Restart,
}

impl DockerAction {
    /// The docker subcommand.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    /// Past-tense wording for the status line.
    pub fn done(self) -> &'static str {
        match self {
            Self::Start => "Started",
            Self::Stop => "Stopped",
            Self::Restart => "Restarted",
        }
    }
}

/// Ids are pasted into a shell command, so only accept what docker actually emits.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The command that performs `action` on `id`.
pub fn container_command(action: DockerAction, id: &str) -> Option<String> {
    is_safe_id(id).then(|| format!("docker {} {id}", action.verb()))
}

/// The command that fetches `id`'s logs, with stderr folded in.
pub fn logs_command(id: &str) -> Option<String> {
    is_safe_id(id).then(|| format!("docker logs --tail {LOG_LINES} {id} 2>&1"))
}

/// Parse `docker ps` output produced with [`LIST_COMMAND`].
pub fn parse_containers(text: &str) -> Vec<Container> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.trim_end().split('|');
            let id = fields.next()?.trim();
            let name = fields.next()?.trim();
            let image = fields.next()?.trim();
            let state = fields.next()?.trim();
            let status = fields.next().unwrap_or_default().trim();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some(Container {
                id: id.to_owned(),
                name: name.to_owned(),
                image: image.to_owned(),
                state: state.to_owned(),
                status: status.to_owned(),
            })
        })
        .collect()
}

/// The Docker Manager pane.
#[derive(Default)]
pub struct DockerManager {
    /// Container the buttons act on.
    selected: Option<String>,
    /// When the list was last requested.
    last_poll: Option<Instant>,
    /// Action in flight, so its result can be reported and the list refreshed.
    pending: Option<(DockerAction, String)>,
    /// Container whose logs are on screen.
    logs_for: Option<String>,
    /// Last thing that happened, shown under the toolbar.
    notice: Option<String>,
}

impl Plugin for DockerManager {
    fn manifest(&self) -> PluginManifest {
        PluginId::DockerManager.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let muted = to_color32(context.palette.text_muted);
        let host_label = context.host_label.clone();
        let Some(access) = context.session.as_deref_mut() else {
            ui.label(
                egui::RichText::new("Containers need an SSH session.")
                    .size(12.0)
                    .color(muted),
            );
            return;
        };
        self.draw(ui, &context.palette, &host_label, access);
    }
}

impl DockerManager {
    /// Draw the pane against `access`.
    fn draw(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        host_label: &str,
        access: &mut dyn CommandAccess,
    ) {
        let muted = to_color32(palette.text_muted);

        self.collect(access);
        if self.needs_poll(access) {
            access.exec(LIST_KEY, LIST_COMMAND);
            self.last_poll = Some(Instant::now());
        }

        let containers = access
            .output(LIST_KEY)
            .map(|output| parse_containers(&output.stdout))
            .unwrap_or_default();
        if let Some(selected) = &self.selected
            && !containers.iter().any(|item| &item.id == selected)
        {
            self.selected = None;
        }

        // Toolbar.
        ui.horizontal_wrapped(|ui| {
            if small_button_enabled(ui, palette, "Refresh", !access.is_running(LIST_KEY)) {
                self.last_poll = None;
            }
            let selected = self
                .selected
                .as_ref()
                .and_then(|id| containers.iter().find(|item| &item.id == id))
                .cloned();
            let busy = access.is_running(ACTION_KEY) || self.pending.is_some();
            match selected {
                Some(container) => {
                    let start = small_button_enabled(ui, palette, "Start", !busy && !container.is_running());
                    let stop = small_button_enabled(ui, palette, "Stop", !busy && container.is_running());
                    let restart = small_button_enabled(ui, palette, "Restart", !busy && container.is_running());
                    let logs = small_button_enabled(ui, palette, "Logs", !busy);
                    let action = if start {
                        Some(DockerAction::Start)
                    } else if stop {
                        Some(DockerAction::Stop)
                    } else if restart {
                        Some(DockerAction::Restart)
                    } else {
                        None
                    };
                    if let Some(action) = action {
                        self.act(access, action, &container);
                    }
                    if logs {
                        self.logs_for = Some(container.id.clone());
                        access.exec(LOGS_KEY, &logs_command(&container.id).unwrap_or_default());
                    }
                }
                None => {
                    small_button_enabled(ui, palette, "Start", false);
                    small_button_enabled(ui, palette, "Stop", false);
                    small_button_enabled(ui, palette, "Restart", false);
                    small_button_enabled(ui, palette, "Logs", false);
                }
            }
        });

        if let Some(notice) = &self.notice {
            ui.label(
                egui::RichText::new(notice)
                    .size(10.0)
                    .color(to_color32(palette.accent)),
            );
        }
        if let Some(failure) = access.failure(LIST_KEY) {
            ui.label(
                egui::RichText::new(failure)
                    .size(11.0)
                    .color(to_color32(palette.danger)),
            );
        }
        ui.add_space(2.0);

        if access.is_running(LIST_KEY) && containers.is_empty() {
            ui.label(egui::RichText::new("Listing containers…").size(11.0).color(muted));
            return;
        }
        if containers.is_empty() {
            ui.label(
                egui::RichText::new("No containers. Is docker running on this host?")
                    .size(11.0)
                    .color(muted),
            );
        }

        let logs_height = if self.logs_for.is_some() { 120.0 } else { 0.0 };
        egui::ScrollArea::vertical()
            .id_salt("docker-containers")
            .max_height((ui.available_height() - logs_height).max(60.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for container in &containers {
                    let selected = self.selected.as_deref() == Some(container.id.as_str());
                    if container_row(ui, palette, container, selected) {
                        self.selected = Some(container.id.clone());
                        self.logs_for = None;
                    }
                }
            });

        if let Some(id) = self.logs_for.clone() {
            self.draw_logs(ui, palette, access, &id);
        }

        let running = containers.iter().filter(|item| item.is_running()).count();
        ui.label(
            egui::RichText::new(format!(
                "{host_label} · {running} up of {} container{}",
                containers.len(),
                if containers.len() == 1 { "" } else { "s" }
            ))
            .size(10.0)
            .color(muted),
        );
    }

    /// Start the selected action and remember it for the refresh.
    fn act(&mut self, access: &mut dyn CommandAccess, action: DockerAction, container: &Container) {
        let Some(command) = container_command(action, &container.id) else {
            self.notice = Some("That container id is not safe to pass to a shell.".to_owned());
            return;
        };
        access.exec(ACTION_KEY, &command);
        self.pending = Some((action, container.name.clone()));
    }

    /// Fold a finished action into the status line and ask for a fresh list.
    fn collect(&mut self, access: &mut dyn CommandAccess) {
        if self.pending.is_some() && !access.is_running(ACTION_KEY) {
            let Some((action, name)) = self.pending.take() else {
                return;
            };
            self.notice = Some(match access.output(ACTION_KEY) {
                Some(output) if output.succeeded() => format!("{} {name}", action.done()),
                Some(output) => output
                    .error_line()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Could not {} {name}", action.verb())),
                None => format!("Could not {} {name}", action.verb()),
            });
            // The container state has changed, so refresh immediately.
            self.last_poll = None;
        }
    }

    /// Whether the list should be requested again.
    fn needs_poll(&self, access: &dyn CommandAccess) -> bool {
        !access.is_running(LIST_KEY)
            && self
                .last_poll
                .is_none_or(|polled| polled.elapsed() >= POLL_INTERVAL)
    }

    /// The log panel under the list.
    fn draw_logs(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn CommandAccess, id: &str) {
        let muted = to_color32(palette.text_muted);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if small_button_enabled(ui, palette, "Close logs", true) {
                self.logs_for = None;
            }
            ui.label(
                egui::RichText::new(format!("logs · {}", short_id(id)))
                    .size(10.0)
                    .color(muted),
            );
        });
        if self.logs_for.is_none() {
            return;
        }
        let text: &str = match access.output(LOGS_KEY) {
            Some(CommandOutput { stdout, .. }) if !stdout.trim().is_empty() => stdout,
            _ => "",
        };
        egui::ScrollArea::vertical()
            .id_salt("docker-logs")
            .max_height(100.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if text.is_empty() {
                    let waiting = if access.is_running(LOGS_KEY) {
                        "Fetching logs…"
                    } else {
                        "No output."
                    };
                    ui.label(egui::RichText::new(waiting).size(11.0).color(muted));
                } else {
                    ui.label(
                        egui::RichText::new(text)
                            .font(egui::FontId::monospace(10.0))
                            .color(to_color32(palette.text)),
                    );
                }
            });
    }
}

/// `abc12345` from a longer id.
fn short_id(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// One container row. Returns true when it was clicked.
fn container_row(ui: &mut Ui, palette: &Palette, container: &Container, selected: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::Vec2::new(ui.available_width(), 22.0), egui::Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            3.0,
            to_color32(if selected {
                palette.bg_panel_2
            } else {
                palette.bg_input
            }),
        );
    }
    let dot = egui::pos2(rect.left() + 6.0, rect.center().y);
    let colour = if container.is_running() {
        to_color32(palette.accent)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter()
        .circle_filled(dot, 3.0, colour);
    ui.painter().text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &container.name,
        egui::FontId::monospace(11.0),
        to_color32(palette.text),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        &container.status,
        egui::FontId::monospace(10.0),
        to_color32(palette.text_muted),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}


#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "8f2c1b7d9e01|nginx|nginx:1.25|running|Up 3 hours\n\
a1b2c3d4e5f6|postgres|postgres:16|exited|Exited (1) 2 days ago\n\
deadbeef0000|my-app|ghcr.io/acme/app:latest|running|Up 10 minutes (healthy)\n";

    /// Recorded command access.
    #[derive(Default)]
    struct FakeCommands {
        sent: Vec<(String, String)>,
        outputs: std::collections::HashMap<String, CommandOutput>,
        running: std::collections::HashSet<String>,
        failures: std::collections::HashMap<String, String>,
        screen: String,
        typed: String,
    }

    impl CommandAccess for FakeCommands {
        fn exec(&mut self, key: &str, command: &str) {
            self.sent.push((key.to_owned(), command.to_owned()));
            self.running.insert(key.to_owned());
        }

        fn output(&self, key: &str) -> Option<&CommandOutput> {
            self.outputs.get(key)
        }

        fn failure(&self, key: &str) -> Option<&str> {
            self.failures.get(key).map(String::as_str)
        }

        fn is_running(&self, key: &str) -> bool {
            self.running.contains(key)
        }
    }

    impl crate::command::TcpAccess for FakeCommands {
        fn open_forward(&mut self, _key: &str, _host: &str, _port: u16) {}

        fn forward_address(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_error(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_pending(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::plugin::TerminalAccess for FakeCommands {
        fn screen_text(&self) -> String {
            self.screen.clone()
        }

        fn type_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }

        fn is_closed(&self) -> bool {
            false
        }
    }


    impl FakeCommands {
        /// Pretend the last command for `key` finished.
        fn finish(&mut self, key: &str, output: CommandOutput) {
            self.running.remove(key);
            self.outputs.insert(key.to_owned(), output);
        }
    }

    fn listing() -> CommandOutput {
        CommandOutput {
            stdout: LISTING.to_owned(),
            stderr: String::new(),
            code: Some(0),
        }
    }

    #[test]
    fn a_docker_listing_parses() {
        let containers = parse_containers(LISTING);
        assert_eq!(containers.len(), 3);
        assert_eq!(containers[0].name, "nginx");
        assert_eq!(containers[0].image, "nginx:1.25");
        assert!(containers[0].is_running());
        assert_eq!(containers[1].status, "Exited (1) 2 days ago");
        assert!(!containers[1].is_running());
        assert_eq!(containers[2].image, "ghcr.io/acme/app:latest");
        assert_eq!(containers[2].status, "Up 10 minutes (healthy)");

        // Junk lines are skipped rather than shown as containers.
        assert!(parse_containers("").is_empty());
        assert!(parse_containers("garbage\n").is_empty());
        assert!(parse_containers("|name|image|running|Up").is_empty());
    }

    #[test]
    fn commands_are_built_from_valid_ids_only() {
        assert_eq!(
            container_command(DockerAction::Stop, "8f2c1b7d9e01").as_deref(),
            Some("docker stop 8f2c1b7d9e01")
        );
        assert_eq!(
            container_command(DockerAction::Restart, "web-1").as_deref(),
            Some("docker restart web-1")
        );
        assert_eq!(
            logs_command("web-1").as_deref(),
            Some("docker logs --tail 200 web-1 2>&1")
        );
        // Nothing that could break out of the command is accepted.
        for bad in ["", "web; rm -rf /", "web$(id)", "web 1", "web|1", "`id`"] {
            assert_eq!(container_command(DockerAction::Start, bad), None, "{bad:?}");
            assert_eq!(logs_command(bad), None, "{bad:?}");
            assert!(!is_safe_id(bad));
        }
    }

    #[test]
    fn the_pane_lists_containers_and_refreshes_itself() {
        let mut pane = DockerManager::default();
        let mut access = FakeCommands::default();
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let frame = |pane: &mut DockerManager, access: &mut FakeCommands| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(320.0, 400.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| pane.draw(ui, &palette, "dev", access));
            output.drop_without_applying_deltas();
        };

        // First frame asks for the list.
        frame(&mut pane, &mut access);
        assert_eq!(access.sent.len(), 1);
        assert_eq!(access.sent[0].0, LIST_KEY);
        assert_eq!(access.sent[0].1, LIST_COMMAND);

        // The answer arrives, and the pane does not ask again before the interval.
        access.finish(LIST_KEY, listing());
        frame(&mut pane, &mut access);
        assert_eq!(access.sent.len(), 1, "polled once within the interval");

        // Selecting a container and stopping it sends one command.
        pane.selected = Some("8f2c1b7d9e01".to_owned());
        pane.act(&mut access, DockerAction::Stop, &parse_containers(LISTING)[0]);
        assert_eq!(access.sent.len(), 2);
        assert_eq!(access.sent[1].0, ACTION_KEY);
        assert_eq!(access.sent[1].1, "docker stop 8f2c1b7d9e01");

        // Once it is done the notice names the container and the list refreshes.
        access.finish(
            ACTION_KEY,
            CommandOutput {
                stdout: "8f2c\n".to_owned(),
                stderr: String::new(),
                code: Some(0),
            },
        );
        frame(&mut pane, &mut access);
        assert_eq!(pane.notice.as_deref(), Some("Stopped nginx"));
        assert_eq!(access.sent.len(), 3, "the list was refreshed");
        assert_eq!(access.sent[2].0, LIST_KEY);
    }

    #[test]
    fn a_failed_action_reports_the_reason() {
        let mut pane = DockerManager::default();
        let mut access = FakeCommands::default();
        pane.pending = Some((DockerAction::Start, "nginx".to_owned()));
        access.finish(
            ACTION_KEY,
            CommandOutput {
                stdout: String::new(),
                stderr: "Error response from daemon: permission denied\n".to_owned(),
                code: Some(1),
            },
        );
        pane.collect(&mut access);
        assert_eq!(
            pane.notice.as_deref(),
            Some("Error response from daemon: permission denied")
        );
        assert!(pane.pending.is_none());
    }

    #[test]
    fn ids_are_shortened_for_the_log_header() {
        assert_eq!(short_id("8f2c1b7d9e010000"), "8f2c1b7d9e01");
        assert_eq!(short_id("abc"), "abc");
    }
}
