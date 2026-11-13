//! The Connection Logger plugin: a per-host history of when the session came up and went
//! down.
//!
//! The session collects [`ConnectionEvent`]s as it changes state; the pane folds the ones it
//! has not stored yet into its own log, so the history survives restarts and is complete even
//! for events that happened while the pane was closed.

use std::time::Duration;

use egui::Ui;
use serde::{Deserialize, Serialize};
use wassh_core::time::now_ms;
use wassh_core::{ConnectionEvent, Palette, SessionStatus};

use wassh_ui::to_color32;
use wassh_ui::widgets::{
    Button, faded, small_button, truncating_galley, truncating_galley_with,
};

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

/// Height of the timeline bar, and the shortest it shrinks to in a low pane.
const TIMELINE_HEIGHT: f32 = 28.0;
const TIMELINE_MIN_HEIGHT: f32 = 12.0;
/// Height of the step chart's box, and the shortest it shrinks to in a low pane.
const STEP_HEIGHT: f32 = 80.0;
const STEP_MIN_HEIGHT: f32 = 36.0;
/// Space above and below the step line.
const STEP_PADDING: f32 = 10.0;
/// Thickness of the step line.
const STEP_WIDTH: f32 = 2.0;
/// Dash pattern of the "up" level line.
const DASH: f32 = 3.0;
const DASH_GAP: f32 = 3.0;
/// Opacity of the filled area at the line and at its baseline.
const AREA_TOP_ALPHA: f32 = 0.45;
const AREA_BOTTOM_ALPHA: f32 = 0.05;
/// Side of one heatmap cell, the gap between cells, and their corner radius.
const HEAT_CELL: f32 = 18.0;
const HEAT_GAP: f32 = 3.0;
const HEAT_RADIUS: u8 = 3;
/// Side of a legend sample.
const HEAT_SAMPLE: f32 = 10.0;
/// Tallest the heatmap gets before it scrolls, and the shortest it shrinks to.
const HEAT_MAX_HEIGHT: f32 = 90.0;
const HEAT_MIN_HEIGHT: f32 = HEAT_CELL * 2.0 + HEAT_GAP;
/// Height of the axis under a graph, and the narrowest a tick label's slot gets before the
/// axis drops one.
const AXIS_HEIGHT: f32 = 14.0;
const TICK_ROOM: f32 = 88.0;
/// Corner radius of every box the pane draws.
const SURFACE_RADIUS: u8 = 4;
/// Gap between the blocks of the pane.
const BLOCK_GAP: f32 = 6.0;
/// Narrowest metric card, and the padding inside it.
const METRIC_MIN_WIDTH: f32 = 130.0;
const METRIC_PAD_X: f32 = 8.0;
const METRIC_PAD_Y: f32 = 6.0;
/// Gap between a card's label and its value, and the border strength of a card.
const METRIC_LINE_GAP: f32 = 2.0;
const METRIC_BORDER: f32 = 0.7;
/// Label and value sizes on a metric card, and the dot marking a live state.
const METRIC_LABEL_SIZE: f32 = 10.0;
const METRIC_VALUE_SIZE: f32 = 13.0;
const METRIC_DOT: f32 = 6.0;
const METRIC_DOT_GAP: f32 = 6.0;
/// One row of the event list, the line a message takes, and the gap between columns.
const LIST_ROW_HEIGHT: f32 = 16.0;
const LIST_MESSAGE_HEIGHT: f32 = 13.0;
const LIST_COLUMN_GAP: f32 = 10.0;
/// Length of a day, for axis labels that have to name the day.
const DAY_MS: u64 = 86_400_000;
/// Narrowest a timeline segment is drawn, so a short blip stays visible.
const MIN_SEGMENT_PX: f32 = 2.0;
/// Padding inside a tooltip, the gap between its lines, and its inset from the graph.
const TOOLTIP_PAD: f32 = 6.0;
const TOOLTIP_GAP: f32 = 2.0;
const TOOLTIP_INSET: f32 = 8.0;
/// Redraw pace for "Now" and the current duration.
const TICK: Duration = Duration::from_secs(5);

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

    /// Every kind, for measuring the widest label the list prints.
    const ALL: [Self; 4] = [
        Self::Connected,
        Self::Reconnecting,
        Self::Disconnected,
        Self::Failed,
    ];

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

/// Which graph the pane draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphMode {
    /// One bar with the states side by side.
    #[default]
    Timeline,
    /// Up/down step line with the area under it filled.
    Step,
    /// One cell per hour, shaded by uptime.
    Heatmap,
}

impl GraphMode {
    /// Every mode, in the order the buttons are drawn.
    pub const ALL: [Self; 3] = [Self::Timeline, Self::Step, Self::Heatmap];

    /// Label on the mode's button.
    pub fn label(self) -> &'static str {
        match self {
            Self::Timeline => "Timeline Bar",
            Self::Step => "Step Chart",
            Self::Heatmap => "Uptime Heatmap",
        }
    }
}

/// How much history the graph covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeWindow {
    /// The last hour.
    Hour,
    /// The last six hours.
    SixHours,
    /// The last day, the default.
    #[default]
    Day,
    /// The last week.
    Week,
    /// Everything that has been logged.
    All,
}

impl TimeWindow {
    /// Every window, in the order the buttons are drawn.
    pub const ALL: [Self; 5] = [
        Self::Hour,
        Self::SixHours,
        Self::Day,
        Self::Week,
        Self::All,
    ];

    /// Label on the window's button.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hour => "1H",
            Self::SixHours => "6H",
            Self::Day => "24H",
            Self::Week => "7D",
            Self::All => "ALL",
        }
    }

    /// How long the window is, or `None` when it starts at the oldest entry.
    fn fixed_duration_ms(self) -> Option<u64> {
        const HOUR: u64 = 3_600_000;
        match self {
            Self::Hour => Some(HOUR),
            Self::SixHours => Some(6 * HOUR),
            Self::Day => Some(24 * HOUR),
            Self::Week => Some(7 * 24 * HOUR),
            Self::All => None,
        }
    }
}

