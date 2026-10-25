//! Transient status toasts.
//!
//! Information and error strips that disappear after five seconds, stacked in the
//! bottom-right corner above the terminal.

use std::time::{Duration, Instant};

use egui::{Align2, Stroke, Ui, Vec2};
use wassh_core::Palette;

use super::surface::Tone;
use crate::terminal_view::to_color32;

/// How long a toast stays on screen.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(5);
/// Minimum width of a toast.
pub const MIN_WIDTH: f32 = 240.0;
/// Maximum width of a toast.
pub const MAX_WIDTH: f32 = 420.0;
/// Margin from the window edges.
pub const MARGIN: f32 = 16.0;

/// Severity of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Neutral confirmation.
    Info,
    /// Something went wrong.
    Error,
}

impl ToastKind {
    fn tone(self) -> Tone {
        match self {
            Self::Info => Tone::Info,
            Self::Error => Tone::Danger,
        }
    }
}

/// One live toast.
#[derive(Clone, Debug)]
pub struct Toast {
    /// Severity, controlling the accent bar.
    pub kind: ToastKind,
    /// Message text.
    pub text: String,
    /// When the toast was raised.
    pub created: Instant,
}

impl Toast {
    /// Whether this toast has outlived [`TOAST_LIFETIME`] at `now`.
    pub fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) >= TOAST_LIFETIME
    }

    /// Progress from 1.0 (just shown) to 0.0 (expired), for a fade-out.
    pub fn opacity(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.created).as_secs_f32();
        let total = TOAST_LIFETIME.as_secs_f32();
        ((total - elapsed) / total).clamp(0.0, 1.0)
    }
}

/// The toast stack.
#[derive(Default)]
pub struct Toasts {
    entries: Vec<Toast>,
}

impl Toasts {
    /// An empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Raise a toast.
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.entries.push(Toast {
            kind,
            text: text.into(),
            created: Instant::now(),
        });
    }

    /// Drop expired toasts at `now`.
    pub fn retain_live(&mut self, now: Instant) {
        self.entries.retain(|toast| !toast.is_expired(now));
    }

    /// Number of live toasts.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget all toasts.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Draw the stack; returns true while any toast is still animating.
    pub fn show(&mut self, ui: &mut Ui, palette: &Palette) -> bool {
        let now = Instant::now();
        self.retain_live(now);
        if self.entries.is_empty() {
            return false;
        }

        egui::Area::new(egui::Id::unique("wassh-toasts"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-MARGIN, -MARGIN))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(MAX_WIDTH);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    for toast in self.entries.iter().rev() {
                        let tone = toast.kind.tone().color(palette);
                        let frame = egui::Frame::NONE
                            .fill(to_color32(palette.bg_panel))
                            .stroke(Stroke::new(1.0, to_color32(palette.border)))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(12, 9));
                        frame.show(ui, |ui| {
                            ui.set_width(MIN_WIDTH);
                            ui.horizontal(|ui| {
                                let height = ui.available_height().max(16.0);
                                let (bar, _) = ui.allocate_exact_size(
                                    Vec2::new(3.0, height),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(bar, 0.0, tone);
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(&toast.text)
                                        .size(12.0)
                                        .color(to_color32(palette.text)),
                                );
                            });
                        });
                        ui.add_space(8.0);
                    }
                });
            });

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toasts_expire_after_five_seconds() {
        let toast = Toast {
            kind: ToastKind::Info,
            text: "Saved".to_owned(),
            created: Instant::now(),
        };
        assert!(!toast.is_expired(toast.created + Duration::from_millis(4_900)));
        assert!(toast.is_expired(toast.created + TOAST_LIFETIME));
    }

    #[test]
    fn opacity_fades_to_zero() {
        let toast = Toast {
            kind: ToastKind::Error,
            text: "Boom".to_owned(),
            created: Instant::now(),
        };
        assert!((toast.opacity(toast.created) - 1.0).abs() < f32::EPSILON);
        let half = toast.created + TOAST_LIFETIME / 2;
        assert!((toast.opacity(half) - 0.5).abs() < 0.02);
        assert_eq!(toast.opacity(toast.created + TOAST_LIFETIME * 2), 0.0);
    }

    #[test]
    fn retain_live_drops_old_entries() {
        let mut toasts = Toasts::new();
        toasts.push(ToastKind::Info, "one");
        assert_eq!(toasts.len(), 1);
        toasts.retain_live(Instant::now() + TOAST_LIFETIME + Duration::from_secs(1));
        assert!(toasts.is_empty());
    }
}
