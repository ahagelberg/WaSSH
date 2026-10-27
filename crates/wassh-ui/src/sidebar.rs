//! The sessions sidebar: a header, saved hosts in optional colour-coded groups, and an
//! ungrouped section.
//!
//! Metrics (`.host-list`, `.host-group`,
//! `.host-item`): 240 px wide, 8 px list padding, group cards with an 8 px radius and
//! 6×8 padding, host rows with 6×4 padding, a 4 px colour bar and 2 px row gaps.

use egui::{Align2, FontId, Id, Pos2, Rect, Sense, Ui, Vec2};
use wassh_core::{HostGroup, HostProfile, HostsDocument, Palette, colors};

use crate::terminal_view::to_color32;
use crate::widgets::{Button, empty_state};

/// Expanded sidebar width.
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// Collapsed rail width.
pub const COLLAPSED_WIDTH: f32 = 40.0;
/// Height of the sidebar header, matching the titlebar.
pub const HEADER_HEIGHT: f32 = 36.0;
/// Padding around the host list.
pub const LIST_PADDING: f32 = 8.0;
/// Gap between group cards.
pub const GROUP_GAP: f32 = 6.0;
/// Padding inside a group card.
pub const GROUP_PADDING: Vec2 = Vec2::new(8.0, 6.0);
/// Corner radius of a group card.
pub const GROUP_RADIUS: u8 = 8;
/// Height of a group header row.
pub const GROUP_HEADER_HEIGHT: f32 = 28.0;
/// Padding inside a host row.
pub const HOST_PADDING: Vec2 = Vec2::new(6.0, 4.0);
/// Gap between host rows.
pub const HOST_GAP: f32 = 2.0;
/// Width of the accent bar on a coloured host row.
pub const ACCENT_BAR: f32 = 4.0;
/// Diameter of the group colour dot.
pub const COLOR_DOT: f32 = 9.0;
/// Size of the collapse twisty.
pub const COLLAPSE_SIZE: f32 = 16.0;
/// Pointer travel before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 4.0;
/// Height of a drop insertion indicator.
pub const DROP_INDICATOR: f32 = 2.0;
/// Width of the colour popover.
pub const COLOR_POPOVER_WIDTH: f32 = 224.0;
/// Swatch size inside the colour popover.
pub const SWATCH: f32 = 24.0;

/// A user action from the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarAction {
    /// Open a saved host in a new tab (or focus its existing tab).
    OpenHost(String),
    /// Open the "new host" dialog.
    NewHost,
    /// Open the "new group" dialog.
    NewGroup,
    /// Collapse or expand the whole sidebar.
    ToggleCollapsed,
    /// Collapse or expand one group.
    ToggleGroup(String),
    /// Commit an inline group rename.
    RenameGroup(String, String),
    /// Set (or clear) a group colour.
    /// Right-clicked a host.
    HostContext(String, (i32, i32)),
    /// Right-clicked a group.
    GroupContext(String, (i32, i32)),
    /// Move a host into a group (or the ungrouped section) at an index.
    MoveHost {
        /// Host being moved.
        host: String,
        /// Destination group, `None` for ungrouped.
        group: Option<String>,
        /// Index within the destination.
        index: usize,
    },
}

/// Transient sidebar state that must survive between frames.
#[derive(Default)]
pub struct SidebarState {
    /// Whether the sidebar is collapsed to the icon rail.
    pub collapsed: bool,
    /// Host highlighted by a single click; opening one needs a double click.
    pub selected: Option<String>,
    /// Group currently being renamed, with its draft text.
    pub renaming: Option<(String, String)>,
    /// Group whose colour popover is open.

    drag: Option<HostDrag>,
}

#[derive(Clone, Debug)]
struct HostDrag {
    host: String,
    active: bool,
}

impl SidebarState {
    /// Fresh state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin an inline rename for `group`.
    pub fn begin_rename(&mut self, group: &HostGroup) {
        self.renaming = Some((group.id.clone(), group.name.clone()));
    }