/// The stretch of time a graph covers, as `[start_ms, end_ms]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// Left edge of the graph.
    pub start_ms: u64,
    /// Right edge, usually now.
    pub end_ms: u64,
}

impl Range {
    /// How much time the graph covers, never zero.
    pub fn duration_ms(self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms).max(1)
    }

    /// Where `at_ms` sits in the window, as 0…1.
    pub fn fraction(self, at_ms: u64) -> f32 {
        ((at_ms.saturating_sub(self.start_ms)) as f64 / self.duration_ms() as f64) as f32
    }
}

/// The window `window` covers, ending at `now`.
///
/// `All` starts at the oldest entry (plus a minute of breathing room), and never covers less
/// than an hour, so a fresh host still gets a graph.
pub fn window_range(window: TimeWindow, entries: &[LogEntry], now: u64) -> Range {
    let duration_ms = window.fixed_duration_ms().unwrap_or_else(|| {
        let earliest = entries
            .iter()
            .map(|entry| entry.at_ms)
            .min()
            .unwrap_or(now);
        (now.saturating_sub(earliest) + 60_000).max(3_600_000)
    });
    Range {
        start_ms: now.saturating_sub(duration_ms),
        end_ms: now,
    }
}

/// One stretch of time during which the state did not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// When the state began.
    pub start_ms: u64,
    /// When it ended.
    pub end_ms: u64,
    /// The state during it.
    pub kind: LogKind,
    /// Why the session dropped, for the entries that carry a reason.
    pub message: Option<String>,
}

impl Span {
    /// How long the span lasted.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// The spans a window shows: each entry runs until the next one, and the last until `now`.
///
/// With nothing logged the window is one span in the session's current state, so the graph
/// still says something truthful about it.
pub fn build_spans(entries: &[LogEntry], range: Range, status: SessionStatus) -> Vec<Span> {
    if entries.is_empty() {
        let kind = if status == SessionStatus::Connected {
            LogKind::Connected
        } else {
            LogKind::Disconnected
        };
        return vec![Span {
            start_ms: range.start_ms,
            end_ms: range.end_ms,
            kind,
            message: None,
        }];
    }
    let mut spans = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let end_ms = entries
            .get(index + 1)
            .map_or(range.end_ms, |next| next.at_ms);
        let start_ms = entry.at_ms.max(range.start_ms);
        let end_ms = end_ms.min(range.end_ms);
        if end_ms > start_ms {
            spans.push(Span {
                start_ms,
                end_ms,
                kind: entry.kind,
                message: entry.message.clone(),
            });
        }
    }
    spans
}

/// What the cards above the graph report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// Share of the window the session was up.
    pub uptime_pct: f64,
    /// Time up, in milliseconds.
    pub connected_ms: u64,
    /// Time down, in milliseconds.
    pub disconnected_ms: u64,
    /// How many times the session went down (disconnects and failures).
    pub disconnects: usize,
    /// Mean length of the up spans.
    pub average_connected_ms: u64,
    /// How long the newest state has lasted.
    pub current_duration_ms: u64,
}

/// Fold the window's spans into the summary the metric cards show.
pub fn summarise(spans: &[Span], range: Range, entries: &[LogEntry], now: u64) -> Summary {
    let mut connected_ms = 0u64;
    let mut disconnected_ms = 0u64;
    let mut disconnects = 0usize;
    let mut sessions = 0u64;
    for span in spans {
        let duration = span.duration_ms();
        if span.kind == LogKind::Connected {
            connected_ms += duration;
            sessions += 1;
        } else {
            disconnected_ms += duration;
            if matches!(span.kind, LogKind::Disconnected | LogKind::Failed) {
                disconnects += 1;
            }
        }
    }
    let current_duration_ms = entries
        .last()
        .map_or(0, |entry| now.saturating_sub(entry.at_ms));
    Summary {
        uptime_pct: (connected_ms as f64 / range.duration_ms() as f64 * 100.0).clamp(0.0, 100.0),
        connected_ms,
        disconnected_ms,
        disconnects,
        average_connected_ms: connected_ms.checked_div(sessions).unwrap_or(0),
        current_duration_ms,
    }
}

/// One hour of the heatmap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    /// Where the hour starts (clipped to the window).
    pub start_ms: u64,
    /// Where it ends (clipped to the window).
    pub end_ms: u64,
    /// Share of the hour the session was up.
    pub uptime_pct: f64,
    /// Time up inside the hour, in milliseconds.
    pub connected_ms: u64,
    /// Length of the hour inside the window, in milliseconds.
    pub total_ms: u64,
    /// How many times the state changed away from "up" inside the hour.
    pub changes: usize,
}

/// Split the window into hour-aligned buckets, each with its uptime.
pub fn hourly_buckets(range: Range, spans: &[Span]) -> Vec<Bucket> {
    const HOUR: u64 = 3_600_000;
    let mut buckets = Vec::new();
    let mut start_ms = range.start_ms / HOUR * HOUR;
    while start_ms < range.end_ms {
        let bucket_start = start_ms.max(range.start_ms);
        let bucket_end = (start_ms + HOUR).min(range.end_ms);
        let total_ms = bucket_end.saturating_sub(bucket_start).max(1);
        let mut connected_ms = 0u64;
        let mut changes = 0usize;
        for span in spans {
            let overlap = bucket_end.min(span.end_ms).saturating_sub(bucket_start.max(span.start_ms));
            if overlap == 0 {
                continue;
            }
            if span.kind == LogKind::Connected {
                connected_ms += overlap;
            } else {
                changes += 1;
            }
        }
        buckets.push(Bucket {
            start_ms: bucket_start,
            end_ms: bucket_end,
            uptime_pct: (connected_ms as f64 / total_ms as f64 * 100.0).clamp(0.0, 100.0),
            connected_ms,
            total_ms,
            changes,
        });
        start_ms += HOUR;
    }
    buckets
}

