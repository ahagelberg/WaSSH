//! egui's own theming.
//!
//! The widget kit paints from [`Palette`] directly, but egui's built-in widgets (combo
//! boxes, their popups, scrollbars, collapsing headers, tooltips) read [`egui::Visuals`].
//! Those have to carry the palette too, or a combo box shows egui's default grey on a dark
//! dialog and its text becomes unreadable.

use egui::style::WidgetVisuals;
use egui::{Color32, CornerRadius, Stroke, Visuals};
use wassh_core::{Palette, ThemeMode};

use crate::terminal_view::to_color32;

/// Corner radius of egui's widget frames, in points.
const WIDGET_RADIUS: u8 = 4;
/// Width of the border egui draws around its widgets, in points.
const WIDGET_BORDER: f32 = 1.0;
/// How far the accent is mixed into the selected-row background, in `0.0..=1.0`.
const SELECTION_TINT: f32 = 0.75;

/// Install the palette as egui's visuals, for both theme slots.
///
/// The app resolves [`ThemeMode::System`] itself and hands over a concrete palette, so egui
/// is pinned to that mode: otherwise egui follows the desktop and its built-in widgets fall
/// back to egui's own light or dark colours while the rest of the window keeps the palette.
pub fn apply_visuals(ctx: &egui::Context, palette: &Palette) {
    let visuals = visuals(palette);
    let theme = match palette.mode {
        ThemeMode::Light => egui::Theme::Light,
        ThemeMode::System | ThemeMode::Dark => egui::Theme::Dark,
    };
    ctx.set_theme(theme);
    // Both slots get the same visuals, so nothing can switch back to egui's defaults.
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

/// The palette expressed as [`Visuals`].
pub fn visuals(palette: &Palette) -> Visuals {
    let dark = palette.mode != ThemeMode::Light;
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    let app = to_color32(palette.bg_app);
    let panel = to_color32(palette.bg_panel);
    let panel_2 = to_color32(palette.bg_panel_2);
    let input = to_color32(palette.bg_input);
    let border = to_color32(palette.border);
    let text = to_color32(palette.text);
    let accent = to_color32(palette.accent);

    visuals.panel_fill = app;
    visuals.window_fill = panel;
    visuals.faint_bg_color = panel_2;
    visuals.extreme_bg_color = input;
    visuals.code_bg_color = input;
    visuals.override_text_color = Some(text);
    visuals.hyperlink_color = accent;
    visuals.warn_fg_color = to_color32(palette.warn);
    visuals.error_fg_color = to_color32(palette.danger);
    visuals.window_stroke = Stroke::new(WIDGET_BORDER, border);
    visuals.selection.bg_fill = accent.gamma_multiply(SELECTION_TINT);
    visuals.selection.stroke = Stroke::new(WIDGET_BORDER, text);
    visuals.text_cursor.stroke = Stroke::new(WIDGET_BORDER, text);

    // Buttons, combo boxes and their popups take their fill from `weak_bg_fill`, text from
    // `fg_stroke`; text edits and lists use `extreme_bg_color` above.
    style_widget(&mut visuals.widgets.noninteractive, panel_2, border, text);
    style_widget(&mut visuals.widgets.inactive, input, border, text);
    style_widget(&mut visuals.widgets.hovered, panel_2, accent, text);
    style_widget(&mut visuals.widgets.active, panel_2, accent, text);
    style_widget(&mut visuals.widgets.open, panel_2, accent, text);

    visuals
}

fn style_widget(widget: &mut WidgetVisuals, fill: Color32, border: Color32, text: Color32) {
    widget.bg_fill = fill;
    widget.weak_bg_fill = fill;
    widget.bg_stroke = Stroke::new(WIDGET_BORDER, border);
    widget.fg_stroke = Stroke::new(WIDGET_BORDER, text);
    widget.corner_radius = CornerRadius::same(WIDGET_RADIUS);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_boxes_follow_the_palette() {
        let palette = Palette::dark();
        let visuals = visuals(&palette);
        assert_eq!(
            visuals.widgets.inactive.weak_bg_fill,
            to_color32(palette.bg_input),
            "a combo box must not paint egui's grey"
        );
        assert_eq!(
            visuals.widgets.inactive.fg_stroke.color,
            to_color32(palette.text)
        );
        assert_eq!(visuals.override_text_color, Some(to_color32(palette.text)));
    }

    #[test]
    fn the_light_palette_gets_readable_widgets_too() {
        let palette = Palette::light();
        let visuals = visuals(&palette);
        assert_eq!(visuals.widgets.inactive.weak_bg_fill, to_color32(palette.bg_input));
        assert_ne!(
            visuals.widgets.inactive.fg_stroke.color,
            visuals.widgets.inactive.weak_bg_fill,
            "the label must not vanish into the fill"
        );
    }
}
