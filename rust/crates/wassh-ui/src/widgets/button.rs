//! Buttons.
//!
//! Custom-drawn so hover, border and padding follow the design tokens exactly
//! (radius 4, padding 6×10, accent border on hover).

use egui::{Align2, Color32, FontId, Response, Sense, Stroke, Ui, Vec2};
use wassh_core::{Palette, RADIUS};

use crate::terminal_view::to_color32;

/// Visual weight of a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// Neutral surface with a border.
    #[default]
    Secondary,
    /// Accent fill; the primary action in a view.
    Primary,
    /// Danger border and text; used for destructive actions.
    Danger,
    /// No fill until hovered.
    Ghost,
}

/// Builder for one button.
#[derive(Clone, Debug)]
pub struct Button<'a> {
    label: &'a str,
    kind: ButtonKind,
    compact: bool,
    enabled: bool,
    /// Drawn as toggled on: accent border and label, no extra fill.
    active: bool,
    min_width: Option<f32>,
}

impl<'a> Button<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            kind: ButtonKind::Secondary,
            compact: false,
            enabled: true,
            active: false,
            min_width: None,
        }
    }

    /// Draw the button as toggled on (a toggle such as "attach the terminal").
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn primary(self) -> Self {
        self.kind(ButtonKind::Primary)
    }

    pub fn danger(self) -> Self {
        self.kind(ButtonKind::Danger)
    }

    pub fn ghost(self) -> Self {
        self.kind(ButtonKind::Ghost)
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
        self
    }

    pub fn show(self, ui: &mut Ui, palette: &Palette) -> Response {
        let font = FontId::proportional(self.font_size());
        let text_width = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x;
        let size = button_size(text_width, self.compact, self.min_width);

        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);

        let hovered = response.hovered() && self.enabled;
        let (fill, border, text) = self.colors(palette, hovered);
        let painter = ui.painter();
        if fill != Color32::TRANSPARENT {
            painter.rect_filled(rect, RADIUS, fill);
        }
        if border != Color32::TRANSPARENT {
            painter.rect_stroke(
                rect,
                RADIUS,
                Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            self.label,
            font,
            text,
        );
        response
    }

    fn font_size(&self) -> f32 {
        if self.compact { 12.0 } else { 13.0 }
    }

    fn colors(
        &self,
        palette: &Palette,
        hovered: bool,
    ) -> (Color32, Color32, Color32) {
        let accent = to_color32(palette.accent);
        let border = to_color32(palette.border);
        let text = to_color32(palette.text);
        let muted = to_color32(palette.text_muted);
        let transparent = Color32::TRANSPARENT;

        if !self.enabled {
            let (fill, border) = match self.kind {
                ButtonKind::Primary => (to_color32(palette.accent).gamma_multiply(0.45), transparent),
                ButtonKind::Secondary => (to_color32(palette.bg_panel_2), border),
                ButtonKind::Danger | ButtonKind::Ghost => (transparent, transparent),
            };
            return (fill, border, muted.gamma_multiply(0.6));
        }

        // A toggle keeps its surface and takes the accent for its border and label, so it is
        // the same shape as its neighbours whether it is on or off.
        if self.active && self.kind == ButtonKind::Secondary {
            return (
                if hovered {
                    to_color32(palette.bg_panel_2).gamma_multiply(1.15)
                } else {
                    to_color32(palette.bg_panel_2)
                },
                if hovered { accent } else { to_color32(palette.accent) },
                if hovered { to_color32(palette.accent_hover) } else { accent },
            );
        }

        match self.kind {
            ButtonKind::Primary => (
                if hovered {
                    to_color32(palette.accent_hover)
                } else {
                    accent
                },
                if hovered {
                    to_color32(palette.accent_hover)
                } else {
                    accent
                },
                Color32::WHITE,
            ),
            ButtonKind::Secondary => (
                if hovered {
                    to_color32(palette.bg_panel_2).gamma_multiply(1.15)
                } else {
                    to_color32(palette.bg_panel_2)
                },
                if hovered { accent } else { border },
                text,
            ),
            ButtonKind::Danger => (
                if hovered {
                    to_color32(palette.danger).gamma_multiply(0.25)
                } else {
                    transparent
                },
                to_color32(palette.danger),
                to_color32(palette.danger),
            ),
            ButtonKind::Ghost => (
                if hovered {
                    to_color32(palette.bg_panel_2)
                } else {
                    transparent
                },
                transparent,
                if hovered { text } else { muted },
            ),
        }
    }
}