/// The Connection Logger pane.
#[derive(Default)]
pub struct ConnectionLogger {
    entries: Vec<LogEntry>,
    loaded: bool,
    /// When the current connection started, for the next duration.
    connected_since: Option<u64>,
    /// Which graph is on screen.
    graph: GraphMode,
    /// How much history the graph covers.
    window: TimeWindow,
    /// Whether the Clear button is asking for a second click.
    clear_confirm: bool,
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

        // The graph and the cards above it all read the same window of the log.
        if self.controls(ui, &palette, enabled) {
            self.clear();
            context.set_value(LOG_KEY, to_value(&self.entries));
        }
        let now = now_ms();
        let range = window_range(self.window, &self.entries, now);
        let spans = build_spans(&self.entries, range, context.status);
        let summary = summarise(&spans, range, &self.entries, now);

        ui.add_space(BLOCK_GAP);
        self.metrics(ui, &palette, context.status, &summary);
        ui.add_space(BLOCK_GAP);
        match self.graph {
            GraphMode::Timeline => {
                let height = graph_height(ui, TIMELINE_HEIGHT, TIMELINE_MIN_HEIGHT);
                timeline(ui, &palette, range, &spans, height);
            }
            GraphMode::Step => {
                let height = graph_height(ui, STEP_HEIGHT, STEP_MIN_HEIGHT);
                step_chart(ui, &palette, range, &spans, height);
            }
            GraphMode::Heatmap => {
                let height = graph_height(ui, HEAT_MAX_HEIGHT, HEAT_MIN_HEIGHT);
                heatmap(ui, &palette, range, &spans, height);
            }
        }
        ui.add_space(BLOCK_GAP);
        self.list(ui, &palette);
        // "Now" and the current duration move on their own.
        context.request_repaint_after(TICK);
    }
}

impl ConnectionLogger {
    /// The mode and window buttons, plus Clear. Returns true when the log should be wiped.
    fn controls(&mut self, ui: &mut Ui, palette: &Palette, enabled: bool) -> bool {
        let mut clear = false;
        ui.horizontal_wrapped(|ui| {
            for mode in GraphMode::ALL {
                let active = mode == self.graph;
                if Button::new(mode.label())
                    .compact()
                    .active(active)
                    .show(ui, palette)
                    .clicked()
                    && !active
                {
                    self.graph = mode;
                }
            }
            ui.add_space(BLOCK_GAP);
            for window in TimeWindow::ALL {
                let active = window == self.window;
                if Button::new(window.label())
                    .compact()
                    .active(active)
                    .show(ui, palette)
                    .clicked()
                    && !active
                {
                    self.window = window;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.clear_confirm {
                    if Button::new("No").compact().show(ui, palette).clicked() {
                        self.clear_confirm = false;
                    }
                    if Button::new("Yes")
                        .compact()
                        .danger()
                        .show(ui, palette)
                        .clicked()
                    {
                        self.clear_confirm = false;
                        clear = true;
                    }
                    ui.label(
                        egui::RichText::new("Clear?")
                            .size(11.0)
                            .color(to_color32(palette.danger)),
                    );
                } else if small_button(ui, palette, "Clear") {
                    self.clear_confirm = true;
                }
                if !enabled {
                    ui.label(
                        egui::RichText::new("logging paused")
                            .size(10.0)
                            .color(to_color32(palette.warn)),
                    );
                }
            });
        });
        clear
    }

    /// The five cards above the graph.
    fn metrics(&self, ui: &mut Ui, palette: &Palette, status: SessionStatus, summary: &Summary) {
        let state = status_colour(palette, status);
        let cards = [
            MetricCard {
                label: format!("uptime ({})", self.window.label()),
                value: format!("{:.1}%", summary.uptime_pct),
                colour: Some(to_color32(palette.ok)),
                dot: None,
            },
            MetricCard {
                label: "current state".to_owned(),
                value: format!(
                    "{} ({})",
                    status_label(status),
                    format_duration(Some(summary.current_duration_ms))
                ),
                colour: Some(state),
                dot: Some(state),
            },
            MetricCard {
                label: "disconnects".to_owned(),
                value: summary.disconnects.to_string(),
                colour: None,
                dot: None,
            },
            MetricCard {
                label: "total connected".to_owned(),
                value: format_duration(Some(summary.connected_ms)),
                colour: None,
                dot: None,
            },
            MetricCard {
                label: "avg session".to_owned(),
                value: format_duration(Some(summary.average_connected_ms)),
                colour: None,
                dot: None,
            },
        ];
        metric_grid(ui, palette, &cards);
    }

    /// The event log under the graph.
    fn list(&self, ui: &mut Ui, palette: &Palette) {
        let muted = to_color32(palette.text_muted);
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
                let time_font = egui::FontId::monospace(10.0);
                let kind_font = egui::FontId::monospace(11.0);
                let time_width = text_width(ui, LIST_TIME_SAMPLE, &time_font) + LIST_COLUMN_GAP;
                let kind_width = LogKind::ALL
                    .iter()
                    .map(|kind| text_width(ui, kind.label(), &kind_font))
                    .fold(0.0, f32::max)
                    + LIST_COLUMN_GAP;
                for entry in self.entries.iter().rev() {
                    let height = LIST_ROW_HEIGHT
                        + if entry.message.is_some() {
                            LIST_MESSAGE_HEIGHT
                        } else {
                            0.0
                        };
                    let (row, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::hover(),
                    );
                    let mid = row.top() + LIST_ROW_HEIGHT / 2.0;
                    let mut x = row.left();
                    for (text, font, colour, column) in [
                        (
                            time::format_millis(entry.at_ms),
                            time_font.clone(),
                            muted,
                            time_width,
                        ),
                        (
                            entry.kind.label().to_owned(),
                            kind_font.clone(),
                            kind_colour(palette, entry.kind),
                            kind_width,
                        ),
                    ] {
                        let galley = truncating_galley_with(ui, text, font, colour, column);
                        ui.painter().galley(
                            egui::pos2(x, mid - galley.size().y / 2.0),
                            galley,
                            colour,
                        );
                        x += column;
                    }
                    if entry.duration_ms.is_some() {
                        let text = format!("after {}", format_duration(entry.duration_ms));
                        let galley = truncating_galley(
                            ui,
                            text,
                            METRIC_LABEL_SIZE,
                            muted,
                            (row.right() - x).max(0.0),
                        );
                        ui.painter().galley(
                            egui::pos2(x, mid - galley.size().y / 2.0),
                            galley,
                            muted,
                        );
                    }
                    if let Some(message) = &entry.message {
                        let galley = truncating_galley(
                            ui,
                            message.clone(),
                            METRIC_LABEL_SIZE,
                            muted,
                            (row.right() - row.left() - time_width).max(0.0),
                        );
                        let y = row.top() + LIST_ROW_HEIGHT;
                        ui.painter().galley(
                            egui::pos2(row.left() + time_width, y),
                            galley,
                            muted,
                        );
                    }
                }
            });
    }
}

