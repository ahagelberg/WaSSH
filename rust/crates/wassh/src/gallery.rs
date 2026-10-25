//! A gallery of the widget kit, reachable with `--widgets`.
//!
//! This is a development aid: it exercises every primitive against the real palette so
//! spacing and colour regressions are obvious before screens are built.

use wassh_core::{ConnectionType, Palette, Rgba};
use wassh_ui::widgets::{
    BannerKind, Button, MenuItem, MenuState, SplitAxis, TableColumn, ToastKind, Toasts, Tone,
    TreeRow, badge, banner, card, checkbox, empty_state, field, hex_color_input, modal,
    number_input, password_input, progress, search_input, section, select, splitter,
    table_header, table_row, text_input, tree_row,
};

pub struct Gallery {
    text: String,
    password: String,
    search: String,
    remember: bool,
    connection: ConnectionType,
    sort: Option<(usize, bool)>,
    selected: Option<usize>,
    font_size: u32,
    scrollback: u32,
    accent: Rgba,
    accent_draft: String,
    menu: MenuState,
    toasts: Toasts,
    modal_open: bool,
    split: f32,
    expanded: [bool; 2],
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            text: "example.com".to_owned(),
            password: String::new(),
            search: String::new(),
            remember: true,
            connection: ConnectionType::Ssh,
            sort: None,
            selected: None,
            font_size: 14,
            scrollback: 10_000,
            accent: Rgba::rgb(0x3d8bfd),
            accent_draft: "#3d8bfd".to_owned(),
            menu: MenuState::new(),
            toasts: Toasts::new(),
            modal_open: false,
            split: 240.0,
            expanded: [true, false],
        }
    }
}

impl Gallery {
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            section(ui, palette, "Buttons", |ui| {
                ui.horizontal_wrapped(|ui| {
                    Button::new("Secondary").show(ui, palette);
                    Button::new("Primary").primary().show(ui, palette);
                    Button::new("Danger").danger().show(ui, palette);
                    Button::new("Ghost").ghost().show(ui, palette);
                    Button::new("Disabled").enabled(false).show(ui, palette);
                    Button::new("Compact").compact().show(ui, palette);
                    Button::new("Wide").min_width(120.0).show(ui, palette);
                });
            });

