//! A segmented control: the options in one track, the active one filled.
//!
//! One control for a choice between a few named states, where a row of buttons would read as
//! separate actions. The options share one width so the track never changes size as the choice
//! moves.

use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Height of the track, and the radius that makes it a pill.
const HEIGHT: f32 = 20.0;
/// Padding either side of an option's text.
const PADDING_X: f32 = 8.0;
/// How far the highlight sits inside the track, clear of its border.
const INSET: f32 = 2.0;
/// Type size of the options.
const TEXT_SIZE: f32 = 12.0;

/// Draw the options in one track and move `active` to the one clicked, reporting it as changed.
pub fn segmented(ui: &mut Ui, palette: &Palette, labels: &[&str], active: &mut usize) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(segmented_size(ui, labels), Sense::click());
    if labels.is_empty() {
        return response;
    }

    let painter = ui.painter();
    let radius = rect.height() / 2.0;
    let accent = to_color32(palette.accent);
    painter.rect_filled(rect, radius, to_color32(palette.bg_input));
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, to_color32(palette.border)),
        egui::StrokeKind::Inside,
    );

    let width = rect.width() / labels.len() as f32;
    let under_pointer = response
        .hover_pos()
        .map(|pos| ((pos.x - rect.left()) / width) as usize)
        .filter(|index| *index < labels.len());
    if response.clicked()
        && let Some(index) = under_pointer
        && index != *active
    {
        *active = index;
        response.mark_changed();
    }

    let font = FontId::proportional(TEXT_SIZE);
    for (index, label) in labels.iter().enumerate() {
        let segment = Rect::from_min_size(
            egui::pos2(rect.left() + index as f32 * width, rect.top()),
            Vec2::new(width, rect.height()),
        );
        let on = index == *active;
        if on || (under_pointer == Some(index) && response.hovered()) {
            painter.rect_filled(
                segment.shrink(INSET),
                radius - INSET,
                if on {
                    accent
                } else {
                    to_color32(palette.bg_panel_2)
                },
            );
        }
        painter.text(
            segment.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if on {
                Color32::WHITE
            } else {
                to_color32(palette.text_muted)
            },
        );
    }
    response
}

/// How much room the options need, for callers that lay the control out by hand.
pub fn segmented_size(ui: &Ui, labels: &[&str]) -> Vec2 {
    let font = FontId::proportional(TEXT_SIZE);
    let widest = labels
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    Vec2::new(
        (widest + PADDING_X * 2.0) * labels.len() as f32,
        HEIGHT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: [&str; 2] = ["Agent", "Plan"];

    /// What a run of frames left behind: the chosen index, the room taken, and the fills drawn.
    struct Drawn {
        active: usize,
        size: Vec2,
        fills: Vec<Color32>,
    }

    /// Run a few frames of the control starting at `active`, optionally clicking at a
    /// fraction of the track's width.
    fn draw(active: usize, click: Option<f32>) -> Drawn {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut chosen = active;
        let mut size = Vec2::ZERO;
        let mut fills = Vec::new();
        for frame in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(200.0, 100.0),
                )),
                time: Some(frame as f64 * 0.05),
                ..Default::default()
            };
            // The press lands after the first frame, placed by the size it measured.
            if frame == 1
                && let Some(fraction) = click
            {
                let position = egui::pos2(size.x * fraction, size.y / 2.0);
                input.events = vec![
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ];
            }
            let output = ctx.run_ui(input, |ui| {
                let response = segmented(ui, &palette, &OPTIONS, &mut chosen);
                size = response.rect.size();
            });
            fills.clear();
            for clipped in &output.shapes {
                if let egui::Shape::Rect(rect) = &clipped.shape
                    && rect.fill != Color32::TRANSPARENT
                {
                    fills.push(rect.fill);
                }
            }
            output.drop_without_applying_deltas();
        }
        Drawn {
            active: chosen,
            size,
            fills,
        }
    }

    #[test]
    fn a_click_picks_the_option_under_it() {
        assert_eq!(
            draw(0, Some(0.75)).active,
            1,
            "the right half did not pick Plan"
        );
        assert_eq!(
            draw(1, Some(0.25)).active,
            0,
            "the left half did not pick Agent"
        );
        assert_eq!(draw(1, None).active, 1, "a frame with no click leaves it");
    }

    #[test]
    fn the_track_keeps_its_size_and_paints_the_choice() {
        let agent = draw(0, None);
        let plan = draw(1, None);
        assert_eq!(agent.size, plan.size, "the track changed size");
        assert_eq!(agent.size.y, HEIGHT);
        let accent = to_color32(Palette::dark().accent);
        assert!(
            agent.fills.contains(&accent) && plan.fills.contains(&accent),
            "the active option is not filled with the accent: {:?}",
            plan.fills
        );
    }
}