/// Widest timestamp the event list prints, so its columns never move.
const LIST_TIME_SAMPLE: &str = "00-00 00:00:00";

/// Width of `text` in `font`.
fn text_width(ui: &Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

/// Height of one line in `font`.
fn line_height(ui: &Ui, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap("X".to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .y
}

/// One card above the graph: a label, a value and how they are coloured.
struct MetricCard {
    label: String,
    value: String,
    colour: Option<egui::Color32>,
    /// A dot painted before the value, for the card that reports the live state.
    dot: Option<egui::Color32>,
}

/// Draw the cards in rows of equal width and height, wrapping only when the pane is too narrow
/// to give every card [`METRIC_MIN_WIDTH`].
///
/// Laid out by hand on purpose: a wrapped `horizontal` layout centres each child on the row's
/// cross axis, so cards of different heights step down the pane, and its item spacing is what
/// decides whether the last card fits, not [`BLOCK_GAP`].
fn metric_grid(ui: &mut Ui, palette: &Palette, cards: &[MetricCard]) {
    let gap = ui.spacing().item_spacing.x;
    let available = ui.available_width();
    let columns = (((available + gap) / (METRIC_MIN_WIDTH + gap)).floor() as usize)
        .clamp(1, cards.len().max(1));
    let width = ((available - gap * columns.saturating_sub(1) as f32) / columns as f32)
        .max(METRIC_MIN_WIDTH * 0.6);
    let label_font = egui::FontId::proportional(METRIC_LABEL_SIZE);
    let value_font = egui::FontId::monospace(METRIC_VALUE_SIZE);
    let height = METRIC_PAD_Y * 2.0
        + METRIC_LINE_GAP
        + line_height(ui, &label_font)
        + line_height(ui, &value_font);
    let rows = cards.len().div_ceil(columns).max(1);
    let total = rows as f32 * height + (rows - 1) as f32 * gap;
    let (grid, _) = ui.allocate_exact_size(egui::vec2(available, total), egui::Sense::hover());
    for (index, card) in cards.iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                grid.left() + (index % columns) as f32 * (width + gap),
                grid.top() + (index / columns) as f32 * (height + gap),
            ),
            egui::vec2(width, height),
        );
        metric_card(ui, palette, rect, card, &label_font, &value_font);
    }
}

/// Paint one metric card: its label above, its value in the monospace face, both cut to the
/// card so a wrapper never makes it taller than its neighbours.
fn metric_card(
    ui: &Ui,
    palette: &Palette,
    rect: egui::Rect,
    card: &MetricCard,
    label_font: &egui::FontId,
    value_font: &egui::FontId,
) {
    let muted = to_color32(palette.text_muted);
    let painter = ui.painter();
    painter.rect_filled(rect, SURFACE_RADIUS, to_color32(palette.bg_input));
    painter.rect_stroke(
        rect,
        SURFACE_RADIUS,
        egui::Stroke::new(1.0, faded(palette.border, METRIC_BORDER)),
        egui::StrokeKind::Inside,
    );
    let room = (rect.width() - METRIC_PAD_X * 2.0).max(0.0);
    let label = truncating_galley_with(
        ui,
        card.label.to_uppercase(),
        label_font.clone(),
        muted,
        room,
    );
    let mut y = rect.top() + METRIC_PAD_Y;
    ui.painter()
        .galley(egui::pos2(rect.left() + METRIC_PAD_X, y), label.clone(), muted);
    y += label.size().y + METRIC_LINE_GAP;

    let colour = card.colour.unwrap_or(to_color32(palette.text));
    let mut x = rect.left() + METRIC_PAD_X;
    if let Some(dot) = card.dot {
        let value_height = line_height(ui, value_font);
        ui.painter().circle_filled(
            egui::pos2(x + METRIC_DOT / 2.0, y + value_height / 2.0),
            METRIC_DOT / 2.0,
            dot,
        );
        x += METRIC_DOT + METRIC_DOT_GAP;
    }
    let value = truncating_galley_with(
        ui,
        card.value.clone(),
        value_font.clone(),
        colour,
        (rect.right() - METRIC_PAD_X - x).max(0.0),
    );
    ui.painter().galley(egui::pos2(x, y), value, colour);
}

/// The status in one word, as the current-state card shows it.
fn status_label(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "IDLE",
        SessionStatus::Connecting => "CONNECTING",
        SessionStatus::AwaitingHostKey => "VERIFYING",
        SessionStatus::Authenticating => "AUTHENTICATING",
        SessionStatus::Connected => "CONNECTED",
        SessionStatus::Reconnecting => "RECONNECTING",
        SessionStatus::Disconnected | SessionStatus::Closed => "DISCONNECTED",
        SessionStatus::Failed => "FAILED",
    }
}

/// Colour the current state is reported in.
fn status_colour(palette: &Palette, status: SessionStatus) -> egui::Color32 {
    match status {
        SessionStatus::Connected => to_color32(palette.ok),
        SessionStatus::Reconnecting | SessionStatus::Connecting => to_color32(palette.warn),
        SessionStatus::Failed => to_color32(palette.danger),
        _ => to_color32(palette.text_muted),
    }
}

