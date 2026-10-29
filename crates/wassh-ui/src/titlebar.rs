//! The window's own chrome.
//!
//! Two layouts, because platforms differ in what feels native:
//!
//! * [`show_header_bar`] — a client-side header bar for Linux and Windows: the menus, the
//!   window title and the minimise/maximise/close buttons live in one 36 px strip, like a GTK
//!   header bar or a modern Windows app. Nothing is imitated beyond the caption buttons'
//!   shapes and hover colours, so it never looks like an unthemed copy of the desktop.
//! * [`show_menu_bar`] — a plain menu strip under the native title, used on macOS where the
//!   traffic lights and title are best left to the system.
//!
//! Either way the app follows the desktop's light/dark preference for its colours (see
//! `ThemeMode::System`), so the header sits on a window that matches the rest of the desktop.
//!
//! The menus are painted by hand rather than built from `Ui::menu_button`: a menu-bar entry
//! must read as flat text (highlighting only on hover) instead of looking like a button.

use egui::{
    Align2, CursorIcon, FontId, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2,
};
use wassh_core::{Palette, MENU_BAR_HEIGHT, WINDOW_HEADER_HEIGHT};

use crate::terminal_view::to_color32;

const MENU_BUTTON_HEIGHT: f32 = 26.0;
const MENU_LABEL_PADDING: f32 = 8.0;
const MENU_ITEM_HEIGHT: f32 = 26.0;
const MENU_ITEM_PADDING: f32 = 10.0;
const MENU_SEPARATOR_HEIGHT: f32 = 9.0;
const MENU_MIN_WIDTH: f32 = 140.0;
/// Space between the longest label and the accelerator column.
const MENU_SHORTCUT_GAP: f32 = 32.0;
/// Accelerators are dimmed until their row is hovered.
const MENU_SHORTCUT_DIM: f32 = 0.75;
/// Width of the check-mark gutter a menu gains when it has checkable entries.
const MENU_CHECK_WIDTH: f32 = 18.0;
/// Width of the gear gutter a menu gains when its entries have settings.
const MENU_GEAR_WIDTH: f32 = 22.0;
/// Radius of the gear's body ring, measured to its centreline.
const GEAR_RING_RADIUS: f32 = 3.3;
/// Thickness of the gear's body ring.
const GEAR_RING_WIDTH: f32 = 2.6;
/// Number of teeth around the gear.
const GEAR_TEETH: usize = 8;
/// Where a tooth starts, measured from the gear's centre.
const GEAR_TOOTH_INNER: f32 = 4.0;
/// Where a tooth ends, measured from the gear's centre.
const GEAR_TOOTH_OUTER: f32 = 6.2;
/// Thickness of a tooth.
const GEAR_TOOTH_WIDTH: f32 = 2.4;
/// Glyph of a checked entry.
const MENU_CHECK_GLYPH: &str = "\u{2713}";
const MENU_PADDING: f32 = 5.0;

/// What the user did in the window chrome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TitlebarAction {
    /// Minimise the window (client-side decorations only).
    Minimize,
    /// Toggle between maximised and restored (client-side decorations only).
    ToggleMaximize,
    /// Close the window (client-side decorations only).
    Close,
    /// A menu entry was chosen; the string is the entry label.
    MenuItem(&'static str),
    /// The settings gear next to a menu entry was chosen.
    MenuGear(&'static str),
}

/// Per-entry state of a menu bar: which entries show a tick and which show a
/// settings gear on their right edge.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuState<'a> {
    pub checked: &'a [&'a str],
    pub gears: &'a [&'a str],
    /// Keys that run a row, as `(row label, accelerator)` for the rows that have one.
    ///
    /// A lookup rather than part of [`Menu`] so the menus stay plain label lists — the
    /// plugin rows in particular have nothing to add here.
    pub shortcuts: &'a [(&'a str, &'a str)],
}

