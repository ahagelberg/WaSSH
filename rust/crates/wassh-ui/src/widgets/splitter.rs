//! Splitters and drag-reorder helpers.
//!
//! Splitters use a 5 px grab band; drag reorder inserts at the nearest midpoint and draws a
//! 2 px accent indicator.

use egui::{CursorIcon, Rect, Sense, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Thickness of a splitter handle.
pub const HANDLE: f32 = 4.0;
/// Thickness of the insertion indicator.
pub const INDICATOR: f32 = 2.0;

/// Which way a splitter moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// A vertical handle that resizes the extent horizontally.
    Horizontal,
    /// A horizontal handle that resizes the extent vertically.
    Vertical,
}

/// Draw a splitter handle, mutating `extent` while dragged.
///
/// Returns true when the extent changed.
pub fn splitter(
    ui: &mut Ui,
    palette: &Palette,
    axis: SplitAxis,
    extent: &mut f32,
    min: f32,
    max: f32,
) -> bool {
    let size = match axis {
        SplitAxis::Horizontal => Vec2::new(HANDLE, ui.available_height().max(HANDLE)),
        SplitAxis::Vertical => Vec2::new(ui.available_width().max(HANDLE), HANDLE),
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let response = response.interact(Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(match axis {
            SplitAxis::Horizontal => CursorIcon::ResizeHorizontal,
            SplitAxis::Vertical => CursorIcon::ResizeVertical,
        });
    }
    let colour = if response.dragged() || response.hovered() {
        to_color32(palette.accent).gamma_multiply(0.7)
    } else {
        to_color32(palette.border)
    };
    let band = match axis {
        SplitAxis::Horizontal => Rect::from_center_size(
            rect.center(),
            Vec2::new(1.0, rect.height()),
        ),
        SplitAxis::Vertical => Rect::from_center_size(
            rect.center(),
            Vec2::new(rect.width(), 1.0),
        ),
    };
    ui.painter().rect_filled(band, 0.0, colour);

    if !response.dragged() {
        return false;
    }
    let delta = response.drag_delta();
    let change = match axis {
        SplitAxis::Horizontal => delta.x,
        SplitAxis::Vertical => delta.y,
    };
    if change == 0.0 {
        return false;
    }
    let next = (*extent + change).clamp(min.max(0.0), max.max(min.max(0.0)));
    let changed = (next - *extent).abs() > f32::EPSILON;
    *extent = next;
    // Consume the delta so the same drag is not applied twice.
    response.drag_delta();
    changed
}

/// Index at which a dragged item should be inserted, given the midpoints of the siblings.
pub fn drop_index(pointer: f32, midpoints: &[f32]) -> usize {
    midpoints.iter().filter(|mid| **mid < pointer).count()
}

/// Midpoints along one axis for a list of item rectangles.
pub fn midpoints(rects: &[Rect], axis: SplitAxis) -> Vec<f32> {
    rects
        .iter()
        .map(|rect| match axis {
            SplitAxis::Horizontal => rect.center().x,
            SplitAxis::Vertical => rect.center().y,
        })
        .collect()
}

/// Draw the thin insertion indicator for a drag reorder.
pub fn drop_indicator(ui: &mut Ui, palette: &Palette, position: egui::Pos2, length: f32, axis: SplitAxis) {
    let size = match axis {
        SplitAxis::Horizontal => Vec2::new(INDICATOR, length),
        SplitAxis::Vertical => Vec2::new(length, INDICATOR),
    };
    let rect = Rect::from_center_size(position, size);
    ui.painter().rect_filled(rect, 1.0, to_color32(palette.accent));
}

/// Whether a drag has moved far enough to count as a reorder rather than a click.
pub fn exceeds_drag_threshold(delta: Vec2, threshold: f32) -> bool {
    delta.x.abs() > threshold || delta.y.abs() > threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    #[test]
    fn drop_index_counts_passed_midpoints() {
        let mids = [10.0, 30.0, 50.0];
        assert_eq!(drop_index(5.0, &mids), 0);
        assert_eq!(drop_index(20.0, &mids), 1);
        assert_eq!(drop_index(60.0, &mids), 3);
    }

    #[test]
    fn midpoints_follow_the_axis() {
        let rects = [
            Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 20.0)),
            Rect::from_min_size(Pos2::new(0.0, 20.0), Vec2::new(100.0, 20.0)),
        ];
        assert_eq!(midpoints(&rects, SplitAxis::Horizontal), vec![50.0, 50.0]);
        assert_eq!(midpoints(&rects, SplitAxis::Vertical), vec![10.0, 30.0]);
    }

    #[test]
    fn small_movements_are_not_drags() {
        assert!(!exceeds_drag_threshold(Vec2::new(2.0, 1.0), 4.0));
        assert!(exceeds_drag_threshold(Vec2::new(5.0, 0.0), 4.0));
    }
}
