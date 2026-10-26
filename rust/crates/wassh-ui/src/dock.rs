//! Draws a tab's plugin docks around the terminal.
//!
//! The geometry rules live in `wassh_core::layout`; this module turns them into
//! rectangles, splitters and pane frames. Every pane keeps a painted header so
//! the terminal stays the only element without chrome.

use egui::{Align2, CursorIcon, FontId, Rect, Sense, Ui, UiBuilder, Vec2};
use wassh_core::{
    clamp_dock_extent, clamp_split_ratio, colors, DockEdge, LayoutNode, Palette, SplitDirection,
    TabPluginLayout, DOCK_DEFAULT_HEIGHT_PX, DOCK_DEFAULT_WIDTH_PX, INNER_DROP_BAND_PX,
    MIN_DOCK_SIZE_PX, MIN_TERMINAL_WIDTH_PX, OUTER_DROP_BAND_PX, SPLITTER_SIZE_PX,
};

use crate::terminal_view::to_color32;
use crate::widgets::splitter::SplitAxis;
use crate::widgets::truncating_galley;

/// Height of a plugin pane's title bar.
pub const PANE_HEADER_HEIGHT: f32 = 28.0;
/// Inset between a pane frame and its content.
pub const PANE_PADDING: f32 = 8.0;
/// Gap between a header status dot and its text.
const DOT_GAP: f32 = 6.0;
/// Smallest a pane may get when two panes share a dock.
const MIN_PANE_PX: f32 = 60.0;
/// Overlay pane size.
const OVERLAY_SIZE: Vec2 = Vec2::new(360.0, 400.0);

/// Why the terminal area's contents changed this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockAction {
    /// The user closed a pane.
    Close(String),
    /// The user clicked the button the pane asked for in its header.
    HeaderAction(String),
    /// A splitter moved, so the layout needs saving.
    Resized,
    /// The user dropped a dragged pane on a zone.
    Drop {
        /// Plugin that was dragged.
        plugin_id: String,
        /// Zone it was dropped on.
        target: DockTarget,
    },
}

/// A plugin being dragged towards a drop zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockDrag {
    /// Plugin being moved.
    pub plugin_id: String,
    /// True when the drag started on a pane header rather than the toolbar.
    pub from_pane: bool,
}

/// Where a dragged pane would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockTarget {
    /// A dock's outer side, further from the terminal than anything already there.
    Outer(DockEdge),
    /// A dock's inner side, next to the terminal.
    Inner(DockEdge),
    /// Floating above the terminal.
    Overlay,
}

/// Which drop zone `pointer` is over, inside `area`.
///
/// The border band of an edge inserts on that dock's outer side; the ring just inside it
/// inserts next to the terminal; everything further in floats the pane. Corners go to the
/// nearest edge.
pub fn drop_target(area: Rect, pointer: egui::Pos2) -> Option<DockTarget> {
    if !area.contains(pointer) {
        return None;
    }
    let distances = [
        (DockEdge::Left, pointer.x - area.left()),
        (DockEdge::Right, area.right() - pointer.x),
        (DockEdge::Top, pointer.y - area.top()),
        (DockEdge::Bottom, area.bottom() - pointer.y),
    ];
    let (edge, distance) = distances
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    Some(if distance <= OUTER_DROP_BAND_PX {
        DockTarget::Outer(edge)
    } else if distance <= OUTER_DROP_BAND_PX + INNER_DROP_BAND_PX {
        DockTarget::Inner(edge)
    } else {
        DockTarget::Overlay
    })
}

/// Apply a finished drag to `layout`.
pub fn apply_drop(layout: &mut TabPluginLayout, plugin_id: &str, target: DockTarget) {
    match target {
        DockTarget::Outer(edge) => {
            layout.dock_on_edge(plugin_id, edge, Some(edge.outer_zone()));
        }
        DockTarget::Inner(edge) => {
            layout.dock_on_edge(plugin_id, edge, Some(edge.inner_zone()));
        }
        DockTarget::Overlay => layout.move_to_overlay(plugin_id),
    }
}

