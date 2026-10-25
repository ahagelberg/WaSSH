//! Cell geometry and PTY sizing.

/// Terminal dimensions in cells, always at least 1×1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
        }
    }

    pub fn cell_count(self) -> usize {
        self.cols as usize * self.rows as usize
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

/// Measured size of one character cell in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
}

impl CellSize {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    pub fn is_usable(self) -> bool {
        self.width > 0.5 && self.height > 0.5 && self.width.is_finite() && self.height.is_finite()
    }
}

/// How many whole cells fit into the available area.
///
/// Whole rows/columns only, so the last line
/// is never clipped in half. Returns the default 80×24 when the cell metrics are unusable.
pub fn fit_cells(area_width: f32, area_height: f32, cell: CellSize) -> TerminalSize {
    if !cell.is_usable() || !area_width.is_finite() || !area_height.is_finite() {
        return TerminalSize::default();
    }
    let cols = (area_width / cell.width).floor();
    let rows = (area_height / cell.height).floor();
    if cols < 1.0 || rows < 1.0 {
        return TerminalSize::default();
    }
    TerminalSize::new(
        cols.min(u16::MAX as f32) as u16,
        rows.min(u16::MAX as f32) as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_never_zero() {
        let size = TerminalSize::new(0, 0);
        assert_eq!(size, TerminalSize { cols: 1, rows: 1 });
        assert_eq!(size.cell_count(), 1);
    }

    #[test]
    fn fits_whole_cells_only() {
        let cell = CellSize::new(8.0, 17.0);
        let size = fit_cells(800.0, 400.0, cell);
        assert_eq!(size, TerminalSize { cols: 100, rows: 23 });
        // 399.9 px is still 23 rows, never a clipped 24th.
        assert_eq!(fit_cells(800.0, 399.9, cell).rows, 23);
    }

    #[test]
    fn degenerate_metrics_fall_back() {
        assert_eq!(fit_cells(800.0, 400.0, CellSize::new(0.0, 0.0)), TerminalSize::default());
        assert_eq!(
            fit_cells(f32::NAN, 400.0, CellSize::new(8.0, 17.0)),
            TerminalSize::default()
        );
        assert_eq!(fit_cells(4.0, 4.0, CellSize::new(8.0, 17.0)), TerminalSize::default());
    }

    #[test]
    fn clamps_absurd_areas() {
        let size = fit_cells(1e9, 1e9, CellSize::new(1.0, 1.0));
        assert_eq!(size.cols, u16::MAX);
        assert_eq!(size.rows, u16::MAX);
    }
}
