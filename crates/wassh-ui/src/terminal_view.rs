//! Terminal widget: whole-cell layout and grid painting.
//!
//! This is the Phase 0 spike renderer. It draws per-style background runs and text runs
//! into egui's painter, using the platform monospace font. The glyph atlas
//! (`wassh-term-render`) replaces the text-run drawing later without changing this API.

use egui::{Align2, Color32, FontId, Rect, Sense, Ui};
use wassh_core::{Palette, Rgba};
use wassh_term::{ScreenSnapshot, TerminalSize};

/// Converts the palette colour type to egui's.
pub fn to_color32(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Stateless terminal widget; the snapshot is owned by the caller.
#[derive(Clone, Debug)]
pub struct TerminalView {
    pub font_size: f32,
    pub padding: f32,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            padding: 4.0,
        }
    }
}

impl TerminalView {
    /// Character-cell size derived from the active font.
    pub fn cell_size(ui: &Ui, font_size: f32) -> (f32, f32) {
        let font = FontId::monospace(font_size);
        let galley =
            ui.painter()
                .layout_no_wrap("MMMMMMMMMM".to_owned(), font, Color32::WHITE);
        let width = (galley.size().x / 10.0).max(1.0);
        let height = galley.size().y.max(1.0);
        (width, height)
    }

    /// Terminal size that fits into `available`, in whole cells.
    pub fn size_for(&self, ui: &Ui, available: egui::Vec2) -> TerminalSize {
        let (cell_w, cell_h) = Self::cell_size(ui, self.font_size);
        let width = (available.x - self.padding * 2.0).max(cell_w);
        let height = (available.y - self.padding * 2.0).max(cell_h);
        TerminalSize::new(
            (width / cell_w).floor().clamp(2.0, 1000.0) as u16,
            (height / cell_h).floor().clamp(1.0, 1000.0) as u16,
        )
    }

    /// The grid cell under `pos`, given the rect [`Self::show`] drew into.
    pub fn cell_at(
        &self,
        ui: &Ui,
        rect: Rect,
        snapshot: &ScreenSnapshot,
        pos: egui::Pos2,
    ) -> Option<(usize, usize)> {
        let (cell_w, cell_h) = Self::cell_size(ui, self.font_size);
        let x = pos.x - (rect.left() + self.padding);
        let y = pos.y - (rect.top() + self.padding);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let col = (x / cell_w).floor() as usize;
        let row = (y / cell_h).floor() as usize;
        if row >= usize::from(snapshot.size.rows) || col >= usize::from(snapshot.size.cols) {
            return None;
        }
        Some((row, col))
    }

    /// Draw the snapshot and return the widget response.
    pub fn show(&self, ui: &mut Ui, snapshot: &ScreenSnapshot, palette: &Palette) -> egui::Response {
        let available = ui.available_size();
        let (cell_w, cell_h) = Self::cell_size(ui, self.font_size);
        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        let background = to_color32(snapshot.base_style.bg);
        painter.rect_filled(rect, 0.0, background);
        let selection = palette.term_selection;
        let selection_colour = Color32::from_rgba_unmultiplied(
            selection.r,
            selection.g,
            selection.b,
            selection.a.max(90),
        );

        let font = FontId::monospace(self.font_size);
        let origin_x = rect.left() + self.padding;
        let origin_y = rect.top() + self.padding;
        let cols = usize::from(snapshot.size.cols);
        let rows = usize::from(snapshot.size.rows);

        for row in 0..rows {
            let y = origin_y + row as f32 * cell_h;
            if y + cell_h > rect.bottom() {
                break;
            }
            let mut col = 0usize;
            while col < cols {
                let Some(cell) = snapshot.cell(row, col) else {
                    break;
                };
                let style = cell.style;
                let selected = snapshot.is_selected(row, col);
                // Extend the run while the style (and the selection state) stays identical.
                let mut end = col + 1;
                while end < cols {
                    match snapshot.cell(row, end) {
                        Some(next)
                            if style.same_run(&next.style)
                                && snapshot.is_selected(row, end) == selected =>
                        {
                            end += 1;
                        }
                        _ => break,
                    }
                }

                let x = origin_x + col as f32 * cell_w;
                let width = (end - col) as f32 * cell_w;
                if selected {
                    // A translucent wash keeps the glyphs readable, like a terminal's own
                    // selection colour.
                    painter.rect_filled(
                        Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, cell_h)),
                        0.0,
                        selection_colour,
                    );
                } else if style.bg != snapshot.base_style.bg {
                    painter.rect_filled(
                        Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, cell_h)),
                        0.0,
                        to_color32(style.bg),
                    );
                }

                let text: String = (col..end)
                    .filter_map(|c| snapshot.cell(row, c))
                    .filter(|c| !c.style.wide_spacer)
                    .map(|c| c.ch)
                    .collect();
                if !text.trim().is_empty() {
                    let color = if style.hidden {
                        to_color32(style.bg)
                    } else {
                        to_color32(style.fg)
                    };
                    painter.text(
                        egui::pos2(x, y),
                        Align2::LEFT_TOP,
                        &text,
                        font.clone(),
                        color,
                    );
                }

                if style.underline {
                    let line_y = y + cell_h - 1.0;
                    painter.line_segment(
                        [egui::pos2(x, line_y), egui::pos2(x + width, line_y)],
                        (1.0, to_color32(style.fg)),
                    );
                }
                if style.strikeout {
                    let line_y = y + cell_h * 0.5;
                    painter.line_segment(
                        [egui::pos2(x, line_y), egui::pos2(x + width, line_y)],
                        (1.0, to_color32(style.fg)),
                    );
                }

                col = end;
            }
        }

        if snapshot.cursor.visible
            && snapshot.cursor.row < rows
            && snapshot.cursor.col < cols
        {
            let x = origin_x + snapshot.cursor.col as f32 * cell_w;
            let y = origin_y + snapshot.cursor.row as f32 * cell_h;
            let cursor_rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w, cell_h));
            let accent = palette.accent;
            let cursor_color =
                Color32::from_rgba_unmultiplied(accent.r, accent.g, accent.b, 160);
            painter.rect_filled(cursor_rect, 0.0, cursor_color);
        }

        response
    }
}