/// The part of `area` a drop on `target` will occupy, for the drag preview.
fn preview_rect(area: Rect, layout: &TabPluginLayout, target: DockTarget) -> Rect {
    match target {
        DockTarget::Overlay => {
            let size = Vec2::new(
                OVERLAY_SIZE.x.min(area.width()),
                OVERLAY_SIZE.y.min(area.height()),
            );
            let rect = Rect::from_min_size(
                egui::pos2(
                    area.right() - size.x - 12.0,
                    area.bottom() - size.y - 12.0,
                ),
                size,
            );
            clamp_to(rect, area)
        }
        DockTarget::Outer(edge) | DockTarget::Inner(edge) => {
            let depth = clamp_dock_extent(
                layout.size(edge),
                if edge.is_vertical() {
                    area.width()
                } else {
                    area.height()
                },
            );
            let depth = if depth > 0.0 {
                depth
            } else if edge.is_vertical() {
                DOCK_DEFAULT_WIDTH_PX
            } else {
                DOCK_DEFAULT_HEIGHT_PX
            };
            let strip = edge_strip(area, edge, depth.min(
                if edge.is_vertical() {
                    area.width()
                } else {
                    area.height()
                },
            ));
            // The outer half is where an outer drop lands; the inner half is where an inner
            // drop lands, so the preview shows which side of the existing panes the new pane
            // will take.
            match (target, edge.is_vertical()) {
                (DockTarget::Outer(_), true) => {
                    let half = strip.width() / 2.0;
                    match edge {
                        DockEdge::Right => {
                            Rect::from_min_max(strip.min, egui::pos2(strip.right() - half, strip.bottom()))
                        }
                        _ => Rect::from_min_max(strip.min, egui::pos2(strip.left() + half, strip.bottom())),
                    }
                }
                (DockTarget::Outer(_), false) => {
                    let half = strip.height() / 2.0;
                    match edge {
                        DockEdge::Bottom => Rect::from_min_max(strip.min, egui::pos2(strip.right(), strip.bottom() - half)),
                        _ => Rect::from_min_max(strip.min, egui::pos2(strip.right(), strip.top() + half)),
                    }
                }
                (_, true) => {
                    let half = strip.width() / 2.0;
                    match edge {
                        DockEdge::Right => {
                            Rect::from_min_max(egui::pos2(strip.left() + half, strip.top()), strip.max)
                        }
                        _ => Rect::from_min_max(egui::pos2(strip.right() - half, strip.top()), strip.max),
                    }
                }
                (_, false) => {
                    let half = strip.height() / 2.0;
                    match edge {
                        DockEdge::Bottom => Rect::from_min_max(egui::pos2(strip.left(), strip.top() + half), strip.max),
                        _ => Rect::from_min_max(egui::pos2(strip.left(), strip.bottom() - half), strip.max),
                    }
                }
            }
        }
    }
}

/// The strip of `depth` along `edge` inside `area`.
fn edge_strip(area: Rect, edge: DockEdge, depth: f32) -> Rect {
    match edge {
        DockEdge::Left => Rect::from_min_size(area.min, Vec2::new(depth, area.height())),
        DockEdge::Right => Rect::from_min_size(
            egui::pos2(area.right() - depth, area.top()),
            Vec2::new(depth, area.height()),
        ),
        DockEdge::Top => Rect::from_min_size(area.min, Vec2::new(area.width(), depth)),
        DockEdge::Bottom => Rect::from_min_size(
            egui::pos2(area.left(), area.bottom() - depth),
            Vec2::new(area.width(), depth),
        ),
    }
}

/// Whatever the docks wrap around.
///
/// The dock renderer borrows this once, so the terminal and the panes can share
/// the same session state without splitting borrows.
/// How a pane's header dot reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderHealth {
    /// Working.
    #[default]
    Ok,
    /// Busy, as in "connecting".
    Warn,
    /// Broken.
    Bad,
}

pub trait DockContent {
    /// Draw the terminal.
    fn terminal(&mut self, ui: &mut Ui);

    /// Draw a plugin pane's contents (the frame is already painted).
    fn pane(&mut self, ui: &mut Ui, plugin_id: &str);

    /// Title shown in a pane header.
    fn pane_title(&self, plugin_id: &str) -> String;

    /// Optional state line painted at the right of the pane header.
    fn pane_status(&self, _plugin_id: &str) -> Option<(String, HeaderHealth)> {
        None
    }

    /// Optional button painted in the pane header, before the state line.
    fn pane_header_action(&self, _plugin_id: &str) -> Option<HeaderAction> {
        None
    }
}

/// A button a pane asks the host to paint in its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderAction {
    /// Caption of the button.
    pub label: &'static str,
    /// Whether it can be clicked right now.
    pub enabled: bool,
}

