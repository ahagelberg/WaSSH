//! Copy-to-clipboard button.
//!
//! A flat icon button that turns into a tick for [`COPY_FEEDBACK_MS`] once the
//! caller has copied something, so a click is confirmed where it happened.

use egui::{Align2, FontId, Rect, Sense, Ui, Vec2};
use wassh_core::Palette;

use crate::to_color32;

/// Side of the button.
pub const COPY_BUTTON_SIZE: f32 = 18.0;
/// How long a click stays confirmed.
pub const COPY_FEEDBACK_MS: u64 = 1_500;
/// Glyph shown while idle.
pub const COPY_GLYPH: &str = "\u{29c9}";
/// Glyph shown once copied.
pub const COPIED_GLYPH: &str = "\u{2713}";
/// Type size of the glyph.
const COPY_GLYPH_SIZE: f32 = 11.0;

/// A copy button in its own square at the cursor; returns whether it was clicked.
pub fn copy_button(ui: &mut Ui, palette: &Palette, copied: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(COPY_BUTTON_SIZE), Sense::click());
    paint(ui, palette, rect, response.hovered(), copied);
    response.clicked()
}

/// A copy button in a rect the caller positioned; returns whether it was clicked.
///
/// `id` identifies the button, so it must be unique per card.
pub fn copy_button_at(
    ui: &mut Ui,
    palette: &Palette,
    rect: Rect,
    id: egui::Id,
    copied: bool,
) -> bool {
    let response = ui.interact(rect, id, Sense::click());
    paint(ui, palette, rect, response.hovered(), copied);
    response.clicked()
}

fn paint(ui: &Ui, palette: &Palette, rect: Rect, hovered: bool, copied: bool) {
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let colour = if copied {
        to_color32(palette.accent)
    } else if hovered {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if copied { COPIED_GLYPH } else { COPY_GLYPH },
        FontId::proportional(COPY_GLYPH_SIZE),
        colour,
    );
}
