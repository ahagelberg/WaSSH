//! The session tab strip.
//!
//! Metrics: a 36 px strip on `bg-panel-2` with
//! a 1 px bottom edge, 2 px gaps, tabs capped at 220 px with a 2 px active border and a
//! 4 px accent bar when the session has a colour.

use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Ui, Vec2};
use wassh_core::{Palette, Rgba, SessionStatus, colors};

use crate::terminal_view::to_color32;
use crate::widgets::truncating_galley;

/// Height of the strip.
pub const TABBAR_HEIGHT: f32 = 36.0;
/// Maximum width of one tab.
pub const TAB_MAX_WIDTH: f32 = 220.0;
/// Preferred minimum width of one tab.
pub const TAB_MIN_WIDTH: f32 = 96.0;
/// Gap between tabs.
pub const TAB_GAP: f32 = 2.0;
/// Horizontal padding inside a tab.
pub const TAB_PADDING_X: f32 = 10.0;
/// Gap between the dot, the title and the close button.
pub const TAB_GAP_ITEMS: f32 = 6.0;
/// Width of the colour bar at the left edge of a tab.
pub const TAB_ACCENT_BAR: f32 = 4.0;
/// Diameter of the status dot.
pub const STATUS_DOT: f32 = 8.0;
/// Border width of the active tab.
pub const ACTIVE_BORDER: f32 = 2.0;
/// Width of the drag insertion indicator.
pub const DROP_INDICATOR: f32 = 2.0;
/// Height of the "new tab" button.
pub const NEW_TAB_WIDTH: f32 = 28.0;

/// One tab to draw.
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a> {
    /// Stable session id.
    pub id: &'a str,
/// Inset of the "new tab" button box inside its slot.
const NEW_TAB_INSET: Vec2 = Vec2::new(3.0, 7.0);
/// Font size of the "new tab" glyph.
const NEW_TAB_GLYPH_SIZE: f32 = 14.0;
/// Font size of the tab title.
const TAB_TITLE_SIZE: f32 = 12.0;
    /// Title shown in the strip.
    pub title: &'a str,
    /// Session status, controlling the dot colour.
    pub status: SessionStatus,
    /// Whether this is the visible tab.
    pub active: bool,
    /// Optional per-session accent.
    pub accent: Option<Rgba>,
}

/// What the user did to the tab strip this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabAction {
    /// Activate a tab.
    Select(String),
    /// Close a tab.
    Close(String),
    /// Open a new local tab.
    NewTab,
    /// Right-clicked a tab at the given position.
    Context(String, (i32, i32)),
    /// Drag reordered tabs.
    Reorder {
        /// Original index.
        from: usize,
        /// Index after removal.
        to: usize,
    },
}

/// Colour of the status dot.
pub fn status_color(status: SessionStatus, palette: &Palette) -> Color32 {
    if status.is_in_progress() {
        to_color32(palette.warn)
    } else {
        match status {
            SessionStatus::Connected => to_color32(palette.ok),
            SessionStatus::Failed | SessionStatus::Disconnected => to_color32(palette.danger),
            _ => to_color32(palette.text_muted),
        }
    }
}

/// Width of every tab for `count` tabs in `available` pixels.
pub fn tab_width(count: usize, available: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let gaps = TAB_GAP * (count.saturating_sub(1)) as f32;
    let share = (available - gaps - NEW_TAB_WIDTH) / count as f32;
    share.clamp(TAB_MIN_WIDTH.min(TAB_MAX_WIDTH), TAB_MAX_WIDTH)
}

/// Cross-frame drag state for the strip.
#[derive(Default)]
pub struct TabBarState {
    drag: Option<DragState>,
}

#[derive(Default)]
struct DragState {
    from: usize,
    active: bool,
}

impl TabBarState {
    /// Nothing being dragged.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.active)
    }
}

