//! A tag input: chips with an ✕, plus an inline field that turns text into chips.
//!
//! Port of `src/renderer/src/components/TagInput.tsx`: Enter or a comma commits
//! the typed word as a chip, Backspace on an empty field removes the last chip,
//! leaving the field commits whatever is half-typed, and a word that is already
//! there (in any case) is not added twice.

use egui::{Align2, CornerRadius, FontId, Response, Sense, Stroke, Ui, Vec2};
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Height the field reserves, so the row never jumps while typing.
pub const TAG_INPUT_MIN_HEIGHT: f32 = 36.0;
/// Space between chips and around them.
pub const TAG_GAP: f32 = 6.0;
/// Padding inside the field's border.
pub const TAG_PADDING: i8 = 8;
/// Radius of the field's border.
pub const TAG_INPUT_RADIUS: u8 = 4;
/// Radius of a chip; large enough to read as a pill.
pub const TAG_PILL_RADIUS: u8 = 10;
/// Width of the ✕ inside a chip.
pub const TAG_REMOVE_WIDTH: f32 = 12.0;
/// Height of the ✕ inside a chip.
pub const TAG_REMOVE_HEIGHT: f32 = 14.0;

/// What the widget did this frame.
#[derive(Clone, Debug)]
pub struct TagInputResponse {
    /// The field's response, for focus checks.
    pub response: Response,
    /// Whether the tag list changed.
    pub changed: bool,
}

impl TagInputResponse {
    /// Whether the tag list changed.
    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// Add `raw` as a tag unless it is blank or already there.
pub fn commit_tag(tags: &mut Vec<String>, raw: &str) -> bool {
    let tag = raw.trim();
    if tag.is_empty() {
        return false;
    }
    if tags.iter().any(|existing| existing.eq_ignore_ascii_case(tag)) {
        return false;
    }
    tags.push(tag.to_owned());
    true
}

/// Commit every finished part of `text` and return what is still being typed.
fn commit_parts(tags: &mut Vec<String>, text: &str) -> (bool, String) {
    let mut changed = false;
    let mut parts: Vec<&str> = text.split(',').collect();
    // The last piece is whatever the user is mid-way through.
    let tail = parts.pop().unwrap_or_default().trim_start().to_owned();
    for part in parts {
        changed |= commit_tag(tags, part);
    }
    (changed, tail)
}

/// Commit finished comma-separated parts, keeping the tail in the field.
pub fn commit_draft(tags: &mut Vec<String>, draft: &mut String) -> bool {
    let (changed, tail) = commit_parts(tags, draft);
    *draft = tail;
    changed
}

/// Commit everything in the field, including the half-typed tail.
pub fn commit_all(tags: &mut Vec<String>, draft: &mut String) -> bool {
    let (mut changed, tail) = commit_parts(tags, draft);
    changed |= commit_tag(tags, &tail);
    draft.clear();
    changed
}

/// Remove the last tag, like Backspace on an empty field.
pub fn remove_last(tags: &mut Vec<String>) -> bool {
    tags.pop().is_some()
}

/// Draw the tag input.
///
/// `draft` is the text in the field: the caller keeps it so half-typed text
/// survives frames and a closed dialog cannot leak it.
pub fn tag_input(
    ui: &mut Ui,
    palette: &Palette,
    id: impl std::hash::Hash + std::fmt::Debug,
    tags: &mut Vec<String>,
    draft: &mut String,
    placeholder: &str,
) -> TagInputResponse {
    let field_id = ui.make_persistent_id(id);
    let focused = ui.memory(|memory| memory.has_focus(field_id));
    let border = if focused {
        to_color32(palette.accent)
    } else {
        to_color32(palette.border)
    };
    let width = ui.available_width();
    let mut changed = false;
    let mut edit_response = None;

    let frame = egui::Frame::new()
        .fill(to_color32(palette.bg_app))
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(TAG_INPUT_RADIUS))
        .inner_margin(egui::Margin::symmetric(TAG_PADDING, 6));

