//! Terminal widget: whole-cell layout and grid painting.
//!
//! The grid is painted per style run into egui's painter, in the monospace family the
//! session's settings name.

use egui::{Align2, Color32, FontId, Rect, Sense, Ui};
use wassh_core::{Palette, Rgba};
use wassh_term::{ScreenSnapshot, TerminalSize};

use crate::fonts;

/// Glyphs measured to derive one cell's width.
const CELL_PROBE: &str = "MMMMMMMMMM";

/// Converts the palette colour type to egui's.
pub fn to_color32(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Converts a palette colour with a replaced alpha.
pub fn with_alpha(color: Rgba, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, alpha)
}

/// Stateless terminal widget; the snapshot is owned by the caller.
#[derive(Clone, Debug)]
pub struct TerminalView {
    pub font_size: f32,
    /// Monospace family from the session's style; empty means egui's own.
    pub font_family: String,
    pub padding: f32,
    /// Text of one style run, reused so a frame does not allocate once per run.
    run: std::cell::RefCell<String>,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: String::new(),
            padding: 4.0,
            run: std::cell::RefCell::new(String::new()),
        }
    }
}

impl TerminalView {
    /// A view with `font_size` chosen by the session's style.
    pub fn with_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            ..Self::default()
        }
    }

    /// The font the grid is drawn with.
    fn font(&self) -> FontId {
        FontId::new(self.font_size, fonts::terminal_family(&self.font_family))
    }

    /// Character-cell size derived from the active font.
    ///
    /// The probe is a fixed string, so egui's galley cache answers every repeat of it; a
    /// measurement of our own would outlive a font atlas that gains the family later, and
    /// the grid would then keep a cell the glyphs no longer advance by.
    pub fn cell_size(&self, ui: &Ui) -> (f32, f32) {
        let galley = ui
            .painter()
            .layout_no_wrap(CELL_PROBE.to_owned(), self.font(), Color32::WHITE);
        (
            (galley.size().x / CELL_PROBE.len() as f32).max(1.0),
            galley.size().y.max(1.0),
        )
    }

    /// Terminal size that fits into `available`, in whole cells.
    pub fn size_for(&self, ui: &Ui, available: egui::Vec2) -> TerminalSize {
        let (cell_w, cell_h) = self.cell_size(ui);
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
        size: TerminalSize,
        pos: egui::Pos2,
    ) -> Option<(usize, usize)> {
        let (cell_w, cell_h) = self.cell_size(ui);
        let x = pos.x - (rect.left() + self.padding);
        let y = pos.y - (rect.top() + self.padding);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let col = (x / cell_w).floor() as usize;
        let row = (y / cell_h).floor() as usize;
        if row >= usize::from(size.rows) || col >= usize::from(size.cols) {
            return None;
        }
        Some((row, col))
    }

    /// Draw the snapshot and return the widget response.
    pub fn show(&self, ui: &mut Ui, snapshot: &ScreenSnapshot, palette: &Palette) -> egui::Response {
        let available = ui.available_size();
        let (cell_w, cell_h) = self.cell_size(ui);
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

        let font = self.font();
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

                let mut text = self.run.borrow_mut();
                text.clear();
                for run_column in col..end {
                    if let Some(cell) = snapshot.cell(row, run_column)
                        && !cell.style.wide_spacer
                    {
                        text.push(cell.ch);
                    }
                }
                if !text.trim().is_empty() {
                    let color = if style.hidden {
                        to_color32(style.bg)
                    } else {
                        to_color32(style.fg)
                    };
                    painter.text(
                        egui::pos2(x, y),
                        Align2::LEFT_TOP,
                        &*text,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell size of a view drawn with `family`.
    fn cell_of(ctx: &egui::Context, family: &str) -> (f32, f32) {
        let view = TerminalView {
            font_family: family.to_owned(),
            ..TerminalView::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(400.0, 200.0),
            )),
            ..Default::default()
        };
        let mut cell = (0.0, 0.0);
        let output = ctx.run_ui(input, |ui| {
            cell = view.cell_size(ui);
        });
        output.drop_without_applying_deltas();
        cell
    }

    /// One frame's cell size and the advance the same font gives a glyph run.
    fn cell_and_advance(ctx: &egui::Context, view: &TerminalView) -> (f32, f32) {
        let font = view.font();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(400.0, 200.0),
            )),
            ..Default::default()
        };
        let mut measured = (0.0, 0.0);
        let output = ctx.run_ui(input, |ui| {
            let (cell_w, _) = view.cell_size(ui);
            let galley = ui.painter().layout_no_wrap(
                CELL_PROBE.to_owned(),
                font.clone(),
                Color32::WHITE,
            );
            measured = (cell_w, galley.size().x / CELL_PROBE.len() as f32);
        });
        output.drop_without_applying_deltas();
        measured
    }

    /// Installs `family` into `ctx` as a named family, the way the app registers it.
    fn install_family(ctx: &egui::Context, family: &str) {
        let face = fonts::face_for(family).expect("the family is installed on this host");
        let mut definitions = egui::FontDefinitions::default();
        let key = format!("term:{family}");
        definitions.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(
                std::fs::read(&face.path).expect("read the face"),
            )),
        );
        let mut chain = vec![key];
        chain.extend(
            definitions
                .families
                .get(&egui::FontFamily::Monospace)
                .cloned()
                .unwrap_or_default(),
        );
        definitions
            .families
            .insert(egui::FontFamily::Name(std::sync::Arc::from(family)), chain);
        ctx.set_fonts(definitions);
    }

    #[test]
    fn a_family_the_atlas_gains_later_moves_the_cell_with_the_glyphs() {
        // Only a host that has a second monospace face can tell the fallback from the real
        // one, and a session opens before its family is in the atlas: the first frame
        // measures egui's face, the next draws the family's glyphs.
        let faces = fonts::installed();
        let Some(other) = faces
            .iter()
            .find(|family| family.to_lowercase().contains("consolas"))
        else {
            return;
        };
        let ctx = egui::Context::default();
        let view = TerminalView {
            font_family: other.to_owned(),
            ..TerminalView::default()
        };
        let (fallback, _) = cell_and_advance(&ctx, &view);
        install_family(&ctx, other);
        let (cell, advance) = cell_and_advance(&ctx, &view);
        assert!(
            cell < fallback,
            "the family did not reach the atlas, so this says nothing"
        );
        assert!(
            (cell - advance).abs() < 0.05,
            "the grid cell is {cell} but the glyphs advance by {advance}"
        );
    }

    #[test]
    fn the_cell_follows_the_family_the_session_names() {
        // Only a host that has a second monospace face can tell the two apart.
        let faces = fonts::installed();
        let Some(other) = faces
            .iter()
            .find(|family| family.to_lowercase().contains("consolas"))
        else {
            return;
        };
        let ctx = egui::Context::default();
        fonts::ensure(&ctx, std::slice::from_ref(other));
        assert_ne!(
            cell_of(&ctx, other),
            cell_of(&ctx, ""),
            "{other} measures like egui's own face, so the setting is not in use"
        );
    }

    #[test]
    fn a_family_the_host_lacks_keeps_the_egui_cell() {
        let ctx = egui::Context::default();
        fonts::ensure(&ctx, &[]);
        assert_eq!(
            cell_of(&ctx, "no-such-font-anywhere"),
            cell_of(&ctx, ""),
            "an unknown family must fall back, not measure nothing"
        );
    }
}