/// Colour a span is painted with.
fn kind_colour(palette: &Palette, kind: LogKind) -> egui::Color32 {
    match kind {
        LogKind::Connected => to_color32(palette.ok),
        LogKind::Reconnecting => to_color32(palette.warn),
        LogKind::Disconnected => to_color32(palette.danger),
        // A failure reads as the same red, a shade deeper.
        LogKind::Failed => to_color32(palette.danger).gamma_multiply(0.8),
    }
}

/// Shade for an hour's uptime: the reference's five steps, red through green.
fn heat_colour(palette: &Palette, uptime_pct: f64) -> egui::Color32 {
    let ok = to_color32(palette.ok);
    if uptime_pct >= 100.0 {
        ok
    } else if uptime_pct >= 85.0 {
        ok.gamma_multiply(0.85)
    } else if uptime_pct >= 50.0 {
        to_color32(palette.warn)
    } else if uptime_pct > 0.0 {
        to_color32(palette.danger).gamma_multiply(1.2)
    } else {
        to_color32(palette.danger)
    }
}

/// `HH:MM` in UTC, for the axis under a graph.
fn short_time(ms: u64) -> String {
    let seconds_of_day = ms / 1000 % 86_400;
    format!(
        "{:02}:{:02}",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60
    )
}

/// A tick label: `HH:MM`, prefixed with the day when the window spans more than one.
fn tick_time(range: Range, at_ms: u64) -> String {
    if range.duration_ms() > DAY_MS {
        let (_, _, day) = time::civil_from_days((at_ms / DAY_MS) as i64);
        format!("{day:02} {}", short_time(at_ms))
    } else {
        short_time(at_ms)
    }
}

/// Room a graph gets: its full height in a tall pane, shrunk to `min` (never below) so the
/// event list under it keeps whatever is left.
fn graph_height(ui: &Ui, full: f32, min: f32) -> f32 {
    (ui.available_height() - AXIS_HEIGHT).clamp(min, full)
}

/// The axis under a graph: time labels from the window's start to "Now", as many as fit.
fn time_axis(ui: &mut Ui, palette: &Palette, range: Range) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), AXIS_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let muted = to_color32(palette.text_muted);
    let font = egui::FontId::monospace(10.0);
    let ticks = ((rect.width() / TICK_ROOM) as usize).clamp(2, 7);
    for index in 0..ticks {
        let fraction = index as f64 / (ticks - 1) as f64;
        let at_ms = range.start_ms + (range.duration_ms() as f64 * fraction) as u64;
        let (align, text) = if index == 0 {
            (egui::Align2::LEFT_TOP, tick_time(range, range.start_ms))
        } else if index + 1 == ticks {
            (egui::Align2::RIGHT_TOP, format!("Now ({})", short_time(at_ms)))
        } else {
            (egui::Align2::CENTER_TOP, tick_time(range, at_ms))
        };
        painter.text(
            egui::pos2(rect.left() + fraction as f32 * rect.width(), rect.top()),
            align,
            text,
            font.clone(),
            muted,
        );
    }
}

/// The timeline bar: one strip, each state in its own colour.
fn timeline(ui: &mut Ui, palette: &Palette, range: Range, spans: &[Span], height: f32) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, SURFACE_RADIUS, to_color32(palette.bg_input));
    let inner = painter.with_clip_rect(rect.shrink(1.0));
    for span in spans {
        let left = rect.left() + range.fraction(span.start_ms) * rect.width();
        let right = rect.left() + range.fraction(span.end_ms) * rect.width();
        let segment = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2((right - left).max(MIN_SEGMENT_PX), rect.height()),
        );
        inner.rect_filled(segment, 0, kind_colour(palette, span.kind));
    }
    painter.rect_stroke(
        rect,
        SURFACE_RADIUS,
        egui::Stroke::new(1.0, to_color32(palette.border)),
        egui::StrokeKind::Inside,
    );
    time_axis(ui, palette, range);

    let Some(position) = response.hover_pos() else {
        return;
    };
    let fraction = ((position.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let at_ms = range.start_ms + (fraction as f64 * range.duration_ms() as f64) as u64;
    let Some(span) = spans
        .iter()
        .find(|span| at_ms >= span.start_ms && at_ms < span.end_ms)
    else {
        return;
    };
    tooltip(
        ui,
        palette,
        rect,
        &span.kind.label().to_uppercase(),
        &format!(
            "{} \u{2192} {} ({})",
            tick_time(range, span.start_ms),
            short_time(span.end_ms),
            format_duration(Some(span.duration_ms()))
        ),
        span.message.as_deref(),
    );
}

/// The step chart: the state as a line that steps between up and down, filled underneath.
fn step_chart(ui: &mut Ui, palette: &Palette, range: Range, spans: &[Span], height: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let border = to_color32(palette.border);
    let ok = to_color32(palette.ok);
    painter.rect_filled(rect, SURFACE_RADIUS, to_color32(palette.bg_input));
    let padding = STEP_PADDING.min((height - STEP_WIDTH) / 2.0).max(2.0);
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, rect.top() + padding),
        egui::pos2(rect.right() - 1.0, rect.bottom() - padding),
    );
    // The two levels: up (dashed) and down (solid), as the reference draws them.
    painter.extend(egui::Shape::dashed_line(
        &[plot.left_top(), plot.right_top()],
        egui::Stroke::new(1.0, border),
        DASH,
        DASH_GAP,
    ));
    painter.hline(plot.x_range(), plot.bottom(), egui::Stroke::new(1.0, border));

    let mut points = Vec::with_capacity(spans.len() * 2);
    for span in spans {
        let y = if span.kind == LogKind::Connected {
            plot.top()
        } else {
            plot.bottom()
        };
        points.push(egui::pos2(
            rect.left() + range.fraction(span.start_ms) * rect.width(),
            y,
        ));
        points.push(egui::pos2(
            rect.left() + range.fraction(span.end_ms) * rect.width(),
            y,
        ));
    }
    if points.is_empty() {
        points.push(plot.left_bottom());
        points.push(plot.right_bottom());
    }
    let plot_painter = painter.with_clip_rect(plot);
    plot_painter.add(area_mesh(&points, plot.bottom(), ok));
    plot_painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(STEP_WIDTH, ok),
    ));
    painter.rect_stroke(
        rect,
        SURFACE_RADIUS,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    time_axis(ui, palette, range);
}

