//! Colour bridge so plugins can paint with the shared palette instead of hard-coded values.

use wassh_core::Rgba;

/// Convert a palette colour to the toolkit's colour type.
pub fn to_color32(color: Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Convert a palette colour with a replaced alpha.
pub fn with_alpha(color: Rgba, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_keep_their_channels() {
        let colour = to_color32(Rgba::rgb(0x123456));
        assert_eq!((colour.r(), colour.g(), colour.b()), (0x12, 0x34, 0x56));
        assert_eq!(with_alpha(Rgba::rgb(0x123456), 0x80).a(), 0x80);
    }
}