    /// Whether a host drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.active)
    }

    /// Whether `id` is the highlighted host.
    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.as_deref() == Some(id)
    }

    /// Highlight a host, or clear the highlight with `None`.
    pub fn select(&mut self, id: Option<&str>) {
        self.selected = id.map(str::to_owned);
    }
}

/// Subtitle for a host row: `user@host:port`, `host:port` or the serial description.
pub fn host_meta(host: &HostProfile) -> String {
    host.connection.subtitle()
}

/// Draw the whole sidebar, returning the first action the user triggered.
pub fn sidebar(
    ui: &mut Ui,
    palette: &Palette,
    hosts: &HostsDocument,
    state: &mut SidebarState,
) -> Option<SidebarAction> {
    let mut action = None;
    let width = if state.collapsed {
        COLLAPSED_WIDTH
    } else {
        SIDEBAR_WIDTH
    };
    // A highlight that outlived its host would point at nothing.
    if let Some(id) = state.selected.clone()
        && hosts.host(&id).is_none()
    {
        state.selected = None;
    }
    let frame = egui::Frame::NONE
        .fill(to_color32(palette.bg_panel))
        .inner_margin(egui::Margin::ZERO);
    frame.show(ui, |ui| {
        ui.set_width(width);
        ui.set_min_height(ui.available_height());
        if state.collapsed {
            action = collapsed_rail(ui, palette);
            return;
        }

        // Header: title plus the collapse button.
        let (header, _) = ui.allocate_exact_size(
            Vec2::new(width, HEADER_HEIGHT),
            Sense::hover(),
        );
        ui.painter()
            .rect_filled(header, 0.0, to_color32(palette.bg_panel));
        ui.painter().hline(
            header.x_range(),
            header.bottom(),
            egui::Stroke::new(1.0, to_color32(palette.border)),
        );
        ui.painter().text(
            Pos2::new(header.left() + LIST_PADDING + 2.0, header.center().y),
            Align2::LEFT_CENTER,
            "Hosts",
            FontId::proportional(13.0),
            to_color32(palette.text),
        );
        let collapse_rect = Rect::from_center_size(
            Pos2::new(header.right() - LIST_PADDING - 11.0, header.center().y),
            Vec2::new(22.0, 22.0),
        );
        let collapse = ui.interact(
            collapse_rect,
            Id::unique("wassh-sidebar-collapse"),
            Sense::click(),
        );
        ui.painter().rect_filled(
            collapse_rect,
            4.0,
            to_color32(if collapse.hovered() {
                palette.bg_panel_2
            } else {
                palette.bg_input
            }),
        );
        ui.painter().rect_stroke(
            collapse_rect,
            4.0,
            egui::Stroke::new(1.0, to_color32(palette.border)),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            collapse_rect.center(),
            Align2::CENTER_CENTER,
            "\u{00ab}",
            FontId::proportional(12.0),
            if collapse.hovered() {
                to_color32(palette.text)
            } else {
                to_color32(palette.text_muted)
            },
        );
        if collapse.clicked() {
            action = Some(SidebarAction::ToggleCollapsed);
        }

        // The two buttons sit under the list rather than in it, so a list that scrolls
        // never carries them off screen.
        egui::Panel::bottom("wassh-sidebar-actions")
            .frame(
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(
                    LIST_PADDING as i8,
                    LIST_PADDING as i8,
                )),
            )
            .show(ui, |ui| {
                if let Some(found) = list_footer(ui, palette) {
                    action = Some(found);
                }
            });

        // Host list.
        let list_rect = Rect::from_min_max(
            Pos2::new(header.left(), header.bottom()),
            Pos2::new(width, ui.available_height() + header.bottom()),
        );
        let mut list = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut list, |ui| {
                ui.add_space(LIST_PADDING);
                ui.horizontal(|ui| {
                    ui.add_space(LIST_PADDING);
                    ui.vertical(|ui| {
                        ui.set_width(width - LIST_PADDING * 2.0);
                        if let Some(found) = host_list(ui, palette, hosts, state) {
                            action = Some(found);
                        }
                    });
                });
                ui.add_space(LIST_PADDING);
            });
    });
    action
}

