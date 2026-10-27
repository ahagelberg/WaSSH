//! Text that never reflows: one line, truncated with an ellipsis.

use std::sync::Arc;

use egui::{Color32, FontId, Galley, Ui};

/// Narrowest width text is laid out at. Below this the layout would be empty anyway.
const MIN_WIDTH: f32 = 8.0;

/// Lay out `text` on a single line, truncated with an ellipsis at `max_width`.
///
/// Tab titles and pane headers must keep their height fixed, so a long value is cut
/// instead of wrapped; callers show the full text in a tooltip where it matters.
pub fn truncating_galley(
    ui: &Ui,
    text: impl Into<String>,
    size: f32,
    colour: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.into(),
        FontId::proportional(size),
        colour,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(MIN_WIDTH));
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_title_stays_on_one_line() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut rows = 0;
        let mut width = 0.0;
        let mut output = ctx.run_ui(input, |ui| {
            let galley = truncating_galley(
                ui,
                "a session title far too long for a narrow tab",
                12.0,
                Color32::WHITE,
                70.0,
            );
            rows = galley.rows.len();
            width = galley.size().x;
        });
        output.textures_delta.clear();
        assert_eq!(rows, 1, "the title wrapped");
        assert!(width <= 70.0 + 0.5, "the title was not truncated: {width}");
    }
}
