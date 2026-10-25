//! Numeric inputs with hard bounds.
//!
//! Font size, scrollback and baud rate all have enforced ranges; the
//! clamp helpers here are the single place those bounds are applied.

use egui::Ui;
use wassh_core::Palette;

use crate::terminal_view::to_color32;

/// Clamp `value` into `min..=max`, tolerating a reversed range.
pub fn clamp_value(value: i64, min: i64, max: i64) -> i64 {
    let (low, high) = if min <= max { (min, max) } else { (max, min) };
    value.clamp(low, high)
}

/// A drag-to-adjust integer field that cannot leave `min..=max`.
///
/// Returns true when the value changed.
pub fn number_input(ui: &mut Ui, palette: &Palette, value: &mut u32, min: u32, max: u32) -> bool {
    *value = clamp_value(i64::from(*value), i64::from(min), i64::from(max)) as u32;
    let mut changed = false;
    let response = ui.scope(|ui| {
        ui.visuals_mut().extreme_bg_color = to_color32(palette.bg_input);
        ui.visuals_mut().widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, to_color32(palette.border));
        ui.visuals_mut().widgets.hovered.bg_stroke =
            egui::Stroke::new(1.0, to_color32(palette.accent));
        ui.visuals_mut().override_text_color = Some(to_color32(palette.text));
        ui.add(
            egui::DragValue::new(value)
                .range(min..=max)
                .speed(1.0)
                .update_while_editing(false),
        )
    });
    if response.inner.changed() {
        changed = true;
    }
    // DragValue can transiently overshoot before clamping; enforce the bound.
    *value = clamp_value(i64::from(*value), i64::from(min), i64::from(max)) as u32;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_into_range() {
        assert_eq!(clamp_value(5, 8, 48), 8);
        assert_eq!(clamp_value(99, 8, 48), 48);
        assert_eq!(clamp_value(20, 8, 48), 20);
    }

    #[test]
    fn tolerates_a_reversed_range() {
        assert_eq!(clamp_value(5, 48, 8), 8);
        assert_eq!(clamp_value(99, 48, 8), 48);
    }

    #[test]
    fn matches_the_documented_bounds() {
        assert_eq!(clamp_value(2, 8, 48), 8, "font size floor");
        assert_eq!(clamp_value(500_000, 100, 100_000), 100_000, "scrollback ceiling");
        assert_eq!(clamp_value(100, 300, 3_000_000), 300, "baud floor");
    }
}