/// Draw `layout` filling the available space, wrapping `content`'s terminal.
pub fn dock_layout(
    ui: &mut Ui,
    palette: &Palette,
    layout: &mut TabPluginLayout,
    content: &mut dyn DockContent,
    drag: &mut Option<DockDrag>,
) -> Vec<DockAction> {
    let mut actions = Vec::new();
    let area = ui.available_rect_before_wrap();
    let mut remaining = area;

    for edge in DockEdge::ALL {
        let Some(node) = layout.dock_mut(edge).take() else {
            continue;
        };
        let mut node = node;
        let available = if edge.is_vertical() {
            remaining.width()
        } else {
            remaining.height()
        };
        let extent = clamp_dock_extent(layout.size(edge), available);
        if extent <= 0.0 {
            // No room: keep the plugin mounted but unrendered.
            *layout.dock_mut(edge) = Some(node);
            continue;
        }
        let (pane_rect, splitter_rect, rest) = carve(remaining, edge, extent);
        remaining = rest;

        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(pane_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        draw_node(
            &mut child,
            palette,
            &mut node,
            pane_rect,
            content,
            &mut actions,
            drag,
        );

        if let Some(new_extent) = edge_splitter(ui, palette, edge, splitter_rect, extent, available)
        {
            layout.set_size(edge, new_extent);
            actions.push(DockAction::Resized);
        }
        *layout.dock_mut(edge) = Some(node);
    }

    ui.painter()
        .rect_filled(remaining, 0.0, to_color32(palette.bg_term));
    let mut terminal_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(remaining)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    content.terminal(&mut terminal_ui);

    if let Some(node) = layout.overlay.take() {
        let mut node = node;
        let rect = Rect::from_min_size(
            egui::pos2(
                remaining.right() - OVERLAY_SIZE.x - 12.0,
                remaining.bottom() - OVERLAY_SIZE.y - 12.0,
            ),
            OVERLAY_SIZE,
        );
        let rect = clamp_to(rect, remaining);
        draw_node(ui, palette, &mut node, rect, content, &mut actions, drag);
        layout.overlay = Some(node);
    }

    // A drag that ends over the docks moves the pane; anywhere else cancels it.
    if drag.is_some() {
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let released = ui.input(|input| input.pointer.any_released());
        let cancelled = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if let Some(pointer) = pointer
            && let Some(target) = drop_target(area, pointer)
        {
            let preview = preview_rect(area, layout, target);
            ui.painter().rect_filled(
                preview,
                4.0,
                to_color32(colors::tint(palette.accent, palette.bg_term, 0.22)),
            );
            ui.painter().rect_stroke(
                preview,
                4.0,
                egui::Stroke::new(1.0, to_color32(palette.accent)),
                egui::StrokeKind::Inside,
            );
            if released {
                let plugin_id = drag.take().map(|drag| drag.plugin_id);
                if let Some(plugin_id) = plugin_id {
                    actions.push(DockAction::Drop {
                        plugin_id,
                        target,
                    });
                }
            }
        } else if released || cancelled {
            *drag = None;
        }
    }

    actions
}

/// Split `remaining` into `(dock, splitter, rest)` for `edge`.
fn carve(remaining: Rect, edge: DockEdge, extent: f32) -> (Rect, Rect, Rect) {
    match edge {
        DockEdge::Left => {
            let pane = Rect::from_min_size(remaining.min, Vec2::new(extent, remaining.height()));
            let splitter = Rect::from_min_size(
                egui::pos2(pane.right(), pane.top()),
                Vec2::new(SPLITTER_SIZE_PX, remaining.height()),
            );
            let rest = Rect::from_min_max(
                egui::pos2(splitter.right(), remaining.top()),
                remaining.max,
            );
            (pane, splitter, rest)
        }
        DockEdge::Right => {
            let pane = Rect::from_min_size(
                egui::pos2(remaining.right() - extent, remaining.top()),
                Vec2::new(extent, remaining.height()),
            );
            let splitter = Rect::from_min_size(
                egui::pos2(pane.left() - SPLITTER_SIZE_PX, remaining.top()),
                Vec2::new(SPLITTER_SIZE_PX, remaining.height()),
            );
            let rest = Rect::from_min_max(
                remaining.min,
                egui::pos2(splitter.left(), remaining.bottom()),
            );
            (pane, splitter, rest)
        }
        DockEdge::Top => {
            let pane = Rect::from_min_size(remaining.min, Vec2::new(remaining.width(), extent));
            let splitter = Rect::from_min_size(
                egui::pos2(remaining.left(), pane.bottom()),
                Vec2::new(remaining.width(), SPLITTER_SIZE_PX),
            );
            let rest = Rect::from_min_max(
                egui::pos2(remaining.left(), splitter.bottom()),
                remaining.max,
            );
            (pane, splitter, rest)
        }
        DockEdge::Bottom => {
            let pane = Rect::from_min_size(
                egui::pos2(remaining.left(), remaining.bottom() - extent),
                Vec2::new(remaining.width(), extent),
            );
            let splitter = Rect::from_min_size(
                egui::pos2(remaining.left(), pane.top() - SPLITTER_SIZE_PX),
                Vec2::new(remaining.width(), SPLITTER_SIZE_PX),
            );
            let rest = Rect::from_min_max(
                remaining.min,
                egui::pos2(remaining.right(), splitter.top()),
            );
            (pane, splitter, rest)
        }
    }
}

/// Keep `rect` inside `bounds`, shrinking it when the area is smaller.
fn clamp_to(rect: Rect, bounds: Rect) -> Rect {
    let size = Vec2::new(
        rect.width().min(bounds.width()),
        rect.height().min(bounds.height()),
    );
    let min = egui::pos2(
        rect.left().max(bounds.left()),
        rect.top().max(bounds.top()),
    );
    let min = egui::pos2(
        min.x.min(bounds.right() - size.x),
        min.y.min(bounds.bottom() - size.y),
    );
    Rect::from_min_size(min, size)
}

/// Draw the resizer between a dock and the terminal, returning a new extent.
fn edge_splitter(
    ui: &mut Ui,
    palette: &Palette,
    edge: DockEdge,
    rect: Rect,
    extent: f32,
    available: f32,
) -> Option<f32> {
    let vertical = edge.is_vertical();
    let axis = if vertical {
        SplitAxis::Horizontal
    } else {
        SplitAxis::Vertical
    };
    let id = egui::Id::unique(("wassh-dock-splitter", edge));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if vertical {
            CursorIcon::ResizeHorizontal
        } else {
            CursorIcon::ResizeVertical
        });
    }
    let colour = if response.dragged() || response.hovered() {
        to_color32(palette.accent).gamma_multiply(0.7)
    } else {
        to_color32(palette.border)
    };
    let band = match axis {
        SplitAxis::Horizontal => Rect::from_center_size(rect.center(), Vec2::new(1.0, rect.height())),
        SplitAxis::Vertical => Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 1.0)),
    };
    ui.painter().rect_filled(band, 0.0, colour);

    if !response.dragged() {
        return None;
    }
    let delta = response.drag_delta();
    let change = match edge {
        DockEdge::Left | DockEdge::Top => {
            if vertical {
                delta.x
            } else {
                delta.y
            }
        }
        DockEdge::Right | DockEdge::Bottom => {
            if vertical {
                -delta.x
            } else {
                -delta.y
            }
        }
    };
    if change == 0.0 {
        return None;
    }
    let max = (available - MIN_TERMINAL_WIDTH_PX - SPLITTER_SIZE_PX).max(MIN_DOCK_SIZE_PX);
    let next = (extent + change).clamp(MIN_DOCK_SIZE_PX.min(max), max);
    ((next - extent).abs() > f32::EPSILON).then_some(next)
}