/// Menu definition: a label and its entries (`""` renders a separator).
pub type Menu = (&'static str, &'static [&'static str]);

/// How the platform draws its window buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionStyle {
    /// Full-height rectangles stacked against the title bar's top edge.
    Windows,
    /// Small rounded buttons with a gap around them.
    Gnome,
}

/// The caption style this build should use.
pub const fn caption_style() -> CaptionStyle {
    if cfg!(windows) {
        CaptionStyle::Windows
    } else {
        CaptionStyle::Gnome
    }
}

/// Draw the client-side header bar: title, menus and window buttons in one strip.
///
/// Use this on platforms where the app draws its own window frame
/// (`ViewportBuilder::with_decorations(false)`); pair it with [`resize_edges`] so the window
/// can still be resized by its border. Everything in it is painted from `palette`, so the
/// window chrome follows the app's theme rather than the desktop's.
pub fn show_header_bar(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    maximized: bool,
    menus: &[Menu],
    state: MenuState<'_>,
) -> Option<TitlebarAction> {
    let mut action = None;

    egui::Panel::top("wassh-header-bar")
        .frame(egui::Frame::NONE.fill(to_color32(palette.bg_app)))
        .show(ui, |ui| {
            ui.set_height(WINDOW_HEADER_HEIGHT);
            let bar = ui.max_rect();

            // Behind the widgets: drag the window, or double-click to maximise. Registered
            // first so the menu labels and buttons on top of it still get their clicks.
            let drag_id = ui.scope_id().with("wassh-header-drag");
            let drag = ui.interact(bar, drag_id, Sense::click_and_drag());
            if !maximized && drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.painter().hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                Stroke::new(1.0, to_color32(palette.border)),
            );

            // Caption buttons claim the right edge; the menus and title share the rest.
            ui.horizontal_centered(|ui| {
                ui.add_space(10.0);
                if !maximized && drag.double_clicked() {
                    action = Some(TitlebarAction::ToggleMaximize);
                }
                ui.label(
                    egui::RichText::new(title)
                        .size(12.5)
                        .strong()
                        .color(to_color32(palette.text)),
                );
                ui.add_space(10.0);

                if let Some(chosen) = menu_strip(ui, palette, menus, state) {
                    action = Some(chosen);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if caption_button(ui, palette, CaptionButton::Close) {
                        action = Some(TitlebarAction::Close);
                    }
                    if caption_button(
                        ui,
                        palette,
                        if maximized {
                            CaptionButton::Restore
                        } else {
                            CaptionButton::Maximize
                        },
                    ) {
                        action = Some(TitlebarAction::ToggleMaximize);
                    }
                    if caption_button(ui, palette, CaptionButton::Minimize) {
                        action = Some(TitlebarAction::Minimize);
                    }
                });
            });
        });

    action
}

/// Draw the menu strip and return the action the user invoked, if any.
pub fn show_menu_bar(
    ui: &mut Ui,
    palette: &Palette,
    menus: &[Menu],
    state: MenuState<'_>,
) -> Option<TitlebarAction> {
    let mut action = None;

    egui::Panel::top("wassh-menubar")
        .frame(egui::Frame::NONE.fill(to_color32(palette.bg_app)))
        .show(ui, |ui| {
            ui.set_height(MENU_BAR_HEIGHT);
            ui.horizontal_centered(|ui| {
                ui.add_space(6.0);
                action = menu_strip(ui, palette, menus, state);
            });
        });

    action
}

/// The accelerator of one menu row, if it has one.
fn shortcut_for<'a>(shortcuts: &[(&'a str, &'a str)], item: &str) -> Option<&'a str> {
    shortcuts
        .iter()
        .find(|(label, _)| *label == item)
        .map(|(_, keys)| *keys)
}

