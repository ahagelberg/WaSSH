//! A small Markdown renderer for assistant messages.
//!
//! The reference renders answers with `react-markdown`, so headings, lists,
//! fenced code and inline emphasis have to look different from plain text. This
//! renderer covers exactly those constructs: everything it does not understand
//! is shown verbatim, which is the safest failure for a chat pane.

use egui::text::LayoutJob;
use egui::{FontId, TextFormat, Ui};
use wassh_core::Palette;

use crate::color::{to_color32, with_alpha};

/// One block of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// `# heading` — level 1-6.
    Heading(usize, String),
    /// A fenced block and its language tag.
    Code(String, Vec<String>),
    /// A bullet list item.
    Bullet(String),
    /// A numbered list item.
    Numbered(u32, String),
    /// A `>` quote.
    Quote(String),
    /// A paragraph line.
    Paragraph(String),
    /// `---`.
    Rule,
}

/// A run of inline text with one style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    /// Ordinary text.
    Normal,
    /// `**bold**`.
    Bold,
    /// `*italic*`.
    Italic,
    /// `` `code` ``.
    Code,
}

/// Split a line into styled runs.
pub fn inline(line: &str) -> Vec<(Inline, String)> {
    let mut runs: Vec<(Inline, String)> = Vec::new();
    let mut buffer = String::new();
    let mut rest = line;
    let flush = |runs: &mut Vec<(Inline, String)>, buffer: &mut String| {
        if !buffer.is_empty() {
            runs.push((Inline::Normal, std::mem::take(buffer)));
        }
    };
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("**")
            && let Some(end) = tail.find("**")
        {
            flush(&mut runs, &mut buffer);
            runs.push((Inline::Bold, tail[..end].to_owned()));
            rest = &tail[end + 2..];
            continue;
        }
        if let Some(tail) = rest.strip_prefix('`')
            && let Some(end) = tail.find('`')
        {
            flush(&mut runs, &mut buffer);
            runs.push((Inline::Code, tail[..end].to_owned()));
            rest = &tail[end + 1..];
            continue;
        }
        if let Some(tail) = rest.strip_prefix('*')
            && let Some(end) = tail.find('*')
            && end > 0
        {
            flush(&mut runs, &mut buffer);
            runs.push((Inline::Italic, tail[..end].to_owned()));
            rest = &tail[end + 1..];
            continue;
        }
        let character = rest.chars().next().unwrap_or(' ');
        buffer.push(character);
        rest = &rest[character.len_utf8()..];
    }
    flush(&mut runs, &mut buffer);
    if runs.is_empty() {
        runs.push((Inline::Normal, String::new()));
    }
    runs
}

/// Strip the inline markers, for places that cannot style text.
pub fn plain(line: &str) -> String {
    inline(line)
        .into_iter()
        .map(|(_, text)| text)
        .collect::<String>()
}

/// Split a document into blocks.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut code: Option<(String, Vec<String>)> = None;

    let flush_paragraph = |paragraph: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join(" ")));
            paragraph.clear();
        }
    };

    for line in text.lines() {
        let trimmed = line.trim_end();
        if let Some((language, lines)) = &mut code {
            if trimmed.trim_start().starts_with("```") {
                blocks.push(Block::Code(language.clone(), std::mem::take(lines)));
                code = None;
            } else {
                lines.push(trimmed.to_owned());
            }
            continue;
        }
        if trimmed.trim_start().starts_with("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            let language = trimmed.trim_start().trim_start_matches('`').trim().to_owned();
            code = Some((language, Vec::new()));
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        let content = trimmed.trim_start();
        if content.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            continue;
        }
        if let Some(level) = heading_level(content) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, plain(content[level + 1..].trim())));
            continue;
        }
        if content == "---" || content == "***" {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = content.strip_prefix("- ").or_else(|| content.strip_prefix("* ")) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(rest.trim().to_owned()));
            continue;
        }
        if let Some((number, rest)) = numbered(content) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Numbered(number, rest));
            continue;
        }
        if let Some(rest) = content.strip_prefix("> ") {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(rest.trim().to_owned()));
            continue;
        }
        // An indented continuation line still belongs to the paragraph above.
        let _ = indent;
        paragraph.push(content.to_owned());
    }
    if let Some((language, lines)) = code {
        // An unterminated fence is still code, never prose.
        blocks.push(Block::Code(language, lines));
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

/// `# ` … `###### ` → the level.
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|character| *character == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line.chars().nth(hashes) == Some(' '))
        .filter(|space| *space)
        .map(|_| hashes)
}

/// `12. text` → (12, "text").
fn numbered(line: &str) -> Option<(u32, String)> {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = line[digits.len()..].strip_prefix(". ")?;
    Some((digits.parse().ok()?, rest.trim().to_owned()))
}