/// Draw one node of a dock tree into `rect`.
fn draw_node(
    ui: &mut Ui,
    palette: &Palette,
    node: &mut LayoutNode,
    rect: Rect,
    content: &mut dyn DockContent,
    actions: &mut Vec<DockAction>,
    drag: &mut Option<DockDrag>,
) {
    match node {
        LayoutNode::Leaf { plugin_id } => {
            draw_pane(ui, palette, plugin_id, rect, content, actions, drag);
        }
        LayoutNode::Split {
            direction,
            ratio,
            a,
            b,
        } => {
            let vertical = *direction == SplitDirection::Row;
            let total = if vertical { rect.width() } else { rect.height() };
            if total <= SPLITTER_SIZE_PX + 2.0 * MIN_PANE_PX {
                // Too tight to split: draw the first pane only.
                draw_node(ui, palette, a, rect, content, actions, drag);
                return;
            }
            let first = (total * *ratio - SPLITTER_SIZE_PX / 2.0).clamp(MIN_PANE_PX, total);
            let (a_rect, splitter_rect, b_rect) = if vertical {
                (
                    Rect::from_min_size(rect.min, Vec2::new(first, rect.height())),
                    Rect::from_min_size(
                        egui::pos2(rect.left() + first, rect.top()),
                        Vec2::new(SPLITTER_SIZE_PX, rect.height()),
                    ),
                    Rect::from_min_max(
                        egui::pos2(rect.left() + first + SPLITTER_SIZE_PX, rect.top()),
                        rect.max,
                    ),
                )
            } else {
                (
                    Rect::from_min_size(rect.min, Vec2::new(rect.width(), first)),
                    Rect::from_min_size(
                        egui::pos2(rect.left(), rect.top() + first),
                        Vec2::new(rect.width(), SPLITTER_SIZE_PX),
                    ),
                    Rect::from_min_max(
                        egui::pos2(rect.left(), rect.top() + first + SPLITTER_SIZE_PX),
                        rect.max,
                    ),
                )
            };
            draw_node(ui, palette, a, a_rect, content, actions, drag);
            draw_node(ui, palette, b, b_rect, content, actions, drag);

            let id = egui::Id::unique(("wassh-dock-inner-splitter", rect.left().to_bits(), rect.top().to_bits()));
            let response = ui.interact(splitter_rect, id, Sense::click_and_drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if vertical {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                });
            }
            let colour = if response.dragged() || response.hovered() {
                to_color32(palette.accent).gamma_multiply(0.7)
            } else {
                to_color32(palette.border)
            };
            ui.painter().rect_filled(splitter_rect, 0.0, colour);
            if response.dragged() {
                let delta = response.drag_delta();
                let change = if vertical { delta.x } else { delta.y };
                if change != 0.0 {
                    let next = clamp_split_ratio(*ratio + change / total, total, MIN_PANE_PX);
                    if (next - *ratio).abs() > f32::EPSILON {
                        *ratio = next;
                        actions.push(DockAction::Resized);
                    }
                }
            }
        }
    }
}

