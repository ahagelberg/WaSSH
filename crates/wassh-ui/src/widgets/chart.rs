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

/// One layer of a [`stacked_chart`]: what one part of a total took, as it grew.
pub struct Layer<'a> {
    /// Values, oldest first.
    pub values: &'a [f32],
    /// Colour its band and its top edge take.
    pub colour: Color32,
}

/// Alpha of a stacked band: enough to read the colour, enough to see the one behind it.
const STACK_FILL_ALPHA: u8 = 110;

/// `values` summed into exactly `columns` columns.
///
/// A chat of thousands of requests must not cost thousands of shapes, and a running total has
/// to keep every sample: each value lands in exactly one column, so the last column's total is
/// the whole of it however coarse the drawing.
fn resample(values: &[f32], columns: usize) -> Vec<f32> {
    if values.is_empty() || columns == 0 {
        return vec![0.0; columns];
    }
    (0..columns)
        .map(|column| {
            let start = (column * values.len()).div_ceil(columns);
            let end = ((column + 1) * values.len()).div_ceil(columns);
            values[start..end.min(values.len())].iter().sum()
        })
        .collect()
}

/// The running total of `layers` at each of `columns` points, which only ever rises: a total
/// of counts spent cannot fall, whatever the per-request numbers did.
pub fn stacked_totals(layers: &[&[f32]], columns: usize) -> Vec<f32> {
    let mut totals = vec![0.0f32; columns];
    for values in layers {
        let mut at = 0.0;
        for (total, value) in totals.iter_mut().zip(resample(values, columns)) {
            at += value;
            *total += at;
        }
    }
    totals
}

/// Draw `layers` stacked one on another: the height at any point is the running total of
/// every layer up to it, the bands are filled in their own colours, and the top never falls.
pub fn stacked_chart(ui: &mut Ui, palette: &Palette, layers: &[Layer<'_>], height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, to_color32(palette.bg_input));
    let columns = (rect.width().max(1.0) as usize).max(2);
    let values: Vec<&[f32]> = layers.iter().map(|layer| layer.values).collect();
    let peak = stacked_totals(&values, columns)
        .last()
        .copied()
        .unwrap_or(0.0)
        .max(1.0);
    let room = rect.height() - 6.0;
    let x = |column: usize| {
        rect.left() + rect.width() * column as f32 / (columns - 1) as f32
    };
    let y = |value: f32| rect.bottom() - 3.0 - (value / peak).clamp(0.0, 1.0) * room;
    // The running total under the layer being drawn, so each band starts where the one
    // below it ended.
    let mut lower = vec![0.0f32; columns];
    for layer in layers {
        let mut cumulative = Vec::with_capacity(columns);
        let mut at = 0.0;
        for value in resample(layer.values, columns) {
            at += value;
            cumulative.push(at);
        }
        let fill = Color32::from_rgba_unmultiplied(
            layer.colour.r(),
            layer.colour.g(),
            layer.colour.b(),
            STACK_FILL_ALPHA,
        );
        let mut mesh = egui::Mesh::default();
        let mut top = Vec::with_capacity(columns);
        for column in 0..columns {
            let upper = lower[column] + cumulative[column];
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(x(column), y(lower[column])),
                uv: egui::epaint::WHITE_UV,
                color: fill,
            });
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(x(column), y(upper)),
                uv: egui::epaint::WHITE_UV,
                color: fill,
            });
            top.push(egui::pos2(x(column), y(upper)));
            if let Some(base) = column.checked_sub(1) {
                let (low, high) = (base as u32 * 2, column as u32 * 2);
                mesh.indices
                    .extend([low, low + 1, high, low + 1, high + 1, high]);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::line(top, Stroke::new(1.2, layer.colour)));
        for (column, value) in lower.iter_mut().enumerate() {
            *value += cumulative[column];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stacked_total_only_rises() {
        // Each layer falls and rises; the total it adds up to does neither.
        let first = [3.0, 0.0, 2.0];
        let second = [1.0, 5.0, 0.0];
        let totals = stacked_totals(&[&first, &second], 3);
        assert_eq!(totals, [4.0, 9.0, 11.0]);
        assert!(totals.windows(2).all(|pair| pair[1] >= pair[0]), "{totals:?}");
    }

    #[test]
    fn a_long_chat_is_drawn_in_the_columns_it_has() {
        let values: Vec<f32> = (0..1000).map(|step| step as f32).collect();
        let totals = stacked_totals(&[&values], 40);
        assert_eq!(totals.len(), 40);
        assert!(totals.windows(2).all(|pair| pair[1] >= pair[0]), "{totals:?}");
        // The last column is the whole of it, however coarse the drawing.
        assert_eq!(totals.last().copied(), Some(values.iter().sum::<f32>()));
    }

    #[test]
    fn one_request_is_one_column() {
        assert_eq!(stacked_totals(&[&[7.0], &[3.0]], 1), [10.0]);
        assert_eq!(stacked_totals(&[], 4), [0.0; 4]);
    }
}
