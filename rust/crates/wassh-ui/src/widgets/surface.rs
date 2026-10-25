//! Surfaces: cards, sections, banners, badges, empty states and progress bars.

use egui::{Align2, Color32, FontId, Pos2, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use wassh_core::{Palette, RADIUS};

use crate::terminal_view::to_color32;

/// Semantic colour role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    /// Resolve the semantic role to a concrete colour for this palette.
    pub fn color(self, palette: &Palette) -> Color32 {
        match self {
            Self::Neutral => to_color32(palette.text_muted),
            Self::Info => to_color32(palette.accent),
            Self::Success => to_color32(palette.ok),
            Self::Warning => to_color32(palette.warn),
            Self::Danger => to_color32(palette.danger),
        }
    }
}

/// Banner severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    Info,
    Warn,
}

impl BannerKind {
    fn tone(self) -> Tone {
        match self {
            Self::Info => Tone::Info,
            Self::Warn => Tone::Warning,
        }
    }
}

/// A bordered card with the standard radius and padding.
pub fn card(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui)) -> Response {
    let frame = egui::Frame::NONE
        .fill(to_color32(palette.bg_panel_2))
        .stroke(Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::same(12));
    frame.show(ui, add).response
}

/// Radius the grouped-facts cards use (`--monitor-card-radius`).
pub const INSET_RADIUS: f32 = 8.0;
/// Border strength of an inset card: `--border` at 70 %.
const INSET_BORDER: f32 = 0.7;

/// A card that sits *inside* a panel: the input background, a softened border
/// and the larger grouping radius. The reference app draws every grouped block
/// (gauges, cores, sparks, facts) this way, so the panel's own background stays
/// the lightest surface in the view.
pub fn inset_card(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui)) -> Response {
    inset_card_with(ui, palette, egui::Margin::same(12), add)
}

/// [`inset_card`] filling a column of exactly `width`.
///
/// `Frame` sizes itself to its content, so a card whose tallest row is narrow
/// would shrink below its neighbours. Pinning the *content* ui to the frame's
/// own inner width makes the card fill the column and keeps a row of cards
/// identical.
pub fn fixed_inset_card(
    ui: &mut Ui,
    palette: &Palette,
    width: f32,
    margin: egui::Margin,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let width = width.max(48.0);
    ui.allocate_ui_with_layout(
        egui::Vec2::new(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            inset_card_with(ui, palette, margin, |ui| {
                ui.set_width(ui.available_width());
                add(ui);
            })
        },
    )
    .inner
}

/// [`inset_card`] with an explicit inner margin.
pub fn inset_card_with(
    ui: &mut Ui,
    palette: &Palette,
    margin: egui::Margin,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let frame = egui::Frame::NONE
        .fill(to_color32(palette.bg_input))
        .stroke(Stroke::new(
            1.0,
            crate::widgets::meter::faded(palette.border, INSET_BORDER),
        ))
        .corner_radius(INSET_RADIUS)
        .inner_margin(margin);
    frame.show(ui, add).response
}

/// A list group heading: a collapse caret, the uppercase title, an optional
/// count on the right and a hairline underneath. Returns the click on the head.
pub fn list_header(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    count: Option<usize>,
    collapsed: bool,
) -> Response {
    let text = to_color32(palette.text_muted);
    let font = FontId::proportional(11.0);
    let height = 26.0;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let hovered = response.hovered();
    let colour = if hovered {
        to_color32(palette.text)
    } else {
        text
    };
    let left = rect.left() + 4.0;
    ui.painter().text(
        Pos2::new(left, rect.center().y),
        Align2::LEFT_CENTER,
        if collapsed { "\u{25b8}" } else { "\u{25be}" },
        FontId::proportional(10.0),
        colour,
    );
    ui.painter().text(
        Pos2::new(left + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        title.to_uppercase(),
        font.clone(),
        colour,
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 4.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            font,
            text,
        );
    }
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        (1.0, to_color32(palette.border)),
    );
    ui.add_space(6.0);
    response
}