/// The area under a step line, fading from the line down to its baseline.
fn area_mesh(points: &[egui::Pos2], baseline: f32, colour: egui::Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    let top = with_alpha(colour, AREA_TOP_ALPHA);
    let bottom = with_alpha(colour, AREA_BOTTOM_ALPHA);
    for pair in points.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        if (right.x - left.x).abs() < 0.5 {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [
            (egui::pos2(left.x, left.y), top),
            (egui::pos2(right.x, right.y), top),
            (egui::pos2(right.x, baseline), bottom),
            (egui::pos2(left.x, baseline), bottom),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: egui::epaint::WHITE_UV,
                color,
            });
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    egui::Shape::mesh(mesh)
}

/// `colour` at `alpha`.
fn with_alpha(colour: egui::Color32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        colour.r(),
        colour.g(),
        colour.b(),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The heatmap: one cell per hour, shaded by how much of it the session was up.
fn heatmap(ui: &mut Ui, palette: &Palette, range: Range, spans: &[Span], max_height: f32) {
    let buckets = hourly_buckets(range, spans);
    let width = ui.available_width();
    let columns = (((width + HEAT_GAP) / (HEAT_CELL + HEAT_GAP)).floor() as usize).max(1);
    let cell_width =
        ((width - HEAT_GAP * (columns - 1) as f32) / columns as f32).max(HEAT_CELL * 0.5);
    let rows = buckets.len().div_ceil(columns).max(1);
    let height = rows as f32 * HEAT_CELL + (rows - 1) as f32 * HEAT_GAP;
    let (grid, hovered) = egui::ScrollArea::vertical()
        .id_salt("conn-log-heatmap")
        .max_height(max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(width, height),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            let mut hovered = None;
            for (index, bucket) in buckets.iter().enumerate() {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + (index % columns) as f32 * (cell_width + HEAT_GAP),
                        rect.top() + (index / columns) as f32 * (HEAT_CELL + HEAT_GAP),
                    ),
                    egui::vec2(cell_width, HEAT_CELL),
                );
                painter.rect_filled(
                    cell,
                    HEAT_RADIUS,
                    heat_colour(palette, bucket.uptime_pct),
                );
                if let Some(position) = response.hover_pos()
                    && cell.contains(position)
                {
                    hovered = Some(*bucket);
                }
            }
            (rect, hovered)
        })
        .inner;
    heat_axis(ui, palette, range);

    let Some(bucket) = hovered else {
        return;
    };
    let mut lines = format!(
        "{} \u{2192} {}",
        tick_time(range, bucket.start_ms),
        short_time(bucket.end_ms)
    );
    lines.push('\n');
    lines.push_str(&format!(
        "Uptime: {:.1}% ({} / {})",
        bucket.uptime_pct,
        format_duration(Some(bucket.connected_ms)),
        format_duration(Some(bucket.total_ms))
    ));
    if bucket.changes > 0 {
        lines.push_str(&format!(
            "\n{} state change{}",
            bucket.changes,
            if bucket.changes == 1 { "" } else { "s" }
        ));
    }
    tooltip(ui, palette, grid, "HOUR", &lines, None);
}

/// The line under the heatmap: the window's start, the colour key, and "Now".
fn heat_axis(ui: &mut Ui, palette: &Palette, range: Range) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), AXIS_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let muted = to_color32(palette.text_muted);
    let font = egui::FontId::monospace(10.0);
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        tick_time(range, range.start_ms),
        font.clone(),
        muted,
    );
    painter.text(
        rect.right_top(),
        egui::Align2::RIGHT_TOP,
        format!("Now ({})", short_time(range.end_ms)),
        font.clone(),
        muted,
    );
    let low = painter.layout_no_wrap("0%".to_owned(), font.clone(), muted);
    let high = painter.layout_no_wrap("100%".to_owned(), font.clone(), muted);
    let samples: Vec<egui::Color32> = [0.0, 25.0, 60.0, 90.0, 100.0]
        .iter()
        .map(|pct| heat_colour(palette, *pct))
        .collect();
    let key_width = low.size().x
        + high.size().x
        + samples.len() as f32 * HEAT_SAMPLE
        + (samples.len() - 1) as f32 * HEAT_GAP
        + HEAT_GAP * 2.0;
    let mut x = rect.center().x - key_width / 2.0;
    painter.galley(egui::pos2(x, rect.top()), low.clone(), muted);
    x += low.size().x + HEAT_GAP;
    for colour in &samples {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + (AXIS_HEIGHT - HEAT_SAMPLE) / 2.0),
                egui::Vec2::splat(HEAT_SAMPLE),
            ),
            HEAT_RADIUS,
            *colour,
        );
        x += HEAT_SAMPLE + HEAT_GAP;
    }
    painter.galley(egui::pos2(x, rect.top()), high, muted);
}

