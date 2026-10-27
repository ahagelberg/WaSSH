//! Table primitives.
//!
//! Header clicks report the column index; the caller owns sort state and row data. Cells are
//! clipped to their column so long commands and paths stay inside the table.

use egui::{Align2, FontId, Rect, Response, Sense, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

const HEADER_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 24.0;
const CELL_PADDING: f32 = 8.0;

/// One column definition.
#[derive(Clone, Copy, Debug)]
pub struct TableColumn {
    pub label: &'static str,
    /// Relative width; fractions are normalised across all columns.
    pub fraction: f32,
    /// Right-align numeric columns.
    pub numeric: bool,
}

impl TableColumn {
    pub fn new(label: &'static str, fraction: f32) -> Self {
        Self {
            label,
            fraction,
            numeric: false,
        }
    }

    pub fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }
}

/// Width of each column for a total width, normalising the configured fractions.
pub fn column_widths(total: f32, columns: &[TableColumn]) -> Vec<f32> {
    if columns.is_empty() {
        return Vec::new();
    }
    let sum: f32 = columns.iter().map(|column| column.fraction.max(0.0)).sum();
    if sum <= 0.0 {
        return vec![total / columns.len() as f32; columns.len()];
    }
    columns
        .iter()
        .map(|column| total * column.fraction.max(0.0) / sum)
        .collect()
}

/// Draw a header row. Returns the clicked column index.
pub fn table_header(
    ui: &mut Ui,
    palette: &Palette,
    columns: &[TableColumn],
    sort: Option<(usize, bool)>,
) -> Option<usize> {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, HEADER_HEIGHT), Sense::click());
    ui.painter()
        .rect_filled(rect, 0.0, to_color32(palette.bg_panel_2));

    let widths = column_widths(width, columns);
    let font = FontId::proportional(11.0);
    let colour = to_color32(palette.text_muted);
    let mut x = rect.left();
    for (index, (column, column_width)) in columns.iter().zip(widths.iter()).enumerate() {
        let active = sort.is_some_and(|(sorted, _)| sorted == index);
        let arrow = if active {
            if sort.is_some_and(|(_, descending)| descending) {
                " ▼"
            } else {
                " ▲"
            }
        } else {
            ""
        };
        let label = format!("{}{}", column.label, arrow);
        let (position, align) = if column.numeric {
            (
                egui::pos2(x + column_width - CELL_PADDING, rect.center().y),
                Align2::RIGHT_CENTER,
            )
        } else {
            (
                egui::pos2(x + CELL_PADDING, rect.center().y),
                Align2::LEFT_CENTER,
            )
        };
        ui.painter()
            .text(position, align, label, font.clone(), colour);
        x += column_width;
    }
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        (1.0, to_color32(palette.border)),
    );

    if response.clicked()
        && let Some(position) = response.interact_pointer_pos()
    {
        let mut left = rect.left();
        for (index, column_width) in widths.iter().enumerate() {
            if position.x >= left && position.x < left + column_width {
                return Some(index);
            }
            left += column_width;
        }
    }
    None
}

/// Draw one body row. Returns the row's response for selection handling.
pub fn table_row(
    ui: &mut Ui,
    palette: &Palette,
    columns: &[TableColumn],
    cells: &[&str],
    selected: bool,
) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());

    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, to_color32(palette.accent).gamma_multiply(0.22));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, to_color32(palette.bg_panel_2));
    }

    let widths = column_widths(width, columns);
    let font = FontId::proportional(12.0);
    let colour = to_color32(palette.text);
    let mut x = rect.left();
    for (index, (column, column_width)) in columns.iter().zip(widths.iter()).enumerate() {
        let text = cells.get(index).copied().unwrap_or("");
        let cell_rect = Rect::from_min_size(
            egui::pos2(x, rect.top()),
            Vec2::new(*column_width, rect.height()),
        );
        let painter = ui.painter().with_clip_rect(cell_rect);
        if column.numeric {
            painter.text(
                egui::pos2(cell_rect.right() - CELL_PADDING, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                font.clone(),
                colour,
            );
        } else {
            painter.text(
                egui::pos2(cell_rect.left() + CELL_PADDING, rect.center().y),
                Align2::LEFT_CENTER,
                text,
                font.clone(),
                colour,
            );
        }
        x += column_width;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> [TableColumn; 3] {
        [
            TableColumn::new("Name", 0.46),
            TableColumn::new("Size", 0.14).numeric(),
            TableColumn::new("Modified", 0.40),
        ]
    }

    #[test]
    fn widths_are_proportional() {
        let widths = column_widths(1000.0, &columns());
        assert!((widths[0] - 460.0).abs() < 0.01);
        assert!((widths[1] - 140.0).abs() < 0.01);
        assert!((widths[2] - 400.0).abs() < 0.01);
        assert!((widths.iter().sum::<f32>() - 1000.0).abs() < 0.01);
    }

    #[test]
    fn widths_are_normalised_when_they_do_not_sum_to_one() {
        let widths = column_widths(300.0, &[TableColumn::new("a", 1.0), TableColumn::new("b", 2.0)]);
        assert!((widths[0] - 100.0).abs() < 0.01);
        assert!((widths[1] - 200.0).abs() < 0.01);
    }

    #[test]
    fn empty_or_zero_fractions_fall_back_to_equal_widths() {
        assert!(column_widths(100.0, &[]).is_empty());
        let widths = column_widths(150.0, &[TableColumn::new("a", 0.0), TableColumn::new("b", 0.0)]);
        assert_eq!(widths, vec![75.0, 75.0]);
    }

    #[test]
    fn negative_fractions_are_ignored() {
        let widths = column_widths(100.0, &[TableColumn::new("a", -1.0), TableColumn::new("b", 1.0)]);
        assert_eq!(widths, vec![0.0, 100.0]);
    }
}