fn collapsed_rail(ui: &mut Ui, palette: &Palette) -> Option<SidebarAction> {
    let mut action = None;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(COLLAPSED_WIDTH, HEADER_HEIGHT),
        Sense::hover(),
    );
    let button = ui.interact(rect, Id::unique("wassh-sidebar-expand"), Sense::click());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        "\u{00bb}",
        FontId::proportional(14.0),
        if button.hovered() {
            to_color32(palette.text)
        } else {
            to_color32(palette.text_muted)
        },
    );
    if button.clicked() {
        // The shell owns the flag: flipping it here as well would toggle twice and
        // leave the sidebar collapsed.
        action = Some(SidebarAction::ToggleCollapsed);
    }
    action
}

fn host_list(
    ui: &mut Ui,
    palette: &Palette,
    hosts: &HostsDocument,
    state: &mut SidebarState,
) -> Option<SidebarAction> {
    let mut action = None;
    let mut drop_target: Option<(Option<String>, usize)> = None;
    let mut shown_groups = 0usize;

    for group in &hosts.groups {
        let members = hosts.hosts_in_group(&group.id);
        // A group with nothing in it is noise, unless its name is being edited (a freshly
        // created group must stay on screen so the rename field has somewhere to live).
        let renaming = state
            .renaming
            .as_ref()
            .is_some_and(|(id, _)| *id == group.id);
        if members.is_empty() && !renaming {
            continue;
        }
        shown_groups += 1;
        if let Some(found) = group_card(ui, palette, group, &members, state, &mut drop_target) {
            action = Some(found);
        }
    }

    let ungrouped = hosts.ungrouped();
    if !ungrouped.is_empty() || (hosts.groups.is_empty() && shown_groups == 0) {
        ui.add_space(GROUP_GAP);
        if hosts.groups.is_empty() {
            // Without groups the hosts are drawn as a flat list.
            for (index, host) in ungrouped.iter().enumerate() {
                if let Some(found) =
                    host_row(ui, palette, host, state, None, index, &mut drop_target)
                {
                    action = Some(found);
                }
                ui.add_space(HOST_GAP);
            }
            if ungrouped.is_empty() {
                empty_hosts(ui, palette);
            }
        } else {
            ui.label(
                egui::RichText::new("UNGROUPED")
                    .size(10.0)
                    .color(to_color32(palette.text_muted)),
            );
            ui.add_space(4.0);
            for (index, host) in ungrouped.iter().enumerate() {
                if let Some(found) =
                    host_row(ui, palette, host, state, None, index, &mut drop_target)
                {
                    action = Some(found);
                }
                ui.add_space(HOST_GAP);
            }
        }
    }

    // Every group is empty and nothing sits outside them.
    if shown_groups == 0 && ungrouped.is_empty() {
        ui.add_space(GROUP_GAP);
        empty_hosts(ui, palette);
    }

    // Finish a drag once the pointer is released.
    let released = ui.ctx().input(|input| input.pointer.any_released());
    if released
        && let Some(drag) = state.drag.take()
        && drag.active
        && let Some((group, index)) = drop_target
    {
        action = Some(SidebarAction::MoveHost {
            host: drag.host,
            group,
            index,
        });
    }

    action
}

/// The "New host…" / "New group" pair, pinned under the scrolling list.
fn list_footer(ui: &mut Ui, palette: &Palette) -> Option<SidebarAction> {
    let mut action = None;
    let button_width = ((ui.available_width() - HOST_GAP) / 2.0).max(60.0);
    ui.horizontal(|ui| {
        if Button::new("New host…")
            .primary()
            .min_width(button_width)
            .show(ui, palette)
            .clicked()
        {
            action = Some(SidebarAction::NewHost);
        }
        ui.add_space(HOST_GAP);
        if Button::new("New group")
            .min_width(button_width)
            .show(ui, palette)
            .clicked()
        {
            action = Some(SidebarAction::NewGroup);
        }
    });
    action
}

