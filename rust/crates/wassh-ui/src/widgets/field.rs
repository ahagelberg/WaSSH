//! Labelled fields and text inputs.

use egui::{Response, Ui};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Draw a label, a control and an optional hint.
pub fn field(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    hint: Option<&str>,
    add: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    ui.label(
        egui::RichText::new(label.to_uppercase())
            .size(11.0)
            .color(to_color32(palette.text_muted)),
    );
    ui.add_space(4.0);
    let response = add(ui);
    if let Some(hint) = hint {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(hint)
                .size(11.0)
                .color(to_color32(palette.text_muted)),
        );
    }
    response
}

/// Single-line text input styled with the palette.
pub fn text_input(ui: &mut Ui, palette: &Palette, text: &mut String, placeholder: &str) -> Response {
    styled(ui, palette, text, placeholder, false)
}

/// Password input (masked).
pub fn password_input(
    ui: &mut Ui,
    palette: &Palette,
    text: &mut String,
    placeholder: &str,
) -> Response {
    styled(ui, palette, text, placeholder, true)
}

/// Multi-line text input styled exactly like [`text_input`].
pub fn text_area(
    ui: &mut Ui,
    palette: &Palette,
    text: &mut String,
    rows: usize,
    placeholder: &str,
) -> Response {
    let edit = egui::TextEdit::multiline(text)
        .hint_text(placeholder)
        .background_color(to_color32(palette.bg_input))
        .text_color(to_color32(palette.text))
        .margin(egui::Margin::symmetric(8, 6))
        .desired_rows(rows)
        .desired_width(f32::INFINITY);
    ui.add(edit)
}

/// Search box with the standard placeholder.
pub fn search_input(ui: &mut Ui, palette: &Palette, text: &mut String) -> Response {
    styled(ui, palette, text, "Search…", false)
}

fn styled(
    ui: &mut Ui,
    palette: &Palette,
    text: &mut String,
    placeholder: &str,
    password: bool,
) -> Response {
    let edit = egui::TextEdit::singleline(text)
        .hint_text(placeholder)
        .background_color(to_color32(palette.bg_input))
        .text_color(to_color32(palette.text))
        .margin(egui::Margin::symmetric(8, 6))
        .desired_width(f32::INFINITY)
        .password(password);
    ui.add(edit)
}