/// Paint the menu labels inside an already-positioned row and handle the open dropdown.
fn menu_strip(
    ui: &mut Ui,
    palette: &Palette,
    menus: &[Menu],
    state: MenuState<'_>,
) -> Option<TitlebarAction> {
    let mut action = None;
    let state_id = Id::unique("wassh-open-menu");
    let mut open: Option<&'static str> = ui
        .ctx()
        .data(|data| data.get_temp::<Option<&'static str>>(state_id))
        .flatten();

    let mut anchors: Vec<(Rect, &'static str, &'static [&'static str])> =
        Vec::with_capacity(menus.len());

    for (label, items) in menus {
        let galley = ui.painter().layout_no_wrap(
            (*label).to_owned(),
            FontId::proportional(12.0),
            to_color32(palette.text),
        );
        let width = galley.size().x + MENU_LABEL_PADDING * 2.0;
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, MENU_BUTTON_HEIGHT), Sense::click());
        let is_open = open == Some(*label);
        // Only the open entry (and the hovered one when nothing is open) gets a background,
        // exactly like a native menu bar.
        if is_open || (response.hovered() && open.is_none()) {
            ui.painter().rect_filled(
                rect,
                wassh_core::RADIUS,
                to_color32(if is_open {
                    palette.bg_panel_2
                } else {
                    palette.bg_panel
                }),
            );
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(12.0),
            to_color32(palette.text),
        );
        if response.clicked() {
            open = if is_open { None } else { Some(*label) };
        } else if open.is_some() && response.hovered() {
            // Sliding across the bar switches menus while one is open.
            open = Some(*label);
        }
        anchors.push((rect, label, items));
    }

    if let Some(label) = open
        && let Some((anchor, _, items)) = anchors.iter().find(|(_, name, _)| *name == label)
    {
        match dropdown(ui, palette, *anchor, label, items, state) {
            DropdownResult::Open => {}
            DropdownResult::Closed => open = None,
            DropdownResult::Chosen(item) => {
                action = Some(TitlebarAction::MenuItem(item));
                open = None;
            }
            DropdownResult::Gear(item) => {
                action = Some(TitlebarAction::MenuGear(item));
                open = None;
            }
        }
    } else {
        open = None;
    }

    ui.ctx()
        .data_mut(|data| data.insert_temp(state_id, open));
    action
}

enum DropdownResult {
    Open,
    Closed,
    Chosen(&'static str),
    /// The settings gear of an entry.
    Gear(&'static str),
}

/// Painted dropdown under a menu-bar entry.
fn dropdown(
    ui: &Ui,
    palette: &Palette,
    anchor: Rect,
    label: &'static str,
    items: &'static [&'static str],
    state: MenuState<'_>,
) -> DropdownResult {
    let MenuState {
        checked,
        gears,
        shortcuts,
    } = state;
    let ctx = ui.ctx();
    let text = to_color32(palette.text);
    let muted = to_color32(palette.text_muted);
    let font = FontId::proportional(12.0);

    // A menu with any checked entry reserves a gutter for every entry, so the
    // labels stay aligned like a native checkable menu.
    let gutter = if items
        .iter()
        .any(|item| !item.is_empty() && checked.contains(item))
    {
        MENU_CHECK_WIDTH
    } else {
        0.0
    };
    let gear_gutter = if items
        .iter()
        .any(|item| !item.is_empty() && gears.contains(item))
    {
        MENU_GEAR_WIDTH
    } else {
        0.0
    };
    // Size the panel before painting so it can be clamped to the window.
    // Only this menu's own rows can contribute an accelerator column.
    let keys: Vec<(&'static str, &str)> = items
        .iter()
        .filter_map(|item| shortcut_for(shortcuts, item).map(|keys| (*item, keys)))
        .collect();
    // The accelerator column is measured like a label column: as wide as the longest
    // shortcut, so the keys line up against the right edge as in a native menu.
    let shortcut_width = keys
        .iter()
        .map(|(_, keys)| {
            ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap((*keys).to_owned(), font.clone(), muted)
                    .size()
                    .x
            })
        })
        .fold(0.0, f32::max);
    let shortcut_gutter = if shortcut_width > 0.0 {
        shortcut_width + MENU_SHORTCUT_GAP
    } else {
        0.0
    };
    let widest = items
        .iter()
        .filter(|item| !item.is_empty())
        .map(|item| {
            ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap((*item).to_owned(), font.clone(), text)
                    .size()
                    .x
            })
        })
        .fold(MENU_MIN_WIDTH, f32::max)
        + MENU_ITEM_PADDING * 2.0
        + gutter
        + gear_gutter
        + shortcut_gutter
        + 16.0;
    let height = items
        .iter()
        .map(|item| {
            if item.is_empty() {
                MENU_SEPARATOR_HEIGHT
            } else {
                MENU_ITEM_HEIGHT
            }
        })
        .sum::<f32>()
        + MENU_PADDING * 2.0;

    let screen = ctx.content_rect();
    let mut origin = Pos2::new(anchor.left(), anchor.bottom() + 2.0);
    if origin.x + widest > screen.right() - 4.0 {
        origin.x = (screen.right() - widest - 4.0).max(screen.left() + 4.0);
    }
    if origin.y + height > screen.bottom() - 4.0 {
        origin.y = (anchor.top() - height - 2.0).max(screen.top() + 4.0);
    }

    let area = egui::Area::new(Id::unique(("wassh-menu", label)))
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(to_color32(palette.bg_panel))
                .stroke(Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(
                    MENU_PADDING as i8,
                    MENU_PADDING as i8,
                ))
                .show(ui, |ui| {
                    ui.set_width(widest - MENU_PADDING * 2.0);
                    for item in items {
                        if item.is_empty() {
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), MENU_SEPARATOR_HEIGHT),
                                Sense::hover(),
                            );
                            ui.painter().hline(
                                rect.x_range(),
                                rect.center().y,
                                Stroke::new(1.0, to_color32(palette.border)),
                            );
                            continue;
                        }
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), MENU_ITEM_HEIGHT),
                            Sense::click(),
                        );
                        if response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                wassh_core::RADIUS,
                                to_color32(palette.bg_panel_2),
                            );
                        }
                        if gutter > 0.0 && checked.contains(item) {
                            ui.painter().text(
                                Pos2::new(rect.left() + MENU_ITEM_PADDING, rect.center().y),
                                Align2::LEFT_CENTER,
                                MENU_CHECK_GLYPH,
                                font.clone(),
                                to_color32(palette.accent),
                            );
                        }
                        ui.painter().text(
                            Pos2::new(
                                rect.left() + MENU_ITEM_PADDING + gutter,
                                rect.center().y,
                            ),
                            Align2::LEFT_CENTER,
                            *item,
                            font.clone(),
                            if response.hovered() { text } else { muted },
                        );
                        if let Some(keys) = shortcut_for(shortcuts, item) {
                            ui.painter().text(
                                Pos2::new(
                                    rect.right() - MENU_ITEM_PADDING - gear_gutter,
                                    rect.center().y,
                                ),
                                Align2::RIGHT_CENTER,
                                keys,
                                font.clone(),
                                muted.gamma_multiply(if response.hovered() {
                                    1.0
                                } else {
                                    MENU_SHORTCUT_DIM
                                }),
                            );
                        }
                        if gear_gutter > 0.0 && gears.contains(item) {
                            let centre = Pos2::new(
                                rect.right() - MENU_ITEM_PADDING - gear_gutter / 2.0,
                                rect.center().y,
                            );
                            let gear_rect = Rect::from_center_size(
                                centre,
                                Vec2::splat(MENU_ITEM_HEIGHT - 6.0),
                            );
                            let gear = ui.interact(
                                gear_rect,
                                Id::unique(("wassh-menu-gear", label, *item)),
                                Sense::click(),
                            );
                            if gear.hovered() {
                                ui.painter().rect_filled(
                                    gear_rect,
                                    wassh_core::RADIUS,
                                    to_color32(palette.bg_input),
                                );
                            }
                            paint_gear(
                                ui.painter(),
                                centre,
                                if gear.hovered() { text } else { muted },
                            );
                            if gear.clicked() {
                                return DropdownResult::Gear(item);
                            }
                        }
                        if response.clicked() {
                            return DropdownResult::Chosen(item);
                        }
                    }
                    DropdownResult::Open
                })
                .inner
        });

    let rect = area.response.rect;
    let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    let pressed_outside = ctx.input(|input| {
        input
            .pointer
            .any_pressed()
            .then(|| input.pointer.interact_pos())
            .flatten()
            .is_some_and(|pos| !rect.contains(pos) && !anchor.contains(pos))
    });

    match area.inner {
        DropdownResult::Chosen(item) => DropdownResult::Chosen(item),
        DropdownResult::Gear(item) => DropdownResult::Gear(item),
        _ if escape || pressed_outside => DropdownResult::Closed,
        _ => DropdownResult::Open,
    }
}