fn empty_hosts(ui: &mut Ui, palette: &Palette) {
    empty_state(
        ui,
        palette,
        "No saved hosts yet.",
        "Use New host… to add your first connection.",
    );
}

fn group_card(
    ui: &mut Ui,
    palette: &Palette,
    group: &HostGroup,
    members: &[&HostProfile],
    state: &mut SidebarState,
    drop_target: &mut Option<(Option<String>, usize)>,
) -> Option<SidebarAction> {
    let mut action = None;
    let card_fill = match group.color {
        Some(accent) => colors::tint(accent, palette.bg_input, colors::TINT_GROUP),
        None => palette.bg_input,
    };

    let frame = egui::Frame::NONE
        .fill(to_color32(card_fill))
        .stroke(egui::Stroke::new(
            1.0,
            to_color32(palette.border).gamma_multiply(0.7),
        ))
        .corner_radius(GROUP_RADIUS)
        .inner_margin(egui::Margin::symmetric(
            GROUP_PADDING.x as i8,
            GROUP_PADDING.y as i8,
        ));
    frame.show(ui, |ui| {
        ui.set_width(SIDEBAR_WIDTH - LIST_PADDING * 2.0 - GROUP_PADDING.x * 2.0);

        // Header row.
        let (header, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), GROUP_HEADER_HEIGHT),
            Sense::click(),
        );
        let mut left = header.left();
        if let Some(accent) = group.color {
            let dot = Pos2::new(left + 2.0 + COLOR_DOT / 2.0, header.center().y);
            ui.painter().circle_filled(
                dot,
                COLOR_DOT / 2.0,
                to_color32(accent),
            );
        }
        left += 2.0 + COLOR_DOT + 4.0;

        let collapse = ui.interact(
            Rect::from_center_size(
                Pos2::new(left + COLLAPSE_SIZE / 2.0, header.center().y),
                Vec2::new(COLLAPSE_SIZE, COLLAPSE_SIZE),
            ),
            Id::unique(("wassh-group-collapse", &group.id)),
            Sense::click(),
        );
        ui.painter().text(
            Pos2::new(left + COLLAPSE_SIZE / 2.0, header.center().y),
            Align2::CENTER_CENTER,
            if group.collapsed { "\u{25b8}" } else { "\u{25be}" },
            FontId::proportional(10.0),
            to_color32(palette.text_muted),
        );
        left += COLLAPSE_SIZE + 4.0;

        let count = members.len().to_string();
        let count_width = 24.0;
        let renaming = state
            .renaming
            .as_ref()
            .is_some_and(|(id, _)| id == &group.id);

        if renaming {
            let draft = state
                .renaming
                .as_mut()
                .map(|(_, draft)| draft)
                .expect("renaming implies a draft");
            let edit_rect = Rect::from_min_max(
                Pos2::new(left, header.top() + 3.0),
                Pos2::new(header.right() - count_width, header.bottom() - 3.0),
            );
            let mut edit = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(edit_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let response = edit.add(
                egui::TextEdit::singleline(draft)
                    .font(FontId::proportional(11.0))
                    .text_color(to_color32(palette.text))
                    .background_color(to_color32(palette.bg_input))
                    .desired_width(edit_rect.width()),
            );
            response.request_focus();
            let commit = edit.input(|input| input.key_pressed(egui::Key::Enter));
            let cancel = edit.input(|input| input.key_pressed(egui::Key::Escape));
            if commit {
                let name = draft.trim().to_owned();
                if !name.is_empty() {
                    action = Some(SidebarAction::RenameGroup(group.id.clone(), name));
                }
                state.renaming = None;
            } else if cancel {
                state.renaming = None;
            }
        } else {
            let title_rect = Rect::from_min_max(
                Pos2::new(left, header.top()),
                Pos2::new(header.right() - count_width - 4.0, header.bottom()),
            );
            let painter = ui.painter().with_clip_rect(title_rect);
            painter.text(
                Pos2::new(title_rect.left(), title_rect.center().y),
                Align2::LEFT_CENTER,
                group.name.to_uppercase(),
                FontId::proportional(11.0),
                to_color32(palette.text_muted),
            );
            ui.painter().text(
                Pos2::new(header.right(), header.center().y),
                Align2::RIGHT_CENTER,
                count,
                FontId::proportional(10.0),
                to_color32(palette.text_muted),
            );
        }

        if collapse.clicked() {
            action = Some(SidebarAction::ToggleGroup(group.id.clone()));
        } else if response.double_clicked() {
            state.begin_rename(group);
        } else if response.secondary_clicked() {
            let pos = ui
                .ctx()
                .input(|input| input.pointer.interact_pos())
                .unwrap_or(header.center());
            action = Some(SidebarAction::GroupContext(
                group.id.clone(),
                (pos.x.round() as i32, pos.y.round() as i32),
            ));
        }

        if !group.collapsed {
            ui.add_space(4.0);
            for (index, host) in members.iter().enumerate() {
                if let Some(found) =
                    host_row(ui, palette, host, state, Some(&group.id), index, drop_target)
                {
                    action = Some(found);
                }
                ui.add_space(HOST_GAP);
            }
            if members.is_empty() {
                ui.label(
                    egui::RichText::new("No hosts")
                        .size(11.0)
                        .italics()
                        .color(to_color32(palette.text_muted)),
                );
            }
        }
    });

    ui.add_space(GROUP_GAP);
    action
}

