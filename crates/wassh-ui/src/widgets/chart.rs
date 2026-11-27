//! Sparkline charts: filled lines with no axes, used by the monitor panes and the usage view.

use egui::{Color32, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Height of a chart inside a card.
pub const CHART_HEIGHT: f32 = 26.0;

/// One line of a chart.
pub struct Series<'a> {
    /// Values, oldest first.
    pub values: &'a [f32],
    /// Colour the line and its fill take.
    pub colour: Color32,
}

/// Draw a line chart of `values`, scaled to `max` (or the largest value when
/// `max` is zero) and filled underneath with a faint tint of `colour`.
pub fn line_chart(ui: &mut Ui, palette: &Palette, values: &[f32], max: f32, colour: Color32) {
    series_chart(
        ui,
        palette,
        &[Series { values, colour }],
        max,
        CHART_HEIGHT,
    );
}

/// Draw several lines on one scale, the tallest of them setting it unless `max` does.
pub fn series_chart(ui: &mut Ui, palette: &Palette, series: &[Series<'_>], max: f32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, to_color32(palette.bg_input));
    let peak = if max > 0.0 {
        max
    } else {
        series
            .iter()
            .flat_map(|line| line.values.iter())
            .copied()
            .fold(0.0f32, f32::max)
            .max(1.0)
    };
    let room = rect.height() - 6.0;
    for line in series {
        let values = line.values;
        if values.is_empty() {
            continue;
        }
        let y = |value: f32| rect.bottom() - 3.0 - (value / peak).clamp(0.0, 1.0) * room;
        if values.len() == 1 {
            // One sample: a level line, so the chart is never blank.
            let at = y(values[0]);
            painter.line_segment(
                [egui::pos2(rect.left(), at), egui::pos2(rect.right(), at)],
                Stroke::new(1.5, line.colour),
            );
            continue;
        }
        let points: Vec<egui::Pos2> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let x = rect.left() + rect.width() * index as f32 / (values.len() - 1) as f32;
                egui::pos2(x, y(*value))
            })
            .collect();
        // Faint area under the line, then the line itself.
        let mut area = points.clone();
        area.push(egui::pos2(rect.right(), rect.bottom()));
        area.push(egui::pos2(rect.left(), rect.bottom()));
        painter.add(egui::Shape::convex_polygon(
            area,
            Color32::from_rgba_unmultiplied(
                line.colour.r(),
                line.colour.g(),
                line.colour.b(),
                20,
            ),
            Stroke::NONE,
        ));
        painter.add(egui::Shape::line(points, Stroke::new(1.5, line.colour)));
    }
}
