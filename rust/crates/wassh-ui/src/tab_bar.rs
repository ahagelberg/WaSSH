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
/// Horizontal padding at both ends of the strip.
const TAB_STRIP_PADDING: f32 = 8.0;
/// Gap between the last tab and the "new tab" button.
const NEW_TAB_GAP: f32 = 2.0;
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
/// How far the active tab overlaps the edge below the strip so it reads as connected.
const TAB_BOTTOM_OVERLAP: f32 = 1.0;
/// Width of the drag insertion indicator.
pub const DROP_INDICATOR: f32 = 2.0;
/// Width of the "new tab" button.
pub const NEW_TAB_WIDTH: f32 = 28.0;
/// Inset of the "new tab" button box inside its slot.
const NEW_TAB_INSET: Vec2 = Vec2::new(3.0, 7.0);
/// Font size of the "new tab" glyph.
const NEW_TAB_GLYPH_SIZE: f32 = 14.0;
/// Font size of the tab title.
const TAB_TITLE_SIZE: f32 = 12.0;

/// One tab to draw.
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a> {
    /// Stable session id.
    pub id: &'a str,
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
    let share = (tabs_space(available) - gaps) / count as f32;
    share.clamp(TAB_MIN_WIDTH.min(TAB_MAX_WIDTH), TAB_MAX_WIDTH)
}

/// Space the tabs may use once the strip padding and the new tab button are reserved.
fn tabs_space(available: f32) -> f32 {
    available - TAB_STRIP_PADDING * 2.0 - NEW_TAB_WIDTH - NEW_TAB_GAP
}

/// How far the tabs can scroll sideways in `available` pixels.
fn max_scroll(count: usize, available: f32) -> f32 {
    let width = tab_width(count, available);
    let content = count.saturating_sub(1) as f32 * (width + TAB_GAP) + width;
    (content - tabs_space(available)).max(0.0)
}

/// Rect of the tab at `index` in a strip scrolled by `scroll`.
fn tab_slot(bar: Rect, width: f32, scroll: f32, index: usize) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            bar.left() + TAB_STRIP_PADDING - scroll + index as f32 * (width + TAB_GAP),
            bar.top(),
        ),
        Vec2::new(width, TABBAR_HEIGHT),
    )
}

/// Rect of the new tab button: after the last tab, at the right edge once they overflow.
fn new_tab_slot(bar: Rect, width: f32, count: usize, scroll: f32) -> Rect {
    let after_tabs = bar.left() + TAB_STRIP_PADDING - scroll + count as f32 * (width + TAB_GAP);
    Rect::from_min_size(
        Pos2::new(
            (after_tabs + NEW_TAB_GAP).min(bar.right() - TAB_STRIP_PADDING - NEW_TAB_WIDTH),
            bar.top(),
        ),
        Vec2::new(NEW_TAB_WIDTH, TABBAR_HEIGHT),
    )
}

/// Cross-frame drag and scroll state for the strip.
#[derive(Default)]
pub struct TabBarState {
    drag: Option<DragState>,
    scroll: f32,
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
    let limit = max_scroll(tabs.len(), bar.width());