/// The small action button used inside panes, rows and dialogs.
///
/// Every panel uses this (or [`Button`]) instead of drawing its own, so the whole
/// program shares one button shape.
pub fn small_button(ui: &mut Ui, palette: &Palette, label: &str) -> bool {
    Button::new(label).compact().show(ui, palette).clicked()
}

/// [`small_button`], greyed out and inert while `enabled` is false.
pub fn small_button_enabled(ui: &mut Ui, palette: &Palette, label: &str, enabled: bool) -> bool {
    Button::new(label)
        .compact()
        .enabled(enabled)
        .show(ui, palette)
        .clicked()
}

/// A small pill used for view filters; returns true when clicked. `active`
/// shows the accent tint.
pub fn chip(ui: &mut Ui, palette: &Palette, label: &str, active: bool) -> bool {
    let font = FontId::proportional(11.0);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let size = Vec2::new(text_width + 20.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let (fill, border, text) = if active {
        (
            to_color32(palette.accent).gamma_multiply(0.22),
            to_color32(palette.accent),
            to_color32(palette.text),
        )
    } else if response.hovered() {
        (
            to_color32(palette.bg_panel_2),
            to_color32(palette.border),
            to_color32(palette.text),
        )
    } else {
        (
            to_color32(palette.bg_input),
            to_color32(palette.border),
            to_color32(palette.text_muted),
        )
    };
    ui.painter().rect_filled(rect, 11.0, fill);
    ui.painter()
        .rect_stroke(rect, 11.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    response.clicked()
}

/// Convenience wrapper for the common secondary button.
pub fn button(ui: &mut Ui, palette: &Palette, label: &str) -> Response {
    Button::new(label).show(ui, palette)
}

/// Size of a button for a measured label width.
pub fn button_size(text_width: f32, compact: bool, min_width: Option<f32>) -> Vec2 {
    let padding = if compact {
        Vec2::new(10.0, 2.0)
    } else {
        Vec2::new(10.0, 6.0)
    };
    let line_height = if compact { 16.0 } else { 18.0 };
    Vec2::new(
        (text_width + padding.x * 2.0).max(min_width.unwrap_or(0.0)),
        line_height + padding.y * 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_grow_with_padding_and_min_width() {
        let normal = button_size(40.0, false, None);
        assert_eq!(normal, Vec2::new(60.0, 30.0));
        let compact = button_size(40.0, true, None);
        assert_eq!(compact, Vec2::new(60.0, 20.0));
        assert_eq!(button_size(10.0, false, Some(80.0)).x, 80.0);
    }

    #[test]
    fn builder_defaults_are_secondary() {
        let button = Button::new("Save");
        assert_eq!(button.kind, ButtonKind::Secondary);
        assert!(button.enabled);
        assert!(!button.compact);
        assert!(button.min_width.is_none());
    }

    #[test]
    fn disabled_buttons_are_not_clickable() {
        let button = Button::new("x").enabled(false);
        assert!(!button.enabled);
    }

    #[test]
    fn a_toggle_is_the_same_button_whether_it_is_on_or_off() {
        // A toggle (an "attach the terminal" button next to a paperclip) must keep the
        // shape of its neighbours; only its border and label take the accent colour.
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let measure = |active: bool| {
            let mut size = Vec2::ZERO;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(200.0, 100.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                size = Button::new(">_")
                    .compact()
                    .active(active)
                    .show(ui, &palette)
                    .rect
                    .size();
            });
            output.drop_without_applying_deltas();
            size
        };
        let off = measure(false);
        let on = measure(true);
        assert_eq!(on, off, "the toggle changed size when it was turned on");
        assert_eq!(off.y, 20.0, "a compact button is 20 pt tall: {off:?}");
    }
}