/// Draw the tab strip and collect the user's actions.
pub fn tab_bar(
    ui: &mut Ui,
    palette: &Palette,
    tabs: &[TabView<'_>],
    state: &mut TabBarState,
) -> Vec<TabAction> {
    let mut actions = Vec::new();
    let (bar, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), TABBAR_HEIGHT),
        Sense::hover(),
    );
    ui.painter().rect_filled(bar, 0.0, to_color32(palette.bg_panel_2));
    ui.painter().hline(
        bar.x_range(),
        bar.bottom(),
        egui::Stroke::new(1.0, to_color32(palette.border)),
    );

    let width = tab_width(tabs.len(), bar.width());
    let mut rects = Vec::with_capacity(tabs.len());
    let mut x = bar.left() + 8.0;
    for (index, tab) in tabs.iter().enumerate() {
        let rect = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(width, TABBAR_HEIGHT));
        rects.push(rect);
        x += width + TAB_GAP;
        draw_tab(ui, palette, tab, rect, index, &mut actions, state);
    }

    // New tab button at the end of the strip.
    let new_rect = Rect::from_min_size(
        Pos2::new(x + 2.0, bar.top()),
        Vec2::new(NEW_TAB_WIDTH, TABBAR_HEIGHT),
    );
    let new_button = ui.interact(new_rect, Id::unique("wassh-tab-new"), Sense::click());
    let colour = if new_button.hovered() {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    if new_button.hovered() {
        ui.painter()
            .rect_filled(new_rect.shrink2(Vec2::new(2.0, 6.0)), 4, to_color32(palette.bg_panel));
    }
    ui.painter().text(
        new_rect.center(),
        Align2::CENTER_CENTER,
        "+",
        FontId::proportional(16.0),
        colour,
    );
    if new_button.clicked() {
        actions.push(TabAction::NewTab);
    }

    // Finish or cancel a drag once the pointer is released.
    let released = ui.ctx().input(|input| input.pointer.any_released());
    if released
        && let Some(drag) = state.drag.take()
        && drag.active
    {
        let pointer = ui
            .ctx()
            .input(|input| input.pointer.interact_pos())
            .unwrap_or(bar.center());
        let mids: Vec<f32> = rects.iter().map(|rect| rect.center().x).collect();
        let mut to = crate::widgets::splitter::drop_index(pointer.x, &mids);
        if drag.from < to {
            to = to.saturating_sub(1);
        }
        if to != drag.from {
            actions.push(TabAction::Reorder { from: drag.from, to });
        }
    }

    actions
}