    let outer = frame.show(ui, |ui| {
        // The frame sizes to its content, so pin the width to the column and the
        // height to the reference's `min-height: 36px`.
        ui.set_min_width((width - f32::from(TAG_PADDING) * 2.0 - 2.0).max(40.0));
        ui.set_min_height(TAG_INPUT_MIN_HEIGHT - 6.0 * 2.0 - 2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(TAG_GAP);
            let mut remove = None;
            for (index, tag) in tags.iter().enumerate() {
                let pill = egui::Frame::new()
                    .fill(to_color32(palette.bg_panel_2))
                    .stroke(Stroke::new(1.0, to_color32(palette.border)))
                    .corner_radius(CornerRadius::same(TAG_PILL_RADIUS))
                    .inner_margin(egui::Margin::symmetric(8, 2));
                pill.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(
                            egui::RichText::new(tag.as_str())
                                .size(12.0)
                                .color(to_color32(palette.text)),
                        );
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(TAG_REMOVE_WIDTH, TAG_REMOVE_HEIGHT),
                            Sense::click(),
                        );
                        let colour = if response.hovered() {
                            to_color32(palette.danger)
                        } else {
                            to_color32(palette.text_muted)
                        };
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "✕",
                            FontId::proportional(10.0),
                            colour,
                        );
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response.clicked() {
                            remove = Some(index);
                        }
                    });
                });
            }
            if let Some(index) = remove {
                tags.remove(index);
                changed = true;
            }

            // The field takes whatever room is left, never under 90 px.
            let room = ui.available_width().max(90.0);
            edit_response = Some(
                ui.add(
                    egui::TextEdit::singleline(draft)
                        .id(field_id)
                        .hint_text(placeholder)
                        .frame(egui::Frame::NONE)
                        .desired_width(room)
                        .margin(egui::Margin::symmetric(2, 2))
                        .text_color(to_color32(palette.text)),
                ),
            );
        });
    });

    // Clicking anywhere in the field focuses the text, like the reference. This
    // deliberately does not take the click, or the chips' ✕ could never see it.
    let clicked_inside = ui.input(|input| {
        input.pointer.any_click()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| outer.response.rect.contains(pos))
    });
    if clicked_inside {
        ui.memory_mut(|memory| memory.request_focus(field_id));
    }

    if let Some(response) = edit_response {
        if response.changed() && draft.contains(',') {
            changed |= commit_draft(tags, draft);
        }
        if response.has_focus() {
            let (enter, backspace) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Enter),
                    input.key_pressed(egui::Key::Backspace),
                )
            });
            if enter {
                changed |= commit_all(tags, draft);
                // Enter commits rather than closing the dialog.
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            } else if backspace && draft.is_empty() {
                changed |= remove_last(tags);
            }
        }
        if response.lost_focus() && !draft.trim().is_empty() {
            changed |= commit_all(tags, draft);
        }
        TagInputResponse { response, changed }
    } else {
        TagInputResponse {
            response: outer.response,
            changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|tag| (*tag).to_owned()).collect()
    }

    #[test]
    fn a_word_becomes_a_tag_once() {
        let mut list = Vec::new();
        assert!(commit_tag(&mut list, "  work "));
        assert_eq!(list, ["work"]);
        // Duplicates are refused, in any case.
        assert!(!commit_tag(&mut list, "WORK"));
        assert_eq!(list, ["work"]);
        // Blank input adds nothing.
        assert!(!commit_tag(&mut list, "   "));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn typing_a_comma_commits_everything_before_it() {
        let mut list = Vec::new();
        let mut draft = String::from("work,");
        assert!(commit_draft(&mut list, &mut draft));
        assert_eq!(list, ["work"]);
        assert_eq!(draft, "");

        // The half-typed word stays in the field.
        draft.push_str("prod, sta");
        assert!(commit_draft(&mut list, &mut draft));
        assert_eq!(list, ["work", "prod"]);
        assert_eq!(draft, "sta");
        // Editing the field with no comma changes nothing.
        assert!(!commit_draft(&mut list, &mut draft));
        assert_eq!(draft, "sta");
    }

    #[test]
    fn enter_commits_the_tail_too() {
        let mut list = tags(&["work"]);
        let mut draft = String::from("prod, stag");
        assert!(commit_all(&mut list, &mut draft));
        assert_eq!(list, ["work", "prod", "stag"]);
        assert_eq!(draft, "");
        // Nothing to add is not a change.
        assert!(!commit_all(&mut list, &mut draft));
    }

    #[test]
    fn backspace_removes_the_last_tag() {
        let mut list = tags(&["a", "b"]);
        assert!(remove_last(&mut list));
        assert_eq!(list, ["a"]);
        assert!(remove_last(&mut list));
        assert!(!remove_last(&mut list));
    }

    /// Draw one frame and return every painted text with its position.
    fn frame(
        ctx: &egui::Context,
        palette: &Palette,
        tags: &mut Vec<String>,
        draft: &mut String,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(420.0, 140.0),
            )),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            tag_input(ui, palette, "tags", tags, draft, "Add tag…");
        });
        let painted = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos)),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        painted
    }

    /// A click and release at `pos`.
    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
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
        ]
    }

    #[test]
    fn every_tag_is_painted_as_its_own_chip() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut tags = tags(&["work", "db"]);
        let mut draft = String::new();
        let mut painted = Vec::new();
        // The first frame is egui's sizing pass.
        for _ in 0..2 {
            painted = frame(&ctx, &palette, &mut tags, &mut draft, Vec::new());
        }
        let texts: Vec<&str> = painted.iter().map(|(text, _)| text.as_str()).collect();
        assert!(texts.contains(&"work"), "chip missing: {texts:?}");
        assert!(texts.contains(&"db"), "chip missing: {texts:?}");
        assert_eq!(
            texts.iter().filter(|text| **text == "✕").count(),
            2,
            "each chip needs its own remove button: {texts:?}"
        );
        // The old comma-joined text field is gone.
        assert!(!texts.contains(&"work, db"), "still a text field: {texts:?}");
        // The placeholder shows while the field is empty.
        assert!(texts.contains(&"Add tag…"), "placeholder missing: {texts:?}");
    }

    #[test]
    fn clicking_a_cross_removes_that_tag() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut tags = tags(&["work", "db"]);
        let mut draft = String::new();
        let mut painted = Vec::new();
        for _ in 0..2 {
            painted = frame(&ctx, &palette, &mut tags, &mut draft, Vec::new());
        }
        // The glyph is painted from its top-left; click its centre.
        let cross = painted
            .iter()
            .find(|(text, _)| text == "✕")
            .map(|(_, pos)| *pos)
            .expect("a remove button");
        let centre = cross + egui::Vec2::new(TAG_REMOVE_WIDTH / 2.0, TAG_REMOVE_HEIGHT / 2.0);
        frame(&ctx, &palette, &mut tags, &mut draft, press(centre));
        assert_eq!(tags, ["db"], "the first chip was not removed");
    }

    #[test]
    fn typing_and_pressing_enter_adds_a_chip() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut tags = tags(&["work"]);
        let mut draft = String::new();
        let mut painted = Vec::new();
        for _ in 0..2 {
            painted = frame(&ctx, &palette, &mut tags, &mut draft, Vec::new());
        }
        // Click past the chip to focus the field.
        let field = egui::Pos2::new(360.0, painted[0].1.y + 6.0);
        frame(&ctx, &palette, &mut tags, &mut draft, press(field));
        // Type, then Enter.
        frame(
            &ctx,
            &palette,
            &mut tags,
            &mut draft,
            vec![egui::Event::Text("prod".to_owned())],
        );
        assert_eq!(draft, "prod", "the text did not reach the field");
        frame(
            &ctx,
            &palette,
            &mut tags,
            &mut draft,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(tags, ["work", "prod"], "Enter did not commit the word");
        assert!(draft.is_empty(), "the field kept the committed text");
    }

    #[test]
    fn typing_a_comma_commits_without_pressing_enter() {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut tags = tags(&["work"]);
        let mut draft = String::new();
        let mut painted = Vec::new();
        for _ in 0..2 {
            painted = frame(&ctx, &palette, &mut tags, &mut draft, Vec::new());
        }
        let field = egui::Pos2::new(360.0, painted[0].1.y + 6.0);
        frame(&ctx, &palette, &mut tags, &mut draft, press(field));
        for chunk in ["pro", "d,"] {
            frame(
                &ctx,
                &palette,
                &mut tags,
                &mut draft,
                vec![egui::Event::Text(chunk.to_owned())],
            );
        }
        assert_eq!(tags, ["work", "prod"], "the comma did not commit");
        assert!(draft.is_empty(), "the field kept the committed text");
    }
}
