//! The Connection Logger plugin: a per-host history of when the session came up and went
//! down.
//!
//! The session collects [`ConnectionEvent`]s as it changes state; the pane folds the ones it
//! has not stored yet into its own log, so the history survives restarts and is complete even
//! for events that happened while the pane was closed.

use egui::Ui;
use serde::{Deserialize, Serialize};
use wassh_core::{ConnectionEvent, SessionStatus};

use crate::color::to_color32;
use wassh_ui::widgets::small_button;

use crate::plugin::{Plugin, PluginContext};
use crate::time;
use crate::{PluginId, PluginManifest};

/// Key the log is stored under in the plugin's data.
pub const LOG_KEY: &str = "events";
/// Key that pauses logging when set to `false`.
pub const ENABLED_KEY: &str = "enabled";
/// Key holding how many entries to keep.
pub const MAX_ENTRIES_KEY: &str = "maxEntries";
/// Entries kept when the settings say nothing.
pub const DEFAULT_MAX_ENTRIES: usize = 500;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogKind {
    Connected,
    Reconnecting,
    Disconnected,
    Failed,
}

impl LogKind {
    /// The kind recorded for `status`, or `None` for states a log should skip.
    pub fn of(status: SessionStatus) -> Option<Self> {
        match status {
            SessionStatus::Connected => Some(Self::Connected),
            SessionStatus::Reconnecting => Some(Self::Reconnecting),
            SessionStatus::Disconnected => Some(Self::Disconnected),
            SessionStatus::Failed => Some(Self::Failed),
            _ => None,
        }
    }

    /// Short label shown in the list.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Disconnected => "disconnected",
            Self::Failed => "failed",
        }
    }

    /// Whether the session was up before this entry.
    fn ends_a_session(self) -> bool {
        matches!(self, Self::Disconnected | Self::Failed | Self::Reconnecting)
    }
}

/// One line of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// What happened.
    pub kind: LogKind,
    /// Reason given by the transport, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How long the session had been up, for entries that ended one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Read a stored log, skipping entries that no longer parse.
pub fn parse_log(stored: Option<&serde_json::Value>) -> Vec<LogEntry> {
    stored
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<LogEntry>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Serialise the log for storage.
pub fn to_value(entries: &[LogEntry]) -> serde_json::Value {
    serde_json::to_value(entries).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

/// `2h 14m`, `45s`, or `—` for a missing duration.
pub fn format_duration(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "—".to_owned();
    };
    let seconds = ms / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        3600..=86_399 => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
        _ => format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600),
    }
}

/// Wall-clock time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The Connection Logger pane.
#[derive(Default)]
pub struct ConnectionLogger {
    entries: Vec<LogEntry>,
    loaded: bool,
    /// When the current connection started, for the next duration.
    connected_since: Option<u64>,
}

impl ConnectionLogger {
    /// The stored log.
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// Open a connection at `at_ms`.
    fn note_connected(&mut self, at_ms: u64) {
        self.connected_since = Some(at_ms);
    }

    /// Fold one event into the log, returning whether anything was added.
    fn merge(&mut self, event: &ConnectionEvent, max_entries: usize) -> bool {
        let Some(kind) = LogKind::of(event.status) else {
            return false;
        };
        if kind == LogKind::Connected {
            self.note_connected(event.at_ms);
        }
        // The same event can be offered again after a restart; only the newest entry can be a
        // duplicate because events arrive in order.
        if self
            .entries
            .last()
            .is_some_and(|last| last.at_ms == event.at_ms && last.kind == kind)
        {
            return false;
        }
        let duration_ms = if kind.ends_a_session() {
            self.connected_since
                .take()
                .map(|start| event.at_ms.saturating_sub(start))
        } else {
            None
        };
        self.entries.push(LogEntry {
            at_ms: event.at_ms,
            kind,
            message: event.message.clone(),
            duration_ms,
        });
        if self.entries.len() > max_entries {
            let excess = self.entries.len() - max_entries;
            self.entries.drain(..excess);
        }
        true
    }

    /// Forget everything.
    fn clear(&mut self) {
        self.entries.clear();
        self.connected_since = None;
    }