/// A small gear: a thick body ring with eight teeth, so it reads as a cog at
/// menu size. Drawn rather than typed so it looks the same whatever fonts are
/// configured.
fn paint_gear(painter: &egui::Painter, centre: Pos2, colour: egui::Color32) {
    for step in 0..GEAR_TEETH {
        // Teeth sit between the gaps of the ring, so the icon is symmetric.
        let angle = std::f32::consts::TAU * (step as f32 + 0.5) / GEAR_TEETH as f32;
        let (sin, cos) = angle.sin_cos();
        let direction = Vec2::new(cos, sin);
        painter.line_segment(
            [
                centre + direction * GEAR_TOOTH_INNER,
                centre + direction * GEAR_TOOTH_OUTER,
            ],
            Stroke::new(GEAR_TOOTH_WIDTH, colour),
        );
    }
    painter.circle_stroke(
        centre,
        GEAR_RING_RADIUS,
        Stroke::new(GEAR_RING_WIDTH, colour),
    );
}

/// Which window button to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl CaptionButton {
    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{2013}",
            Self::Maximize => "\u{25a1}",
            Self::Restore => "\u{2750}",
            Self::Close => "\u{2715}",
        }
    }
}

/// One window button, shaped the way the platform shapes them.
fn caption_button(ui: &mut Ui, palette: &Palette, button: CaptionButton) -> bool {
    let closing = button == CaptionButton::Close;
    let subtle = to_color32(palette.bg_panel_2);

    match caption_style() {
        // Full-height rectangles flush with the title bar's top edge, like Windows.
        CaptionStyle::Windows => {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(46.0, WINDOW_HEADER_HEIGHT),
                Sense::click(),
            );
            if response.hovered() {
                let fill = if closing {
                    to_color32(palette.danger)
                } else {
                    subtle.gamma_multiply(0.9)
                };
                ui.painter().rect_filled(rect, 0.0, fill);
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                button.glyph(),
                FontId::proportional(12.0),
                if response.hovered() && closing {
                    egui::Color32::WHITE
                } else {
                    to_color32(palette.text)
                },
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            response.clicked()
        }
        // Small rounded buttons with breathing room, like Adwaita.
        CaptionStyle::Gnome => {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(34.0, WINDOW_HEADER_HEIGHT),
                Sense::click(),
            );
            let circle = Rect::from_center_size(rect.center(), Vec2::splat(24.0));
            let fill = if response.hovered() {
                if closing {
                    to_color32(palette.danger)
                } else {
                    subtle
                }
            } else {
                to_color32(palette.bg_panel)
            };
            ui.painter().rect_filled(circle, 12.0, fill);
            ui.painter().text(
                circle.center(),
                Align2::CENTER_CENTER,
                button.glyph(),
                FontId::proportional(11.0),
                if response.hovered() && closing {
                    egui::Color32::WHITE
                } else {
                    to_color32(palette.text)
                },
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            response.clicked()
        }
    }
}

