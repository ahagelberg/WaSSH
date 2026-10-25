//! Dropdown selection.

use std::fmt::Debug;
use std::hash::Hash;

use egui::Ui;

/// Show a combo box. Returns the newly chosen value, or `None` when unchanged.
pub fn select<T: Clone + PartialEq, Id: Hash + Debug>(
    ui: &mut Ui,
    id: Id,
    selected: &T,
    options: &[(T, &'static str)],
) -> Option<T> {
    let current_label = options
        .iter()
        .find(|(value, _)| value == selected)
        .map(|(_, label)| *label)
        .unwrap_or("");

    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current_label)
        .width(ui.available_width().min(280.0))
        .show_ui(ui, |ui| {
            for (value, label) in options {
                if ui.selectable_label(value == selected, *label).clicked() {
                    chosen = Some(value.clone());
                }
            }
        });
    chosen
}

/// Convenience for string options.
pub fn select_str(
    ui: &mut Ui,
    id: impl Hash + Debug,
    selected: &str,
    options: &[&'static str],
) -> Option<String> {
    let owned: Vec<(String, &'static str)> = options
        .iter()
        .map(|label| ((*label).to_owned(), *label))
        .collect();
    select(ui, id, &selected.to_owned(), &owned)
}
