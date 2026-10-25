//! Tree rows with expand/collapse.
//!
//! Used by the host group list and the MQTT topic browser. A row is one line: optional
//! twisty, indented label, and an optional trailing badge.

use egui::{Align2, FontId, Rect, Sense, Ui, Vec2};
use wassh_core::Palette;

use super::surface::Tone;
use crate::terminal_view::to_color32;

/// Height of one row.
pub const ROW_HEIGHT: f32 = 24.0;
/// Horizontal indent per depth level.
pub const INDENT: f32 = 14.0;
/// Width of the twisty hit area.
pub const TWISTY_WIDTH: f32 = 16.0;

/// One row to draw.
#[derive(Clone, Copy, Debug)]
pub struct TreeRow<'a> {
    /// Row text.
    pub label: &'a str,
    /// Nesting depth, starting at 0.
    pub depth: u16,
    /// Whether the row can be expanded.
    pub expandable: bool,
    /// Whether the row is currently expanded.
    pub expanded: bool,
    /// Whether the row is selected.
    pub selected: bool,
    /// Optional trailing badge text.
    pub badge: Option<&'a str>,
    /// Colour of the badge.
    pub badge_tone: Tone,
    /// Draw the label muted.
    pub muted: bool,
}

impl<'a> TreeRow<'a> {
    /// A plain leaf row.
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            depth: 0,
            expandable: false,
            expanded: false,
            selected: false,
            badge: None,
            badge_tone: Tone::Neutral,
            muted: false,
        }
    }

    /// Indent the row.
    pub fn depth(mut self, depth: u16) -> Self {
        self.depth = depth;
        self
    }

    /// Make the row a branch with the given expansion state.
    pub fn branch(mut self, expanded: bool) -> Self {
        self.expandable = true;
        self.expanded = expanded;
        self
    }

    /// Mark the row as selected.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Attach a trailing badge.
    pub fn badge(mut self, text: &'a str, tone: Tone) -> Self {
        self.badge = Some(text);
        self.badge_tone = tone;
        self
    }

    /// Draw the label muted.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// X offset of the twisty for this depth.
    pub fn twisty_x(&self) -> f32 {
        self.depth as f32 * INDENT
    }
}

/// What the user did to a row this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeRowResponse {
    /// The row body was clicked.
    pub clicked: bool,
    /// The twisty was clicked; `expanded` should be flipped.
    pub toggled: bool,
    /// The row body was double-clicked.
    pub double_clicked: bool,
}

/// Draw one tree row.
pub fn tree_row(ui: &mut Ui, palette: &Palette, row: TreeRow<'_>) -> TreeRowResponse {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ROW_HEIGHT),
        Sense::click(),
    );
    let mut out = TreeRowResponse {
        clicked: response.clicked(),
        toggled: false,
        double_clicked: response.double_clicked(),
    };

    if row.selected {
        ui.painter().rect_filled(
            rect,
            wassh_core::RADIUS,
            to_color32(palette.accent).gamma_multiply(0.28),
        );
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, wassh_core::RADIUS, to_color32(palette.bg_panel_2));
    }

    let text_left = rect.left() + 8.0 + row.twisty_x() + TWISTY_WIDTH;
    let text_colour = if row.muted {
        to_color32(palette.text_muted)
    } else {
        to_color32(palette.text)
    };

    if row.expandable {
        let twisty_rect = Rect::from_center_size(
            egui::Pos2::new(rect.left() + 8.0 + row.twisty_x() + TWISTY_WIDTH / 2.0, rect.center().y),
            Vec2::new(TWISTY_WIDTH, ROW_HEIGHT),
        );
        let twisty = ui.interact(
            twisty_rect,
            response.id.with("twisty"),
            Sense::click(),
        );
        let glyph = if row.expanded { "\u{25be}" } else { "\u{25b8}" };
        let glyph_colour = if twisty.hovered() {
            to_color32(palette.text)
        } else {
            to_color32(palette.text_muted)
        };
        ui.painter().text(
            twisty_rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(11.0),
            glyph_colour,
        );
        if twisty.clicked() {
            out.toggled = true;
            out.clicked = false;
        }
    }

    let badge_width = row.badge.map_or(0.0, |text| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(11.0), text_colour)
            .size()
            .x
            + 16.0
    });
    let label_rect = Rect::from_min_max(
        egui::Pos2::new(text_left, rect.top()),
        egui::Pos2::new(rect.right() - badge_width - 8.0, rect.bottom()),
    );
    if label_rect.width() > 0.0 {
        let painter = ui.painter().with_clip_rect(label_rect);
        painter.text(
            egui::Pos2::new(label_rect.left(), label_rect.center().y),
            Align2::LEFT_CENTER,
            row.label,
            FontId::proportional(13.0),
            text_colour,
        );
    }

    if let Some(text) = row.badge {
        ui.painter().text(
            egui::Pos2::new(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            text,
            FontId::proportional(11.0),
            row.badge_tone.color(palette),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twisty_indent_grows_with_depth() {
        assert_eq!(TreeRow::new("a").twisty_x(), 0.0);
        assert_eq!(TreeRow::new("a").depth(3).twisty_x(), 3.0 * INDENT);
    }

    #[test]
    fn branch_sets_expandable_and_state() {
        let row = TreeRow::new("group").branch(true);
        assert!(row.expandable && row.expanded);
        let collapsed = TreeRow::new("group").branch(false);
        assert!(collapsed.expandable && !collapsed.expanded);
    }

    #[test]
    fn leaves_are_not_expandable() {
        assert!(!TreeRow::new("host").expandable);
    }
}