/// A full-width clickable card: the standard card surface, highlighted on hover.
pub fn click_card(ui: &mut Ui, palette: &Palette, add: impl FnOnce(&mut Ui)) -> Response {
    let inner = egui::Frame::NONE
        .fill(to_color32(palette.bg_panel_2))
        .stroke(Stroke::new(1.0, to_color32(palette.border)))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, add);
    let rect = inner.response.rect;
    let response = ui.interact(rect, inner.response.id, Sense::click());
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            RADIUS,
            Stroke::new(1.0, to_color32(palette.accent)),
            StrokeKind::Inside,
        );
    }
    response
}

/// A label/value row with the value right-aligned, as used by the status grid.
pub fn stat_row(ui: &mut Ui, palette: &Palette, label: &str, value: &str) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.0),
        to_color32(palette.text_muted),
    );
    painter.text(
        Pos2::new(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        value,
        FontId::monospace(11.0),
        to_color32(palette.text),
    );
}

/// A titled section: uppercase heading over a hairline, then the body.
pub fn section(ui: &mut Ui, palette: &Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .size(14.0)
            .color(to_color32(palette.text_muted)),
    );
    ui.add_space(6.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter()
        .line_segment([rect.left_center(), rect.right_center()], (1.0, to_color32(palette.border)));
    ui.add_space(10.0);
    add(ui);
    ui.add_space(18.0);
}

/// Full-width inline banner with a coloured left edge.
pub fn banner(ui: &mut Ui, palette: &Palette, kind: BannerKind, text: &str) -> Response {
    banner_with(ui, palette, kind, |ui| {
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(to_color32(palette.text)),
        );
    })
}

/// Banner whose body is supplied by the caller (for action buttons).
pub fn banner_with(
    ui: &mut Ui,
    palette: &Palette,
    kind: BannerKind,
    add: impl FnOnce(&mut Ui),
) -> Response {
    let tone = kind.tone().color(palette);
    let background = tone.gamma_multiply(0.12);
    let frame = egui::Frame::NONE
        .fill(background)
        .inner_margin(egui::Margin::symmetric(12, 8));
    frame
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let height = ui.available_height().max(18.0);
                let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, height), egui::Sense::hover());
                ui.painter().rect_filled(bar, 0.0, tone);
                ui.add_space(6.0);
                add(ui);
            });
        })
        .response
}

/// Small rounded label with a tinted background.
pub fn badge(ui: &mut Ui, palette: &Palette, text: &str, tone: Tone) -> Response {
    let colour = tone.color(palette);
    let font = FontId::proportional(11.0);
    let width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width + 12.0, 18.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, colour.gamma_multiply(0.18));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        colour,
    );
    response
}

/// Centred placeholder used by empty panels.
pub fn empty_state(ui: &mut Ui, palette: &Palette, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .color(to_color32(palette.text)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(hint)
                .size(12.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(24.0);
    });
}

/// Horizontal progress track; `fraction` is clamped to `0.0..=1.0`.
pub fn progress(ui: &mut Ui, palette: &Palette, fraction: f32) {
    let fraction = fraction.clamp(0.0, 1.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 3.0, to_color32(palette.bg_input));
    if fraction > 0.0 {
        let filled = egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * fraction, rect.height()));
        ui.painter()
            .rect_filled(filled, 3.0, to_color32(palette.accent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::ThemeMode;

    #[test]
    fn tones_map_to_palette_colours() {
        let dark = Palette::for_mode(ThemeMode::Dark);
        assert_eq!(Tone::Danger.color(&dark), to_color32(dark.danger));
        assert_eq!(Tone::Success.color(&dark), to_color32(dark.ok));
        assert_eq!(BannerKind::Warn.tone(), Tone::Warning);
        assert_eq!(BannerKind::Info.tone(), Tone::Info);
    }
}
