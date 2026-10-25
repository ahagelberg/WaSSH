//! Sparkline charts: a filled line with no axes, used by the monitor panes.

use egui::{Color32, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Height of a chart inside a card.
pub const CHART_HEIGHT: f32 = 26.0;

/// Draw a line chart of `values`, scaled to `max` (or the largest value when
/// `max` is zero) and filled underneath with a faint tint of `colour`.
pub fn line_chart(ui: &mut Ui, palette: &Palette, values: &[f32], max: f32, colour: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CHART_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, to_color32(palette.bg_input));
    if values.is_empty() {
        return;
    }
    if values.len() == 1 {
        // One sample: a level line, so the card is never blank.
        let peak = if max > 0.0 { max } else { values[0].max(1.0) };
        let y = rect.bottom() - 3.0 - (values[0] / peak).clamp(0.0, 1.0) * (rect.height() - 6.0);
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.5, colour),
        );
        return;
    }
    let peak = if max > 0.0 {
        max
    } else {
        values.iter().copied().fold(0.0f32, f32::max).max(1.0)
    };
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = rect.left() + rect.width() * index as f32 / (values.len() - 1) as f32;
            let normalised = (value / peak).clamp(0.0, 1.0);
            egui::pos2(x, rect.bottom() - 3.0 - normalised * (rect.height() - 6.0))
        })
        .collect();
    // Faint area under the line, then the line itself.
    let mut area = points.clone();
    area.push(egui::pos2(rect.right(), rect.bottom()));
    area.push(egui::pos2(rect.left(), rect.bottom()));
    painter.add(egui::Shape::convex_polygon(
        area,
        Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 28),
        Stroke::NONE,
    ));
    painter.add(egui::Shape::line(points, Stroke::new(1.5, colour)));
}