#[allow(clippy::too_many_arguments)]
fn host_row(
    ui: &mut Ui,
    palette: &Palette,
    host: &HostProfile,
    state: &mut SidebarState,
    group: Option<&str>,
    index: usize,
    drop_target: &mut Option<(Option<String>, usize)>,
) -> Option<SidebarAction> {
    let mut action = None;
    let height = 36.0;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::click_and_drag(),
    );
    let dragging = state
        .drag
        .as_ref()
        .is_some_and(|drag| drag.host == host.id && drag.active);

    let backdrop = palette.bg_panel_2;
    let selected = state.is_selected(&host.id);
    let fill = if host.connection.style.tab_color.is_some() {
        colors::row_background(
            host.connection.style.tab_color,
            backdrop,
            selected,
            response.hovered(),
        )
    } else if selected {
        colors::tint(palette.accent, backdrop, colors::TINT_IDLE)
    } else {
        backdrop
    };
    if response.hovered() || dragging || selected {
        ui.painter()
            .rect_filled(rect, wassh_core::RADIUS, to_color32(fill));
        if response.hovered() && !selected {
            ui.painter().rect_stroke(
                rect,
                wassh_core::RADIUS,
                egui::Stroke::new(1.0, to_color32(palette.border)),
                egui::StrokeKind::Inside,
            );
        }
    }

    let mut left = rect.left() + HOST_PADDING.x;
    let bar = Rect::from_min_max(
        Pos2::new(left, rect.top() + 2.0),
        Pos2::new(left + ACCENT_BAR, rect.bottom() - 2.0),
    );
    let bar_colour = match host.connection.style.tab_color {
        Some(accent) => to_color32(accent),
        None if selected => to_color32(palette.accent),
        None => egui::Color32::TRANSPARENT,
    };
    ui.painter().rect_filled(bar, 2, bar_colour);
    left += ACCENT_BAR + 6.0;

    let name_rect = Rect::from_min_max(
        Pos2::new(left, rect.top() + 3.0),
        Pos2::new(rect.right() - HOST_PADDING.x, rect.center().y),
    );
    let painter = ui.painter().with_clip_rect(name_rect);
    painter.text(
        Pos2::new(name_rect.left(), name_rect.center().y),
        Align2::LEFT_CENTER,
        host.display_name(),
        FontId::proportional(12.0),
        to_color32(palette.text),
    );
    let meta_rect = Rect::from_min_max(
        Pos2::new(left, rect.center().y),
        Pos2::new(rect.right() - HOST_PADDING.x, rect.bottom() - 3.0),
    );
    let painter = ui.painter().with_clip_rect(meta_rect);
    painter.text(
        Pos2::new(meta_rect.left(), meta_rect.center().y),
        Align2::LEFT_CENTER,
        host_meta(host),
        FontId::proportional(11.0),
        to_color32(palette.text_muted),
    );

    if dragging {
        ui.painter().rect_stroke(
            rect,
            wassh_core::RADIUS,
            egui::Stroke::new(
                1.0,
                to_color32(palette.accent).gamma_multiply(0.5),
            ),
            egui::StrokeKind::Inside,
        );
    }

    // Track drag progress to compute the drop slot under the pointer.
    if response.drag_started() {
        state.drag = Some(HostDrag {
            host: host.id.clone(),
            active: false,
        });
    }
    if let Some(drag) = state.drag.as_mut()
        && drag.host == host.id
        && response.dragged()
    {
        drag.active = true;
    }
    if state.is_dragging()
        && response.hovered()
        && let Some(pointer) = ui.ctx().input(|input| input.pointer.hover_pos())
    {
        let ratio = (pointer.y - rect.top()) / rect.height();
        let slot = if ratio <= 0.5 { index } else { index + 1 };
        *drop_target = Some((group.map(str::to_owned), slot));
        let y = if ratio <= 0.5 {
            rect.top()
        } else {
            rect.bottom()
        };
        ui.painter().hline(
            rect.x_range(),
            y,
            egui::Stroke::new(DROP_INDICATOR, to_color32(palette.accent)),
        );
    }

    // Double click is tested first: egui reports the second click of a double click as a
    // click as well, and opening must win over plain selection.
    if response.double_clicked() {
        state.select(Some(&host.id));
        action = Some(SidebarAction::OpenHost(host.id.clone()));
    } else if response.clicked() {
        // A single click only highlights the row; opening needs a double click, the way
        // PuTTY's session list and the original build behave.
        state.select(Some(&host.id));
    } else if response.secondary_clicked() {
        // The context menu acts on the highlighted row.
        state.select(Some(&host.id));
        let pos = ui
            .ctx()
            .input(|input| input.pointer.interact_pos())
            .unwrap_or(rect.center());
        action = Some(SidebarAction::HostContext(
            host.id.clone(),
            (pos.x.round() as i32, pos.y.round() as i32),
        ));
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Click the middle of `rect` and return the action the sidebar produced.
    fn click_at(pos: egui::Pos2, state: &mut SidebarState) -> Option<SidebarAction> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let hosts = hosts_doc();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(400.0, 600.0),
            )),
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        };
        let mut action = None;
        for _ in 0..2 {
            let output = ctx.run_ui(input.clone(), |ui| {
                action = sidebar(ui, &palette, &hosts, state);
            });
            output.drop_without_applying_deltas();
        }
        action
    }

    #[test]
    fn the_collapsed_rail_asks_the_shell_to_expand() {
        let mut state = SidebarState {
            collapsed: true,
            ..SidebarState::default()
        };
        // The chevron sits in the rail's header cell.
        let action = click_at(egui::pos2(COLLAPSED_WIDTH / 2.0, HEADER_HEIGHT / 2.0), &mut state);
        assert_eq!(action, Some(SidebarAction::ToggleCollapsed));
        // The widget must not flip the flag itself: the shell does, and a second
        // flip here would immediately collapse the sidebar again.
        assert!(state.collapsed, "the sidebar leaves the toggle to the shell");
    }

    fn hosts_doc() -> HostsDocument {
        let mut group = HostGroup {
            id: "g1".to_owned(),
            name: "Dubbelanka".to_owned(),
            color: None,
            collapsed: false,
        };
        group.name = "Dubbelanka".to_owned();
        HostsDocument {
            groups: vec![group],
            hosts: vec![
                HostProfile {
                    id: "h1".to_owned(),
                    name: "Dev".to_owned(),
                    group_id: Some("g1".to_owned()),
                    connection: ConnectionParams {
                        host: "192.168.42.7".to_owned(),
                        port: 22,
                        username: "brainchild".to_owned(),
                        style: wassh_core::SessionStyle {
                            tab_color: Some(wassh_core::colors::GROUP_COLOR_PRESETS[4]),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                HostProfile {
                    id: "h2".to_owned(),
                    name: "Torrent".to_owned(),
                    group_id: Some("g1".to_owned()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
        .normalized()
    }

    /// Number of painted labels containing `needle`, used to check what is on screen.
    fn painted_text(ctx: &egui::Context, hosts: &HostsDocument, state: &mut SidebarState) -> Vec<String> {
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 700.0))),
            ..Default::default()
        };
        let mut labels = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                sidebar(ui, &palette, hosts, state);
            });
            let shapes = std::mem::take(&mut output.shapes);
            labels.clear();
            labels.extend(shapes.into_iter().filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            }));
            output.drop_without_applying_deltas();
        }
        labels
    }

    /// Painted labels with the position each was drawn at.
    fn label_positions(
        input: &egui::RawInput,
        hosts: &HostsDocument,
        state: &mut SidebarState,
    ) -> Vec<(String, egui::Pos2)> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut labels = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                sidebar(ui, &palette, hosts, state);
            });
            let shapes = std::mem::take(&mut output.shapes);
            labels.clear();
            labels.extend(shapes.into_iter().filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos)),
                _ => None,
            }));
            output.drop_without_applying_deltas();
        }
        labels
    }

    #[test]
    fn the_new_host_buttons_stay_under_a_scrolling_list() {
        let hosts = HostsDocument {
            hosts: (0..40)
                .map(|index| HostProfile {
                    id: format!("h{index}"),
                    name: format!("Host {index}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
        .normalized();
        let mut state = SidebarState::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 400.0))),
            ..Default::default()
        };
        let label = |labels: &[(String, egui::Pos2)], needle: &str| {
            labels
                .iter()
                .find(|(text, _)| text == needle)
                .map(|(_, pos)| *pos)
                .unwrap_or_else(|| panic!("{needle} was not painted"))
        };
        let before = label_positions(&input, &hosts, &mut state);
        let new_host = label(&before, "New host…");
        assert!(
            before.iter().any(|(text, _)| text == "Host 0"),
            "the list painted no hosts: {before:?}"
        );
        assert!(
            new_host.y > 340.0,
            "the button sits at {} instead of under the list",
            new_host.y
        );

        // Scrolling the list must not move the buttons.
        let scrolled = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(Pos2::new(120.0, 150.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -300.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..input.clone()
        };
        let after = label_positions(&scrolled, &hosts, &mut state);
        assert_eq!(label(&after, "New host…"), new_host);
        let host0 = after
            .iter()
            .find(|(text, _)| text == "Host 0")
            .map(|(_, pos)| *pos);
        assert!(
            host0.is_none_or(|pos| pos.y < 0.0),
            "the list did not scroll: {host0:?}"
        );
    }

    #[test]
    fn groups_without_hosts_are_hidden() {
        let ctx = egui::Context::default();
        let hosts = HostsDocument {
            groups: vec![
                HostGroup {
                    id: "g1".to_owned(),
                    name: "Dubbelanka".to_owned(),
                    color: None,
                    collapsed: false,
                },
                HostGroup {
                    id: "empty".to_owned(),
                    name: "UNGROUPED".to_owned(),
                    color: None,
                    collapsed: false,
                },
            ],
            hosts: vec![HostProfile {
                id: "h1".to_owned(),
                name: "Dev".to_owned(),
                group_id: Some("g1".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut state = SidebarState::new();
        let labels = painted_text(&ctx, &hosts, &mut state);
        assert!(labels.iter().any(|label| label.contains("DUBBELANKA")));
        assert!(
            !labels.iter().any(|label| label.contains("UNGROUPED")),
            "the empty group still painted: {labels:?}"
        );
        assert!(!labels.iter().any(|label| label == "No hosts"));
    }

    #[test]
    fn a_group_being_renamed_stays_visible_even_when_empty() {
        let ctx = egui::Context::default();
        let hosts = HostsDocument {
            groups: vec![HostGroup {
                id: "empty".to_owned(),
                name: "Fresh".to_owned(),
                color: None,
                collapsed: false,
            }],
            ..Default::default()
        };
        let mut state = SidebarState::new();
        let group = hosts.groups[0].clone();
        state.begin_rename(&group);
        let labels = painted_text(&ctx, &hosts, &mut state);
        // The rename field is a text edit, but the card header itself is still drawn.
        assert!(
            !labels.iter().any(|label| label.contains("No saved hosts")),
            "the panel fell back to the empty state: {labels:?}"
        );
    }

    #[test]
    fn the_panel_renders_with_groups_and_selection() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let hosts = hosts_doc();
        let mut state = SidebarState::new();
        state.select(Some("h1"));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 700.0))),
            ..Default::default()
        };
        let mut painted = 0;
        for _ in 0..2 {
            let output = ctx.run_ui(input.clone(), |ui| {
                sidebar(ui, &palette, &hosts, &mut state);
            });
            painted = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        assert!(painted > 20, "the sidebar painted only {painted} shapes");
        assert!(state.is_selected("h1"));
    }

    #[test]
    fn selecting_a_host_highlights_only_that_host() {
        let mut state = SidebarState::new();
        assert!(!state.is_selected("alpha"));
        state.select(Some("alpha"));
        assert!(state.is_selected("alpha"));
        assert!(!state.is_selected("beta"));
        state.select(Some("beta"));
        assert!(state.is_selected("beta"));
        assert!(!state.is_selected("alpha"));
        state.select(None);
        assert!(!state.is_selected("beta"));
    }

    use wassh_core::{ConnectionParams, ConnectionType};

    fn host(id: &str) -> HostProfile {
        HostProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            connection: ConnectionParams {
                host: "example.com".to_owned(),
                port: 22,
                username: "alice".to_owned(),
                kind: ConnectionType::Ssh,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn host_meta_shows_the_endpoint() {
        assert_eq!(host_meta(&host("a")), "alice@example.com:22");
    }

    #[test]
    fn sidebar_widths_are_fixed() {
        assert_eq!(SIDEBAR_WIDTH, 240.0);
        assert_eq!(COLLAPSED_WIDTH, 40.0);
        assert_eq!(GROUP_RADIUS, 8);
        assert_eq!(HOST_GAP, 2.0);
        assert_eq!(ACCENT_BAR, 4.0);
        assert_eq!(COLOR_DOT, 9.0);
    }

    #[test]
    fn rename_state_round_trips() {
        let mut state = SidebarState::new();
        let group = HostGroup {
            id: "g1".to_owned(),
            name: "Production".to_owned(),
            color: None,
            collapsed: false,
        };
        state.begin_rename(&group);
        assert_eq!(
            state.renaming,
            Some(("g1".to_owned(), "Production".to_owned()))
        );
    }

    #[test]
    fn drag_threshold_is_four_pixels() {
        assert_eq!(DRAG_THRESHOLD, 4.0);
        let mut state = SidebarState::new();
        assert!(!state.is_dragging());
        state.drag = Some(HostDrag {
            host: "a".to_owned(),
            active: true,
        });
        assert!(state.is_dragging());
    }
}
