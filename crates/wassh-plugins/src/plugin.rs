//! The plugin trait, the palette they draw with, and the built-in registry.
//!
//! A plugin is a plain Rust value: the host creates one per tab, hands it a
//! [`PluginContext`] and a `Ui` once per frame, and keeps it until the tab closes. Nothing
//! is loaded dynamically, so a plugin cannot outlive the build it shipped with.

use egui::Ui;
use wassh_core::{Palette, SessionStatus};

use wassh_ui::dock::HeaderHealth;

use crate::{PluginId, PluginManifest};

/// Everything a plugin may read or store about the session it is attached to.
pub struct PluginContext<'a> {
    /// Active colour palette; plugins must not hard-code colours.
    pub palette: Palette,
    /// Label of the host or shell this tab is attached to.
    pub host_label: String,
    /// Current session status.
    pub status: SessionStatus,
    /// Stored data for this plugin in this scope (a host or an ad-hoc tab).
    ///
    /// A plugin may edit this map; the host persists it to
    /// `plugin-<id>.<scope>.json` after the pane is drawn.
    pub settings: &'a mut serde_json::Map<String, serde_json::Value>,
    /// Scope the data is filed under, shown in the pane for orientation.
    pub scope: String,
    /// Remote files and system statistics, present for live transports.
    pub session: Option<&'a mut dyn SessionAccess>,
    /// Status changes this session went through, oldest first.
    pub events: &'a [wassh_core::ConnectionEvent],
    /// Bytes the plugin wants typed into the session once the pane is drawn.
    pub pending_input: Vec<u8>,
    /// Decrypted values of the plugin's `secret: true` settings fields.
    ///
    /// A pane cannot reach the credential vault itself, so the host seals those
    /// values and hands the plain text over here, keyed by field.
    pub secrets: &'a std::collections::BTreeMap<String, String>,
    /// Secret edits the pane asked for while drawing; the host writes them to
    /// the vault as `(field, Some(new value))` or `(field, None)` to forget it.
    pub secret_writes: Vec<(String, Option<String>)>,
}

impl PluginContext<'_> {
    /// Whether the session can talk to its host right now.
    pub fn is_connected(&self) -> bool {
        self.status == SessionStatus::Connected
    }

    /// Type `text` into the session after the pane is drawn.
    pub fn send_text(&mut self, text: &str) {
        self.pending_input.extend_from_slice(text.as_bytes());
    }

    /// Type a single control byte, such as a carriage return.
    pub fn send_byte(&mut self, byte: u8) {
        self.pending_input.push(byte);
    }

    /// Text value stored under `key`, if any.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.settings.get(key).and_then(|value| value.as_str())
    }

    /// Any stored value under `key`, for structured plugin data.
    pub fn value(&self, key: &str) -> Option<&serde_json::Value> {
        self.settings.get(key)
    }

    /// Store any JSON value under `key`.
    pub fn set_value(&mut self, key: &str, value: serde_json::Value) {
        self.settings.insert(key.to_owned(), value);
    }

    /// Store a text value under `key`.
    pub fn set_text(&mut self, key: &str, value: impl Into<String>) {
        self.settings.insert(
            key.to_owned(),
            serde_json::Value::String(value.into()),
        );
    }

    /// A secret setting's value, decrypted by the host.
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(String::as_str)
    }

    /// Whether the vault already holds a value for this secret field.
    pub fn has_secret(&self, key: &str) -> bool {
        self.secrets.contains_key(key)
    }

    /// Ask the host to store `value` as this secret field.
    ///
    /// The pane never sees the sealed form and the value is not written to the
    /// plugin's data file.
    pub fn set_secret(&mut self, key: &str, value: impl Into<String>) {
        self.secret_writes
            .push((key.to_owned(), Some(value.into())));
    }

    /// Ask the host to forget this secret field.
    pub fn clear_secret(&mut self, key: &str) {
        self.secret_writes.push((key.to_owned(), None));
    }
}

/// The live terminal, as the AI agent tools see it.
///
/// `terminal_read`, `terminal_write`, `terminal_send_keys` and `terminal_wait`
/// work on the real screen, so a pane can watch a program it typed into.
pub trait TerminalAccess {
    /// The visible screen as text, one line per row, trimmed at the right.
    fn screen_text(&self) -> String;

    /// Type `text` into the terminal exactly as a paste would.
    fn type_text(&mut self, text: &str);

    /// Whether the shell has gone away (the pane shows "Closed").
    fn is_closed(&self) -> bool;
}

/// Everything a pane can ask the live session to do.
///
/// A pane gets one of these rather than separate handles, because the session itself can only
/// be borrowed mutably once per frame.
pub trait SessionAccess:
    crate::files::SftpAccess
    + crate::monitor::MonitorAccess
    + crate::command::CommandAccess
    + crate::command::TcpAccess
    + TerminalAccess
{
}

impl<
    T: crate::files::SftpAccess
        + crate::monitor::MonitorAccess
        + crate::command::CommandAccess
        + crate::command::TcpAccess
        + TerminalAccess,
> SessionAccess for T
{
}

/// A built-in plugin view.
pub trait Plugin {
    /// Manifest describing this plugin's id, labels and default placement.
    fn manifest(&self) -> PluginManifest;

