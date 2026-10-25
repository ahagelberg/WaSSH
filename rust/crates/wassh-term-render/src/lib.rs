//! Terminal renderer.
//!
//! Phase 0 spike A fills this in: `swash` rasterises glyphs into an atlas texture, and the
//! grid is emitted as a single mesh of per-cell background quads plus glyph quads. Until
//! then this crate defines the layout contract the rest of the UI depends on.

#![forbid(unsafe_code)]

use wassh_term::CellSize;

/// Where a cell sits inside the viewport, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Maps grid coordinates to pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    pub origin_x: f32,
    pub origin_y: f32,
    pub cell: CellSize,
}

impl GridMetrics {
    pub fn new(origin_x: f32, origin_y: f32, cell: CellSize) -> Self {
        Self {
            origin_x,
            origin_y,
            cell,
        }
    }

    pub fn cell_rect(&self, col: u16, row: u16) -> CellRect {
        CellRect {
            x: self.origin_x + col as f32 * self.cell.width,
            y: self.origin_y + row as f32 * self.cell.height,
            width: self.cell.width,
            height: self.cell.height,
        }
    }

    /// Whole-row height for a pane, so the bottom row is never clipped.
    pub fn snapped_height(&self, rows: u16) -> f32 {
        rows as f32 * self.cell.height
    }

    /// Grid cell under a point, if the point is inside the grid bounds.
    pub fn cell_at(&self, x: f32, y: f32, cols: u16, rows: u16) -> Option<(u16, u16)> {
        if !self.cell.is_usable() {
            return None;
        }
        let local_x = x - self.origin_x;
        let local_y = y - self.origin_y;
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }
        let col = (local_x / self.cell.width).floor();
        let row = (local_y / self.cell.height).floor();
        if col >= cols as f32 || row >= rows as f32 {
            return None;
        }
        Some((col as u16, row as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> GridMetrics {
        GridMetrics::new(4.0, 2.0, CellSize::new(8.0, 16.0))
    }

    #[test]
    fn cell_rect_accounts_for_origin() {
        let rect = metrics().cell_rect(3, 2);
        assert_eq!(
            rect,
            CellRect {
                x: 28.0,
                y: 34.0,
                width: 8.0,
                height: 16.0
            }
        );
    }

    #[test]
    fn hit_testing_respects_bounds() {
        let m = metrics();
        assert_eq!(m.cell_at(4.0, 2.0, 80, 24), Some((0, 0)));
        assert_eq!(m.cell_at(11.9, 17.9, 80, 24), Some((0, 0)));
        assert_eq!(m.cell_at(12.0, 18.0, 80, 24), Some((1, 1)));
        assert_eq!(m.cell_at(3.9, 2.0, 80, 24), None);
        assert_eq!(m.cell_at(4.0 + 8.0 * 80.0, 2.0, 80, 24), None);
        assert_eq!(m.cell_at(4.0, 2.0 + 16.0 * 24.0, 80, 24), None);
    }

    #[test]
    fn snapped_height_is_whole_rows() {
        assert_eq!(metrics().snapped_height(24), 384.0);
    }
}