/// A small box over a graph, like the reference's hover tooltip.
fn tooltip(
    ui: &Ui,
    palette: &Palette,
    anchor: egui::Rect,
    title: &str,
    body: &str,
    note: Option<&str>,
) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect());
    let title = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(11.0),
        to_color32(palette.accent),
    );
    let body = painter.layout_no_wrap(
        body.to_owned(),
        egui::FontId::proportional(11.0),
        to_color32(palette.text),
    );
    let note = note.map(|note| {
        painter.layout_no_wrap(
            note.to_owned(),
            egui::FontId::proportional(11.0),
            to_color32(palette.text_muted),
        )
    });
    let widths = std::iter::once(&title).chain([&body]).chain(note.iter());
    let width = widths.map(|galley| galley.size().x).fold(0.0, f32::max) + TOOLTIP_PAD * 2.0;
    let mut height = title.size().y + body.size().y + TOOLTIP_PAD * 2.0 + TOOLTIP_GAP;
    if let Some(note) = &note {
        height += note.size().y + TOOLTIP_GAP;
    }
    let box_rect = egui::Rect::from_min_size(
        egui::pos2(
            anchor.right() - TOOLTIP_INSET - width,
            anchor.top() + TOOLTIP_INSET,
        ),
        egui::vec2(width, height),
    );
    painter.rect_filled(box_rect, SURFACE_RADIUS, to_color32(palette.bg_panel));
    painter.rect_stroke(
        box_rect,
        SURFACE_RADIUS,
        egui::Stroke::new(1.0, to_color32(palette.accent)),
        egui::StrokeKind::Inside,
    );
    let mut y = box_rect.top() + TOOLTIP_PAD;
    for galley in std::iter::once(title).chain([body]).chain(note) {
        let size = galley.size();
        painter.galley(egui::pos2(box_rect.left() + TOOLTIP_PAD, y), galley, to_color32(palette.text));
        y += size.y + TOOLTIP_GAP;
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

    fn entry(at_ms: u64, kind: LogKind) -> LogEntry {
        LogEntry {
            at_ms,
            kind,
            message: None,
            duration_ms: None,
        }
    }

    fn span(start_ms: u64, end_ms: u64, kind: LogKind) -> Span {
        Span {
            start_ms,
            end_ms,
            kind,
            message: None,
        }
    }

    #[test]
    fn each_window_covers_its_own_stretch() {
        let now = 10_000_000_000u64;
        assert_eq!(
            window_range(TimeWindow::Hour, &[], now),
            Range {
                start_ms: now - 3_600_000,
                end_ms: now
            }
        );
        assert_eq!(
            window_range(TimeWindow::Week, &[], now).duration_ms(),
            7 * 24 * 3_600_000
        );
        // "All" starts a minute before the oldest entry, so its first span has room.
        let entries = [entry(now - 7_200_000, LogKind::Connected)];
        assert_eq!(
            window_range(TimeWindow::All, &entries, now).start_ms,
            now - 7_200_000 - 60_000
        );
        // A host with nothing logged still gets an hour to draw.
        assert_eq!(
            window_range(TimeWindow::All, &[], now).duration_ms(),
            3_600_000
        );
    }

    #[test]
    fn spans_run_from_each_event_to_the_next() {
        let now = 100_000;
        let range = Range {
            start_ms: 0,
            end_ms: now,
        };
        let entries = [
            entry(10_000, LogKind::Connected),
            entry(40_000, LogKind::Disconnected),
        ];
        let spans = build_spans(&entries, range, SessionStatus::Disconnected);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0], span(10_000, 40_000, LogKind::Connected));
        assert_eq!(spans[1], span(40_000, now, LogKind::Disconnected));

        // An event before the window is clipped to its left edge, not dropped.
        let narrow = Range {
            start_ms: 20_000,
            end_ms: now,
        };
        let spans = build_spans(&entries, narrow, SessionStatus::Disconnected);
        assert_eq!(spans[0].start_ms, 20_000);
        assert_eq!(spans[0].duration_ms(), 20_000);

        // An empty log shows the session's current state across the window.
        let spans = build_spans(&[], narrow, SessionStatus::Connected);
        assert_eq!(spans, [span(20_000, now, LogKind::Connected)]);
    }

    #[test]
    fn the_summary_counts_time_up_and_drops() {
        let range = Range {
            start_ms: 0,
            end_ms: 100_000,
        };
        let spans = [
            span(0, 60_000, LogKind::Connected),
            span(60_000, 70_000, LogKind::Reconnecting),
            span(70_000, 90_000, LogKind::Connected),
            span(90_000, 100_000, LogKind::Failed),
        ];
        let entries = [entry(90_000, LogKind::Failed)];
        let summary = summarise(&spans, range, &entries, 130_000);
        assert_eq!(summary.connected_ms, 80_000);
        assert_eq!(summary.disconnected_ms, 20_000);
        assert_eq!(summary.disconnects, 1, "a reconnect is not a disconnect");
        assert_eq!(summary.average_connected_ms, 40_000);
        assert!((summary.uptime_pct - 80.0).abs() < 0.01);
        assert_eq!(summary.current_duration_ms, 40_000);
    }

    #[test]
    fn the_heatmap_buckets_by_hour() {
        const HOUR: u64 = 3_600_000;
        let range = Range {
            start_ms: 0,
            end_ms: 2 * HOUR + HOUR / 2,
        };
        let spans = [
            span(0, HOUR, LogKind::Connected),
            span(HOUR, range.end_ms, LogKind::Disconnected),
        ];
        let buckets = hourly_buckets(range, &spans);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].uptime_pct, 100.0);
        assert_eq!(buckets[0].connected_ms, HOUR);
        assert_eq!(buckets[1].uptime_pct, 0.0);
        assert_eq!(buckets[1].changes, 1);
        // The last bucket is cut off by the end of the window.
        assert_eq!(buckets[2].total_ms, HOUR / 2);
        assert_eq!(buckets[2].uptime_pct, 0.0);
    }

    #[test]
    fn short_times_read_as_hours_and_minutes() {
        assert_eq!(short_time(0), "00:00");
        assert_eq!(short_time(86_400_000 + 5_400_000), "01:30");
        assert_eq!(short_time(3_600_000 + 3_599_000), "01:59");
    }

    #[test]
    fn the_heatmap_shades_from_red_to_green() {
        let palette = Palette::dark();
        assert_eq!(heat_colour(&palette, 0.0), to_color32(palette.danger));
        assert_eq!(heat_colour(&palette, 60.0), to_color32(palette.warn));
        assert_eq!(heat_colour(&palette, 100.0), to_color32(palette.ok));
        let shades: Vec<_> = [0.0, 25.0, 60.0, 90.0, 100.0]
            .iter()
            .map(|pct| heat_colour(&palette, *pct))
            .collect();
        assert_eq!(shades.len(), 5);
        assert!(
            shades[0] != shades[1] && shades[3] != shades[4],
            "every step is visible"
        );
    }

    /// Paint the pane and return its filled rects and painted texts.
    fn paint(
        logger: &mut ConnectionLogger,
        status: SessionStatus,
    ) -> (Vec<(egui::Rect, egui::Color32)>, Vec<String>) {
        paint_at(logger, status, egui::vec2(600.0, 900.0))
    }

    /// [`paint`] in a viewport of `size`, for the geometry of a docked pane.
    fn paint_at(
        logger: &mut ConnectionLogger,
        status: SessionStatus,
        size: egui::Vec2,
    ) -> (Vec<(egui::Rect, egui::Color32)>, Vec<String>) {
        let ctx = egui::Context::default();
        let mut settings = serde_json::Map::new();
        let secrets = std::collections::BTreeMap::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut fills = Vec::new();
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                let mut context = PluginContext {
                    palette: Palette::dark(),
                    host_label: "dev".to_owned(),
                    environment: "a test shell".to_owned(),
                    status,
                    settings: &mut settings,
                    scope: "host-1".to_owned(),
                    session: None,
                    events: &[],
                    pending_input: Vec::new(),
                    repaint_after: None,
                    secrets: &secrets,
                    secret_writes: Vec::new(),
                };
                logger.show(ui, &mut context);
            });
            fills.clear();
            texts.clear();
            for clipped in std::mem::take(&mut output.shapes) {
                match clipped.shape {
                    egui::Shape::Rect(rect) => fills.push((rect.rect, rect.fill)),
                    egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                    _ => {}
                }
            }
            output.drop_without_applying_deltas();
        }
        (fills, texts)
    }

    /// A log that was up an hour ago and dropped a minute ago.
    fn recent_logger() -> ConnectionLogger {
        let mut logger = ConnectionLogger::default();
        let now = now_ms();
        logger.entries = vec![
            entry(now - 3_600_000, LogKind::Connected),
            entry(now - 60_000, LogKind::Disconnected),
        ];
        logger.loaded = true;
        logger
    }

    #[test]
    fn the_pane_draws_the_controls_cards_and_timeline() {
        let mut logger = recent_logger();
        let (fills, texts) = paint(&mut logger, SessionStatus::Disconnected);
        for label in [
            "Timeline Bar",
            "Step Chart",
            "Uptime Heatmap",
            "1H",
            "6H",
            "24H",
            "7D",
            "ALL",
            "Clear",
            "UPTIME (24H)",
            "CURRENT STATE",
            "DISCONNECTS",
            "TOTAL CONNECTED",
            "AVG SESSION",
        ] {
            assert!(texts.contains(&label.to_owned()), "{label:?} missing: {texts:?}");
        }
        // Both states are on the strip.
        let palette = Palette::dark();
        let strip = |colour: egui::Color32| {
            fills
                .iter()
                .any(|(rect, fill)| *fill == colour && (rect.height() - TIMELINE_HEIGHT).abs() < 0.5)
        };
        assert!(strip(to_color32(palette.ok)), "the time it was up");
        assert!(strip(to_color32(palette.danger)), "and the time it was down");
    }

    #[test]
    fn the_step_and_heatmap_modes_replace_the_timeline() {
        let mut logger = recent_logger();
        logger.graph = GraphMode::Heatmap;
        let (fills, _) = paint(&mut logger, SessionStatus::Disconnected);
        let cells = fills
            .iter()
            .filter(|(rect, _)| (rect.height() - HEAT_CELL).abs() < 0.5)
            .count();
        assert!(cells >= 24, "a day of hourly cells, got {cells}");
        assert!(
            !fills
                .iter()
                .any(|(rect, _)| (rect.height() - TIMELINE_HEIGHT).abs() < 0.5),
            "the timeline is gone"
        );

        let mut logger = recent_logger();
        logger.graph = GraphMode::Step;
        let (fills, _) = paint(&mut logger, SessionStatus::Disconnected);
        assert!(
            fills
                .iter()
                .any(|(rect, _)| (rect.height() - STEP_HEIGHT).abs() < 0.5),
            "the step chart's box"
        );
    }

    #[test]
    fn a_low_wide_dock_keeps_the_cards_on_one_row_and_the_graph_on_screen() {
        let mut logger = recent_logger();
        const PANE: egui::Vec2 = egui::Vec2::new(1400.0, 132.0);
        let (fills, texts) = paint_at(&mut logger, SessionStatus::Disconnected, PANE);
        let palette = Palette::dark();
        let cards: Vec<egui::Rect> = fills
            .iter()
            .filter(|(rect, fill)| {
                *fill == to_color32(palette.bg_input) && rect.height() > TIMELINE_HEIGHT
            })
            .map(|(rect, _)| *rect)
            .collect();
        assert_eq!(cards.len(), 5, "one card each: {cards:?}");
        let top = cards[0].top();
        assert!(
            cards.iter().all(|card| (card.top() - top).abs() < 0.5),
            "the cards share one row: {cards:?}"
        );
        assert!(texts.contains(&"AVG SESSION".to_owned()), "{texts:?}");

        let strip = fills.iter().find(|(rect, fill)| {
            *fill == to_color32(palette.ok) && (rect.height() - TIMELINE_HEIGHT).abs() < 0.5
        });
        let (strip, _) = strip.expect("the timeline is drawn");
        assert!(
            strip.top() >= 0.0 && strip.bottom() <= PANE.y,
            "the graph sits inside the pane: {strip:?}"
        );

        // The step chart is the tallest graph, so it is the one that has to give way.
        let mut logger = recent_logger();
        logger.graph = GraphMode::Step;
        let (fills, _) = paint_at(&mut logger, SessionStatus::Disconnected, PANE);
        let box_ = fills
            .iter()
            .map(|(rect, _)| *rect)
            .find(|rect| {
                rect.height() >= STEP_MIN_HEIGHT && rect.height() <= STEP_HEIGHT
                    && rect.width() > PANE.x * 0.9
            })
            .expect("the step chart is drawn");
        assert!(
            box_.bottom() <= PANE.y,
            "the step chart shrinks to the pane: {box_:?}"
        );
    }
}
