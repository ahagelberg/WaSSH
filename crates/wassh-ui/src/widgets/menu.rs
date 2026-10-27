//! Context menus.
//!
//! Menus are anchored at the pointer offset by 4 px with a minimum width of
//! 120 px, flipping above and clamping horizontally so a menu never leaves the window.

use egui::{Context, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Gap between the pointer and the menu.
pub const ANCHOR_OFFSET: f32 = 4.0;
/// Smallest menu width.
pub const MIN_WIDTH: f32 = 120.0;
/// Height of a single item row.
pub const ITEM_HEIGHT: f32 = 28.0;
/// Height reserved for a separator line.
pub const SEPARATOR_HEIGHT: f32 = 9.0;
/// Horizontal padding inside the menu.
pub const PADDING_X: f32 = 6.0;
/// Vertical padding above the first and below the last item.
pub const PADDING_Y: f32 = 5.0;

/// A single entry in a context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// A normal, clickable row.
    Action {
        /// Text shown in the row.
        label: &'static str,
        /// Greyed-out and unclickable when false.
        enabled: bool,
        /// Red text and hover fill, used for destructive actions.
        danger: bool,
    },
    /// A horizontal rule between groups.
    Separator,
}

impl MenuItem {
    /// A clickable row.
    pub fn new(label: &'static str) -> Self {
        Self::Action {
            label,
            enabled: true,
            danger: false,
        }
    }

    /// A destructive row, drawn in the danger colour.
    pub fn danger(label: &'static str) -> Self {
        Self::Action {
            label,
            enabled: true,
            danger: true,
        }
    }

    /// A row that is visible but cannot be activated.
    pub fn disabled(label: &'static str) -> Self {
        Self::Action {
            label,
            enabled: false,
            danger: false,
        }
    }

    /// A horizontal rule.
    pub fn separator() -> Self {
        Self::Separator
    }

    fn is_action(&self) -> bool {
        matches!(self, Self::Action { .. })
    }
}

/// Where a menu of `size` should be placed for a click at `anchor` inside `screen`.
///
/// Prefers below-right of the pointer, flips left/up when it would overflow, then clamps
/// so the menu always stays fully visible.
pub fn menu_origin(anchor: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    let max_x = (screen.right() - size.x - ANCHOR_OFFSET).max(screen.left() + ANCHOR_OFFSET);
    let max_y = (screen.bottom() - size.y - ANCHOR_OFFSET).max(screen.top() + ANCHOR_OFFSET);

    let mut x = anchor.x + ANCHOR_OFFSET;
    if x > max_x {
        x = anchor.x - size.x - ANCHOR_OFFSET;
    }
    let mut y = anchor.y + ANCHOR_OFFSET;
    if y > max_y {
        y = anchor.y - size.y - ANCHOR_OFFSET;
    }

    Pos2::new(
        x.clamp(screen.left() + ANCHOR_OFFSET, max_x),
        y.clamp(screen.top() + ANCHOR_OFFSET, max_y),
    )
}

/// An open menu and the options it is showing.
pub struct MenuState {
    open: Option<OpenMenu>,
}

struct OpenMenu {
    position: Pos2,
    items: Vec<MenuItem>,
    /// Filled in after the first paint so outside clicks can be detected next frame.
    rect: Option<Rect>,
}

impl MenuState {
    /// No menu open.
    pub fn new() -> Self {
        Self { open: None }
    }

    /// Show `items` at `position` on the next call to [`Self::show`].
    pub fn open_at(&mut self, position: Pos2, items: Vec<MenuItem>) {
        self.open = Some(OpenMenu {
            position,
            items,
            rect: None,
        });
    }

    /// Whether a menu is currently open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Dismiss the open menu, if any.
    pub fn close(&mut self) {
        self.open = None;
    }

