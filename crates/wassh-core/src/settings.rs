//! Application settings (`settings.json`).

use serde::{Deserialize, Serialize};

use crate::app::{
    DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE_PX, DEFAULT_SCROLLBACK_LINES, DEFAULT_TERM_TYPE,
    MAX_FONT_SIZE_PX, MAX_SCROLLBACK_LINES, MIN_FONT_SIZE_PX, MIN_SCROLLBACK_LINES,
};
use crate::connection::{BellMode, CursorStyle, Tunnel};
use crate::theme::{Rgba, ThemeMode};

/// Settings a plugin keeps per plugin id: the values Options and Host settings
/// edit for it. Each plugin declares its own field schema; this only stores what
/// the user changed.
pub type PluginSettings = std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

/// The values stored for one plugin.
pub fn plugin_setting_values<'a>(
    settings: &'a PluginSettings,
    plugin_id: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    settings.get(plugin_id)
}

/// Persisted window geometry.
///
/// `x`/`y` are the outer position in physical pixels, which are unambiguous across monitors
/// with different scaling. `width`/`height` are the inner size in points, so the window keeps
/// its apparent size wherever it is restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

/// A reusable tunnel definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelTemplate {
    pub id: String,
    pub name: String,
    pub tunnel: Tunnel,
}

/// Defaults applied to sessions that do not override a style value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStyleDefaults {
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub tab_color: Option<Rgba>,
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub term_background: Option<Rgba>,
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub term_foreground: Option<Rgba>,
    pub font_size: u16,
    pub font_family: String,
    pub bell: BellMode,
    pub cursor: CursorStyle,
    pub cursor_blink: bool,
    pub scrollback: u32,
}

impl Default for SessionStyleDefaults {
    fn default() -> Self {
        Self {
            tab_color: None,
            term_background: None,
            term_foreground: None,
            font_size: DEFAULT_FONT_SIZE_PX as u16,
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            bell: BellMode::InvertLine,
            cursor: CursorStyle::Block,
            cursor_blink: true,
            scrollback: DEFAULT_SCROLLBACK_LINES,
        }
    }
}

impl SessionStyleDefaults {
    pub fn normalized(mut self) -> Self {
        self.font_size = self
            .font_size
            .clamp(MIN_FONT_SIZE_PX as u16, MAX_FONT_SIZE_PX as u16);
        self.scrollback = self
            .scrollback
            .clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        if self.font_family.trim().is_empty() {
            self.font_family = DEFAULT_FONT_FAMILY.to_owned();
        }
        self
    }
}

/// Everything in `settings.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub term_type: String,
    pub reconnect_on_startup: bool,
    pub sidebar_collapsed: bool,
    pub enabled_plugins: Vec<String>,
    /// Per-plugin settings shown in Options.
    #[serde(default)]
    pub plugin_settings: PluginSettings,
    #[serde(default)]
    pub tunnel_templates: Vec<TunnelTemplate>,
    pub session_style_defaults: SessionStyleDefaults,
    pub window_bounds: Option<WindowBounds>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            term_type: DEFAULT_TERM_TYPE.to_owned(),
            reconnect_on_startup: true,
            sidebar_collapsed: false,
            enabled_plugins: Vec::new(),
            plugin_settings: PluginSettings::new(),
            tunnel_templates: Vec::new(),
            session_style_defaults: SessionStyleDefaults::default(),
            window_bounds: None,
        }
    }
}

impl AppSettings {
    pub fn normalized(mut self) -> Self {
        if self.term_type.trim().is_empty() {
            self.term_type = DEFAULT_TERM_TYPE.to_owned();
        }
        self.session_style_defaults = self.session_style_defaults.normalized();
        self.enabled_plugins.sort();
        self.enabled_plugins.dedup();
        // A plugin that is switched off keeps its saved values but they are no
        // longer reachable, so drop the empty maps either way.
        self.plugin_settings.retain(|_, values| !values.is_empty());
        self.tunnel_templates.retain(|template| !template.id.is_empty());
        self
    }

    pub fn plugin_enabled(&self, id: &str) -> bool {
        self.enabled_plugins.iter().any(|enabled| enabled == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleared_colours_in_the_defaults_parse_as_unset() {
        let json = r#"{"sessionStyleDefaults":{"tabColor":"","termBackground":"","termForeground":""}}"#;
        let settings: AppSettings = serde_json::from_str(json).expect("settings");
        assert_eq!(settings.session_style_defaults.tab_color, None);
        assert_eq!(settings.session_style_defaults.term_background, None);
        assert_eq!(settings.session_style_defaults.term_foreground, None);
    }

    #[test]
    fn the_default_bell_blinks_the_current_line() {
        assert_eq!(
            SessionStyleDefaults::default().bell,
            crate::connection::BellMode::InvertLine
        );
    }

    #[test]
    fn defaults_match_the_documented_values() {
        let settings = AppSettings::default();
        // New installs follow the desktop's light/dark preference.
        assert_eq!(settings.theme, ThemeMode::System);
        assert_eq!(settings.term_type, "xterm-256color");
        assert!(settings.reconnect_on_startup);
        assert_eq!(settings.session_style_defaults.font_size, 14);
        assert_eq!(settings.session_style_defaults.scrollback, 10_000);
        assert_eq!(settings.session_style_defaults.font_family, "JetBrains Mono");
        assert!(settings.session_style_defaults.cursor_blink);
    }

    #[test]
    fn normalization_clamps_and_deduplicates() {
        let settings = AppSettings {
            term_type: "   ".to_owned(),
            enabled_plugins: vec!["b".into(), "a".into(), "a".into()],
            session_style_defaults: SessionStyleDefaults {
                font_size: 1,
                scrollback: 1_000_000,
                font_family: " ".to_owned(),
                ..SessionStyleDefaults::default()
            },
            ..Default::default()
        }
        .normalized();
        assert_eq!(settings.term_type, "xterm-256color");
        assert_eq!(settings.enabled_plugins, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(settings.session_style_defaults.font_size, 8);
        assert_eq!(settings.session_style_defaults.scrollback, 100_000);
        assert_eq!(settings.session_style_defaults.font_family, "JetBrains Mono");
    }

    #[test]
    fn round_trips_through_json() {
        let settings = AppSettings {
            enabled_plugins: vec!["sftp".to_owned()],
            session_style_defaults: SessionStyleDefaults {
                tab_color: Some(Rgba::rgb(0x3d8bfd)),
                ..SessionStyleDefaults::default()
            },
            window_bounds: Some(WindowBounds {
                x: 10,
                y: 20,
                width: 1280,
                height: 800,
                maximized: true,
            }),
            ..AppSettings::default()
        };
        let json = serde_json::to_string_pretty(&settings).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
        assert!(json.contains("\"tabColor\": \"#3d8bfd\""));
        assert!(json.contains("\"reconnectOnStartup\""));
    }

    #[test]
    fn partial_documents_get_defaults() {
        let settings: AppSettings = serde_json::from_str("{\"theme\":\"light\"}").unwrap();
        assert_eq!(settings.theme, ThemeMode::Light);
        assert_eq!(settings.term_type, "xterm-256color");
        assert!(settings.reconnect_on_startup);
    }

    #[test]
    fn plugin_lookup_is_exact() {
        let settings = AppSettings {
            enabled_plugins: vec!["sftp".to_owned()],
            ..AppSettings::default()
        };
        assert!(settings.plugin_enabled("sftp"));
        assert!(!settings.plugin_enabled("SFTP"));
    }
}
