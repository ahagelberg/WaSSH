//! Custom checkbox with the 14 px box used throughout the app.

use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

const BOX: f32 = 14.0;
const GAP: f32 = 8.0;
const ROW_HEIGHT: f32 = 20.0;

/// Draw a checkbox plus label; toggles `checked` when clicked.
pub fn checkbox(ui: &mut Ui, palette: &Palette, checked: &mut bool, label: &str) -> Response {
    let font = FontId::proportional(13.0);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(BOX + GAP + text_width, ROW_HEIGHT), Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }

    let hovered = response.hovered();
    let box_rect = Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - BOX / 2.0),
        Vec2::splat(BOX),
    );
    let accent = to_color32(palette.accent);
    let border = if *checked || hovered {
        accent
    } else {
        to_color32(palette.border)
    };
    ui.painter().rect_filled(
        box_rect,
        3.0,
        if *checked {
            accent
        } else {
            to_color32(palette.bg_input)
        },
    );
    ui.painter()
        .rect_stroke(box_rect, 3.0, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    if *checked {
        ui.painter().text(
            box_rect.center(),
            Align2::CENTER_CENTER,
            "✓",
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    ui.painter().text(
        egui::pos2(box_rect.right() + GAP, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        to_color32(palette.text),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_constants_match_the_design_tokens() {
        assert_eq!(BOX, 14.0);
        assert_eq!(GAP, 8.0);
        assert_eq!(ROW_HEIGHT, 20.0);
    }
}