    /// Draw the open menu, returning the index into the item list that was activated.
    pub fn show(&mut self, ctx: &Context, palette: &Palette) -> Option<usize> {
        let open = self.open.as_ref()?;
        let position = open.position;
        let items = &open.items;

        let screen = ctx.content_rect();
        let text_colour = to_color32(palette.text);
        let font = FontId::proportional(13.0);

        // Measure first so the frame can be sized before anything is painted.
        let width = items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { label, .. } => {
                    Some(ctx.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(label.to_string(), font.clone(), text_colour)
                            .size()
                            .x
                    }))
                }
                MenuItem::Separator => None,
            })
            .fold(MIN_WIDTH, f32::max)
            + PADDING_X * 2.0
            + 16.0;
        let height = items
            .iter()
            .map(|item| {
                if item.is_action() {
                    ITEM_HEIGHT
                } else {
                    SEPARATOR_HEIGHT
                }
            })
            .sum::<f32>()
            + PADDING_Y * 2.0;
        let size = Vec2::new(width, height);
        let origin = menu_origin(position, size, screen);
        let area_rect = Rect::from_min_size(origin, size);

        // A press anywhere outside last frame's rect dismisses the menu.
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let scrolled = ctx.input(|input| input.smooth_scroll_delta != Vec2::ZERO);
        let pressed_outside = ctx.input(|input| {
            input
                .pointer
                .any_pressed()
                .then(|| input.pointer.interact_pos())
                .flatten()
                .is_some_and(|pos| !area_rect.contains(pos))
        });
        if escape || scrolled || pressed_outside {
            self.open = None;
            return None;
        }

        let mut activated = None;
        let response = egui::Area::new(Id::unique("wassh-context-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(origin)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(to_color32(palette.bg_panel))
                    .stroke(Stroke::new(1.0, to_color32(palette.border)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(
                        PADDING_X as i8,
                        PADDING_Y as i8,
                    ));
                frame.show(ui, |ui| {
                    ui.set_width(size.x - PADDING_X * 2.0);
                    for (index, item) in items.iter().enumerate() {
                        match item {
                            MenuItem::Separator => {
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), SEPARATOR_HEIGHT),
                                    Sense::hover(),
                                );
                                let y = rect.center().y;
                                ui.painter().hline(
                                    rect.x_range(),
                                    y,
                                    Stroke::new(1.0, to_color32(palette.border)),
                                );
                            }
                            MenuItem::Action {
                                label,
                                enabled,
                                danger,
                            } => {
                                let (rect, item_response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), ITEM_HEIGHT),
                                    if *enabled {
                                        Sense::click()
                                    } else {
                                        Sense::hover()
                                    },
                                );
                                let hovered = *enabled && item_response.hovered();
                                if hovered {
                                    let fill = if *danger {
                                        to_color32(palette.danger).gamma_multiply(0.25)
                                    } else {
                                        to_color32(palette.bg_panel_2)
                                    };
                                    ui.painter().rect_filled(rect, 4, fill);
                                }
                                let colour = if !*enabled {
                                    to_color32(palette.text_muted)
                                } else if *danger {
                                    to_color32(palette.danger)
                                } else if hovered {
                                    to_color32(palette.text)
                                } else {
                                    text_colour
                                };
                                ui.painter().text(
                                    Pos2::new(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    font.clone(),
                                    colour,
                                );
                                if item_response.clicked() {
                                    activated = Some(index);
                                }
                            }
                        }
                    }
                });
            });

        if let Some(open) = self.open.as_mut() {
            open.rect = Some(response.response.rect);
        }
        if activated.is_some() {
            self.open = None;
        }
        activated
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience: draw a one-off menu at `position`, closing on any activation.
///
/// Prefer [`MenuState`] when the menu must survive across frames.
pub fn context_menu(
    ui: &mut Ui,
    palette: &Palette,
    position: Pos2,
    items: &[MenuItem],
) -> Option<usize> {
    let mut chosen = None;
    let screen = ui.ctx().content_rect();
    let width = items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action { label, .. } => Some(label.len() as f32 * 7.0),
            MenuItem::Separator => None,
        })
        .fold(MIN_WIDTH, f32::max)
        + PADDING_X * 2.0;
    let height = items
        .iter()
        .map(|item| {
            if item.is_action() {
                ITEM_HEIGHT
            } else {
                SEPARATOR_HEIGHT
            }
        })
        .sum::<f32>()
        + PADDING_Y * 2.0;
    let origin = menu_origin(position, Vec2::new(width, height), screen);

    egui::Area::new(Id::unique("wassh-one-off-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(to_color32(palette.bg_panel))
                .stroke(Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(PADDING_X as i8, PADDING_Y as i8))
                .show(ui, |ui| {
                    ui.set_width(width - PADDING_X * 2.0);
                    for (index, item) in items.iter().enumerate() {
                        match item {
                            MenuItem::Separator => {
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), SEPARATOR_HEIGHT),
                                    Sense::hover(),
                                );
                                ui.painter().hline(
                                    rect.x_range(),
                                    rect.center().y,
                                    Stroke::new(1.0, to_color32(palette.border)),
                                );
                            }
                            MenuItem::Action {
                                label,
                                enabled,
                                danger,
                            } => {
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), ITEM_HEIGHT),
                                    if *enabled {
                                        Sense::click()
                                    } else {
                                        Sense::hover()
                                    },
                                );
                                let hovered = *enabled && response.hovered();
                                if hovered {
                                    let fill = if *danger {
                                        to_color32(palette.danger).gamma_multiply(0.25)
                                    } else {
                                        to_color32(palette.bg_panel_2)
                                    };
                                    ui.painter().rect_filled(rect, 4, fill);
                                }
                                let colour = if !*enabled {
                                    to_color32(palette.text_muted)
                                } else if *danger {
                                    to_color32(palette.danger)
                                } else {
                                    to_color32(palette.text)
                                };
                                ui.painter().text(
                                    Pos2::new(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    FontId::proportional(13.0),
                                    colour,
                                );
                                if response.clicked() {
                                    chosen = Some(index);
                                }
                            }
                        }
                    }
                });
        });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))
    }

    #[test]
    fn places_below_right_of_the_pointer() {
        let origin = menu_origin(Pos2::new(100.0, 100.0), Vec2::new(160.0, 120.0), screen());
        assert_eq!(origin, Pos2::new(104.0, 104.0));
    }

    #[test]
    fn flips_left_and_up_near_the_bottom_right_corner() {
        let origin = menu_origin(Pos2::new(1270.0, 790.0), Vec2::new(160.0, 120.0), screen());
        assert!(origin.x + 160.0 <= 1280.0, "menu overflows right: {origin:?}");
        assert!(origin.y + 120.0 <= 800.0, "menu overflows bottom: {origin:?}");
        assert!(origin.x < 1270.0 && origin.y < 790.0);
    }

    #[test]
    fn clamps_when_the_menu_is_larger_than_the_screen() {
        let origin = menu_origin(Pos2::new(600.0, 400.0), Vec2::new(2000.0, 2000.0), screen());
        assert!(origin.x >= ANCHOR_OFFSET);
        assert!(origin.y >= ANCHOR_OFFSET);
    }

    #[test]
    fn item_constructors_set_the_right_flags() {
        assert_eq!(
            MenuItem::new("Reconnect"),
            MenuItem::Action {
                label: "Reconnect",
                enabled: true,
                danger: false
            }
        );
        assert!(matches!(
            MenuItem::danger("Close"),
            MenuItem::Action { danger: true, .. }
        ));
        assert!(matches!(
            MenuItem::disabled("Paste"),
            MenuItem::Action {
                enabled: false,
                ..
            }
        ));
    }
}