fn draw_tab(
    ui: &mut Ui,
    palette: &Palette,
    tab: &TabView<'_>,
    rect: Rect,
    index: usize,
    actions: &mut Vec<TabAction>,
    state: &mut TabBarState,
) {
    let response = ui.interact(rect, Id::unique(("wassh-tab", index)), Sense::click_and_drag());
    let dragging_this = state
        .drag
        .as_ref()
        .is_some_and(|drag| drag.from == index && drag.active);

    // Backdrop: the active tab merges with the terminal area below.
    let backdrop = if tab.active {
        palette.bg_app
    } else if response.hovered() {
        palette.bg_panel
    } else {
        palette.bg_panel_2
    };
    let fill = colors::row_background(tab.accent, backdrop, tab.active, response.hovered());
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.bottom() + 1.0)),
        egui::CornerRadius {
            nw: wassh_core::RADIUS as u8,
            ne: wassh_core::RADIUS as u8,
            sw: 0,
            se: 0,
        },
        to_color32(fill),
    );

    if tab.active {
        let border = to_color32(palette.tab_active_border);
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius {
                nw: wassh_core::RADIUS as u8,
                ne: wassh_core::RADIUS as u8,
                sw: 0,
                se: 0,
            },
            egui::Stroke::new(ACTIVE_BORDER, border),
            egui::StrokeKind::Inside,
        );
        // Repaint the bottom edge so the active tab reads as connected to the terminal.
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, to_color32(palette.bg_app)),
        );
    }

    let mut left = rect.left() + TAB_PADDING_X;
    if let Some(accent) = tab.accent {
        let bar = Rect::from_min_max(
            Pos2::new(left, rect.top() + 6.0),
            Pos2::new(left + TAB_ACCENT_BAR, rect.bottom() - 6.0),
        );
        ui.painter()
            .rect_filled(bar, 2, to_color32(accent).gamma_multiply(if tab.active { 1.0 } else { 0.8 }));
        left += TAB_ACCENT_BAR + TAB_GAP_ITEMS;
    }

    // Status dot.
    let dot_centre = Pos2::new(left + STATUS_DOT / 2.0, rect.center().y);
    ui.painter().circle_filled(
        dot_centre,
        STATUS_DOT / 2.0,
        status_color(tab.status, palette),
    );
    left += STATUS_DOT + TAB_GAP_ITEMS;

    // Close button.
    let close_rect = Rect::from_center_size(
        Pos2::new(rect.right() - TAB_PADDING_X - 5.0, rect.center().y),
        Vec2::splat(16.0),
    );
    let close = ui.interact(
        close_rect,
        Id::unique(("wassh-tab-close", index)),
        Sense::click(),
    );
    let close_colour = if close.hovered() {
        to_color32(palette.danger)
    } else {
        to_color32(palette.text_muted)
    };
    ui.painter().text(
        close_rect.center(),
        Align2::CENTER_CENTER,
        "\u{2715}",
        FontId::proportional(11.0),
        close_colour,
    );

    // Title: one line, truncated rather than wrapped; hovering shows the full text.
    let title_rect = Rect::from_min_max(
        Pos2::new(left, rect.top()),
        Pos2::new(close_rect.left() - 4.0, rect.bottom()),
    );
    let colour = if tab.active {
        to_color32(palette.text)
    } else {
        to_color32(palette.text_muted)
    };
    let galley = truncating_galley(ui, tab.title, TAB_TITLE_SIZE, colour, title_rect.width());
    ui.painter().with_clip_rect(title_rect).galley(
        Pos2::new(title_rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );

    // Interaction.
    if close.clicked() {
        actions.push(TabAction::Close(tab.id.to_owned()));
        return;
    }
    if response.clicked() {
        actions.push(TabAction::Select(tab.id.to_owned()));
    }
    if response.secondary_clicked() {
        let pos = ui
            .ctx()
            .input(|input| input.pointer.interact_pos())
            .unwrap_or(rect.center());
        actions.push(TabAction::Context(
            tab.id.to_owned(),
            (pos.x.round() as i32, pos.y.round() as i32),
        ));
    }
    if response.drag_started() {
        state.drag = Some(DragState {
            from: index,
            active: false,
        });
    }
    if let Some(drag) = state.drag.as_mut()
        && drag.from == index
        && response.dragged()
    {
        drag.active = true;
    }
    if dragging_this {
        ui.painter().rect_stroke(
            rect,
            wassh_core::RADIUS,
            egui::Stroke::new(1.0, to_color32(palette.accent)),
            egui::StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_width_is_capped_and_floored() {
        assert_eq!(tab_width(1, 1200.0), TAB_MAX_WIDTH);
        assert_eq!(tab_width(20, 400.0), TAB_MIN_WIDTH);
        let natural = tab_width(4, 4.0 * 150.0 + 3.0 * TAB_GAP + NEW_TAB_WIDTH);
        assert!((natural - 150.0).abs() < 0.01, "got {natural}");
    }

    #[test]
    fn no_tabs_no_width() {
        assert_eq!(tab_width(0, 800.0), 0.0);
    }

    #[test]
    fn status_colours_follow_the_css() {
        let palette = Palette::dark();
        assert_eq!(status_color(SessionStatus::Connected, &palette), to_color32(palette.ok));
        assert_eq!(
            status_color(SessionStatus::Connecting, &palette),
            to_color32(palette.warn)
        );
        assert_eq!(
            status_color(SessionStatus::Failed, &palette),
            to_color32(palette.danger)
        );
        assert_eq!(
            status_color(SessionStatus::Closed, &palette),
            to_color32(palette.text_muted)
        );
    }
}
    response.on_hover_text(tab.title);
