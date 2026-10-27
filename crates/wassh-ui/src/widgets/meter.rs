//! Meters: ring gauges, bar meters and segmented bars.

use std::f32::consts::TAU;

use egui::{Align2, Color32, FontId, Pos2, Sense, Ui, Vec2};
use wassh_core::{Palette, Rgba};

use crate::terminal_view::to_color32;

/// Thickness of a gauge ring, matching the reference's SVG `stroke-width`.
pub const RING_STROKE: f32 = 7.0;
/// Thickness of a bar meter.
pub const BAR_HEIGHT: f32 = 8.0;
/// Space between the ring and its caption.
const CAPTION_GAP: f32 = 6.0;
/// Line height of the caption.
const CAPTION_HEIGHT: f32 = 15.0;
/// Line height of the detail line.
const DETAIL_HEIGHT: f32 = 14.0;
/// Density of a full circle; arcs use the same step count.
const ARC_STEPS: usize = 96;

/// A donut gauge with the value in the middle, its caption and an optional
/// detail line underneath.
///
/// The track and the progress arc are the same annulus mesh, so the filled part
/// always sits exactly on the track. The labels own their height, so they never
/// overlap the ring or each other.
pub fn ring_gauge(
    ui: &mut Ui,
    palette: &Palette,
    size: f32,
    fraction: Option<f32>,
    colour: Color32,
    caption: &str,
    detail: &str,
) {
    let has_detail = !detail.is_empty();
    // The labels get the whole column, not just the ring: the caption sits
    // under a 64 px donut but may be much wider than it.
    let room = ui.available_width().max(size);
    let label_height =
        CAPTION_GAP + CAPTION_HEIGHT + if has_detail { DETAIL_HEIGHT } else { 0.0 };
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(size, size + label_height), Sense::hover());
    let ring = egui::Rect::from_min_size(rect.min, Vec2::splat(size));
    let centre = ring.center();
    // The radius the reference SVG uses: (size - stroke) / 2.
    let radius = ((size - RING_STROKE) / 2.0).max(2.0);

    let painter = ui.painter();
    let track = Arc {
        painter,
        centre,
        radius,
        thickness: RING_STROKE,
        colour: faded(palette.border, 0.8),
    };
    track.draw(0.0, 1.0, false);

    let fraction = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    if fraction > 0.0 {
        Arc { colour, ..track }.draw(0.0, fraction, fraction < 1.0);
    }

    painter.text(
        centre,
        Align2::CENTER_CENTER,
        format!("{:.0}%", fraction * 100.0),
        FontId::monospace(12.0),
        to_color32(palette.text),
    );

    // The labels never spill out of the gauge's column: they are shortened with
    // an ellipsis instead.
    painter.text(
        Pos2::new(centre.x, ring.bottom() + CAPTION_GAP + CAPTION_HEIGHT / 2.0),
        Align2::CENTER_CENTER,
        fit(painter, caption, FontId::proportional(12.0), room),
        FontId::proportional(12.0),
        to_color32(palette.text),
    );
    if has_detail {
        painter.text(
            Pos2::new(
                centre.x,
                ring.bottom() + CAPTION_GAP + CAPTION_HEIGHT + DETAIL_HEIGHT / 2.0,
            ),
            Align2::CENTER_CENTER,
            fit(painter, detail, FontId::proportional(10.0), room),
            FontId::proportional(10.0),
            to_color32(palette.text_muted),
        );
    }
}

/// One ring segment, `from`..`to` as fractions of a turn starting at twelve
/// o'clock.
struct Arc<'a> {
    painter: &'a egui::Painter,
    centre: Pos2,
    radius: f32,
    thickness: f32,
    colour: Color32,
}

impl Arc<'_> {
    /// Stroke `from`..`to`.
    ///
    /// A stroked polyline is used rather than a triangle mesh because epaint
    /// feathers (anti-aliases) paths but never raw mesh edges — a mesh ring
    /// looks visibly jagged. The track and the progress are the same radius and
    /// the same thickness, so they sit exactly on top of each other.
    fn draw(&self, from: f32, to: f32, rounded: bool) {
        let span = (to - from).abs();
        if span <= 0.0 {
            return;
        }
        let stroke = egui::Stroke::new(self.thickness, self.colour);
        let full = span >= 1.0 - f32::EPSILON;
        if full {
            // Drop the duplicate closing point: `closed_line` joins it itself.
            let points = self.points(from, to, false);
            self.painter.add(egui::Shape::closed_line(points, stroke));
            return;
        }

        let points = self.points(from, to, true);
        if rounded {
            // `Stroke` has no round cap, so the ends get a disc of their own.
            for end in [points[0], *points.last().expect("at least two points")] {
                self.painter.circle_filled(end, self.thickness / 2.0, self.colour);
            }
        }
        self.painter.add(egui::Shape::line(points, stroke));
    }

    /// Points along the ring centreline, optionally including both ends.
    fn points(&self, from: f32, to: f32, inclusive: bool) -> Vec<Pos2> {
        let span = (to - from).abs();
        let steps = ((span * ARC_STEPS as f32).ceil() as usize).max(2);
        let last = if inclusive { steps } else { steps.saturating_sub(1) };
        (0..=last)
            .map(|step| {
                let turn = from + (to - from) * step as f32 / steps as f32;
                let angle = -TAU / 4.0 + TAU * turn;
                self.centre + Vec2::new(angle.cos(), angle.sin()) * self.radius
            })
            .collect()
    }
}