/// Frame a single plugin pane and draw its contents.
fn draw_pane(
    ui: &mut Ui,
    palette: &Palette,
    plugin_id: &str,
    rect: Rect,
    content: &mut dyn DockContent,
    actions: &mut Vec<DockAction>,
    drag: &mut Option<DockDrag>,
) {
    ui.painter()
        .rect_filled(rect, 0.0, to_color32(palette.bg_panel));
    let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), PANE_HEADER_HEIGHT));
    ui.painter()
        .rect_filled(header, 0.0, to_color32(palette.bg_panel_2));
    ui.painter().rect_filled(
        Rect::from_min_size(
            egui::pos2(header.left(), header.bottom() - 1.0),
            Vec2::new(header.width(), 1.0),
        ),
        0.0,
        to_color32(palette.border),
    );

    let close_rect = Rect::from_center_size(
        egui::pos2(header.right() - 14.0, header.center().y),
        Vec2::splat(18.0),
    );

    // A pane's own state line lives in the header: a health dot and the text,
    // right-aligned before the close button, with the title taking what is left.
    let title = content.pane_title(plugin_id);

    // A pane may also ask for one small button (MQTT's Reconnect); it sits
    // right before the state line so the two never overlap.
    let mut status_right = close_rect.left() - 10.0;
    let mut action_left = None;
    if let Some(action) = content.pane_header_action(plugin_id) {
        let galley = truncating_galley(
            ui,
            action.label.to_owned(),
            11.0,
            to_color32(palette.text),
            header.width() * 0.4,
        );
        let size = Vec2::new(galley.size().x + 16.0, 18.0);
        let rect = Rect::from_min_size(
            egui::pos2(status_right - size.x, header.center().y - size.y / 2.0),
            size,
        );
        let id = egui::Id::unique((
            "wassh-dock-action",
            plugin_id,
            rect.left().to_bits(),
            rect.top().to_bits(),
        ));
        let button = ui.interact(rect, id, Sense::click());
        let hovered = button.hovered() && action.enabled;
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        ui.painter().rect_filled(rect, 4.0, to_color32(palette.bg_input));
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                1.0,
                to_color32(if hovered { palette.accent } else { palette.border }),
            ),
            egui::StrokeKind::Inside,
        );
        let colour = if !action.enabled {
            to_color32(palette.text_muted).gamma_multiply(0.5)
        } else if hovered {
            to_color32(palette.text)
        } else {
            to_color32(palette.text_muted)
        };
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x / 2.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            colour,
        );
        if button.clicked() && action.enabled {
            actions.push(DockAction::HeaderAction(plugin_id.to_owned()));
        }
        status_right = rect.left() - 10.0;
        action_left = Some(rect.left());
    }

    let title_left = header.left() + PANE_PADDING;
    let mut title_room = status_right + 8.0 - title_left;
    if let Some((text, health)) = content.pane_status(plugin_id) {
        let galley = truncating_galley(
            ui,
            text,
            10.5,
            to_color32(palette.text_muted),
            header.width() * 0.55,
        );
        let right = status_right;
        let left = (right - galley.size().x).max(title_left);
        ui.painter().circle_filled(
            egui::pos2(left - DOT_GAP - 3.0, header.center().y),
            3.0,
            match health {
                HeaderHealth::Ok => to_color32(palette.ok),
                HeaderHealth::Warn => to_color32(palette.warn),
                HeaderHealth::Bad => to_color32(palette.danger),
            },
        );
        ui.painter().galley(
            egui::pos2(left, header.center().y - galley.size().y / 2.0),
            galley,
            to_color32(palette.text_muted),
        );
        title_room = (left - DOT_GAP - 6.0 - title_left).max(24.0);
    }
    let title = truncating_galley(
        ui,
        title,
        12.0,
        to_color32(palette.text),
        title_room,
    );
    ui.painter().galley(
        egui::pos2(title_left, header.center().y - title.size().y / 2.0),
        title,
        to_color32(palette.text),
    );

    // The header itself is the drag handle, so a pane can be moved to another edge.
    // It stops short of the action button: the button is created first, so the
    // drag handle would otherwise sit on top of it and swallow the click.
    let handle = Rect::from_min_max(
        header.min,
        egui::pos2(
            action_left.map_or(close_rect.left() - 4.0, |left| left - 4.0),
            header.bottom(),
        ),
    );
    let handle_id = egui::Id::unique((
        "wassh-dock-header",
        plugin_id,
        rect.left().to_bits(),
        rect.top().to_bits(),
    ));
    let handle = ui.interact(handle, handle_id, Sense::click_and_drag());
    if handle.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    if handle.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        *drag = Some(DockDrag {
            plugin_id: plugin_id.to_owned(),
            from_pane: true,
        });
    }
    let close_id = egui::Id::unique(("wassh-dock-close", plugin_id, rect.left().to_bits()));
    let close = ui.interact(close_rect, close_id, Sense::click());
    if close.hovered() {
        ui.painter().rect_filled(
            close_rect,
            4.0,
            to_color32(palette.bg_input).gamma_multiply(0.9),
        );
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    ui.painter().text(
        close_rect.center(),
        Align2::CENTER_CENTER,
        "✕",
        FontId::proportional(11.0),
        to_color32(if close.hovered() {
            palette.danger
        } else {
            palette.text_muted
        }),
    );
    if close.clicked() {
        actions.push(DockAction::Close(plugin_id.to_owned()));
    }

    let body = Rect::from_min_max(
        egui::pos2(rect.left() + PANE_PADDING, header.bottom() + PANE_PADDING),
        egui::pos2(rect.right() - PANE_PADDING, rect.bottom() - PANE_PADDING),
    );
    if body.width() <= 1.0 || body.height() <= 1.0 {
        return;
    }
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body.intersect(ui.clip_rect()));
    content.pane(&mut child, plugin_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(egui::pos2(100.0, 50.0), Vec2::new(900.0, 600.0))
    }

    /// Pane stub used by the header tests.
    struct FakePane {
        status: Option<(String, HeaderHealth)>,
        action: Option<HeaderAction>,
    }

    impl DockContent for FakePane {
        fn terminal(&mut self, _ui: &mut Ui) {}

        fn pane(&mut self, _ui: &mut Ui, _plugin_id: &str) {}

        fn pane_title(&self, _plugin_id: &str) -> String {
            "Files".to_owned()
        }

        fn pane_status(&self, _plugin_id: &str) -> Option<(String, HeaderHealth)> {
            self.status.clone()
        }

        fn pane_header_action(&self, _plugin_id: &str) -> Option<HeaderAction> {
            self.action
        }
    }

    /// Paint one docked pane and return (text, rect) for every text shape plus
    /// the colour of every small circle.
    fn draw_pane_with(
        status: Option<(String, HeaderHealth)>,
    ) -> (Vec<(String, Rect)>, Vec<egui::Color32>) {
        draw_pane_with_action(status, None)
    }

    /// Paint one docked pane whose plugin asked for a header button.
    fn draw_pane_with_action(
        status: Option<(String, HeaderHealth)>,
        action: Option<HeaderAction>,
    ) -> (Vec<(String, Rect)>, Vec<egui::Color32>) {
        let mut layout = TabPluginLayout::default();
        layout.dock_on_edge("sftp", DockEdge::Left, None);
        let palette = Palette::dark();
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut content = FakePane { status, action };
        let mut drag = None;
        let mut output = ctx.run_ui(input, |ui| {
            dock_layout(ui, &palette, &mut layout, &mut content, &mut drag);
        });
        let mut texts = Vec::new();
        let mut dots = Vec::new();
        for clipped in std::mem::take(&mut output.shapes) {
            match clipped.shape {
                egui::Shape::Text(text) => {
                    let rect = Rect::from_min_size(text.pos, text.galley.size());
                    texts.push((text.galley.text().to_owned(), rect));
                }
                egui::Shape::Circle(circle) if circle.radius <= 4.0 => {
                    dots.push(circle.fill);
                }
                _ => {}
            }
        }
        output.drop_without_applying_deltas();
        (texts, dots)
    }

    #[test]
    fn a_pane_status_line_is_painted_in_the_header() {
        let status = "Connected — uploads go to /home/dev".to_owned();
        let (texts, dots) = draw_pane_with(Some((status.clone(), HeaderHealth::Ok)));
        let palette = Palette::dark();
        let caption = texts
            .iter()
            .find(|(text, _)| text.contains("uploads go to"))
            .expect("the header shows the caption");
        let title = texts
            .iter()
            .find(|(text, _)| text == "Files")
            .expect("the header keeps its title");

        // Same strip as the title, not a row below it.
        assert!(
            (caption.1.center().y - title.1.center().y).abs() < 1.0,
            "caption at {:?}, title at {:?}",
            caption.1,
            title.1
        );
        assert!(caption.1.top() < title.1.top() + PANE_HEADER_HEIGHT);
        // Right of the title, and clear of the close button.
        assert!(caption.1.left() > title.1.right());
        assert!(caption.1.right() <= 1200.0 - PANE_PADDING);
        // A healthy dot is painted in front of it.
        assert!(
            dots.iter()
                .any(|fill| *fill == to_color32(palette.ok)),
            "no healthy dot in {dots:?}"
        );
    }

    #[test]
    fn a_pane_can_ask_for_a_button_in_its_header() {
        let (texts, _) = draw_pane_with_action(
            Some(("Disconnected".to_owned(), HeaderHealth::Bad)),
            Some(HeaderAction {
                label: "Reconnect",
                enabled: true,
            }),
        );
        let button = texts
            .iter()
            .find(|(text, _)| text == "Reconnect")
            .expect("the header paints the button");
        let caption = texts
            .iter()
            .find(|(text, _)| text == "Disconnected")
            .expect("the caption is still there");
        let title = texts
            .iter()
            .find(|(text, _)| text == "Files")
            .expect("the title is still there");
        // One strip: the button sits right of the caption, and both stay left
        // of the close button.
        assert!(button.1.left() > caption.1.right(), "button overlaps the caption");
        assert!(button.1.center().y - title.1.center().y < 1.0, "not in the header strip");
        assert!(button.1.right() < 1200.0 - PANE_PADDING);
    }

    #[test]
    fn a_pane_without_a_status_shows_only_its_title() {
        let (texts, dots) = draw_pane_with(None);
        assert!(texts.iter().any(|(text, _)| text == "Files"));
        assert!(!texts.iter().any(|(text, _)| text.contains("Connected")));
        assert!(dots.is_empty(), "unexpected dots: {dots:?}");
    }

    #[test]
    fn carving_an_edge_leaves_the_rest_for_the_terminal() {
        let (pane, splitter, rest) = carve(area(), DockEdge::Left, 300.0);
        assert_eq!(pane.width(), 300.0);
        assert_eq!(splitter.width(), SPLITTER_SIZE_PX);
        assert_eq!(rest.left(), pane.right() + SPLITTER_SIZE_PX);
        assert_eq!(rest.height(), area().height());
    }

    #[test]
    fn carving_the_right_edge_keeps_the_terminal_on_the_left() {
        let (pane, splitter, rest) = carve(area(), DockEdge::Right, 300.0);
        assert_eq!(pane.right(), area().right());
        assert_eq!(splitter.right(), pane.left());
        assert_eq!(rest.right(), splitter.left());
    }

    #[test]
    fn the_drop_bands_pick_an_edge_and_a_side() {
        let area = area();
        // The outer band hugs the border.
        assert_eq!(drop_target(area, egui::pos2(110.0, 300.0)), Some(DockTarget::Outer(DockEdge::Left)));
        // The ring just inside it inserts next to the terminal.
        assert_eq!(
            drop_target(area, egui::pos2(140.0, 300.0)),
            Some(DockTarget::Inner(DockEdge::Left))
        );
        // Well inside floats the pane.
        assert_eq!(drop_target(area, area.center()), Some(DockTarget::Overlay));
        // Outside the docks is not a target at all.
        assert_eq!(drop_target(area, egui::pos2(10.0, 10.0)), None);
        // Corners go to the nearest edge.
        assert_eq!(
            drop_target(area, egui::pos2(area.right() - 5.0, area.bottom() - 200.0)),
            Some(DockTarget::Outer(DockEdge::Right))
        );
        assert_eq!(
            drop_target(area, egui::pos2(300.0, area.bottom() - 5.0)),
            Some(DockTarget::Outer(DockEdge::Bottom))
        );
    }

    #[test]
    fn a_drop_lands_on_the_side_of_the_existing_panes() {
        let mut layout = TabPluginLayout::default();
        layout.dock_on_edge("sftp", DockEdge::Left, None);
        assert_eq!(layout.plugin_ids(), vec!["sftp".to_owned()]);

        // Outer puts the new pane further from the terminal.
        apply_drop(&mut layout, "monitor", DockTarget::Outer(DockEdge::Left));
        assert_eq!(
            layout.plugin_ids(),
            vec!["monitor".to_owned(), "sftp".to_owned()]
        );

        // Inner puts it between the dock and the terminal.
        apply_drop(&mut layout, "monitor", DockTarget::Inner(DockEdge::Left));
        assert_eq!(
            layout.plugin_ids(),
            vec!["sftp".to_owned(), "monitor".to_owned()]
        );

        // A drop in the middle floats the pane instead.
        apply_drop(&mut layout, "monitor", DockTarget::Overlay);
        assert_eq!(
            layout.dock(DockEdge::Left).map(LayoutNode::plugin_ids),
            Some(vec!["sftp".to_owned()]),
            "the dock keeps only its own pane"
        );
        assert_eq!(
            layout.overlay.as_ref().map(LayoutNode::plugin_ids),
            Some(vec!["monitor".to_owned()])
        );
    }

    #[test]
    fn moving_a_pane_between_edges_keeps_one_copy() {
        let mut layout = TabPluginLayout::default();
        layout.dock_on_edge("sftp", DockEdge::Left, None);
        assert!(layout.contains("sftp"));
        apply_drop(&mut layout, "sftp", DockTarget::Outer(DockEdge::Bottom));
        assert_eq!(layout.plugin_ids(), vec!["sftp".to_owned()]);
        assert!(layout.dock(DockEdge::Left).is_none(), "left dock is empty");
        assert!(layout.dock(DockEdge::Bottom).is_some());
    }

    #[test]
    fn the_preview_shows_the_half_the_pane_will_take() {
        let mut layout = TabPluginLayout::default();
        layout.dock_on_edge("sftp", DockEdge::Left, None);
        layout.set_size(DockEdge::Left, 300.0);
        let area = area();

        let outer = preview_rect(area, &layout, DockTarget::Outer(DockEdge::Left));
        let inner = preview_rect(area, &layout, DockTarget::Inner(DockEdge::Left));
        assert_eq!(outer.left(), area.left());
        assert_eq!(outer.width(), 150.0);
        assert_eq!(inner.right(), area.left() + 300.0);
        assert_eq!(inner.width(), 150.0);
        assert!(outer.right() <= inner.left(), "halves do not overlap");

        // The overlay preview sits inside the terminal area.
        let floating = preview_rect(area, &layout, DockTarget::Overlay);
        assert!(area.contains_rect(floating));
    }

    #[test]
    fn carving_the_bottom_edge_stacks_the_terminal_above() {
        let (pane, splitter, rest) = carve(area(), DockEdge::Bottom, 160.0);
        assert_eq!(pane.bottom(), area().bottom());
        assert_eq!(splitter.bottom(), pane.top());
        assert_eq!(rest.bottom(), splitter.top());
    }

    #[test]
    fn overlays_are_kept_inside_the_terminal_area() {
        let bounds = area();
        let oversized = Rect::from_min_size(
            egui::pos2(bounds.right() - 50.0, bounds.bottom() - 50.0),
            Vec2::new(2000.0, 2000.0),
        );
        let clamped = clamp_to(oversized, bounds);
        assert!(bounds.contains_rect(clamped));
        assert_eq!(clamped.width(), bounds.width());
    }
}
