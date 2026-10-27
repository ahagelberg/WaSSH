//! Step slider.
//!
//! One snapped position per step, drawn with the shared tokens: a 3 px track
//! and a 12 px accent thumb, the same look the reference app's `input[type=
//! range]` controls have. Any pane that needs to pick between a handful of
//! values uses this instead of drawing its own.

use egui::{Color32, Pos2, Sense, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Width of the track in the default layout (the reference's 96 px control).
pub const SLIDER_WIDTH: f32 = 96.0;
/// Height of the whole control; the track is only 3 px of it.
pub const SLIDER_HEIGHT: f32 = 22.0;
/// Thickness of the track.
const TRACK_HEIGHT: f32 = 3.0;
/// Diameter of the thumb.
const THUMB_SIZE: f32 = 12.0;

/// A slider that snaps to `steps` positions, `0..steps`.
///
/// `index` is clamped into range; clicking or dragging anywhere on the track
/// moves the thumb to the nearest step. Returns true when the index changed.
pub fn step_slider(
    ui: &mut Ui,
    palette: &Palette,
    width: f32,
    steps: usize,
    index: &mut usize,
) -> bool {
    let steps = steps.max(1);
    *index = (*index).min(steps - 1);

    let width = width.max(THUMB_SIZE * 2.0);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, SLIDER_HEIGHT),
        Sense::click_and_drag(),
    );

    // The thumb travels between the two halves so it never leaves the track.
    let half = THUMB_SIZE / 2.0;
    let start = rect.left() + half;
    let end = rect.right() - half;
    let travel = (end - start).max(1.0);

    let changed = match response.interact_pointer_pos() {
        Some(pos) => {
            let fraction = ((pos.x - start) / travel).clamp(0.0, 1.0);
            let next = ((fraction * (steps - 1) as f32).round() as usize).min(steps - 1);
            if next != *index {
                *index = next;
                true
            } else {
                false
            }
        }
        None => false,
    };

    let painter = ui.painter();
    let track = egui::Rect::from_center_size(
        rect.center(),
        Vec2::new(travel, TRACK_HEIGHT),
    );
    painter.rect_filled(track, TRACK_HEIGHT / 2.0, faded(palette.border, 0.8));

    let position = if steps == 1 {
        0.0
    } else {
        *index as f32 / (steps - 1) as f32
    };
    let thumb = Pos2::new(start + travel * position, rect.center().y);
    let colour = if response.dragged() || response.hovered() {
        to_color32(palette.accent_hover)
    } else {
        to_color32(palette.accent)
    };
    painter.circle_filled(thumb, half, colour);

    if response.hovered() || response.dragged() {
        ui.ctx().request_repaint();
    }
    changed
}

/// The slider and the label a pane puts beside it.
///
/// `label` is the uppercase muted caption on the left and `value` is the
/// resolved step shown on the right, so the control reads as one field.
pub fn slider_field(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    value: &str,
    width: f32,
    steps: usize,
    index: &mut usize,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(10.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(6.0);
        let changed = step_slider(ui, palette, width, steps, index);
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(value)
                .size(10.0)
                .color(to_color32(palette.text)),
        );
        changed
    })
    .inner
}

/// `rgba` drawn at `alpha` over whatever is behind it.
fn faded(rgba: wassh_core::Rgba, alpha: f32) -> Color32 {
    let base = to_color32(rgba);
    Color32::from_rgba_unmultiplied(
        base.r(),
        base.g(),
        base.b(),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(400.0, 200.0))
    }

    /// A frame with no events; egui needs one to learn a widget's rect.
    fn idle() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        }
    }

    /// Press at the left end of the track, drag to `x` and release.
    fn drag_to(x: f32, steps: usize, start: usize) -> usize {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut index = start;
        let grab = egui::Pos2::new(6.0, 11.0);
        let release = egui::Pos2::new(x, 11.0);
        let mut press = idle();
        press.events.push(egui::Event::PointerMoved(grab));
        press.events.push(egui::Event::PointerButton {
            pos: grab,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let mut drag = idle();
        drag.events.push(egui::Event::PointerMoved(release));
        let mut up = idle();
        up.events.push(egui::Event::PointerButton {
            pos: release,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });

        for input in [idle(), press, drag, up, idle()] {
            let output = ctx.run_ui(input, |ui| {
                step_slider(ui, &palette, SLIDER_WIDTH, steps, &mut index);
            });
            output.drop_without_applying_deltas();
        }
        index
    }

    #[test]
    fn dragging_snaps_to_the_nearest_step() {
        // The thumb travels between 6 and 90 px, so the far right is the last
        // step and the middle is the middle step.
        assert_eq!(drag_to(90.0, 9, 1), 8);
        assert_eq!(drag_to(6.0, 9, 4), 0);
        assert_eq!(drag_to(48.0, 9, 0), 4);
    }

    #[test]
    fn the_index_is_clamped_to_the_available_steps() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut index = 9;
        let input = egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        };
        let output = ctx.run_ui(input.clone(), |ui| {
            step_slider(ui, &palette, SLIDER_WIDTH, 3, &mut index);
        });
        output.drop_without_applying_deltas();
        assert_eq!(index, 2);
    }
}