/// `rgba` drawn at `alpha` over whatever is behind it.
pub fn faded(rgba: Rgba, alpha: f32) -> Color32 {
    let base = to_color32(rgba);
    Color32::from_rgba_unmultiplied(
        base.r(),
        base.g(),
        base.b(),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// `text`, shortened with an ellipsis when it is wider than `width`.
fn fit(painter: &egui::Painter, text: &str, font: FontId, width: f32) -> String {
    let measure = |candidate: &str| {
        painter
            .layout_no_wrap(candidate.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if measure(text) <= width {
        return text.to_owned();
    }
    let mut shortened: Vec<char> = text.chars().collect();
    while !shortened.is_empty() {
        shortened.pop();
        let candidate = format!("{}…", shortened.iter().collect::<String>());
        if measure(&candidate) <= width {
            return candidate.trim_end().to_owned();
        }
    }
    String::new()
}

/// A thin bar showing `fraction` filled with `colour`.
pub fn bar_meter(ui: &mut Ui, palette: &Palette, fraction: Option<f32>, colour: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, BAR_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, BAR_HEIGHT / 2.0, faded(palette.border, 0.55));
    let fraction = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    if fraction > 0.0 {
        let filled = egui::Rect::from_min_size(
            rect.min,
            Vec2::new((width * fraction).max(BAR_HEIGHT), BAR_HEIGHT),
        );
        painter.rect_filled(filled, BAR_HEIGHT / 2.0, colour);
    }
}

/// A bar split into segments, drawn left to right; `(fraction, colour)` pairs.
pub fn segmented_bar(ui: &mut Ui, palette: &Palette, segments: &[(f32, Color32)]) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, BAR_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, BAR_HEIGHT / 2.0, faded(palette.border, 0.55));
    let mut left = rect.left();
    let total: f32 = segments.iter().map(|(fraction, _)| *fraction).sum();
    let scale = if total > 1.0 { 1.0 / total } else { 1.0 };
    for (fraction, colour) in segments {
        let width = (width * fraction * scale).max(0.0);
        if width <= 0.0 {
            continue;
        }
        let segment = egui::Rect::from_min_size(
            Pos2::new(left, rect.top()),
            Vec2::new(width, BAR_HEIGHT),
        );
        painter.rect_filled(segment, 2.0, *colour);
        left += width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes a gauge emits when drawn at `fraction`.
    fn gauge_shapes(fraction: Option<f32>) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(200.0, 120.0),
            )),
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                ring_gauge(
                    ui,
                    &palette,
                    64.0,
                    fraction,
                    Color32::from_rgb(0x3d, 0x8b, 0xfd),
                    "CPU",
                    "Load 0.42",
                );
            });
            shapes.clear();
            for clipped in std::mem::take(&mut output.shapes) {
                shapes.push(clipped.shape);
            }
            output.drop_without_applying_deltas();
        }
        shapes
    }

    #[test]
    fn the_ring_is_antialiased() {
        // epaint feathers paths but never mesh edges, so a mesh ring renders
        // with hard, jagged edges: the gauge must only emit paths and discs.
        let shapes = gauge_shapes(Some(0.55));
        assert!(
            !shapes.iter().any(|shape| matches!(shape, egui::Shape::Mesh(_))),
            "the ring is drawn as a mesh, which cannot be anti-aliased"
        );
        let paths = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Path(_)))
            .count();
        assert!(paths >= 2, "the track and the progress arc must both be paths");
    }

    #[test]
    fn the_ring_and_the_progress_share_one_radius() {
        let shapes = gauge_shapes(Some(0.25));
        let mut radii: Vec<f32> = Vec::new();
        for shape in &shapes {
            if let egui::Shape::Path(path) = shape {
                for point in &path.points {
                    radii.push((*point - egui::Pos2::new(32.0, 32.0)).length());
                }
            }
        }
        assert!(!radii.is_empty());
        let smallest = radii.iter().copied().fold(f32::MAX, f32::min);
        let largest = radii.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            largest - smallest < 0.01,
            "the strokes are not on one circle: {smallest}..{largest}"
        );
        assert!(
            (smallest - (64.0 - RING_STROKE) / 2.0).abs() < 0.01,
            "the ring radius is {smallest}, expected {}",
            (64.0 - RING_STROKE) / 2.0
        );
    }
}