            section(ui, palette, "Fields", |ui| {
                field(ui, palette, "Host", Some("hostname or IP address"), |ui| {
                    text_input(ui, palette, &mut self.text, "example.com")
                });
                ui.add_space(10.0);
                field(ui, palette, "Password", Some("optional"), |ui| {
                    password_input(ui, palette, &mut self.password, "(stored)")
                });
                ui.add_space(10.0);
                field(ui, palette, "Search", None, |ui| {
                    search_input(ui, palette, &mut self.search)
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    checkbox(ui, palette, &mut self.remember, "Remember this host");
                });
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("TYPE")
                        .size(11.0)
                        .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
                );
                let options = [
                    (ConnectionType::Ssh, "SSH"),
                    (ConnectionType::Telnet, "Telnet"),
                    (ConnectionType::Serial, "Serial"),
                    (ConnectionType::Local, "Local terminal"),
                ];
                if let Some(chosen) = select(ui, "gallery-connection", &self.connection, &options) {
                    self.connection = chosen;
                }
            });

            section(ui, palette, "Surfaces", |ui| {
                card(ui, palette, |ui| {
                    ui.label(
                        egui::RichText::new("Card body")
                            .size(13.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text)),
                    );
                    ui.add_space(6.0);
                    progress(ui, palette, 0.62);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        badge(ui, palette, "connected", Tone::Success);
                        badge(ui, palette, "2 updates", Tone::Warning);
                        badge(ui, palette, "error", Tone::Danger);
                        badge(ui, palette, "info", Tone::Info);
                    });
                });
                ui.add_space(10.0);
                banner(ui, palette, BannerKind::Info, "Saved password will be used for this session.");
                ui.add_space(6.0);
                banner(ui, palette, BannerKind::Warn, "WARNING: remote host key does not match the cached key!");
            });

            section(ui, palette, "Table", |ui| {
                let columns = [
                    TableColumn::new("Name", 0.46),
                    TableColumn::new("Size", 0.14).numeric(),
                    TableColumn::new("Modified", 0.40),
                ];
                if let Some(index) = table_header(ui, palette, &columns, self.sort) {
                    self.sort = Some(match self.sort {
                        Some((sorted, descending)) if sorted == index => (index, !descending),
                        _ => (index, true),
                    });
                }
                let rows = [
                    ["notes.md", "4 KiB", "2026-10-20 14:02"],
                    ["screenshot.png", "812 KiB", "2026-10-21 09:15"],
                    ["backup.tar.zst", "1.2 GiB", "2026-10-22 23:40"],
                ];
                for (index, row) in rows.iter().enumerate() {
                    let response = table_row(ui, palette, &columns, row, self.selected == Some(index));
                    if response.clicked() {
                        self.selected = Some(index);
                    }
                }
            });

            section(ui, palette, "Empty state", |ui| {
                empty_state(
                    ui,
                    palette,
                    "No saved hosts yet.",
                    "Use New host… to add your first connection.",
                );
            });

            section(ui, palette, "Numbers and colours", |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Font size")
                            .size(12.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
                    );
                    number_input(ui, palette, &mut self.font_size, 8, 48);
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Scrollback")
                            .size(12.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
                    );
                    number_input(ui, palette, &mut self.scrollback, 100, 100_000);
                });
                ui.add_space(8.0);
                hex_color_input(ui, palette, &mut self.accent_draft, &mut self.accent);
            });

            section(ui, palette, "Tree", |ui| {
                let group = ["Production", "Staging"];
                let children = ["db-1.example.com", "cache-1.example.com"];
                for (index, name) in group.iter().enumerate() {
                    let row = tree_row(
                        ui,
                        palette,
                        TreeRow::new(name)
                            .branch(self.expanded[index])
                            .badge(if index == 0 { "2 hosts" } else { "1 host" }, Tone::Info),
                    );
                    if row.toggled {
                        self.expanded[index] = !self.expanded[index];
                    }
                }
                if self.expanded[0] {
                    for (child, label) in children.iter().enumerate() {
                        tree_row(
                            ui,
                            palette,
                            TreeRow::new(label).depth(1).selected(child == 0),
                        );
                    }
                }
            });

            section(ui, palette, "Menus, toasts and modals", |ui| {
                ui.horizontal_wrapped(|ui| {
                    if Button::new("Open context menu").show(ui, palette).clicked() {
                        let position = ui
                            .ctx()
                            .input(|input| input.pointer.interact_pos())
                            .unwrap_or(egui::Pos2::ZERO);
                        self.menu.open_at(
                            position,
                            vec![
                                MenuItem::new("Reconnect"),
                                MenuItem::new("Session settings"),
                                MenuItem::disabled("Paste"),
                                MenuItem::separator(),
                                MenuItem::danger("Close session"),
                            ],
                        );
                    }
                    if Button::new("Info toast").show(ui, palette).clicked() {
                        self.toasts.push(ToastKind::Info, "Host saved.");
                    }
                    if Button::new("Error toast")
                        .danger()
                        .show(ui, palette)
                        .clicked()
                    {
                        self.toasts
                            .push(ToastKind::Error, "Connection refused (os error 111)");
                    }
                    if Button::new("Open modal")
                        .primary()
                        .show(ui, palette)
                        .clicked()
                    {
                        self.modal_open = true;
                    }
                });
            });

            section(ui, palette, "Splitter", |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui(egui::Vec2::new(self.split, 60.0), |ui| {
                        ui.label(
                            egui::RichText::new("sidebar")
                                .size(12.0)
                                .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
                        );
                    });
                    splitter(ui, palette, SplitAxis::Horizontal, &mut self.split, 120.0, 420.0);
                    ui.label(
                        egui::RichText::new("terminal")
                            .size(12.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text_muted)),
                    );
                });
            });
        });

        if let Some(index) = self.menu.show(ui.ctx(), palette) {
            self.toasts
                .push(ToastKind::Info, format!("menu item {index} activated"));
        }

        if self.modal_open {
            let response = modal(
                ui.ctx(),
                "gallery-modal",
                palette,
                "Delete session?",
                420.0,
                |ui| {
                    ui.label(
                        egui::RichText::new("This closes the session and drops its scrollback.")
                            .size(13.0)
                            .color(wassh_ui::terminal_view::to_color32(palette.text)),
                    );
                    wassh_ui::widgets::footer(ui, |ui| {
                        if Button::new("Cancel").show(ui, palette).clicked() {
                            self.modal_open = false;
                        }
                        if Button::new("Delete").danger().show(ui, palette).clicked() {
                            self.modal_open = false;
                        }
                    });
                },
            );
            if response.dismissed {
                self.modal_open = false;
            }
        }

        self.toasts.show(ui, palette);
    }
}
