//! A switch: a pill with a knob that slides from off to on.
//!
//! Where a button reports "clicked", this reports the state it was left in: the knob is the
//! state, so the caption names what the switch turns on rather than what it does next.

use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Width of the track.
const TRACK_WIDTH: f32 = 26.0;
/// Height of the track, and the radius that makes it a pill.
const TRACK_HEIGHT: f32 = 14.0;
/// Diameter of the knob.
const KNOB: f32 = 10.0;
/// Space between the track and its caption.
const GAP: f32 = 6.0;
/// Height of the row, so a switch lines up beside a button.
const ROW_HEIGHT: f32 = 20.0;
/// Seconds the knob takes to cross, so a click reads as a switch thrown.
const SLIDE_SECONDS: f32 = 0.12;
/// Type size of the caption.
const CAPTION_SIZE: f32 = 12.0;

/// Draw a switch and its caption; flips `on` when clicked and reports it as changed.
pub fn switch(ui: &mut Ui, palette: &Palette, on: &mut bool, label: &str) -> Response {
    let font = FontId::proportional(CAPTION_SIZE);
    let (rect, mut response) = ui.allocate_exact_size(switch_size(ui, label), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }

    let hovered = response.hovered();
    let accent = to_color32(palette.accent);
    let track = Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - TRACK_HEIGHT / 2.0),
        Vec2::new(TRACK_WIDTH, TRACK_HEIGHT),
    );
    let radius = TRACK_HEIGHT / 2.0;
    ui.painter().rect_filled(
        track,
        radius,
        match (*on, hovered) {
            (true, true) => to_color32(palette.accent_hover),
            (true, false) => accent,
            (false, _) => to_color32(palette.bg_input),
        },
    );
    ui.painter().rect_stroke(
        track,
        radius,
        Stroke::new(
            1.0,
            match (*on, hovered) {
                (true, _) | (false, true) => accent,
                (false, false) => to_color32(palette.border),
            },
        ),
        egui::StrokeKind::Inside,
    );
    let slide = ui.ctx().animate_bool_with_time(response.id, *on, SLIDE_SECONDS);
    let inset = (TRACK_HEIGHT - KNOB) / 2.0;
    let knob_x = egui::lerp(
        (track.left() + inset + KNOB / 2.0)..=(track.right() - inset - KNOB / 2.0),
        slide,
    );
    ui.painter().circle_filled(
        egui::pos2(knob_x, track.center().y),
        KNOB / 2.0,
        if *on {
            Color32::WHITE
        } else {
            to_color32(palette.text_muted)
        },
    );
    ui.painter().text(
        egui::pos2(track.right() + GAP, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        to_color32(palette.text),
    );
    response
}

/// How much room a switch and its caption need, for callers that lay it out by hand.
pub fn switch_size(ui: &Ui, label: &str) -> Vec2 {
    let font = FontId::proportional(CAPTION_SIZE);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::WHITE)
        .size()
        .x;
    Vec2::new(TRACK_WIDTH + GAP + text_width, ROW_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one run of frames left behind: the state, the room taken, and where the knob
    /// came to rest.
    struct Drawn {
        on: bool,
        size: Vec2,
        knob: Option<egui::Pos2>,
    }

    /// Run several frames of a switch that starts `on`, optionally clicking it, with the
    /// clock moving so the knob finishes its slide.
    fn draw(on: bool, click_at: Option<egui::Pos2>) -> Drawn {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut state = on;
        let mut size = Vec2::ZERO;
        let mut knob = None;
        for frame in 0..5 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(200.0, 100.0),
                )),
                time: Some(frame as f64 * 0.05),
                ..Default::default()
            };
            // The press lands after the first frame, once the switch has an id to be clicked.
            if frame == 1
                && let Some(position) = click_at
            {
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
                let response = switch(ui, &palette, &mut state, "Plan");
                size = response.rect.size();
            });
            knob = None;
            for clipped in &output.shapes {
                if let egui::Shape::Circle(circle) = &clipped.shape {
                    knob = Some(circle.center);
                }
            }
            output.drop_without_applying_deltas();
        }
        Drawn {
            on: state,
            size,
            knob,
        }
    }

    #[test]
    fn a_click_throws_the_switch_the_other_way() {
        assert!(draw(false, Some(egui::pos2(13.0, 10.0))).on);
        assert!(!draw(true, Some(egui::pos2(13.0, 10.0))).on);
        assert!(!draw(false, None).on, "a frame with no click leaves it");
    }

    #[test]
    fn the_knob_sits_left_when_off_and_right_when_on() {
        let off = draw(false, None).knob.expect("the switch draws a knob");
        let on = draw(true, None).knob.expect("the switch draws a knob");
        assert!(
            on.x > off.x + 1.0,
            "the knob did not move across: {off:?} -> {on:?}"
        );
    }

    #[test]
    fn the_row_lines_up_with_a_button_and_keeps_its_width() {
        let off = draw(false, None);
        let on = draw(true, None);
        assert_eq!(
            off.size, on.size,
            "the switch changed size when it was turned on"
        );
        assert_eq!(off.size.y, ROW_HEIGHT);
    }
}