    // The wheel scrolls the tabs sideways; a vertical wheel works like a horizontal one
    // and the dominant axis wins, so trackpad tilt works too.
    let over_strip = ui
        .ctx()
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| bar.contains(pos));
    if over_strip && limit > 0.0 {
        let delta = ui.ctx().input(|input| input.smooth_scroll_delta);
        let shift = -(if delta.y.abs() >= delta.x.abs() { delta.y } else { delta.x });
        state.scroll += shift;
    }
    state.scroll = state.scroll.clamp(0.0, limit);

    let new_rect = new_tab_slot(bar, width, tabs.len(), state.scroll);

    // Tabs stop short of the button and of the strip's ends, whatever the scroll offset.
    let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(bar));
    let clip = Rect::from_min_max(
        bar.min,
        Pos2::new(new_rect.left() - NEW_TAB_GAP, bar.bottom() + TAB_BOTTOM_OVERLAP),
    );
    strip.set_clip_rect(clip.intersect(ui.clip_rect()));
    let mut rects = Vec::with_capacity(tabs.len());
    for (index, tab) in tabs.iter().enumerate() {
        let rect = tab_slot(bar, width, state.scroll, index);
        rects.push(rect);
        draw_tab(&mut strip, palette, tab, rect, index, &mut actions, state);
    }

    let new_button = ui.interact(new_rect, Id::unique("wassh-tab-new"), Sense::click());
    let hovered = new_button.hovered();
    let button = new_rect.shrink2(NEW_TAB_INSET);
    ui.painter().rect_filled(
        button,
        wassh_core::RADIUS,
        to_color32(if hovered {
            palette.bg_panel
        } else {
            palette.bg_input
        }),
    );
    ui.painter().rect_stroke(
        button,
        wassh_core::RADIUS,
        egui::Stroke::new(
            1.0,
            to_color32(if hovered {
                palette.accent
            } else {
                palette.border
            }),
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        button.center(),
        Align2::CENTER_CENTER,
        "+",
        FontId::proportional(NEW_TAB_GLYPH_SIZE),
        to_color32(if hovered {
            palette.text
        } else {
            palette.text_muted
        }),
    );
    if new_button.clicked() {
        actions.push(TabAction::NewTab);
    }
    new_button.on_hover_text("New local terminal tab");

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
        Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.bottom() + TAB_BOTTOM_OVERLAP)),
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
    response.on_hover_text(tab.title);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1200.0, 600.0));
    const TITLES: [&str; 20] = [
        "t00", "t01", "t02", "t03", "t04", "t05", "t06", "t07", "t08", "t09", "t10", "t11", "t12",
        "t13", "t14", "t15", "t16", "t17", "t18", "t19",
    ];

    fn tabs(count: usize) -> Vec<TabView<'static>> {
        TITLES[..count]
            .iter()
            .map(|title| TabView {
                id: title,
                title,
                status: SessionStatus::Connected,
                active: false,
                accent: None,
            })
            .collect()
    }

    /// One frame of the strip; returns the strip rect. `pointer` and `wheel` are the input.
    fn frame(
        ctx: &egui::Context,
        tabs: &[TabView<'static>],
        state: &mut TabBarState,
        pointer: Option<Pos2>,
        wheel: Vec2,
    ) -> Rect {
        let mut input = egui::RawInput {
            screen_rect: Some(SCREEN),
            ..Default::default()
        };
        if let Some(pos) = pointer {
            input.events.push(egui::Event::PointerMoved(pos));
        }
        if wheel != Vec2::ZERO {
            input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: wheel,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut bar = Rect::NOTHING;
        ctx.run_ui(input, |ui| {
            tab_bar(ui, &Palette::dark(), tabs, state);
            bar = ui.min_rect();
        })
        .drop_without_applying_deltas();
        bar
    }

    /// Frames needed for egui's smoothed wheel delta to be fully delivered.
    const WHEEL_FRAMES: usize = 20;

    /// Turn the wheel at `pointer` and let egui finish delivering the delta.
    fn wheel(
        ctx: &egui::Context,
        tabs: &[TabView<'static>],
        state: &mut TabBarState,
        pointer: Pos2,
        delta: Vec2,
    ) {
        frame(ctx, tabs, state, Some(pointer), delta);
        for _ in 0..WHEEL_FRAMES {
            frame(ctx, tabs, state, Some(pointer), Vec2::ZERO);
        }
    }

    fn assert_scrolled(state: &TabBarState, expected: f32) {
        assert!(
            (state.scroll - expected).abs() < 0.5,
            "expected {expected}, got {}",
            state.scroll
        );
    }

    #[test]
    fn the_wheel_scrolls_the_tabs_sideways() {
        let ctx = egui::Context::default();
        let tabs = tabs(TITLES.len());
        let mut state = TabBarState::new();
        let bar = frame(&ctx, &tabs, &mut state, None, Vec2::ZERO);
        assert!(max_scroll(tabs.len(), bar.width()) > 0.0, "the tabs must overflow");
        assert_eq!(state.scroll, 0.0);

        // Wheel down reveals the tabs on the right.
        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(0.0, -40.0));
        assert_scrolled(&state, 40.0);

        // Wheel up scrolls back towards the first tab.
        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(0.0, 15.0));
        assert_scrolled(&state, 25.0);

        // Horizontal wheels and trackpad tilt work the same way.
        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(10.0, 0.0));
        assert_scrolled(&state, 15.0);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let ctx = egui::Context::default();
        let tabs = tabs(TITLES.len());
        let mut state = TabBarState::new();
        let bar = frame(&ctx, &tabs, &mut state, None, Vec2::ZERO);
        let limit = max_scroll(tabs.len(), bar.width());

        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(0.0, -10_000.0));
        assert_eq!(state.scroll, limit);

        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(0.0, 10_000.0));
        assert_eq!(state.scroll, 0.0);
    }

    #[test]
    fn the_wheel_outside_the_strip_leaves_the_tabs_alone() {
        let ctx = egui::Context::default();
        let tabs = tabs(TITLES.len());
        let mut state = TabBarState::new();
        let bar = frame(&ctx, &tabs, &mut state, None, Vec2::ZERO);

        let below = Pos2::new(bar.center().x, bar.bottom() + 200.0);
        wheel(&ctx, &tabs, &mut state, below, Vec2::new(0.0, -40.0));
        assert_eq!(state.scroll, 0.0);
    }

    #[test]
    fn tabs_that_fit_never_scroll() {
        let ctx = egui::Context::default();
        let tabs = tabs(2);
        let mut state = TabBarState::new();
        let bar = frame(&ctx, &tabs, &mut state, None, Vec2::ZERO);
        assert_eq!(max_scroll(tabs.len(), bar.width()), 0.0);

        wheel(&ctx, &tabs, &mut state, bar.center(), Vec2::new(0.0, -400.0));
        assert_eq!(state.scroll, 0.0);
    }

    #[test]
    fn closing_tabs_back_into_view_resets_the_scroll() {
        let ctx = egui::Context::default();
        let many = tabs(TITLES.len());
        let mut state = TabBarState::new();
        let bar = frame(&ctx, &many, &mut state, None, Vec2::ZERO);
        wheel(&ctx, &many, &mut state, bar.center(), Vec2::new(0.0, -400.0));
        assert!(state.scroll > 0.0);

        frame(&ctx, &tabs(2), &mut state, None, Vec2::ZERO);
        assert_eq!(state.scroll, 0.0);
    }

    #[test]
    fn tab_width_is_capped_and_floored() {
        assert_eq!(tab_width(1, 1200.0), TAB_MAX_WIDTH);
        assert_eq!(tab_width(20, 400.0), TAB_MIN_WIDTH);
        let available = 4.0 * 150.0
            + 3.0 * TAB_GAP
            + 2.0 * TAB_STRIP_PADDING
            + NEW_TAB_WIDTH
            + NEW_TAB_GAP;
        let natural = tab_width(4, available);
        assert!((natural - 150.0).abs() < 0.01, "got {natural}");
    }

    #[test]
    fn tabs_that_fit_end_before_the_new_tab_button() {
        let bar = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, TABBAR_HEIGHT));
        for count in 1..=40 {
            let width = tab_width(count, bar.width());
            if max_scroll(count, bar.width()) > 0.5 {
                continue;
            }
            let last = tab_slot(bar, width, 0.0, count - 1);
            let button = new_tab_slot(bar, width, count, 0.0);
            assert!(
                last.right() <= button.left(),
                "{count} tabs: tab ends at {} but the button starts at {}",
                last.right(),
                button.left()
            );
        }
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
