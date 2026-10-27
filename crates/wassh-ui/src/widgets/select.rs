//! Dropdown selection.

use std::fmt::Debug;
use std::hash::Hash;

use egui::Ui;

/// Widest a combo box gets when it fills the space it is given.
const MAX_WIDTH: f32 = 280.0;

/// Show a combo box as wide as the space allows. Returns the newly chosen value,
/// or `None` when unchanged.
pub fn select<T: Clone + PartialEq, Id: Hash + Debug>(
    ui: &mut Ui,
    id: Id,
    selected: &T,
    options: &[(T, &str)],
) -> Option<T> {
    select_width(ui, id, selected, options, ui.available_width().min(MAX_WIDTH))
}

/// Show a combo box `width` points wide.
pub fn select_width<T: Clone + PartialEq, Id: Hash + Debug>(
    ui: &mut Ui,
    id: Id,
    selected: &T,
    options: &[(T, &str)],
    width: f32,
) -> Option<T> {
    let current_label = options
        .iter()
        .find(|(value, _)| value == selected)
        .map(|(_, label)| *label)
        .unwrap_or("");

    let mut chosen = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current_label)
        .width(width)
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
    options: &[&str],
) -> Option<String> {
    let owned: Vec<(String, &str)> = options
        .iter()
        .map(|label| ((*label).to_owned(), *label))
        .collect();
    select(ui, id, &selected.to_owned(), &owned)
}
