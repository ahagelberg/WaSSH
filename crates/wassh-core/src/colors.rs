//! Accent colours shared by tabs, host rows and group cards.
//!
//! A per-session accent is a hex string; the tinted
//! backgrounds with CSS `color-mix(in srgb, var(--tab-accent-color) N%, base)`. These
//! helpers reproduce the same ratios so the native build looks identical.

use crate::theme::Rgba;

/// Accents offered in the group colour picker.
pub const GROUP_COLOR_PRESETS: [Rgba; 12] = [
    Rgba::rgb(0xe35d6a),
    Rgba::rgb(0xf0b429),
    Rgba::rgb(0x3dd68c),
    Rgba::rgb(0x14b8a6),
    Rgba::rgb(0x3d8bfd),
    Rgba::rgb(0x5a9dff),
    Rgba::rgb(0x8b5cf6),
    Rgba::rgb(0xd946ef),
    Rgba::rgb(0xec4899),
    Rgba::rgb(0xf97316),
    Rgba::rgb(0x84cc16),
    Rgba::rgb(0x94a3b8),
];

/// Shown in the native swatch while a group has no colour yet.
pub const GROUP_COLOR_FALLBACK: Rgba = Rgba::rgb(0x3d8bfd);

/// Accent share of an inactive tab or host row background.
pub const TINT_IDLE: f32 = 0.12;
/// Accent share of an active tab background.
pub const TINT_ACTIVE: f32 = 0.22;
/// Accent share of a hovered host row.
pub const TINT_HOVER: f32 = 0.16;
/// Accent share of a group card background.
pub const TINT_GROUP: f32 = 0.09;

/// Whether `color` is one of the presets.
pub fn is_preset(color: Rgba) -> bool {
    GROUP_COLOR_PRESETS.contains(&color)
}

/// Blend `accent` over `backdrop` at the given share, matching `color-mix`.
pub fn tint(accent: Rgba, backdrop: Rgba, ratio: f32) -> Rgba {
    accent.with_alpha(ratio.clamp(0.0, 1.0)).over(backdrop)
}

/// Background for a tab or host row carrying `accent`.
pub fn row_background(accent: Option<Rgba>, backdrop: Rgba, active: bool, hovered: bool) -> Rgba {
    let Some(accent) = accent else {
        return backdrop;
    };
    let ratio = if active {
        TINT_ACTIVE
    } else if hovered {
        TINT_HOVER
    } else {
        TINT_IDLE
    };
    tint(accent, backdrop, ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_opaque_and_unique() {
        assert_eq!(GROUP_COLOR_PRESETS.len(), 12);
        for (index, colour) in GROUP_COLOR_PRESETS.iter().enumerate() {
            assert!(colour.is_opaque(), "preset {index} is translucent");
            for other in &GROUP_COLOR_PRESETS[index + 1..] {
                assert_ne!(colour, other, "duplicate preset at {index}");
            }
        }
    }

    #[test]
    fn tint_moves_towards_the_accent() {
        let backdrop = Rgba::rgb(0x23262b);
        let accent = Rgba::rgb(0x3d8bfd);
        let idle = tint(accent, backdrop, TINT_IDLE);
        let active = tint(accent, backdrop, TINT_ACTIVE);
        assert!(idle.is_opaque() && active.is_opaque());
        // The active tint carries more of the accent's blue than the idle one.
        assert!(active.b > idle.b && idle.b > backdrop.b);
        assert!(active.r > idle.r, "red channel should also rise");
    }

    #[test]
    fn rows_without_an_accent_keep_the_backdrop() {
        let backdrop = Rgba::rgb(0x23262b);
        assert_eq!(row_background(None, backdrop, false, true), backdrop);
        assert_eq!(
            row_background(Some(GROUP_COLOR_FALLBACK), backdrop, false, false),
            tint(GROUP_COLOR_FALLBACK, backdrop, TINT_IDLE)
        );
    }

    #[test]
    fn preset_lookup_is_exact() {
        assert!(is_preset(GROUP_COLOR_FALLBACK));
        assert!(!is_preset(Rgba::rgb(0x123456)));
    }
}
