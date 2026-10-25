//! Colour input: a swatch that opens a full picker, plus a hex field.
//!
//! The field only commits a complete `#rrggbb` (or `#rrggbbaa`) value; partial input stays
//! in the draft string so typing is never interrupted. The swatch behaves like the
//! original program's colour input: clicking it opens a picker with a saturation/value
//! square, a hue slider and numeric fields.

use egui::{Ui, Vec2};
use wassh_core::{Palette, Rgba};

use super::field::text_input;
use crate::terminal_view::to_color32;

/// Size of the swatch (and of the picker button).
pub const SWATCH: f32 = 22.0;

/// Convert a toolkit colour back into a palette colour.
pub fn rgba_from_color32(color: egui::Color32) -> Rgba {
    Rgba::rgb(u32::from(color.r()) << 16 | u32::from(color.g()) << 8 | u32::from(color.b()))
}

/// Swatch button that opens a colour picker popup. Returns true when the colour changed.
pub fn color_picker_button(ui: &mut Ui, value: &mut Rgba) -> bool {
    let mut changed = false;
    let mut srgba = to_color32(*value);
    ui.scope(|ui| {
        // Same footprint as the swatch the original `<input type="color">` used.
        ui.spacing_mut().interact_size = Vec2::new(SWATCH * 1.6, SWATCH);
        ui.spacing_mut().button_padding = Vec2::new(2.0, 2.0);
        let response = egui::color_picker::color_edit_button_srgba(
            ui,
            &mut srgba,
            egui::color_picker::Alpha::Opaque,
        );
        changed = response.changed();
    });
    if changed {
        *value = rgba_from_color32(srgba);
    }
    changed
}

/// Parse a draft into a colour, accepting `#rgb`, `#rrggbb` and `#rrggbbaa`.
pub fn parse_hex_draft(text: &str) -> Option<Rgba> {
    let trimmed = text.trim();
    let body = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // `Rgba::parse_hex` only understands 6 and 8 digit forms, so expand shorthand here.
    let expanded = if body.len() == 3 {
        body.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        body.to_owned()
    };
    Rgba::parse_hex(&format!("#{expanded}"))
}

/// Draw a swatch plus text field. Returns true when a complete colour was committed.
pub fn hex_color_input(
    ui: &mut Ui,
    palette: &Palette,
    draft: &mut String,
    value: &mut Rgba,
) -> bool {
    let mut committed = false;
    ui.horizontal(|ui| {
        if color_picker_button(ui, value) {
            *draft = value.to_hex();
            committed = true;
        }
        ui.add_space(4.0);
        if text_input(ui, palette, draft, "#rrggbb").changed()
            && let Some(parsed) = parse_hex_draft(draft)
        {
            *value = parsed;
            committed = true;
        }
    });
    committed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_and_short_values() {
        assert_eq!(parse_hex_draft("#3d8bfd"), Some(Rgba::rgb(0x3d8bfd)));
        assert_eq!(parse_hex_draft("3d8bfd"), Some(Rgba::rgb(0x3d8bfd)));
        assert_eq!(parse_hex_draft("#fff"), Some(Rgba::rgb(0xffffff)));
    }

    fn frame(ctx: &egui::Context, input: egui::RawInput, value: &mut Rgba) -> usize {
        let output = ctx.run_ui(input, |ui| {
            hex_color_input(ui, &Palette::dark(), &mut String::new(), value);
        });
        let shapes = output.shapes.len();
        output.drop_without_applying_deltas();
        shapes
    }

    #[test]
    fn clicking_the_swatch_opens_the_picker() {
        let ctx = egui::Context::default();
        let mut value = Rgba::rgb(0x3d8bfd);
        let screen = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(900.0, 600.0),
            )),
            ..Default::default()
        };
        let closed = frame(&ctx, screen(), &mut value);

        // Press and release on the swatch, which sits at the top left.
        let click = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(egui::pos2(16.0, 10.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(16.0, 10.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: egui::pos2(16.0, 10.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..screen()
        };
        let _ = frame(&ctx, click, &mut value);
        let open = frame(&ctx, screen(), &mut value);
        assert!(
            open > closed + 5,
            "the picker popup did not appear ({closed} -> {open} shapes)"
        );
    }

    #[test]
    fn colours_survive_a_round_trip_through_the_toolkit() {
        let colour = Rgba::rgb(0x3d8bfd);
        assert_eq!(rgba_from_color32(to_color32(colour)), colour);
    }

    #[test]
    fn rejects_partial_or_invalid_input() {
        assert_eq!(parse_hex_draft("#3d8b"), None);
        assert_eq!(parse_hex_draft(""), None);
        assert_eq!(parse_hex_draft("#gggggg"), None);
    }
}
