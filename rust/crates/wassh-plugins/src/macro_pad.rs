//! The Macro Pad plugin: saved commands that are typed into the session with one click.

use egui::Ui;
use serde::{Deserialize, Serialize};
use wassh_ui::widgets::{
    Button, click_card, empty_state, field, footer, list_header, modal, text_area, text_input,
};

use crate::color::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::{PluginId, PluginManifest};

/// Key the macros are stored under in the plugin's data.
pub const MACROS_KEY: &str = "macros";
/// Key the ungrouped section's collapse state is stored under. It is also an
/// Options field, so toggling it here updates the dialog too.
pub const UNGROUPED_COLLAPSED_KEY: &str = "ungroupedCollapsed";

/// One saved macro.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    /// Stable id, used as the button's widget id and for editing.
    pub id: String,
    /// Group heading it appears under; empty means ungrouped.
    #[serde(default)]
    pub group: String,
    /// Button text.
    pub label: String,
    /// Text typed into the session, verbatim (end it with a newline to run it).
    pub text: String,
}

impl Macro {
    /// The bytes a click sends.
    pub fn payload(&self) -> Vec<u8> {
        payload(&self.text)
    }
}

/// The macros a fresh install starts with, matching the reference app.
pub fn default_macros() -> Vec<Macro> {
    [
        ("m1", "syslog", "tail -n 100 /var/log/syslog\n"),
        (
            "m2",
            "apt upgrade",
            "sudo apt update;sudo apt upgrade -y;sudo apt autoremove -y\n",
        ),
        ("m3", "create venv", "python3 -m venv .venv\n"),
        ("m4", "ps aux", "ps aux\n"),
    ]
    .into_iter()
    .map(|(id, label, text)| Macro {
        id: id.to_owned(),
        group: String::new(),
        label: label.to_owned(),
        text: text.to_owned(),
    })
    .collect()
}

/// Read the stored macros, falling back to the defaults when nothing is stored yet.
///
/// A stored empty array means the user deleted every macro, and stays empty.
pub fn parse_macros(stored: Option<&serde_json::Value>) -> Vec<Macro> {
    match stored.and_then(serde_json::Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(|item| serde_json::from_value::<Macro>(item.clone()).ok())
            .collect(),
        None => default_macros(),
    }
}

/// Serialise the macros for storage.
pub fn to_value(macros: &[Macro]) -> serde_json::Value {
    serde_json::to_value(macros).unwrap_or(serde_json::Value::Array(Vec::new()))
}

/// Group headings in display order: ungrouped first, then named groups as they appear.
pub fn group_order(macros: &[Macro]) -> Vec<String> {
    let mut groups: Vec<String> = Vec::new();
    let mut ungrouped = false;
    for item in macros {
        if item.group.is_empty() {
            ungrouped = true;
        } else if !groups.iter().any(|group| group == &item.group) {
            groups.push(item.group.clone());
        }
    }
    if ungrouped {
        groups.insert(0, String::new());
    }
    groups
}

/// Bytes typed into the session for `text`.
///
/// A macro is a command, so it is submitted with Enter like a click on a terminal
/// would: a text that does not end with a line break gets one. Without it a
/// multi-line macro runs only its first line, and a single-line macro does not
/// run at all until the user presses Enter.
pub fn payload(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    let terminated = text.ends_with('\n') || text.ends_with('\r');
    if !terminated {
        bytes.push(b'\n');
    }
    bytes
}

/// Add `item`, or replace the macro with the same id.
pub fn upsert(macros: &mut Vec<Macro>, item: Macro) {
    match macros.iter_mut().find(|existing| existing.id == item.id) {
        Some(slot) => *slot = item,
        None => macros.push(item),
    }
}

/// Drop the macro with `id`.
pub fn remove(macros: &mut Vec<Macro>, id: &str) {
    macros.retain(|item| item.id != id);
}