/// Lay out one line with its inline styles.
pub fn inline_job(line: &str, palette: &Palette, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = to_color32(palette.text);
    for (kind, text) in inline(line) {
        let mut format = TextFormat {
            font_id: FontId::proportional(size),
            color: body,
            ..Default::default()
        };
        match kind {
            Inline::Normal => {}
            Inline::Bold => {
                format.font_id = FontId::proportional(size);
                format.color = to_color32(palette.text);
                format.background = with_alpha(palette.accent, 18);
            }
            Inline::Italic => {
                format.italics = true;
            }
            Inline::Code => {
                format.font_id = FontId::monospace(size - 0.5);
                format.color = to_color32(palette.accent);
                format.background = with_alpha(palette.text_muted, 24);
            }
        }
        job.append(&text, 0.0, format);
    }
    job
}

/// Draw a Markdown document.
pub fn render(ui: &mut Ui, palette: &Palette, text: &str) {
    let muted = to_color32(palette.text_muted);
    for block in blocks(text) {
        match block {
            Block::Heading(level, title) => {
                let size = match level {
                    1 => 15.0,
                    2 => 14.0,
                    3 => 13.0,
                    _ => 12.5,
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(size)
                        .strong()
                        .color(to_color32(palette.text)),
                );
            }
            Block::Paragraph(line) => {
                ui.add(egui::Label::new(inline_job(&line, palette, 12.5)).wrap());
            }
            Block::Bullet(item) => {
                ui.horizontal_top(|ui| {
                    ui.label(egui::RichText::new("•").size(12.5).color(muted));
                    ui.add(egui::Label::new(inline_job(&item, palette, 12.5)).wrap());
                });
            }
            Block::Numbered(number, item) => {
                ui.horizontal_top(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{number}."))
                            .size(12.0)
                            .color(muted),
                    );
                    ui.add(egui::Label::new(inline_job(&item, palette, 12.5)).wrap());
                });
            }
            Block::Quote(line) => {
                ui.horizontal_top(|ui| {
                    ui.label(egui::RichText::new("▏").size(12.5).color(muted));
                    ui.add(egui::Label::new(inline_job(&line, palette, 12.0)).wrap());
                });
            }
            Block::Code(language, lines) => {
                if !language.is_empty() {
                    ui.label(egui::RichText::new(language).size(10.0).color(muted));
                }
                egui::Frame::new()
                    .fill(with_alpha(palette.text_muted, 18))
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(lines.join("\n"))
                                .size(11.0)
                                .monospace()
                                .color(to_color32(palette.text)),
                        );
                    });
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_markers_become_runs() {
        let runs = inline("see **this** and `that` plus *other*");
        assert_eq!(
            runs,
            vec![
                (Inline::Normal, "see ".to_owned()),
                (Inline::Bold, "this".to_owned()),
                (Inline::Normal, " and ".to_owned()),
                (Inline::Code, "that".to_owned()),
                (Inline::Normal, " plus ".to_owned()),
                (Inline::Italic, "other".to_owned()),
            ]
        );
        // An unclosed marker stays literal.
        assert_eq!(inline("a ** b"), vec![(Inline::Normal, "a ** b".to_owned())]);
        assert_eq!(plain("**bold** text"), "bold text");
    }

    #[test]
    fn blocks_cover_the_constructs_the_reference_uses() {
        let text = "# Title\n\nsome **text** here\n\n- one\n- two\n\n1. first\n\n> quote\n\n```sh\nls -la\necho hi\n```\n\n---\n";
        let parsed = blocks(text);
        assert_eq!(
            parsed,
            vec![
                Block::Heading(1, "Title".to_owned()),
                Block::Paragraph("some **text** here".to_owned()),
                Block::Bullet("one".to_owned()),
                Block::Bullet("two".to_owned()),
                Block::Numbered(1, "first".to_owned()),
                Block::Quote("quote".to_owned()),
                Block::Code("sh".to_owned(), vec!["ls -la".to_owned(), "echo hi".to_owned()]),
                Block::Rule,
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_is_still_code() {
        let parsed = blocks("```\nstill code\n");
        assert_eq!(parsed, vec![Block::Code(String::new(), vec!["still code".to_owned()])]);
    }

    #[test]
    fn headings_need_a_space_and_six_levels_max() {
        assert_eq!(heading_level("### three"), Some(3));
        assert_eq!(heading_level("#nospace"), None);
        assert_eq!(heading_level("####### seven"), None);
        assert_eq!(numbered("12. item"), Some((12, "item".to_owned())));
        assert_eq!(numbered("12 item"), None);
        assert_eq!(numbered("item"), None);
    }

    #[test]
    fn a_plain_message_is_one_paragraph() {
        assert_eq!(blocks("just words"), vec![Block::Paragraph("just words".to_owned())]);
        assert!(blocks("").is_empty());
    }
}
