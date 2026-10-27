//! Application-wide constants.
//!
//! Values come from the design tokens (`types.ts` and the CSS design
//! tokens in `src/renderer/src/styles/app.css`) and must stay in sync with the UI spec.

/// Product name shown in the titlebar, About dialog and menus.
pub const APP_NAME: &str = "WaSSH";

/// Semantic version of the application.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default window size in logical pixels.
pub const DEFAULT_WINDOW_WIDTH: u32 = 1280;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 800;

/// Smallest window the shell allows.
pub const WINDOW_MIN_WIDTH: u32 = 800;
pub const WINDOW_MIN_HEIGHT: u32 = 500;

/// A restored window must expose at least this many pixels on a monitor.
pub const WINDOW_VISIBLE_MARGIN_PX: i32 = 50;

/// Window bounds are persisted after this quiet period.
pub const WINDOW_BOUNDS_DEBOUNCE_MS: u64 = 500;

/// Chrome measurements (logical pixels).
///
/// The window frame, title and window buttons belong to the platform; only the menu strip is
/// ours to size.
pub const MENU_BAR_HEIGHT: f32 = 30.0;
/// Height of the client-side header bar that holds the menus and the window buttons.
pub const WINDOW_HEADER_HEIGHT: f32 = 36.0;
pub const TABBAR_HEIGHT: f32 = 36.0;
pub const PLUGIN_TOOLBAR_HEIGHT: f32 = 32.0;
pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const SIDEBAR_COLLAPSED_WIDTH: f32 = 40.0;
pub const RADIUS: f32 = 4.0;
pub const GAP: f32 = 8.0;
pub const PADDING: f32 = 12.0;
pub const SCROLLBAR_SIZE: f32 = 4.0;

/// Base UI font size (CSS `13px`).
pub const UI_FONT_SIZE: f32 = 13.0;

/// Default terminal font size and scrollback.
pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
pub const DEFAULT_FONT_SIZE_PX: f32 = 14.0;
pub const MIN_FONT_SIZE_PX: f32 = 8.0;
pub const MAX_FONT_SIZE_PX: f32 = 48.0;
pub const DEFAULT_SCROLLBACK_LINES: u32 = 10_000;
pub const MIN_SCROLLBACK_LINES: u32 = 100;
pub const MAX_SCROLLBACK_LINES: u32 = 100_000;

/// Default `TERM` value sent to remote hosts.
pub const DEFAULT_TERM_TYPE: &str = "xterm-256color";

/// Closed-session undo stack.
pub const CLOSED_SESSION_STACK_MAX: usize = 20;
pub const CLOSED_SESSION_RETENTION_MS: u64 = 60 * 60 * 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_limits_are_sane() {
        const { assert!(WINDOW_MIN_WIDTH < DEFAULT_WINDOW_WIDTH) };
        const { assert!(WINDOW_MIN_HEIGHT < DEFAULT_WINDOW_HEIGHT) };
        const { assert!(MIN_FONT_SIZE_PX < DEFAULT_FONT_SIZE_PX) };
        const { assert!(DEFAULT_FONT_SIZE_PX < MAX_FONT_SIZE_PX) };
    }
}