/// An id that is not in use yet.
pub fn next_id(macros: &[Macro]) -> String {
    let mut number = macros.len() + 1;
    loop {
        let candidate = format!("m{number}");
        if !macros.iter().any(|item| item.id == candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// The macro being edited, if the editor is open.
#[derive(Clone, Debug, Default)]
struct Draft {
    /// Id of the macro being edited; empty while adding.
    id: String,
    group: String,
    label: String,
    text: String,
}

impl Draft {
    fn of(item: &Macro) -> Self {
        Self {
            id: item.id.clone(),
            group: item.group.clone(),
            label: item.label.clone(),
            text: item.text.clone(),
        }
    }
}

/// The Macro Pad pane.
#[derive(Default)]
pub struct MacroPad {
    macros: Vec<Macro>,
    /// Named groups the user collapsed.
    collapsed: std::collections::BTreeSet<String>,
    /// Whether the ungrouped section is collapsed.
    ungrouped_collapsed: bool,
    loaded: bool,
    draft: Option<Draft>,
}

impl MacroPad {
    /// The macros on the pad.
    pub fn macros(&self) -> &[Macro] {
        &self.macros
    }

    /// Store the pad after a change.
    fn save(&self, context: &mut PluginContext<'_>) {
        context.set_value(MACROS_KEY, to_value(&self.macros));
    }
}

/// Group headings the user collapsed, stored in the plugin's own data.
pub const COLLAPSED_KEY: &str = "collapsedGroups";

impl MacroPad {
    /// Read the pad from the plugin data the first time it is drawn.
    fn load(&mut self, context: &PluginContext<'_>) {
        if self.loaded {
            return;
        }
        self.macros = parse_macros(context.value(MACROS_KEY));
        self.collapsed = context
            .value(COLLAPSED_KEY)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        self.ungrouped_collapsed = context
            .value(UNGROUPED_COLLAPSED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        self.loaded = true;
    }

    /// The groups to draw: named groups in order, then the ungrouped bucket.
    fn sections(&self) -> Vec<(String, bool)> {
        let mut sections: Vec<(String, bool)> = Vec::new();
        for group in group_order(&self.macros) {
            if group.is_empty() {
                continue;
            }
            let collapsed = self.collapsed.contains(&group);
            sections.push((group, collapsed));
        }
        // Ungrouped always comes last, even when it is empty.
        let collapsed = self.ungrouped_collapsed;
        sections.push((String::new(), collapsed));
        sections
    }
}

impl Plugin for MacroPad {
    fn manifest(&self) -> PluginManifest {
        PluginId::MacroPad.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        self.load(context);
        let palette = context.palette;
        let muted = to_color32(palette.text_muted);

        // The list fills the pane; the action row is pinned to its bottom edge.
        let full = ui.available_rect_before_wrap();
        let action_height = 34.0;
        let list_rect = egui::Rect::from_min_max(
            full.min,
            egui::Pos2::new(full.right(), (full.bottom() - action_height - 8.0).max(full.top())),
        );
        let action_rect = egui::Rect::from_min_max(
            egui::Pos2::new(full.left(), list_rect.bottom() + 8.0),
            full.max,
        );

        let mut run: Option<Macro> = None;
        let mut edit: Option<Macro> = None;
        let mut add_group = false;
        let mut toggle: Option<(String, bool)> = None;

        let mut list = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .id_salt("wassh-macro-list")
            .auto_shrink([false, false])
            .show(&mut list, |ui| {
                if self.macros.is_empty() {
                    empty_state(
                        ui,
                        &palette,
                        "No macros yet",
                        "Use “Add macro” to define one.",
                    );
                    return;
                }
                for (group, collapsed) in self.sections() {
                    let members: Vec<&Macro> = self
                        .macros
                        .iter()
                        .filter(|item| item.group == group)
                        .collect();
                    let title = if group.is_empty() { "Ungrouped" } else { &group };
                    if list_header(ui, &palette, title, Some(members.len()), collapsed).clicked() {
                        toggle = Some((group.clone(), !collapsed));
                    }
                    if collapsed {
                        continue;
                    }
                    let empty = members.is_empty();
                    for item in members {
                        let response = click_card(ui, &palette, |ui| {
                            ui.label(
                                egui::RichText::new(if item.label.trim().is_empty() {
                                    "Untitled"
                                } else {
                                    item.label.as_str()
                                })
                                .size(12.0)
                                .strong()
                                .color(to_color32(palette.accent)),
                            );
                            if !item.text.is_empty() {
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(preview(&item.text))
                                        .size(11.0)
                                        .color(muted),
                                );
                            }
                        })
                        .on_hover_text("Click to run · right-click to edit")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            run = Some(item.clone());
                        } else if response.secondary_clicked() {
                            edit = Some(item.clone());
                        }
                        ui.add_space(6.0);
                    }
                    if empty {
                        ui.label(
                            egui::RichText::new("No macros")
                                .size(11.0)
                                .color(muted),
                        );
                        ui.add_space(6.0);
                    }
                }
            });

        let mut actions = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(action_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if Button::new("＋ Add macro").primary().show(&mut actions, &palette).clicked() {
            self.draft = Some(Draft {
                id: next_id(&self.macros),
                ..Draft::default()
            });
        }
        actions.add_space(6.0);
        if Button::new("New group").show(&mut actions, &palette).clicked() {
            add_group = true;
        }
        // Claim the space even though both halves were drawn by child uis.
        ui.allocate_rect(full, egui::Sense::hover());

        if let Some((group, collapsed)) = toggle {
            if group.is_empty() {
                self.ungrouped_collapsed = collapsed;
                context.set_value(
                    UNGROUPED_COLLAPSED_KEY,
                    serde_json::Value::Bool(self.ungrouped_collapsed),
                );
            } else if collapsed {
                self.collapsed.insert(group);
            } else {
                self.collapsed.remove(&group);
            }
            let names: Vec<serde_json::Value> = self
                .collapsed
                .iter()
                .map(|name| serde_json::Value::from(name.clone()))
                .collect();
            context.set_value(COLLAPSED_KEY, serde_json::Value::Array(names));
        }
        if add_group {
            let mut name = String::from("New group");
            let mut number = 2;
            while self.macros.iter().any(|item| item.group == name) {
                name = format!("New group {number}");
                number += 1;
            }
            self.draft = Some(Draft {
                id: next_id(&self.macros),
                group: name,
                ..Draft::default()
            });
        }
        if let Some(item) = run {
            context.pending_input.extend_from_slice(&item.payload());
        }
        if let Some(item) = edit {
            self.draft = Some(Draft::of(&item));
        }

        self.editor(ui, context);
    }
}

impl MacroPad {
    /// The add/edit dialog.
    fn editor(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let editing = !draft.id.is_empty() && self.macros.iter().any(|item| item.id == draft.id);
        let mut save = false;
        let mut cancel = false;
        let mut delete = false;
        let response = modal(
            ui.ctx(),
            "macro-editor",
            &palette,
            if editing { "Edit macro" } else { "Add macro" },
            420.0,
            |ui| {
                field(ui, &palette, "Label", None, |ui| {
                    text_input(ui, &palette, &mut draft.label, "deploy")
                });
                ui.add_space(8.0);
                field(ui, &palette, "Group", Some("Leave empty to use Ungrouped"), |ui| {
                    text_input(ui, &palette, &mut draft.group, "optional")
                });
                ui.add_space(8.0);
                field(
                    ui,
                    &palette,
                    "Text to send",
                    Some("Typed into the terminal; end it with a newline to run it."),
                    |ui| text_area(ui, &palette, &mut draft.text, 3, "command"),
                );
                if save && draft.label.trim().is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("A label is required")
                            .size(11.0)
                            .color(to_color32(palette.danger)),
                    );
                }
                footer(ui, |ui| {
                    if editing && Button::new("Delete").danger().show(ui, &palette).clicked() {
                        delete = true;
                    }
                    if Button::new("Cancel").show(ui, &palette).clicked() {
                        cancel = true;
                    }
                    if Button::new("Save").primary().show(ui, &palette).clicked() {
                        save = true;
                    }
                });
            },
        );

        if response.dismissed || cancel {
            self.draft = None;
        } else if save && !draft.label.trim().is_empty() {
            upsert(
                &mut self.macros,
                Macro {
                    id: draft.id.clone(),
                    group: draft.group.trim().to_owned(),
                    label: draft.label.trim().to_owned(),
                    text: draft.text.clone(),
                },
            );
            self.draft = None;
            self.save(context);
        } else if delete {
            remove(&mut self.macros, &draft.id);
            self.draft = None;
            self.save(context);
        }
    }
}

/// Single-line preview of a (possibly multi-line) macro text.
fn preview(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{21b5} ")
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    use super::*;
    use wassh_core::SessionStatus;

    fn macro_with(id: &str, group: &str, label: &str, text: &str) -> Macro {
        Macro {
            id: id.to_owned(),
            group: group.to_owned(),
            label: label.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_fresh_pad_starts_with_the_reference_macros() {
        let macros = parse_macros(None);
        let labels: Vec<_> = macros.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["syslog", "apt upgrade", "create venv", "ps aux"]);
        assert!(macros.iter().all(|item| item.group.is_empty()));
    }

    #[test]
    fn deleting_every_macro_stays_empty() {
        let stored = serde_json::json!([]);
        assert!(parse_macros(Some(&stored)).is_empty());
        // The value survives a round trip.
        assert_eq!(to_value(&[]), serde_json::json!([]));
    }

    #[test]
    fn macros_round_trip_through_storage() {
        let macros = vec![
            macro_with("m1", "Deploy", "ship", "git push\n"),
            macro_with("m2", "", "logs", "journalctl -f\n"),
        ];
        let value = to_value(&macros);
        assert_eq!(parse_macros(Some(&value)), macros);
        // Nonsense entries are skipped rather than failing the whole pad.
        let mixed = serde_json::json!([{ "id": "m1", "label": "ok", "text": "x" }, 42]);
        assert_eq!(parse_macros(Some(&mixed)).len(), 1);
    }

    #[test]
    fn ungrouped_macros_come_first() {
        let macros = vec![
            macro_with("m1", "Deploy", "ship", ""),
            macro_with("m2", "", "logs", ""),
            macro_with("m3", "Deploy", "rollback", ""),
            macro_with("m4", "Database", "psql", ""),
        ];
        assert_eq!(group_order(&macros), vec!["", "Deploy", "Database"]);
    }

    #[test]
    fn clicking_sends_the_text_verbatim() {
        let item = macro_with("m1", "", "ps", "ps aux\n");
        assert_eq!(item.payload(), b"ps aux\n".to_vec());
    }

    #[test]
    fn editing_replaces_and_deleting_removes() {
        let mut macros = vec![macro_with("m1", "", "ps", "ps aux\n")];
        upsert(&mut macros, macro_with("m1", "Procs", "ps", "ps -ef\n"));
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].group, "Procs");
        assert_eq!(macros[0].text, "ps -ef\n");

        upsert(&mut macros, macro_with("m2", "", "top", "top\n"));
        assert_eq!(macros.len(), 2);
        remove(&mut macros, "m1");
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].id, "m2");
    }

    #[test]
    fn new_ids_do_not_collide() {
        let macros = vec![macro_with("m1", "", "a", ""), macro_with("m2", "", "b", "")];
        assert_eq!(next_id(&macros), "m3");
        let sparse = vec![macro_with("m1", "", "a", ""), macro_with("m5", "", "b", "")];
        assert_eq!(next_id(&sparse), "m3");
    }

    /// Draw the pane headlessly and return the painted shape count.
    fn draw(plugin: &mut MacroPad, settings: &mut serde_json::Map<String, serde_json::Value>) -> usize {
        let ctx = egui::Context::default();
        let palette = wassh_core::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(320.0, 400.0),
            )),
            ..Default::default()
        };
        let mut painted = 0;
        for _ in 0..2 {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status: SessionStatus::Connected,
                settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            painted = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        painted
    }

    /// Draw the pane headlessly and return the painted text.
    fn draw_text(
        plugin: &mut MacroPad,
        settings: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        let palette = wassh_core::Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(360.0, 500.0),
            )),
            ..Default::default()
        };
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status: SessionStatus::Connected,
                settings,
                scope: "host-1".to_owned(),
                session: None,
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            texts.clear();
            for clipped in std::mem::take(&mut output.shapes) {
                if let egui::Shape::Text(text) = clipped.shape {
                    texts.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        texts
    }

    #[test]
    fn a_macro_is_submitted_with_enter() {
        // One line with several commands keeps them on one line, so the shell
        // runs all of them.
        assert_eq!(
            payload("sudo apt update;sudo apt upgrade -y"),
            b"sudo apt update;sudo apt upgrade -y\n"
        );
        // An already terminated macro is sent verbatim.
        assert_eq!(payload("ls\n"), b"ls\n");
        assert_eq!(payload("ls\r"), b"ls\r");
        // Multi-line macros run every line: the last one gets its Enter too,
        // which is what made them stop after the first command.
        assert_eq!(
            payload("echo one\necho two"),
            b"echo one\necho two\n"
        );
        assert_eq!(payload(""), b"\n");
    }

    #[test]
    fn a_click_sends_the_whole_macro() {
        let item = macro_with("m1", "", "upgrade", "sudo apt update;sudo apt upgrade -y");
        assert_eq!(
            item.payload(),
            b"sudo apt update;sudo apt upgrade -y\n"
        );
    }

    #[test]
    fn the_pane_looks_like_the_reference_pad() {
        let mut plugin = MacroPad::default();
        let mut settings = serde_json::Map::new();
        let texts = draw_text(&mut plugin, &mut settings);
        let has = |needle: &str| texts.iter().any(|text| text == needle);
        // A group header with its count, then one card per macro (label + preview).
        assert!(has("UNGROUPED"), "headers are uppercase: {texts:?}");
        assert!(has("4"), "the header counts its macros");
        assert!(has("syslog"));
        assert!(has("tail -n 100 /var/log/syslog"), "the text preview is shown");
        assert!(has("ps aux"));
        // The shared footer buttons are at the bottom of the pane.
        assert!(has("＋ Add macro"));
        assert!(has("New group"));
        // The old chip layout and the count line are gone.
        assert!(!texts.iter().any(|text| text.contains("Click to run")));
    }

    #[test]
    fn collapsing_a_section_is_remembered() {
        let mut plugin = MacroPad::default();
        let mut settings = serde_json::Map::new();
        // A stored collapse state hides the ungrouped macros.
        settings.insert(
            UNGROUPED_COLLAPSED_KEY.to_owned(),
            serde_json::Value::Bool(true),
        );
        let texts = draw_text(&mut plugin, &mut settings);
        assert!(texts.iter().any(|text| text == "UNGROUPED"));
        assert!(!texts.iter().any(|text| text == "syslog"));
        // Section order puts named groups before the ungrouped bucket.
        assert_eq!(plugin.sections().last().map(|(name, _)| name.as_str()), Some(""));
    }

    #[test]
    fn the_pad_paints_its_buttons() {
        let mut plugin = MacroPad::default();
        let mut settings = serde_json::Map::new();
        let painted = draw(&mut plugin, &mut settings);
        assert!(painted >= 8, "the pane painted {painted} shapes");
        assert_eq!(plugin.macros().len(), 4, "the defaults are loaded");
        // Loading alone does not rewrite storage.
        assert!(!settings.contains_key(MACROS_KEY));
    }

    #[test]
    fn stored_macros_replace_the_defaults() {
        let mut plugin = MacroPad::default();
        let mut settings = serde_json::Map::new();
        settings.insert(
            MACROS_KEY.to_owned(),
            to_value(&[macro_with("m1", "Ops", "status", "systemctl status\n")]),
        );
        draw(&mut plugin, &mut settings);
        assert_eq!(plugin.macros().len(), 1);
        assert_eq!(plugin.macros()[0].group, "Ops");
    }
}
