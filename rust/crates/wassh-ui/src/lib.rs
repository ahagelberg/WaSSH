//! egui shell.
//!
//! Phase 1 builds the widget kit and screens here. The palette and measurements come from
//! `wassh-core`, so the widget kit never hard-codes a colour.

#![forbid(unsafe_code)]

pub mod dock;
pub mod input;
pub mod plugin_toolbar;
pub mod sidebar;
pub mod tab_bar;
pub mod terminal_view;
pub mod titlebar;
pub mod widgets;

pub use dock::{
    DockAction, DockContent, DockDrag, DockTarget, apply_drop, dock_layout, drop_target,
};
pub use input::{TerminalInput, collect as collect_terminal_input};
pub use plugin_toolbar::{ToolbarButton, plugin_toolbar};
pub use sidebar::{SidebarAction, SidebarState, sidebar};
pub use tab_bar::{TabAction, TabBarState, TabView, tab_bar};
pub use terminal_view::TerminalView;
pub use titlebar::{
    CaptionStyle, TitlebarAction, caption_style, resize_edges, show_header_bar, show_menu_bar,
};

use wassh_core::{Palette, ThemeMode};

/// UI state that outlives a single frame (theme, palette and frame timings).
#[derive(Clone, Debug)]
pub struct ShellStyle {
    pub palette: Palette,
    pub ui_scale: f32,
}

impl Default for ShellStyle {
    fn default() -> Self {
        Self {
            palette: Palette::for_mode(ThemeMode::Dark),
            ui_scale: 1.0,
        }
    }
}

impl ShellStyle {
    pub fn set_theme(&mut self, mode: ThemeMode) {
        self.palette = Palette::for_mode(mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_the_theme_updates_the_palette() {
        let mut style = ShellStyle::default();
        assert_eq!(style.palette.mode, ThemeMode::Dark);
        style.set_theme(ThemeMode::Light);
        assert_eq!(style.palette.mode, ThemeMode::Light);
        assert!(style.palette.terminal_contrast() > 7.0);
    }
}
