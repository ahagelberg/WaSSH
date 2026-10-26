//! Modal dialogs.
//!
//! A dimmed backdrop (`rgba(0,0,0,.55)`), a centred panel with radius 8, a 16 px title and
//! a right-aligned footer. Escape, a backdrop click or the
//! ✕ button dismisses.

use std::fmt::Debug;
use std::hash::Hash;

use egui::{Align2, Color32, Context, FontId, Id, Pos2, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Alpha of the backdrop, ~0.55 black.
pub const BACKDROP_ALPHA: u8 = 140;
/// Corner radius of the dialog panel.
pub const DIALOG_RADIUS: u8 = 8;
/// Default dialog width.
pub const DIALOG_WIDTH: f32 = 520.0;
/// Height of the title strip.
pub const HEADER_HEIGHT: f32 = 46.0;
/// Height of the footer strip.
pub const FOOTER_HEIGHT: f32 = 54.0;
/// Padding around the dialog body.
pub const BODY_PADDING: f32 = 16.0;

/// How a modal was closed this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModalResponse {
    /// True when the user asked to close the dialog.
    pub dismissed: bool,
}

impl ModalResponse {
    /// True while the dialog should stay open.
    pub fn is_open(&self) -> bool {
        !self.dismissed
    }
}

/// Draw a modal dialog and return whether it was dismissed.
///
/// `body` receives a [`Ui`] inside the dialog and is expected to draw the footer itself with
/// [`footer`] when it needs action buttons.
pub fn modal(
    ctx: &Context,
    id: impl Hash + Debug,
    palette: &Palette,
    title: &str,
    width: f32,
    body: impl FnOnce(&mut Ui),
) -> ModalResponse {
    let screen = ctx.content_rect();
    let mut dismissed = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    // Every id below hangs off the dialog's own id: two modals can be open in one frame
    // (a plugin's editor and a dialog it opens on top of itself), and shared ids in two
    // layers trip egui's "widget id changed layer_id during the frame" assertion.
    let backdrop_id = Id::unique("wassh-modal-backdrop").with(&id);
    let panel_id = Id::unique("wassh-modal").with(&id);
    let close_id = Id::unique("wassh-modal-close").with(&id);

    egui::Area::new(backdrop_id)
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(BACKDROP_ALPHA));
            if response.clicked() {
                dismissed = true;
            }
        });

    egui::Area::new(panel_id)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_width(width);
            let frame = egui::Frame::NONE
                .fill(to_color32(palette.bg_panel))
                .stroke(Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(DIALOG_RADIUS)
                .inner_margin(egui::Margin::ZERO);
            frame.show(ui, |ui| {
                ui.set_width(width);

                // Header: title on the left, close button on the right.
                let (header, _) = ui.allocate_exact_size(
                    Vec2::new(width, HEADER_HEIGHT),
                    Sense::hover(),
                );
                ui.painter().text(
                    Pos2::new(header.left() + BODY_PADDING, header.center().y),
                    Align2::LEFT_CENTER,
                    title,
                    FontId::proportional(16.0),
                    to_color32(palette.text),
                );
                let close_rect = egui::Rect::from_center_size(
                    Pos2::new(header.right() - BODY_PADDING - 10.0, header.center().y),
                    Vec2::splat(24.0),
                );
                let close = ui.interact(close_rect, close_id, Sense::click());
                let close_colour = if close.hovered() {
                    to_color32(palette.danger)
                } else {
                    to_color32(palette.text_muted)
                };
                if close.hovered() {
                    ui.painter()
                        .rect_filled(close_rect, 4, to_color32(palette.bg_panel_2));
                }
                ui.painter().text(
                    close_rect.center(),
                    Align2::CENTER_CENTER,
                    "\u{2715}",
                    FontId::proportional(13.0),
                    close_colour,
                );
                if close.clicked() {
                    dismissed = true;
                }
                ui.painter().hline(
                    header.x_range(),
                    header.bottom(),
                    Stroke::new(1.0, to_color32(palette.border)),
                );

                ui.add_space(BODY_PADDING);
                ui.horizontal(|ui| {
                    ui.add_space(BODY_PADDING);
                    ui.vertical(|ui| {
                        ui.set_width(width - BODY_PADDING * 2.0);
                        body(ui);
                    });
                });
                ui.add_space(BODY_PADDING);
            });
        });

    ModalResponse { dismissed }
}

/// Draw a footer with right-aligned action buttons.
pub fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.horizontal(|ui| {
            add(ui);
        });
    });
    let _ = ui.available_height();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_is_about_fifty_five_percent_black() {
        let alpha = BACKDROP_ALPHA as f32 / 255.0;
        assert!((alpha - 0.55).abs() < 0.01, "alpha was {alpha}");
    }

    #[test]
    fn modal_response_reports_open_state() {
        assert!(ModalResponse { dismissed: false }.is_open());
        assert!(!ModalResponse { dismissed: true }.is_open());
    }

    #[test]
    fn two_modals_can_be_open_in_the_same_frame() {
        // A plugin editor can open a dialog on top of itself (the AI agent's "Add a
        // provider" sits on its settings editor). Both use a ✕ button and a backdrop, so
        // those widget ids have to be per dialog — shared ones made egui panic with
        // "widget id changed layer_id during the frame".
        let ctx = Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 700.0),
            )),
            ..Default::default()
        };
        for _ in 0..2 {
            let output = ctx.run_pass(input.clone(), |ctx| {
                let outer = modal(
                    ctx,
                    ("plugin-settings-editor", "ai-agent"),
                    &palette,
                    "AI agent providers",
                    420.0,
                    |ui| {
                        ui.label("outer body");
                    },
                );
                let inner = modal(
                    ctx,
                    "ai-agent-add-provider",
                    &palette,
                    "Add a provider",
                    360.0,
                    |ui| {
                        ui.label("inner body");
                    },
                );
                assert!(outer.is_open() && inner.is_open(), "no events, no dismissal");
            });
            output.drop_without_applying_deltas();
        }
    }
}