    /// Read the stored log and rebuild the current connection's start time.
    fn load(&mut self, stored: Option<&serde_json::Value>) {
        self.entries = parse_log(stored);
        self.connected_since = self
            .entries
            .last()
            .filter(|entry| entry.kind == LogKind::Connected)
            .map(|entry| entry.at_ms);
        self.loaded = true;
    }

    /// Fold in everything new from the session, returning whether the log changed.
    fn absorb(&mut self, events: &[ConnectionEvent], max_entries: usize) -> bool {
        let mut changed = false;
        for event in events {
            changed |= self.merge(event, max_entries);
        }
        changed
    }
}

impl Plugin for ConnectionLogger {
    fn manifest(&self) -> PluginManifest {
        PluginId::ConnectionLogger.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        let muted = to_color32(palette.text_muted);
        if !self.loaded {
            self.load(context.value(LOG_KEY));
        }

        let enabled = context
            .value(ENABLED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let max_entries = context
            .value(MAX_ENTRIES_KEY)
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as usize)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_ENTRIES);

        if enabled && self.absorb(context.events, max_entries) {
            context.set_value(LOG_KEY, to_value(&self.entries));
        }

        let mut clear = false;
        ui.horizontal(|ui| {
            if small_button(ui, &palette, "Clear") {
                clear = true;
            }
            let summary = match (enabled, self.connected_since) {
                (false, _) => "logging paused".to_owned(),
                (true, Some(since)) => format!(
                    "up {}",
                    format_duration(Some(now_ms().saturating_sub(since)))
                ),
                (true, None) => format!(
                    "{} event{}",
                    self.entries.len(),
                    if self.entries.len() == 1 { "" } else { "s" }
                ),
            };
            ui.label(egui::RichText::new(summary).size(10.0).color(muted));
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.label(
                        egui::RichText::new("Nothing logged yet for this host.")
                            .size(11.0)
                            .color(muted),
                    );
                    return;
                }
                for entry in self.entries.iter().rev() {
                    let colour = match entry.kind {
                        LogKind::Connected => to_color32(palette.accent),
                        LogKind::Reconnecting => to_color32(palette.text),
                        LogKind::Disconnected => muted,
                        LogKind::Failed => to_color32(palette.danger),
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(time::format_millis(entry.at_ms))
                                .font(egui::FontId::monospace(10.0))
                                .color(muted),
                        );
                        ui.label(
                            egui::RichText::new(entry.kind.label())
                                .font(egui::FontId::monospace(11.0))
                                .color(colour),
                        );
                        if entry.duration_ms.is_some() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "after {}",
                                    format_duration(entry.duration_ms)
                                ))
                                .size(10.0)
                                .color(muted),
                            );
                        }
                    });
                    if let Some(message) = &entry.message {
                        ui.label(
                            egui::RichText::new(message)
                                .size(10.0)
                                .color(to_color32(palette.text_muted)),
                        );
                    }
                }
            });

        if clear {
            self.clear();
            context.set_value(LOG_KEY, to_value(&self.entries));
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::SessionStatus;

    fn event(at_ms: u64, status: SessionStatus, message: Option<&str>) -> ConnectionEvent {
        ConnectionEvent {
            at_ms,
            status,
            message: message.map(str::to_owned),
        }
    }

    #[test]
    fn a_connection_and_a_drop_record_a_duration() {
        let mut logger = ConnectionLogger::default();
        assert!(logger.merge(&event(1_000, SessionStatus::Connected, None), 10));
        assert!(logger.merge(&event(61_000, SessionStatus::Disconnected, None), 10));

        let entries = logger.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, LogKind::Connected);
        assert_eq!(entries[1].kind, LogKind::Disconnected);
        assert_eq!(entries[1].duration_ms, Some(60_000));
        assert!(logger.connected_since.is_none());
    }

    #[test]
    fn intermediate_states_are_not_logged() {
        let mut logger = ConnectionLogger::default();
        for status in [
            SessionStatus::Idle,
            SessionStatus::Connecting,
            SessionStatus::AwaitingHostKey,
            SessionStatus::Authenticating,
            SessionStatus::Closed,
        ] {
            assert!(!logger.merge(&event(1_000, status, None), 10), "{status:?}");
        }
        assert!(logger.entries().is_empty());
    }

    #[test]
    fn the_same_event_is_only_recorded_once() {
        let mut logger = ConnectionLogger::default();
        let connect = event(1_000, SessionStatus::Connected, None);
        assert!(logger.merge(&connect, 10));
        assert!(!logger.merge(&connect, 10), "a replay changes nothing");
        assert_eq!(logger.entries().len(), 1);
    }

    #[test]
    fn failures_keep_their_reason_and_end_the_session() {
        let mut logger = ConnectionLogger::default();
        logger.merge(&event(5_000, SessionStatus::Connected, None), 10);
        logger.merge(
            &event(9_000, SessionStatus::Failed, Some("connection refused")),
            10,
        );
        let last = logger.entries().last().expect("an entry");
        assert_eq!(last.kind, LogKind::Failed);
        assert_eq!(last.message.as_deref(), Some("connection refused"));
        assert_eq!(last.duration_ms, Some(4_000));
    }

    #[test]
    fn a_reconnect_starts_a_new_duration() {
        let mut logger = ConnectionLogger::default();
        logger.merge(&event(1_000, SessionStatus::Connected, None), 10);
        logger.merge(&event(2_000, SessionStatus::Reconnecting, None), 10);
        logger.merge(&event(3_000, SessionStatus::Connected, None), 10);
        logger.merge(&event(8_000, SessionStatus::Disconnected, None), 10);
        let durations: Vec<_> = logger
            .entries()
            .iter()
            .map(|entry| entry.duration_ms)
            .collect();
        assert_eq!(durations, vec![None, Some(1_000), None, Some(5_000)]);
    }

    #[test]
    fn the_log_keeps_the_newest_entries() {
        let mut logger = ConnectionLogger::default();
        for step in 0..6u64 {
            let status = if step % 2 == 0 {
                SessionStatus::Connected
            } else {
                SessionStatus::Disconnected
            };
            logger.merge(&event(step * 1_000, status, None), 4);
        }
        let entries = logger.entries();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[3].at_ms, 5_000);
    }

    #[test]
    fn stored_logs_round_trip_and_skip_junk() {
        let mut logger = ConnectionLogger::default();
        logger.merge(&event(1_000, SessionStatus::Connected, None), 10);
        let value = to_value(logger.entries());
        assert_eq!(parse_log(Some(&value)), logger.entries().to_vec());

        let mixed = serde_json::json!([{ "atMs": 5, "kind": "connected" }, "nope"]);
        assert_eq!(parse_log(Some(&mixed)).len(), 1);
        assert!(parse_log(None).is_empty());
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(format_duration(None), "—");
        assert_eq!(format_duration(Some(45_000)), "45s");
        assert_eq!(format_duration(Some(125_000)), "2m 05s");
        assert_eq!(format_duration(Some(8_040_000)), "2h 14m");
        assert_eq!(format_duration(Some(90_000_000)), "1d 1h");
    }

    #[test]
    fn a_stored_log_keeps_its_uptime_after_a_restart() {
        let stored = to_value(&[
            LogEntry {
                at_ms: 10_000,
                kind: LogKind::Connected,
                message: None,
                duration_ms: None,
            },
            LogEntry {
                at_ms: 20_000,
                kind: LogKind::Disconnected,
                message: None,
                duration_ms: Some(10_000),
            },
            LogEntry {
                at_ms: 30_000,
                kind: LogKind::Connected,
                message: None,
                duration_ms: None,
            },
        ]);
        let mut logger = ConnectionLogger::default();
        logger.load(Some(&stored));
        assert_eq!(logger.entries.len(), 3);
        // The session is still up, so the next drop measures from the last connect.
        assert_eq!(logger.connected_since, Some(30_000));

        let mut ended = ConnectionLogger::default();
        ended.load(Some(&to_value(&[LogEntry {
            at_ms: 10_000,
            kind: LogKind::Failed,
            message: Some("boom".to_owned()),
            duration_ms: None,
        }])));
        assert_eq!(ended.connected_since, None);
    }
}