    /// Draw the pane. The host draws the frame, title bar and close button.
    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>);

    /// Short state line for the pane header: the text and how it reads.
    ///
    /// The host paints it in the header strip next to the title, so a pane does
    /// not need a status row of its own. `None` leaves only the title.
    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        None
    }

    /// Optional button for the pane header, e.g. "Reconnect".
    ///
    /// The host draws it in the header strip and reports a click through
    /// [`Plugin::header_action_clicked`], so a pane never needs a toolbar row.
    fn header_action(&self) -> Option<HeaderButton> {
        None
    }

    /// The header button was clicked.
    fn header_action_clicked(&mut self) {}

    /// App-wide settings the pane changed while drawing, keyed by settings key.
    ///
    /// Options owns those settings, but a pane can hold fresher data, such as a
    /// provider's model list it just fetched. The host saves them.
    fn take_app_settings(&mut self) -> Vec<(String, serde_json::Value)> {
        Vec::new()
    }
}

/// A button a plugin asks the host to paint in its pane header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderButton {
    /// Caption of the button.
    pub label: &'static str,
    /// Whether it can be clicked right now.
    pub enabled: bool,
}

/// A free-form notepad, the simplest real plugin.
///
/// The text is kept in the plugin's stored data, so it survives restarts for the same host.
#[derive(Default)]
pub struct Scratchpad {
    text: String,
    loaded: bool,
}

impl Scratchpad {
    /// Current buffer contents.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Key the notepad text is stored under.
pub const SCRATCHPAD_TEXT_KEY: &str = "text";

impl Plugin for Scratchpad {
    fn manifest(&self) -> PluginManifest {
        PluginId::Scratchpad.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        if !self.loaded {
            self.text = context.text(SCRATCHPAD_TEXT_KEY).unwrap_or_default().to_owned();
            self.loaded = true;
        }
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(
                    egui::RichText::new("Clear")
                        .size(11.0)
                        .color(crate::color::to_color32(palette.text_muted)),
                ))
                .clicked()
            {
                self.text.clear();
                context.set_text(SCRATCHPAD_TEXT_KEY, "");
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · {}",
                    context.host_label,
                    context.scope,
                    if context.is_connected() {
                        "connected"
                    } else {
                        "offline"
                    }
                ))
                .size(11.0)
                .color(crate::color::to_color32(palette.text_muted)),
            );
        });
        ui.add_space(4.0);
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .font(egui::FontId::monospace(12.0))
                        .hint_text("Notes for this session")
                        .text_color(crate::color::to_color32(palette.text))
                        .desired_width(f32::INFINITY)
                        .desired_rows(20),
                );
                changed = response.changed();
            });
        if changed {
            let text = self.text.clone();
            context.set_text(SCRATCHPAD_TEXT_KEY, text);
        }
    }
}

/// Create the plugin instance for `id`.
pub fn create(id: PluginId) -> Box<dyn Plugin> {
    match id {
        PluginId::Sftp => Box::new(crate::files::Files::default()),
        PluginId::MacroPad => Box::new(crate::macro_pad::MacroPad::default()),
        PluginId::ConnectionLogger => Box::new(crate::connection_logger::ConnectionLogger::default()),
        PluginId::ServerMonitor => Box::new(crate::monitor::ServerMonitor::default()),
        PluginId::DockerManager => Box::new(crate::docker::DockerManager::default()),
        PluginId::MqttAnalyser => Box::new(crate::mqtt::MqttAnalyser::default()),
        PluginId::Scratchpad => Box::new(Scratchpad::default()),
        PluginId::AiAgent => Box::new(crate::ai_agent::AiAgent::default()),
    }
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    use super::*;

    #[test]
    fn the_registry_covers_every_plugin() {
        for id in PluginId::ALL {
            let plugin = create(id);
            assert_eq!(plugin.manifest().id, id);
        }
    }

    #[test]
    fn the_scratchpad_is_a_real_plugin() {
        let plugin = create(PluginId::Scratchpad);
        assert_eq!(plugin.manifest().name, "Scratchpad");
        let scratch = Scratchpad::default();
        assert!(scratch.text().is_empty());
    }

    #[test]
    fn unimplemented_plugins_announce_themselves() {
        let plugin = create(PluginId::DockerManager);
        assert_eq!(plugin.manifest().id, PluginId::DockerManager);
        assert_eq!(plugin.manifest().name, "Docker");
    }

    #[test]
    fn context_reports_connection_state_and_stores_text() {
        let mut settings = serde_json::Map::new();
        let mut context = PluginContext {
            palette: Palette::dark(),
            host_label: "dev".to_owned(),
            status: SessionStatus::Connected,
            settings: &mut settings,
            scope: "host-1".to_owned(),
            session: None,
            events: &[],
            pending_input: Vec::new(),
            secrets: &EMPTY_SECRETS,
            secret_writes: Vec::new(),
        };
        assert!(context.is_connected());
        context.set_text("text", "hello");
        assert_eq!(context.text("text"), Some("hello"));
        assert_eq!(context.scope, "host-1");
        drop(context);

        let context = PluginContext {
            palette: Palette::dark(),
            host_label: "dev".to_owned(),
            status: SessionStatus::Disconnected,
            settings: &mut settings,
            scope: "host-1".to_owned(),
            session: None,
            events: &[],
            pending_input: Vec::new(),
            secrets: &EMPTY_SECRETS,
            secret_writes: Vec::new(),
        };
        assert!(!context.is_connected());
        assert_eq!(context.text("text"), Some("hello"));
    }
}
