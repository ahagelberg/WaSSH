//! Colour and palette model.
//!
//! The palette is the single source of truth for both themes. Keeping it in Rust lets us
//! type-check contrast and reuse the values for native drawing.
//!
//! The light terminal uses the light theme's text colour: the dark theme's `#e8eaed` is
//! unreadable on white (contrast 1.2:1).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Straight (non-premultiplied) sRGB colour.
///
/// Serialises as `#rrggbb` (or `#rrggbbaa` when translucent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex8())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse_hex(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid colour: {text}")))
    }
}

/// Deserialise an optional colour, treating an empty or malformed value as "no override".
///
/// A cleared colour is written as `""` by some tools; that must never make the whole
/// configuration unreadable.
pub fn deserialize_optional_color<'de, D>(deserializer: D) -> Result<Option<Rgba>, D::Error>
where
    D: Deserializer<'de>,
{
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .and_then(|text| Rgba::parse_hex(&text)))
}

impl Rgba {
    /// Opaque colour from `0xRRGGBB`.
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
            a: 0xff,
        }
    }

    /// Colour from `0xRRGGBBAA` (channels in that order).
    pub const fn hex8(hex: u32) -> Self {
        Self {
            r: ((hex >> 24) & 0xff) as u8,
            g: ((hex >> 16) & 0xff) as u8,
            b: ((hex >> 8) & 0xff) as u8,
            a: (hex & 0xff) as u8,
        }
    }

    /// Parse `#rrggbb` or `#rrggbbaa` (the `#` is optional). Returns `None` for
    /// anything else, including shorthand notation.
    pub fn parse_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        let value = u32::from_str_radix(digits, 16).ok()?;
        match digits.len() {
            6 => Some(Self::rgb(value)),
            8 => Some(Self::hex8(value)),
            _ => None,
        }
    }

    /// Lowercase `#rrggbb` representation, dropping alpha.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex8(self) -> String {
        if self.is_opaque() {
            self.to_hex()
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn is_opaque(self) -> bool {
        self.a == 0xff
    }

    /// Replace the alpha channel. `alpha` is in `0.0..=1.0`.
    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// Linear interpolation in sRGB coordinates, matching CSS `color-mix(in srgb, …)`.
    /// `t == 0.0` yields `self`, `t == 1.0` yields `other`.
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| {
            (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }

    /// Source-over compositing of `self` on an opaque backdrop.
    pub fn over(self, backdrop: Self) -> Self {
        if self.is_opaque() {
            return self;
        }
        let sa = self.a as f32 / 255.0;
        let blend = |fg: u8, bg: u8| (fg as f32 * sa + bg as f32 * (1.0 - sa)).round() as u8;
        Self {
            r: blend(self.r, backdrop.r),
            g: blend(self.g, backdrop.g),
            b: blend(self.b, backdrop.b),
            a: 0xff,
        }
    }

    /// WCAG 2.x relative luminance of the RGB channels (alpha ignored).
    pub fn relative_luminance(self) -> f32 {
        let channel = |value: u8| {
            let s = value as f32 / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// WCAG 2.x contrast ratio, always `>= 1.0`.
    pub fn contrast_ratio(self, other: Self) -> f32 {
        let a = self.relative_luminance();
        let b = other.relative_luminance();
        let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }
}

/// Which palette the user asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the desktop's light/dark preference. This is the default.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Label shown in the theme picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    /// Pick the concrete palette, following the desktop when the setting says so.
    pub const fn resolve(self, system_dark: bool) -> ThemeMode {
        match self {
            Self::System => {
                if system_dark {
                    Self::Dark
                } else {
                    Self::Light
                }
            }
            other => other,
        }
    }
}

/// All theme tokens used by the shell and the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub mode: ThemeMode,
    pub bg_app: Rgba,
    pub bg_panel: Rgba,
    pub bg_panel_2: Rgba,
    pub bg_input: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub danger: Rgba,
    pub ok: Rgba,
    pub warn: Rgba,
    pub bg_term: Rgba,
    pub term_fg: Rgba,
    pub term_selection: Rgba,
    pub tab_active_border: Rgba,
}

impl Palette {
    pub const fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            bg_app: Rgba::rgb(0x1b1d21),
            bg_panel: Rgba::rgb(0x23262b),
            bg_panel_2: Rgba::rgb(0x2a2e35),
            bg_input: Rgba::rgb(0x15171a),
            border: Rgba::rgb(0x3a3f48),
            text: Rgba::rgb(0xe8eaed),
            text_muted: Rgba::rgb(0x9aa0a6),
            accent: Rgba::rgb(0x3d8bfd),
            accent_hover: Rgba::rgb(0x5a9dff),
            danger: Rgba::rgb(0xe35d6a),
            ok: Rgba::rgb(0x3dd68c),
            warn: Rgba::rgb(0xf0b429),
            bg_term: Rgba::rgb(0x0c0d0f),
            term_fg: Rgba::rgb(0xe8eaed),
            term_selection: Rgba::hex8(0x3d8b_fd66),
            tab_active_border: Rgba::rgb(0xffffff),
        }
    }

    /// Light palette. `term_fg` uses the light theme's text colour, which stays readable on
    /// the white terminal background.
    pub const fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            bg_app: Rgba::rgb(0xf3f4f6),
            bg_panel: Rgba::rgb(0xffffff),
            bg_panel_2: Rgba::rgb(0xe8eaee),
            bg_input: Rgba::rgb(0xffffff),
            border: Rgba::rgb(0xc9cdd4),
            text: Rgba::rgb(0x1b1d21),
            text_muted: Rgba::rgb(0x5f6368),
            accent: Rgba::rgb(0x1a6fe8),
            accent_hover: Rgba::rgb(0x1558c0),
            danger: Rgba::rgb(0xd93025),
            ok: Rgba::rgb(0x1e8e3e),
            warn: Rgba::rgb(0xc47d00),
            bg_term: Rgba::rgb(0xffffff),
            term_fg: Rgba::rgb(0x1b1d21),
            term_selection: Rgba::hex8(0x1a6f_e84d),
            tab_active_border: Rgba::rgb(0x000000),
        }
    }

    pub const fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            // A palette is always concrete; `System` falls back to dark until the shell
            // resolves it against the desktop.
            ThemeMode::System | ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    /// Palette for a saved setting, resolving `System` against the desktop preference.
    pub const fn for_setting(mode: ThemeMode, system_dark: bool) -> Self {
        match mode.resolve(system_dark) {
            ThemeMode::Light => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Window background reported to the compositor before the first frame.
    pub const fn window_background(self) -> Rgba {
        self.bg_app
    }

    /// Contrast of terminal text against the terminal background.
    pub fn terminal_contrast(self) -> f32 {
        self.term_fg.contrast_ratio(self.bg_term)
    }

    /// Contrast of primary text against panels.
    pub fn text_contrast(self) -> f32 {
        self.text.contrast_ratio(self.bg_panel)
    }

    /// Contrast of secondary text against panels.
    pub fn muted_contrast(self) -> f32 {
        self.text_muted.contrast_ratio(self.bg_panel)
    }

    /// Contrast of the accent colour used as text against panels.
    pub fn accent_contrast(self) -> f32 {
        self.accent.contrast_ratio(self.bg_panel)
    }
}

/// Terminal selection background resolved against the terminal background.
pub fn resolved_selection(palette: Palette) -> Rgba {
    palette.term_selection.over(palette.bg_term)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn parses_six_and_eight_digit_hex() {
        assert_eq!(Rgba::parse_hex("#3d8bfd"), Some(Rgba::rgb(0x3d8bfd)));
        assert_eq!(Rgba::parse_hex("3d8bfd"), Some(Rgba::rgb(0x3d8bfd)));
        assert_eq!(Rgba::parse_hex("#3d8bfd66"), Some(Rgba::hex8(0x3d8b_fd66)));
        assert_eq!(Rgba::parse_hex("#fff"), None);
        assert_eq!(Rgba::parse_hex("nope"), None);
    }

    #[test]
    fn formats_lowercase_hex() {
        assert_eq!(Rgba::rgb(0x3d8bfd).to_hex(), "#3d8bfd");
        assert_eq!(Rgba::hex8(0x3d8b_fd66).to_hex(), "#3d8bfd");
    }

    #[test]
    fn mix_matches_css_srgb_interpolation() {
        let black = Rgba::rgb(0x000000);
        let white = Rgba::rgb(0xffffff);
        assert_eq!(black.mix(white, 0.5), Rgba::rgb(0x808080));
        assert_eq!(black.mix(white, 0.0), black);
        assert_eq!(black.mix(white, 1.0), white);
    }

    #[test]
    fn compositing_blends_towards_backdrop() {
        let half_accent = Rgba::rgb(0x3d8bfd).with_alpha(0.5);
        let over_black = half_accent.over(Rgba::rgb(0x000000));
        assert_eq!(over_black, Rgba::rgb(0x1f467f));
        assert!(over_black.is_opaque());
    }

    #[test]
    fn luminance_endpoints() {
        assert!(close(Rgba::rgb(0x000000).relative_luminance(), 0.0));
        assert!(close(Rgba::rgb(0xffffff).relative_luminance(), 1.0));
        assert!(close(Rgba::rgb(0x000000).contrast_ratio(Rgba::rgb(0xffffff)), 21.0));
    }

    #[test]
    fn dark_palette_meets_contrast_targets() {
        let p = Palette::dark();
        assert!(p.terminal_contrast() >= 7.0, "terminal {}", p.terminal_contrast());
        assert!(p.text_contrast() >= 7.0, "text {}", p.text_contrast());
        assert!(p.muted_contrast() >= 4.5, "muted {}", p.muted_contrast());
        assert!(p.accent_contrast() >= 4.5, "accent {}", p.accent_contrast());
    }

    #[test]
    fn light_palette_meets_contrast_targets() {
        let p = Palette::light();
        assert!(p.terminal_contrast() >= 7.0, "terminal {}", p.terminal_contrast());
        assert!(p.text_contrast() >= 7.0, "text {}", p.text_contrast());
        assert!(p.muted_contrast() >= 4.5, "muted {}", p.muted_contrast());
        assert!(p.accent_contrast() >= 4.5, "accent {}", p.accent_contrast());
    }

    #[test]
    fn cleared_colours_deserialise_to_none() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "deserialize_optional_color")]
            color: Option<Rgba>,
        }
        for json in [r#"{"color": ""}"#, r#"{"color": null}"#, r#"{"color": "nonsense"}"#] {
            let holder: Holder = serde_json::from_str(json).unwrap_or(Holder { color: None });
            assert_eq!(holder.color, None, "{json}");
        }
        let holder: Holder = serde_json::from_str(r##"{"color": "#3d8bfd"}"##).unwrap();
        assert_eq!(holder.color, Some(Rgba::rgb(0x3d8bfd)));
    }

    #[test]
    fn light_terminal_foreground_stays_readable() {
        // The dark foreground would be invisible on white, so the light palette must not
        // use it.
        let inherited = Rgba::rgb(0xe8eaed);
        let white = Palette::light().bg_term;
        assert!(inherited.contrast_ratio(white) < 1.5);
        assert!(Palette::light().terminal_contrast() > 15.0);
    }

    #[test]
    fn selection_resolves_over_terminal_background() {
        let selection = resolved_selection(Palette::dark());
        assert!(selection.is_opaque());
        assert_ne!(selection, Palette::dark().bg_term);
    }

    #[test]
    fn theme_mode_round_trips_as_lowercase() {
        assert_eq!(ThemeMode::Dark.as_str(), "dark");
        assert_eq!(ThemeMode::Light.as_str(), "light");
        assert_eq!(ThemeMode::System.as_str(), "system");
        assert_eq!(Palette::for_mode(ThemeMode::Light), Palette::light());
        assert_eq!(
            serde_json::from_str::<ThemeMode>("\"system\"").unwrap(),
            ThemeMode::System
        );
    }

    #[test]
    fn the_system_setting_follows_the_desktop() {
        assert_eq!(ThemeMode::System.resolve(true), ThemeMode::Dark);
        assert_eq!(ThemeMode::System.resolve(false), ThemeMode::Light);
        // An explicit choice ignores the desktop.
        assert_eq!(ThemeMode::Light.resolve(true), ThemeMode::Light);
        assert_eq!(ThemeMode::Dark.resolve(false), ThemeMode::Dark);

        assert_eq!(Palette::for_setting(ThemeMode::System, true), Palette::dark());
        assert_eq!(Palette::for_setting(ThemeMode::System, false), Palette::light());
        assert_eq!(
            Palette::for_setting(ThemeMode::Light, true).mode,
            ThemeMode::Light
        );
    }

    #[test]
    fn colours_serialize_as_hex() {
        let opaque = Rgba::rgb(0x3d8bfd);
        assert_eq!(serde_json::to_string(&opaque).unwrap(), "\"#3d8bfd\"");
        let translucent = opaque.with_alpha(0.5);
        assert_eq!(serde_json::to_string(&translucent).unwrap(), "\"#3d8bfd80\"");
        let back: Rgba = serde_json::from_str("\"#3d8bfd66\"").unwrap();
        assert_eq!((back.r, back.g, back.b, back.a), (0x3d, 0x8b, 0xfd, 0x66));
        assert!(serde_json::from_str::<Rgba>("\"nonsense\"").is_err());
    }
}
