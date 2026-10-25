//! The session plugin toolbar.
//!
//! A 32 px strip under the tab bar with one toggle button per
//! built-in plugin, tinted with the accent colour while that plugin's pane is open.

use egui::{Align2, FontId, Rect, Sense, Ui, Vec2};
use wassh_core::{colors, Palette};

use crate::terminal_view::to_color32;

/// Height of the strip.
pub const TOOLBAR_HEIGHT: f32 = 32.0;
/// Height of a button inside the strip.
pub const BUTTON_HEIGHT: f32 = 20.0;
/// Horizontal padding inside a button, per side.
pub const BUTTON_PADDING: f32 = 10.0;
/// Gap between buttons.
pub const BUTTON_GAP: f32 = 4.0;
/// Left inset before the first button.
pub const TOOLBAR_PADDING: f32 = 8.0;
/// Corner radius of a button.
pub const BUTTON_RADIUS: u8 = 4;

/// One toggle button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarButton<'a> {
    /// Plugin id, returned when the button is clicked.
    pub id: &'a str,
    /// Label painted on the button.
    pub label: &'a str,
    /// Whether the plugin's pane is currently open.
    pub active: bool,
    /// Tooltip shown on hover.
    pub tooltip: &'a str,
}

/// What the user did with the strip this frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolbarOutput {
    /// Button clicked: toggle that plugin.
    pub clicked: Option<String>,
    /// Button being dragged: the caller starts a dock drag for it.
    pub dragging: Option<String>,
}

/// Draw the toolbar and report what the user did.
///
/// A click toggles the plugin (open when closed, close when open); a drag starts moving it to
/// another dock, which is how a closed pane gets placed.
pub fn plugin_toolbar(
    ui: &mut Ui,
    palette: &Palette,
    buttons: &[ToolbarButton<'_>],
) -> ToolbarOutput {
    let height = TOOLBAR_HEIGHT;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 0.0, to_color32(palette.bg_panel));
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, to_color32(palette.border)),
    );

    let mut output = ToolbarOutput::default();
    let mut left = rect.left() + TOOLBAR_PADDING;
    for button in buttons {
        let galley = ui.painter().layout_no_wrap(
            button.label.to_owned(),
            FontId::proportional(12.0),
            to_color32(palette.text),
        );
        let width = galley.size().x + BUTTON_PADDING * 2.0;
        let button_rect = Rect::from_min_size(
            egui::pos2(left, rect.center().y - BUTTON_HEIGHT / 2.0),
            Vec2::new(width, BUTTON_HEIGHT),
        );
        left += width + BUTTON_GAP;
        if !rect.contains_rect(button_rect) {
            break;
        }

        let id = egui::Id::unique(("wassh-plugin-toolbar", button.id));
        let response = ui.interact(button_rect, id, Sense::click_and_drag());
        let fill = if button.active {
            colors::tint(palette.accent, palette.bg_panel_2, 0.18)
        } else if response.hovered() {
            palette.bg_panel_2
        } else {
            palette.bg_panel
        };
        ui.painter().rect_filled(button_rect, BUTTON_RADIUS, to_color32(fill));
        if button.active {
            ui.painter().rect_stroke(
                button_rect,
                BUTTON_RADIUS,
                egui::Stroke::new(1.0, to_color32(palette.accent)),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            button_rect.center(),
            Align2::CENTER_CENTER,
            button.label,
            FontId::proportional(12.0),
            to_color32(if button.active {
                palette.text
            } else {
                palette.text_muted
            }),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let response = if button.tooltip.is_empty() {
            response
        } else {
            response.on_hover_text(button.tooltip)
        };
        if response.dragged() {
            output.dragging = Some(button.id.to_owned());
        } else if response.clicked() {
            output.clicked = Some(button.id.to_owned());
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_carry_their_own_state() {
        let button = ToolbarButton {
            id: "sftp",
            label: "Files",
            active: true,
            tooltip: "Browse the host's files",
        };
        assert_eq!(button.id, "sftp");
        assert!(button.active);
    }

    #[test]
    fn the_strip_is_tall_enough_for_its_buttons() {
        const {
            assert!(BUTTON_HEIGHT < TOOLBAR_HEIGHT);
        }
    }
}
