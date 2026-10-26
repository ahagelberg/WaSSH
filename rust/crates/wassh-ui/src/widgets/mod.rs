//! Shared widget primitives.
//!
//! Every widget takes the [`Palette`] explicitly so no colour is hard-coded, and all
//! measurements come from `wassh-core` design tokens. Widgets are immediate-mode: they
//! draw and return their [`egui::Response`] in one call.

pub mod button;
pub mod chart;
pub mod checkbox;
pub mod color;
pub mod field;
pub mod menu;
pub mod meter;
pub mod modal;
pub mod number;
pub mod select;
pub mod slider;
pub mod splitter;
pub mod surface;
pub mod table;
pub mod tag_input;
pub mod text;
pub mod toast;
pub mod tree;

pub use button::{Button, ButtonKind, button, chip, small_button, small_button_enabled};
pub use chart::line_chart;
pub use meter::{RING_STROKE, bar_meter, faded, ring_gauge, segmented_bar};
pub use checkbox::checkbox;
pub use color::{color_picker_button, hex_color_input, parse_hex_draft, rgba_from_color32};
pub use field::{field, password_input, search_input, text_area, text_input};
pub use menu::{MenuItem, MenuState, context_menu};
pub use modal::{ModalResponse, footer, modal};
pub use number::{clamp_value, number_input};
pub use select::{select, select_str};
pub use slider::{SLIDER_HEIGHT, SLIDER_WIDTH, slider_field, step_slider};
pub use splitter::{SplitAxis, drop_index, drop_indicator, splitter};
pub use surface::{
    BannerKind, INSET_RADIUS, Tone, badge, banner, banner_with, card, click_card, empty_state,
    fixed_inset_card, inset_card, inset_card_with, list_header, progress, section, stat_row,
};
pub use table::{TableColumn, column_widths, table_header, table_row};
pub use tag_input::{
    TagInputResponse, commit_all, commit_draft, commit_tag, remove_last, tag_input,
};
pub use text::truncating_galley;
pub use toast::{Toast, ToastKind, Toasts};
pub use tree::{TreeRow, TreeRowResponse, tree_row};