/// Invisible band along the window edge that starts a resize.
pub const RESIZE_BAND: f32 = 5.0;

/// Handle the invisible resize bands around the window.
///
/// Only needed when the app draws its own frame (`with_decorations(false)`); with native
/// decorations the window manager owns the edges. Called once per frame before any panel, so
/// the cursor feedback is correct even when the pointer is over a child widget.
pub fn resize_edges(ctx: &egui::Context, maximized: bool) {
    if maximized {
        return;
    }
    let Some(position) = ctx.pointer_hover_pos() else {
        return;
    };
    let rect = ctx.viewport_rect();
    if !rect.contains(position) {
        return;
    }
    let band = RESIZE_BAND;

    let west = position.x - rect.left() < band;
    let east = rect.right() - position.x < band;
    let north = position.y - rect.top() < band;
    let south = rect.bottom() - position.y < band;

    let direction = match (west, east, north, south) {
        (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNwSe)),
        (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNeSw)),
        (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeNeSw)),
        (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeNwSe)),
        (true, _, _, _) => Some((ResizeDirection::West, CursorIcon::ResizeHorizontal)),
        (_, true, _, _) => Some((ResizeDirection::East, CursorIcon::ResizeHorizontal)),
        (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeVertical)),
        (_, _, _, true) => Some((ResizeDirection::South, CursorIcon::ResizeVertical)),
        _ => None,
    };

    if let Some((direction, icon)) = direction {
        ctx.set_cursor_icon(icon);
        if ctx.input(|input| input.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}

/// Height reserved for the bar, re-exported so screens can offset content if needed.
pub const HEIGHT: f32 = WINDOW_HEADER_HEIGHT;
/// Height of menu buttons, matching the design tokens.
pub const MENU_HEIGHT: f32 = MENU_BUTTON_HEIGHT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_bar_holds_its_buttons() {
        const {
            // The client-side header bar has to fit a caption button.
            assert!(HEIGHT == WINDOW_HEADER_HEIGHT);
            assert!(MENU_HEIGHT == MENU_BUTTON_HEIGHT);
            assert!(MENU_BUTTON_HEIGHT < WINDOW_HEADER_HEIGHT);
            // The macOS menu strip stays a slim row under the native title.
            assert!(MENU_BAR_HEIGHT <= 32.0);
        }
    }

    #[test]
    fn caption_buttons_name_their_glyphs() {
        assert_eq!(CaptionButton::Minimize.glyph(), "\u{2013}");
        assert_eq!(CaptionButton::Close.glyph(), "\u{2715}");
        assert_ne!(CaptionButton::Maximize.glyph(), CaptionButton::Restore.glyph());
    }

    #[test]
    fn menu_type_accepts_separators() {
        const ITEMS: &[&str] = &["Preferences…", "", "Quit"];
        let menu: Menu = ("File", ITEMS);
        assert_eq!(menu.1.len(), 3);
        assert!(menu.1[1].is_empty());
    }

    #[test]
    fn accelerators_are_looked_up_by_row_label() {
        const SHORTCUTS: &[(&str, &str)] = &[("Preferences…", "Ctrl+,")];
        assert_eq!(shortcut_for(SHORTCUTS, "Preferences…"), Some("Ctrl+,"));
        // A row without a binding, and a label that is not a row at all, both have none.
        assert_eq!(shortcut_for(SHORTCUTS, "Quit"), None);
        assert_eq!(shortcut_for(SHORTCUTS, "Nope"), None);
    }

    /// Run two frames of a menu bar with `Plugins` open and collect painted text.
    fn open_menu_rows(checked: &[&'static str]) -> Vec<String> {
        const MENUS: &[Menu] = &[("Help", &["About"]), ("Plugins", &["Files", "", "Docker"])];
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 400.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut rows = Vec::new();
        for _ in 0..3 {
            // Keep the menu open; the first frame only sizes the dropdown area, so
            // three frames are needed before its text is painted.
            ctx.data_mut(|data| {
                data.insert_temp(Id::unique("wassh-open-menu"), Some("Plugins"));
            });
            let mut output = ctx.run_ui(input.clone(), |ui| {
                let palette = Palette::dark();
                let state = MenuState {
                    checked,
                    gears: &[],
                    shortcuts: &[],
                };
                let _ = show_menu_bar(ui, &palette, MENUS, state);
            });
            let shapes = std::mem::take(&mut output.shapes);
            rows.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    rows.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        rows
    }

    /// Open the `Plugins` menu and collect every painted text with its left edge, right
    /// edge and y.
    fn open_menu_texts() -> Vec<(f32, f32, f32, String)> {
        const MENUS: &[Menu] = &[
            ("Help", &["About"]),
            ("Plugins", &["Files", "Docker", "", "Scratch"]),
        ];
        const SHORTCUTS: &[(&str, &str)] = &[("Files", "Ctrl+Shift+F"), ("Docker", "Ctrl+F4")];
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 400.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut rows = Vec::new();
        for _ in 0..3 {
            ctx.data_mut(|data| {
                data.insert_temp(Id::unique("wassh-open-menu"), Some("Plugins"));
            });
            let mut output = ctx.run_ui(input.clone(), |ui| {
                let palette = Palette::dark();
                let state = MenuState {
                    checked: &[],
                    gears: &[],
                    shortcuts: SHORTCUTS,
                };
                let _ = show_menu_bar(ui, &palette, MENUS, state);
            });
            let shapes = std::mem::take(&mut output.shapes);
            rows.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    rows.push((
                        text.pos.x,
                        text.pos.x + text.galley.size().x,
                        text.pos.y,
                        text.galley.text().to_owned(),
                    ));
                }
            }
            output.drop_without_applying_deltas();
        }
        rows
    }

    #[test]
    fn menu_rows_show_their_accelerators_against_the_right_edge() {
        // A menu reads like a native one: label left, keys right, and the keys line up in
        // one column whatever the length of the label or the chord.
        let rows = open_menu_texts();
        let find = |label: &str| {
            rows.iter()
                .find(|(_, _, _, text)| text == label)
                .map(|(left, right, y, _)| (*left, *right, *y))
                .unwrap_or_else(|| panic!("{label:?} was not painted: {rows:?}"))
        };
        let (label_left, label_right, label_y) = find("Files");
        let (keys_left, keys_right, keys_y) = find("Ctrl+Shift+F");
        let (other_left, other_right, _) = find("Ctrl+F4");
        assert!(
            keys_left > label_right,
            "the keys sit to the right of the label: {label_right} vs {keys_left}"
        );
        assert!(
            (keys_y - label_y).abs() < 1.0,
            "and on the same row as it: {label_y} vs {keys_y}"
        );
        assert!(
            (keys_right - other_right).abs() < 1.0,
            "the accelerators do not line up: {keys_right} vs {other_right}"
        );
        assert!(
            other_left > label_left,
            "the shorter accelerator starts further right: {other_left} vs {label_left}"
        );
        // A row with no binding is untouched by the column.
        assert!(
            rows.iter().any(|(_, _, _, text)| text == "Scratch"),
            "a shortcut-free row is still drawn"
        );
    }

    /// Open the `Plugins` menu and click at `pointer` on the third frame.
    fn click_in_menu(gears: &[&'static str], pointer: impl Fn(egui::Rect) -> egui::Pos2) -> Option<TitlebarAction> {
        const MENUS: &[Menu] = &[("Help", &["About"]), ("Plugins", &["Files", "Docker"])];
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 400.0));
        let mut action = None;
        let mut click: Option<egui::Pos2> = None;
        for frame in 0..4 {
            ctx.data_mut(|data| {
                data.insert_temp(Id::unique("wassh-open-menu"), Some("Plugins"));
            });
            let mut events = Vec::new();
            if let Some(pos) = click {
                events.push(egui::Event::PointerMoved(pos));
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            }
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let state = MenuState {
                    checked: &[],
                    gears,
                    shortcuts: &[],
                };
                if let Some(found) = show_menu_bar(ui, &palette, MENUS, state) {
                    action = Some(found);
                }
            });
            output.drop_without_applying_deltas();
            if frame == 2 {
                // The dropdown has been laid out by now; aim at its first row.
                let panel = ctx
                    .memory(|memory| memory.area_rect(Id::unique(("wassh-menu", "Plugins"))))
                    .expect("the dropdown exists");
                click = Some(pointer(panel));
            }
        }
        action
    }

    #[test]
    fn the_gear_of_a_menu_entry_reports_itself() {
        // The gear sits at the right edge of the first row.
        let action = click_in_menu(&["Files"], |panel| {
            egui::pos2(
                panel.right() - MENU_PADDING - MENU_ITEM_PADDING - MENU_GEAR_WIDTH / 2.0,
                panel.top() + MENU_PADDING + MENU_ITEM_HEIGHT / 2.0,
            )
        });
        assert_eq!(action, Some(TitlebarAction::MenuGear("Files")));
    }

    #[test]
    fn clicking_the_entry_itself_still_picks_it() {
        let action = click_in_menu(&["Files"], |panel| {
            egui::pos2(
                panel.left() + MENU_PADDING + MENU_ITEM_PADDING + 10.0,
                panel.top() + MENU_PADDING + MENU_ITEM_HEIGHT / 2.0,
            )
        });
        assert_eq!(action, Some(TitlebarAction::MenuItem("Files")));
    }

    #[test]
    fn a_menu_without_checks_has_no_tick() {
        let rows = open_menu_rows(&[]);
        assert!(!rows.contains(&MENU_CHECK_GLYPH.to_owned()), "rows: {rows:?}");
    }

    #[test]
    fn menu_metrics_fit_inside_the_bar() {
        const {
            assert!(MENU_ITEM_HEIGHT < MENU_BAR_HEIGHT);
            assert!(MENU_BUTTON_HEIGHT < MENU_BAR_HEIGHT);
            assert!(MENU_MIN_WIDTH >= 120.0);
        }
    }

    /// Paint the client-side header bar, returning its filled rects and painted texts.
    fn paint_header_bar(
        palette: &Palette,
        menus: &'static [Menu],
    ) -> (Vec<(egui::Rect, egui::Color32)>, Vec<String>) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 400.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut fills = Vec::new();
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                let state = MenuState::default();
                let _ = show_header_bar(ui, palette, "WaSSH", false, menus, state);
            });
            fills.clear();
            texts.clear();
            for clipped in std::mem::take(&mut output.shapes) {
                match clipped.shape {
                    egui::Shape::Rect(rect) if rect.fill != egui::Color32::TRANSPARENT => {
                        fills.push((rect.rect, rect.fill));
                    }
                    egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
                    _ => {}
                }
            }
            output.drop_without_applying_deltas();
        }
        (fills, texts)
    }

    #[test]
    fn the_title_bar_is_painted_from_the_app_theme() {
        // The app draws its own title bar on Windows and Linux, so its background has to
        // come from the palette: a light theme must not sit under a dark strip (or the
        // other way round) the way a desktop-painted title bar would.
        const MENUS: &[Menu] = &[("File", &["Quit"]), ("Help", &["About"])];
        let strip = |fills: &[(egui::Rect, egui::Color32)]| {
            fills
                .iter()
                .find(|(rect, _)| (rect.height() - WINDOW_HEADER_HEIGHT).abs() < 0.5)
                .map(|(_, fill)| *fill)
                .expect("the bar paints its background")
        };
        let (dark_fills, dark_texts) = paint_header_bar(&Palette::dark(), MENUS);
        let (light_fills, light_texts) = paint_header_bar(&Palette::light(), MENUS);
        assert_eq!(strip(&dark_fills), to_color32(Palette::dark().bg_app));
        assert_eq!(strip(&light_fills), to_color32(Palette::light().bg_app));
        assert_ne!(strip(&dark_fills), strip(&light_fills));

        // The title and the menus live in the same strip, whichever theme it is.
        for texts in [dark_texts, light_texts] {
            for label in ["WaSSH", "File", "Help"] {
                assert!(texts.contains(&label.to_owned()), "{label:?} in {texts:?}");
            }
        }
    }
}
