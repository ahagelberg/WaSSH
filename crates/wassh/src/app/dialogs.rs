//! The application dialogs: Options, Host/Session settings, About, the command palette
//! and the terminal find bar.
//!
//! Both settings dialogs share one layout: a 920×640 panel with a 180 px section
//! rail on the left and one scrolling column of sections on the right (`h3` heading,
//! hairline, then label/control rows). Each dialog owns a draft copy of the model it edits
//! and reports a [`DialogAction`] when the user commits.

use egui::{Align, Color32, Context, Id, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use wassh_core::{
    APP_NAME, APP_VERSION, AppSettings, AuthMethod, BellMode, BusyPolicy, ConnectionType,
    CursorStyle, HostProfile, MAX_FONT_SIZE_PX, MAX_SCROLLBACK_LINES, MIN_FONT_SIZE_PX,
    MIN_SCROLLBACK_LINES, Multiplexer, Palette, Parity, ReconnectMode, Rgba, SessionStyle,
    SessionStyleDefaults, StopBits, ThemeMode, Tunnel, TunnelKind, colors,
};
use wassh_plugins::settings::{self as plugin_settings, FieldKind, SettingsField};
use wassh_plugins::PluginId;
use wassh_ui::widgets::{
    BannerKind, Button, Tone, badge, banner, card, checkbox, footer, hex_color_input, modal,
    number_input, password_input, select, select_str, small_button, small_button_enabled, tag_input,
    text_input,
};

use wassh_ui::terminal_view::to_color32;

// --- dialog metrics (from settings.css) -----------------------------------

/// Width of the settings panel.
const DIALOG_MAX_WIDTH: f32 = 920.0;
/// Height of the settings panel.
const DIALOG_MAX_HEIGHT: f32 = 640.0;
/// Width of the section rail.
const NAV_WIDTH: f32 = 180.0;
/// Height of one rail button.
const NAV_ITEM_HEIGHT: f32 = 38.0;
/// Header height (12 px padding, 16 px title, 12 px padding).
const HEADER_HEIGHT: f32 = 49.0;
/// Footer height (12 px padding + 32 px buttons).
const FOOTER_HEIGHT: f32 = 56.0;
/// Padding around the scrolling content.
const CONTENT_PAD: f32 = 20.0;
/// Gap between a row's label column and its control.
const ROW_GAP: f32 = 16.0;
/// Vertical offset that lines a 13 px label up with a taller control.
const LABEL_OFFSET: f32 = 5.0;
/// Preferred width of a row's control column.
const CONTROL_MAX: f32 = 280.0;
/// Smallest width a control column may shrink to.
const CONTROL_MIN: f32 = 160.0;
/// Vertical padding inside a row.
const ROW_PAD: f32 = 10.0;
/// Space below a section.
const SECTION_GAP: f32 = 28.0;
/// Backdrop alpha, matching `rgba(0,0,0,.55)`.
const BACKDROP_ALPHA: u8 = 140;
/// Width of a plugin's dedicated settings editor.
const EDITOR_WIDTH: f32 = 620.0;
/// Shortest a plugin's dedicated settings editor gets, whatever the window.
const EDITOR_MIN_HEIGHT: f32 = 240.0;
/// Share of the window height the editor may take: a dialog, not a second window.
const EDITOR_HEIGHT_SHARE: f32 = 0.5;
/// Height the modal header, footer and padding take from the window.
const EDITOR_CHROME: f32 = 132.0;
/// Width of a tunnel's port box, which sits next to its host field.
const TUNNEL_PORT_WIDTH: f32 = 64.0;
/// Smallest width a tunnel's host field keeps when the port box takes its share.
const TUNNEL_HOST_MIN_WIDTH: f32 = 80.0;

/// A group being edited in its own dialog.
#[derive(Clone, Debug)]
pub struct GroupEditState {
    /// Group being edited.
    pub group_id: String,
    /// Name as it is stored, restored when the field is emptied.
    pub original_name: String,
    /// Draft name.
    pub name: String,
    /// Colour being edited; `None` means "no colour".
    pub color: Option<Rgba>,
    /// Hex text backing the picker.
    pub hex: String,
}

impl GroupEditState {
    /// Start editing a group.
    pub fn new(group: &wassh_core::HostGroup) -> Self {
        Self {
            group_id: group.id.clone(),
            original_name: group.name.clone(),
            name: group.name.clone(),
            color: group.color,
            hex: group.color.map(|color| color.to_hex()).unwrap_or_default(),
        }
    }

    /// The name to commit: an emptied field keeps the stored name.
    pub fn committed_name(&self) -> &str {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            self.original_name.as_str()
        } else {
            trimmed
        }
    }

    /// The colour the dialog would commit, shown as the current swatch.
    pub fn shown_color(&self) -> Rgba {
        self.color
            .unwrap_or(wassh_core::colors::GROUP_COLOR_FALLBACK)
    }
}

/// What a dialog asks the shell to do once the user commits.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogAction {
    /// Persist these settings.
    SaveSettings(AppSettings, PluginSecretDrafts),
    /// Persist this host (new or edited), with the credentials typed into the dialog.
    SaveHost(Box<HostSave>),
    /// Apply these settings to one open session only.
    ///
    /// The saved host and the host's other sessions keep their own settings.
    SaveSession {
        /// Id of the session the dialog was opened from.
        session: String,
        /// Settings and credentials to apply.
        save: Box<HostSave>,
    },
    /// Rename a group and set (or clear) its colour.
    SaveGroup {
        group_id: String,
        name: String,
        color: Option<Rgba>,
    },
    /// Persist the host and push it to the sessions already open for it.
    ApplyHost(Box<HostSave>),
    /// Delete the host being edited.
    DeleteHost(String),
    /// Write or restore the configuration archive at `path`.
    Backup {
        /// True to restore, false to write.
        import: bool,
        /// Archive to read or write.
        path: String,
    },
    /// Run a command palette entry.
    Command(Command),
    /// Move within the find results.
    Find(FindCommand),
    /// Close every open dialog.
    Close,
}

/// What to do with one stored secret when a host is saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SecretEdit {
    /// Leave whatever is stored alone.
    #[default]
    Unchanged,
    /// Store this value.
    Set(String),
    /// Delete the stored value.
    Clear,
}

impl SecretEdit {
    /// Interpret the dialog field: empty text keeps the stored secret untouched.
    fn from_field(value: &str, clear: bool) -> Self {
        if clear {
            Self::Clear
        } else if value.is_empty() {
            Self::Unchanged
        } else {
            Self::Set(value.to_owned())
        }
    }
}

/// Plugin secret drafts and their keys live in `wassh_plugins`, so a plugin's own
/// settings editor can edit its secrets without going through the app.
pub use wassh_plugins::settings::{PluginSecretDrafts, SecretDraft, secret_field_key};

/// Whether saving this draft would change the vault.
pub fn draft_changes(draft: &SecretDraft) -> bool {
    draft.clear || !draft.draft.is_empty()
}

/// The value a plugin secret draft stands for: `None` keeps what is stored.
pub fn plugin_secret_value(draft: &SecretDraft) -> Option<Option<&str>> {
    if draft.clear {
        Some(None)
    } else if draft.draft.is_empty() {
        None
    } else {
        Some(Some(draft.draft.as_str()))
    }
}

/// Which plugin a settings section belongs to, and where its secrets go.
pub struct SecretEditing<'a> {
    /// The plugin the schema came from.
    pub id: PluginId,
    /// Secret drafts shared by the whole dialog.
    pub secrets: &'a mut PluginSecretDrafts,
}

/// A host save request from the settings dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct HostSave {
    /// The host to persist.
    pub host: HostProfile,
    /// New password, or nothing to keep the stored one.
    pub password: SecretEdit,
    /// New key passphrase.
    pub passphrase: SecretEdit,
    /// Plugin secrets to seal for this host, keyed `"<plugin>|<field>"`.
    pub plugin_secrets: PluginSecretDrafts,
}

/// Entries offered by the command palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    NewLocalTab,
    NewHost,
    EditHost,
    SaveAsHost,
    OpenOptions,
    OpenTunnels,
    ToggleSidebar,
    Reconnect,
    ReconnectAll,
    CloseTab,
    ReopenTab,
    NextTab,
    PreviousTab,
    Find,
    About,
    Quit,
}

impl Command {
    /// Every command with its palette label.
    pub const ALL: [(Command, &'static str); 16] = [
        (Command::NewLocalTab, "New local terminal tab"),
        (Command::NewHost, "New host…"),
        (Command::EditHost, "Edit current session settings…"),
        (Command::SaveAsHost, "Save session as host…"),
        (Command::OpenOptions, "Options…"),
        (Command::OpenTunnels, "Tunnels…"),
        (Command::ToggleSidebar, "Toggle sidebar"),
        (Command::Reconnect, "Reconnect current session"),
        (Command::ReconnectAll, "Reconnect all sessions"),
        (Command::CloseTab, "Close current tab"),
        (Command::ReopenTab, "Reopen closed tab"),
        (Command::NextTab, "Next tab"),
        (Command::PreviousTab, "Previous tab"),
        (Command::Find, "Find in terminal"),
        (Command::About, "About WaSSH"),
        (Command::Quit, "Quit"),
    ];

    /// Palette label.
    #[cfg(test)]
    pub fn label(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(command, _)| *command == self)
            .map(|(_, label)| *label)
            .unwrap_or("")
    }
}

/// Find bar navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindCommand {
    Next,
    Previous,
    Close,
}

/// Which section the rail shows as active, and a pending scroll request.
#[derive(Clone, Debug, Default)]
pub struct SettingsNav {
    /// Index of the highlighted section.
    pub active: usize,
    /// Section the user just clicked; cleared once scrolled to.
    pub scroll_to: Option<usize>,
}

impl SettingsNav {
    /// Ask the dialog to jump to `index`.
    pub fn jump_to(&mut self, index: usize) {
        self.active = index;
        self.scroll_to = Some(index);
    }
}

/// Options dialog state.
pub struct OptionsState {
    /// Section rail state.
    pub nav: SettingsNav,
    /// Draft copy of `settings.json`.
    pub draft: AppSettings,
    /// Hex text backing the default tab colour.
    pub tab_color_draft: String,
    /// Hex text backing the default terminal background.
    pub term_background_draft: String,
    /// Hex text backing the default terminal text colour.
    pub term_foreground_draft: String,
    /// Archive path used by the Backup section.
    pub backup_path: String,
    /// Plugin whose dedicated settings editor is open, if any.
    pub editor: Option<PluginId>,
    /// Secrets of app-wide plugin settings (`secret: true` fields).
    pub plugin_secrets: PluginSecretDrafts,
    /// State of the AI agent's provider editor (a model fetch in flight).
    pub ai_providers: wassh_plugins::ai_agent::ProviderEditor,
}

impl OptionsState {
    /// Open the dialog on a copy of the current settings.
    pub fn new(settings: &AppSettings) -> Self {
        Self {
            nav: SettingsNav::default(),
            draft: settings.clone(),
            tab_color_draft: String::new(),
            term_background_draft: String::new(),
            term_foreground_draft: String::new(),
            backup_path: String::new(),
            editor: None,
            plugin_secrets: PluginSecretDrafts::new(),
            ai_providers: wassh_plugins::ai_agent::ProviderEditor::default(),
        }
    }

    /// Select the section that belongs to `id`, so a gear next to a plugin opens
    /// Options straight on its settings. Plugins without a section land on the
    /// "Plugins" list of switches instead.
    pub fn focusing_plugin(mut self, id: PluginId) -> Self {
        let index = app_plugin_sections(&self.draft)
            .iter()
            .position(|section| section.id == id)
            .map(|index| OPTION_SECTIONS.len() + index)
            .unwrap_or(OPTION_SECTIONS.len() - 1);
        self.nav.jump_to(index);
        self
    }

    /// Fill the backup path with a timestamped default inside `dir`.
    pub fn with_backup_default(mut self, dir: &std::path::Path, now_ms: u64) -> Self {
        self.backup_path = wassh_store::default_archive_path(dir, now_ms)
            .to_string_lossy()
            .into_owned();
        self
    }
}

/// Whether a settings dialog edits the stored host or one live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostTarget {
    /// The host in `hosts.json`: Save persists it; "Apply to sessions" also pushes the
    /// change to the sessions already open for it.
    Host,
    /// One open session, by id: Save changes that tab and nothing else.
    Session(String),
}

/// Host/Session settings state.
pub struct HostState {
    /// Section rail state.
    pub nav: SettingsNav,
    /// What Save writes: the stored host or the session the dialog was opened from.
    pub target: HostTarget,
    /// Draft copy of the host.
    pub draft: HostProfile,
    /// Whether the host is in the saved configuration.
    ///
    /// Only a saved host can be applied to open sessions or deleted; a session
    /// that was never saved has nothing to update or remove. The shell sets this
    /// from the configuration, so a stale `is_new` flag cannot leak the buttons
    /// into a session dialog.
    pub saved: bool,
    /// Hex text backing the tab-colour field.
    pub tab_color_draft: String,
    /// Hex text backing the terminal-background field.
    pub term_background_draft: String,
    /// Hex text backing the terminal-text field.
    pub term_foreground_draft: String,
    /// App-wide defaults the appearance rows fall back to.
    pub defaults: SessionStyleDefaults,
    /// Whether sessions of this host are open, which enables "Apply to sessions".
    pub has_sessions: bool,
    /// Text typed into the tag field but not committed yet.
    pub tag_draft: String,
    /// Secrets of this host's plugin settings (`secret: true` fields).
    pub plugin_secrets: PluginSecretDrafts,
    /// Password typed into the dialog; empty keeps whatever is stored.
    pub password: String,
    /// Key passphrase typed into the dialog.
    pub passphrase: String,
    /// Whether a password is already stored for this host.
    pub stored_password: bool,
    /// Whether a passphrase is already stored for this host.
    pub stored_passphrase: bool,
    /// The user pressed "Clear" on the stored password.
    pub clear_password: bool,
    /// The user pressed "Clear" on the stored passphrase.
    pub clear_passphrase: bool,
}

impl HostState {
    /// Start from an arbitrary draft (used for ad-hoc sessions turned into hosts).
    pub fn draft(host: HostProfile, is_new: bool) -> Self {
        Self {
            nav: SettingsNav::default(),
            target: HostTarget::Host,
            tab_color_draft: host
                .connection
                .style
                .tab_color
                .map(|color| color.to_hex())
                .unwrap_or_default(),
            term_background_draft: host
                .connection
                .style
                .term_background
                .map(|color| color.to_hex())
                .unwrap_or_default(),
            term_foreground_draft: host
                .connection
                .style
                .term_foreground
                .map(|color| color.to_hex())
                .unwrap_or_default(),
            defaults: SessionStyleDefaults::default(),
            has_sessions: false,
            saved: !is_new,
            tag_draft: String::new(),
            plugin_secrets: PluginSecretDrafts::new(),
            password: String::new(),
            passphrase: String::new(),
            stored_password: false,
            stored_passphrase: false,
            clear_password: false,
            clear_passphrase: false,
            draft: host,
        }
    }

    /// Use the app-wide defaults the appearance rows resolve to.
    pub fn with_defaults(mut self, defaults: &SessionStyleDefaults) -> Self {
        self.defaults = defaults.clone();
        let style = &self.draft.connection.style;
        if style.tab_color.is_none() {
            self.tab_color_draft = resolved_colour(defaults.tab_color, None).to_hex();
        }
        if style.term_background.is_none() {
            self.term_background_draft =
                resolved_colour(defaults.term_background, None).to_hex();
        }
        if style.term_foreground.is_none() {
            self.term_foreground_draft =
                resolved_colour(defaults.term_foreground, None).to_hex();
        }
        self
    }

    /// Mark whether sessions of this host are currently open.
    pub fn with_open_sessions(mut self, open: bool) -> Self {
        self.has_sessions = open;
        self
    }

    /// Point the dialog at the session it was opened from, so Save stays local to it.
    pub fn with_target(mut self, target: HostTarget) -> Self {
        self.target = target;
        self
    }

    /// Note which plugin secrets the vault already holds for this host.
    ///
    /// Pass `(plugin, field)` pairs; the dialog shows "(stored)" instead of a
    /// value, because the secret never travels through the draft.
    pub fn with_plugin_secrets(
        mut self,
        stored: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        for (plugin, field) in stored {
            self.plugin_secrets
                .insert(secret_field_key(&plugin, &field), SecretDraft::stored());
        }
        self
    }


    /// Mark whether this host is in the saved configuration.
    pub fn with_saved(mut self, saved: bool) -> Self {
        self.saved = saved;
        self
    }

    /// Whether "Apply to sessions" applies to this dialog.
    ///
    /// Only a saved host can push its settings to the sessions open for it; a session
    /// dialog saves to that one tab.
    pub fn can_apply_to_sessions(&self) -> bool {
        self.target == HostTarget::Host && self.saved && self.has_sessions
    }

    /// Whether the dialog deletes a host rather than discarding a draft.
    pub fn can_delete(&self) -> bool {
        self.target == HostTarget::Host && self.saved
    }

    /// Record which secrets are already stored for this host.
    pub fn with_stored_secrets(mut self, password: bool, passphrase: bool) -> Self {
        self.stored_password = password;
        self.stored_passphrase = passphrase;
        self
    }
}

/// Command palette state.
pub struct PaletteState {
    /// Current filter text.
    pub query: String,
    /// Highlighted entry.
    pub selected: usize,
}

impl PaletteState {
    /// Empty palette focused on the first entry.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
        }
    }

    /// Commands matching the current query.
    pub fn matches(&self) -> Vec<(Command, &'static str)> {
        let query = self.query.trim().to_lowercase();
        Command::ALL
            .iter()
            .copied()
            .filter(|(_, label)| query.is_empty() || label.to_lowercase().contains(&query))
            .collect()
    }
}

impl Default for PaletteState {
    fn default() -> Self {
        Self::new()
    }
}

/// Find bar state.
pub struct FindState {
    /// Current query.
    pub query: String,
    /// Number of hits in the active terminal.
    pub matches: usize,
    /// Index of the highlighted hit.
    pub index: usize,
}

impl FindState {
    /// An empty find bar.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: 0,
            index: 0,
        }
    }
}

impl Default for FindState {
    fn default() -> Self {
        Self::new()
    }
}

/// Every dialog that can be open at once.
#[derive(Default)]
pub struct Dialogs {
    /// Options dialog.
    pub options: Option<OptionsState>,
    /// Host/Session settings dialog.
    pub host: Option<HostState>,
    /// About window.
    pub about: bool,
    /// Command palette.
    pub palette: Option<PaletteState>,
    /// Terminal find bar.
    pub find: Option<FindState>,
    /// Edit-group dialog.
    pub group_edit: Option<GroupEditState>,
    /// Plugin ids enabled in the app settings, so Host settings can list the
    /// per-host fields of every enabled plugin.
    pub enabled_plugins: Vec<String>,
}

impl Dialogs {
    /// Whether any modal is open (the terminal must not receive keys).
    pub fn modal_open(&self) -> bool {
        self.options.is_some()
            || self.host.is_some()
            || self.about
            || self.group_edit.is_some()
            || self.palette.is_some()
    }

    /// Draw every open dialog, returning the actions they produced.
    pub fn show(
        &mut self,
        ctx: &Context,
        palette: &Palette,
        hosts: &[HostProfile],
        find_matches: usize,
    ) -> Vec<DialogAction> {
        let mut actions = Vec::new();
        if let Some(state) = self.options.as_mut() {
            let before = actions.len();
            actions.extend(options_dialog(ctx, palette, state));
            // A "Configure…" row opens the plugin's editor on top of Options,
            // unless the dialog itself was just committed or closed.
            let committed = actions[before..].iter().any(|action| {
                matches!(
                    action,
                    DialogAction::Close | DialogAction::SaveSettings(_, _)
                )
            });
            if !committed {
                options_plugin_editor(ctx, palette, state);
                options_provider_dialog(ctx, palette, state);
            }
        }
        if let Some(state) = self.host.as_mut() {
            actions.extend(host_dialog(
                ctx,
                palette,
                state,
                hosts,
                &self.enabled_plugins,
            ));
        }
        if self.about {
            actions.extend(about_dialog(ctx, palette));
        }
        if let Some(state) = self.group_edit.as_mut() {
            actions.extend(group_edit_dialog(ctx, palette, state));
        }
        if let Some(state) = self.palette.as_mut() {
            actions.extend(palette_dialog(ctx, palette, state));
        }
        if let Some(state) = self.find.as_mut()
            && let Some(command) = find_bar(ctx, palette, state, find_matches)
        {
            actions.push(DialogAction::Find(command));
        }
        actions
    }
}

// --- the settings shell ---------------------------------------------------

/// Buttons the settings shell draws in its footer, right to left.
#[derive(Clone, Copy, Debug)]
struct ShellButtons<'a> {
    /// The commit button (right-most).
    primary: &'a str,
    /// The cancel button.
    secondary: &'a str,
    /// Optional extra action between the cancel and commit buttons.
    apply: Option<&'a str>,
    /// Optional destructive button (left-most).
    danger: Option<&'a str>,
}

/// Static description of a settings dialog.
#[derive(Clone, Copy, Debug)]
struct ShellSpec<'a> {
    /// Id salt, unique per dialog.
    id: &'a str,
    /// Window title.
    title: &'a str,
    /// Section titles, in order.
    sections: &'a [&'static str],
    /// Footer buttons.
    buttons: ShellButtons<'a>,
}

/// What the user chose in a settings dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellOutcome {
    /// Nothing chosen yet: the dialog stays open.
    Pending,
    /// Escape, the backdrop or the close button: drop the draft.
    Dismissed,
    Save,
    /// Save the host and push it to the sessions already open for it.
    Apply,
    Cancel,
    Delete,
}

/// Draw the two-column settings panel used by Options and Host settings.
fn settings_shell(
    ctx: &Context,
    palette: &Palette,
    nav: &mut SettingsNav,
    spec: ShellSpec<'_>,
    mut content: impl FnMut(&mut Ui, usize),
) -> ShellOutcome {
    let ShellSpec {
        id: id_salt,
        title,
        sections,
        buttons,
    } = spec;
    let screen = ctx.content_rect();
    let border = to_color32(palette.border);
    let text = to_color32(palette.text);
    let muted = to_color32(palette.text_muted);
    let mut dismissed = ctx.input(|input| input.key_pressed(egui::Key::Escape));
    // Stays `Pending` until a footer button is pressed, so a plain frame never closes the
    // dialog.
    let mut outcome = ShellOutcome::Pending;

    // Backdrop.
    egui::Area::new(Id::unique(("wassh-settings-backdrop", id_salt)))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(rect, 0.0, Color32::from_black_alpha(BACKDROP_ALPHA));
            if response.clicked() {
                dismissed = true;
            }
        });

    let size = Vec2::new(
        DIALOG_MAX_WIDTH.min(screen.width() - 48.0).max(480.0),
        DIALOG_MAX_HEIGHT.min(screen.height() - 48.0).max(320.0),
    );
    let panel = Rect::from_center_size(screen.center(), size);

    egui::Area::new(Id::unique(("wassh-settings", id_salt)))
        .order(egui::Order::Foreground)
        .fixed_pos(panel.min)
        .show(ctx, |ui| {
            ui.set_min_size(size);
            let rect = Rect::from_min_size(panel.min, size);
            // Swallow clicks so they do not reach the backdrop.
            ui.interact(rect, Id::unique(("wassh-settings-sink", id_salt)), Sense::click());

            ui.painter().rect_filled(rect, 8.0, to_color32(palette.bg_panel));
            ui.painter().rect_stroke(
                rect,
                8.0,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );

            // Header.
            let header = Rect::from_min_size(rect.min, Vec2::new(size.x, HEADER_HEIGHT));
            ui.painter().text(
                Pos2::new(header.left() + 16.0, header.center().y),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(16.0),
                text,
            );
            ui.painter().hline(
                header.x_range(),
                header.bottom(),
                Stroke::new(1.0, border),
            );
            let close_rect = Rect::from_center_size(
                Pos2::new(header.right() - 24.0, header.center().y),
                Vec2::splat(24.0),
            );
            let close = ui.interact(
                close_rect,
                Id::unique(("wassh-settings-close", id_salt)),
                Sense::click(),
            );
            if close.hovered() {
                ui.painter()
                    .rect_filled(close_rect, 4.0, to_color32(palette.bg_panel_2));
            }
            ui.painter().text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{2715}",
                egui::FontId::proportional(13.0),
                if close.hovered() {
                    to_color32(palette.danger)
                } else {
                    muted
                },
            );
            if close.clicked() {
                dismissed = true;
            }

            let body_top = rect.top() + HEADER_HEIGHT;
            let body_bottom = rect.bottom() - FOOTER_HEIGHT;

            // Section rail.
            let nav_rect = Rect::from_min_max(
                Pos2::new(rect.left(), body_top),
                Pos2::new(rect.left() + NAV_WIDTH, body_bottom),
            );
            ui.painter()
                .rect_filled(nav_rect, 0.0, to_color32(palette.bg_panel_2));
            ui.painter().vline(
                nav_rect.right(),
                nav_rect.y_range(),
                Stroke::new(1.0, border),
            );
            let mut rail = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(nav_rect.shrink(8.0))
                    .layout(Layout::top_down(Align::Min)),
            );
            for (index, label) in sections.iter().enumerate() {
                let (button, response) = rail.allocate_exact_size(
                    Vec2::new(NAV_WIDTH - 16.0, NAV_ITEM_HEIGHT),
                    Sense::click(),
                );
                let active = index == nav.active;
                if active {
                    rail.painter()
                        .rect_filled(button, wassh_core::RADIUS, to_color32(palette.bg_app));
                } else if response.hovered() {
                    rail.painter()
                        .rect_filled(button, wassh_core::RADIUS, to_color32(palette.bg_panel));
                }
                rail.painter().text(
                    Pos2::new(button.left() + 12.0, button.center().y),
                    egui::Align2::LEFT_CENTER,
                    *label,
                    egui::FontId::proportional(12.0),
                    if active || response.hovered() { text } else { muted },
                );
                if response.clicked() {
                    nav.jump_to(index);
                }
                rail.add_space(2.0);
            }

            // Scrolling section column.
            let content_rect = Rect::from_min_max(
                Pos2::new(nav_rect.right(), body_top),
                Pos2::new(rect.right(), body_bottom),
            );
            let mut column = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(content_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            let mut tops = vec![f32::MAX; sections.len()];
            egui::ScrollArea::vertical()
                .id_salt(("wassh-settings-scroll", id_salt))
                .auto_shrink([false, false])
                .show(&mut column, |ui| {
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space(CONTENT_PAD);
                        ui.vertical(|ui| {
                            ui.set_width((content_rect.width() - CONTENT_PAD * 2.0).max(120.0));
                            for (index, heading) in sections.iter().enumerate() {
                                let section_top = ui.cursor().top();
                                tops[index] = section_top;
                                ui.label(
                                    egui::RichText::new(heading.to_uppercase())
                                        .size(14.0)
                                        .color(muted),
                                );
                                let rule = ui.cursor().top();
                                ui.painter().hline(
                                    ui.max_rect().x_range(),
                                    rule,
                                    Stroke::new(1.0, border),
                                );
                                ui.add_space(12.0);
                                content(ui, index);
                                ui.add_space(SECTION_GAP);
                                if nav.scroll_to == Some(index) {
                                    ui.scroll_to_rect(
                                        Rect::from_min_size(
                                            Pos2::new(ui.max_rect().left(), section_top),
                                            Vec2::new(ui.max_rect().width(), 1.0),
                                        ),
                                        Some(Align::Min),
                                    );
                                }
                            }
                        });
                    });
                    ui.add_space(16.0);
                });
            nav.scroll_to = None;

            // Scroll-spy: the active section is the last heading at or above the viewport
            // top (matching `SECTION_ACTIVE_OFFSET_PX = 16`).
            if sections.is_empty() {
                nav.active = 0;
            } else {
                let threshold = content_rect.top() + 16.0;
                let mut active = 0;
                for (index, top) in tops.iter().enumerate() {
                    if *top <= threshold {
                        active = index;
                    }
                }
                nav.active = active;
            }

            // Footer.
            let footer_rect =
                Rect::from_min_max(Pos2::new(rect.left(), body_bottom), rect.max);
            ui.painter()
                .hline(footer_rect.x_range(), footer_rect.top(), Stroke::new(1.0, border));
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(footer_rect.shrink2(Vec2::new(16.0, 12.0)))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            if Button::new(buttons.primary)
                .primary()
                .show(&mut bar, palette)
                .clicked()
            {
                outcome = ShellOutcome::Save;
            }
            if let Some(apply) = buttons.apply
                && Button::new(apply).show(&mut bar, palette).clicked()
            {
                outcome = ShellOutcome::Apply;
            }
            if Button::new(buttons.secondary).show(&mut bar, palette).clicked() {
                outcome = ShellOutcome::Cancel;
            }
            if let Some(danger) = buttons.danger
                && Button::new(danger).danger().show(&mut bar, palette).clicked()
            {
                outcome = ShellOutcome::Delete;
            }
        });

    if dismissed {
        ShellOutcome::Dismissed
    } else {
        outcome
    }
}

// --- row helpers ----------------------------------------------------------

/// A label/control row with a hairline underneath, matching `.settings-row`.
fn row(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    description: Option<&str>,
    add: impl FnOnce(&mut Ui),
) {
    let text = to_color32(palette.text);
    let muted = to_color32(palette.text_muted);
    ui.add_space(ROW_PAD);
    let width = ui.available_width();
    let control = (width * 0.42).clamp(CONTROL_MIN, CONTROL_MAX);
    let label_width = (width - control - ROW_GAP).max(80.0);
    ui.horizontal_top(|ui| {
        let start = ui.cursor().left();
        ui.allocate_ui_with_layout(
            Vec2::new(label_width, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                // A 13 px label next to a ~30 px control reads better nudged down a little.
                ui.add_space(LABEL_OFFSET);
                ui.label(egui::RichText::new(label).size(13.0).strong().color(text));
                if let Some(description) = description {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(description)
                            .size(12.0)
                            .color(muted),
                    );
                }
            },
        );
        // A child ui shrinks to the width it used; pad back out to the full label column so
        // every control starts at the same x whatever the label's length.
        let gap = ui.spacing().item_spacing.x;
        let used = (ui.cursor().left() - start).max(0.0);
        ui.add_space((label_width - used - gap).max(0.0) + ROW_GAP);
        ui.allocate_ui_with_layout(
            Vec2::new(control, 0.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(control);
                add(ui);
            },
        );
    });
    ui.add_space(ROW_PAD);
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        Stroke::new(1.0, to_color32(palette.border).gamma_multiply(0.5)),
    );
}

/// A row whose control sits under the label (wide editors such as the tunnel list).
fn row_block(ui: &mut Ui, palette: &Palette, label: &str, add: impl FnOnce(&mut Ui)) {
    let text = to_color32(palette.text);
    ui.add_space(ROW_PAD);
    ui.label(egui::RichText::new(label).size(13.0).strong().color(text));
    ui.add_space(6.0);
    add(ui);
    ui.add_space(ROW_PAD);
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        Stroke::new(1.0, to_color32(palette.border).gamma_multiply(0.5)),
    );
}

fn control_label(ui: &mut Ui, palette: &Palette, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(11.0)
            .color(to_color32(palette.text_muted)),
    );
}

/// A text field that leaves `reserved` pixels free for the widget after it.
///
/// [`text_input`] fills whatever room it is given, so a fixed-width sibling would be
/// pushed past the row's edge without this.
fn text_input_before(
    ui: &mut Ui,
    palette: &Palette,
    text: &mut String,
    placeholder: &str,
    reserved: f32,
) {
    let width = (ui.available_width() - reserved - ui.spacing().item_spacing.x)
        .max(TUNNEL_HOST_MIN_WIDTH);
    ui.scope(|ui| {
        ui.set_max_width(width);
        text_input(ui, palette, text, placeholder);
    });
}

/// A tunnel's port box, sized to [`TUNNEL_PORT_WIDTH`].
fn tunnel_port(ui: &mut Ui, palette: &Palette, port: &mut u16) {
    let mut value = u32::from(*port);
    ui.scope(|ui| {
        ui.set_width(TUNNEL_PORT_WIDTH);
        if number_input(ui, palette, &mut value, 0, u32::from(u16::MAX)) {
            *port = value as u16;
        }
    });
}

// --- Options --------------------------------------------------------------

const OPTION_SECTIONS: [&str; 6] = [
    "Appearance",
    "General",
    "Terminal",
    "Backup",
    "Host defaults",
    "Plugins",
];

fn options_dialog(ctx: &Context, palette: &Palette, state: &mut OptionsState) -> Vec<DialogAction> {
    let mut actions = Vec::new();
    let OptionsState {
        nav,
        draft,
        tab_color_draft,
        term_background_draft,
        term_foreground_draft,
        backup_path,
        editor,
        plugin_secrets,
        ai_providers,
        ..
    } = state;
    // Set while the section closure runs; applied once the shell returns.
    let mut open_editor = None;
    // Dialogs the sections open on top of the settings: a plugin's own editor and the
    // AI agent's provider draft. Escape closes the top one, not Options.
    let sub_dialog = editor.is_some() || ai_providers.dialog_open();
    // Plugins add one section each after the built-in ones, in plugin order, the
    // same way the original app appends them to its section list.
    let plugin_sections = app_plugin_sections(draft);
    let mut sections: Vec<&'static str> = OPTION_SECTIONS.to_vec();
    sections.extend(plugin_sections.iter().map(|plug| plug.heading));
    // The section renderer cannot push into `actions` while the shell closure holds it, so
    // backup requests are collected here.
    let mut backup_actions = Vec::new();
    let outcome = settings_shell(
        ctx,
        palette,
        nav,
        ShellSpec {
            id: "options",
            title: "Options",
            sections: &sections,
            buttons: ShellButtons {
                primary: "Save",
                secondary: "Cancel",
                apply: None,
                danger: None,
            },
        },
        |ui, index| match index {
            0 => options_appearance(ui, palette, draft),
            1 => options_general(ui, palette, draft),
            2 => options_terminal(ui, palette, draft),
            3 => options_backup(ui, palette, backup_path, &mut backup_actions),
            4 => options_host_defaults(
                ui,
                palette,
                draft,
                &mut DefaultColourDrafts {
                    tab: tab_color_draft,
                    background: term_background_draft,
                    foreground: term_foreground_draft,
                },
            ),
            5 => options_plugins(ui, palette, draft),
            _ => {
                if let Some(section) = plugin_sections.get(index - OPTION_SECTIONS.len()) {
                    let values = draft
                        .plugin_settings
                        .entry(section.id.as_str().to_owned())
                        .or_default();
                    if section.presentation == plugin_settings::Presentation::Editor {
                        // The original app puts a single "Configure…" row here and
                        // renders the plugin's own editor in a modal.
                        let manifest = section.id.manifest();
                        let mut configure = false;
                        row(
                            ui,
                            palette,
                            &format!("{} settings", manifest.name),
                            Some("Configured in a dedicated editor."),
                            |ui| {
                                configure = small_button(ui, palette, "Configure…");
                            },
                        );
                        if configure {
                            open_editor = Some(section.id);
                        }
                    } else {
                        plugin_section_rows(
                            ui,
                            palette,
                            section,
                            values,
                            ai_providers,
                            plugin_secrets,
                        );
                    }
                }
            }
        },
    );
    if let Some(id) = open_editor {
        *editor = Some(id);
    }
    actions.append(&mut backup_actions);
    match outcome {
        ShellOutcome::Save => {
            actions.push(DialogAction::SaveSettings(
                draft.clone().normalized(),
                plugin_secrets.clone(),
            ));
        }
        // A dialog on top of the settings takes the dismissal, so the settings stay open.
        ShellOutcome::Dismissed if sub_dialog => {
            *editor = None;
            ai_providers.cancel();
        }
        ShellOutcome::Cancel | ShellOutcome::Dismissed => actions.push(DialogAction::Close),
        ShellOutcome::Delete | ShellOutcome::Apply | ShellOutcome::Pending => {}
    }
    actions
}

fn options_appearance(ui: &mut Ui, palette: &Palette, draft: &mut AppSettings) {
    row(
        ui,
        palette,
        "Theme",
        Some("Applies to the whole app. \u{201c}System\u{201d} follows the desktop's light or dark mode."),
        |ui| {
            let options = [
                (ThemeMode::System, ThemeMode::System.label()),
                (ThemeMode::Dark, ThemeMode::Dark.label()),
                (ThemeMode::Light, ThemeMode::Light.label()),
            ];
            if let Some(mode) = select(ui, "options-theme", &draft.theme, &options) {
                draft.theme = mode;
            }
        },
    );
}

fn options_general(ui: &mut Ui, palette: &Palette, draft: &mut AppSettings) {
    row(
        ui,
        palette,
        "Reconnect sessions on startup",
        Some("Re-open and reconnect every tab that was open when WaSSH exited or crashed."),
        |ui| {
            checkbox(ui, palette, &mut draft.reconnect_on_startup, "Enabled");
        },
    );
}

fn options_terminal(ui: &mut Ui, palette: &Palette, draft: &mut AppSettings) {
    row(
        ui,
        palette,
        "TERM",
        Some("PTY terminal type sent to the remote host."),
        |ui| {
            text_input(ui, palette, &mut draft.term_type, "xterm-256color");
        },
    );
}

/// Hex drafts backing the default-colour rows, borrowed together.
struct DefaultColourDrafts<'a> {
    tab: &'a mut String,
    background: &'a mut String,
    foreground: &'a mut String,
}

fn options_host_defaults(
    ui: &mut Ui,
    palette: &Palette,
    draft: &mut AppSettings,
    hex: &mut DefaultColourDrafts<'_>,
) {
    ui.label(
        egui::RichText::new(
            "Applied to new hosts and quick connect, and to any host or session field set to \
             \u{201c}Use default\u{201d}. Changing a value here updates those fields immediately.",
        )
        .size(12.0)
        .color(to_color32(palette.text_muted)),
    );
    ui.add_space(8.0);
    let defaults = &mut draft.session_style_defaults;

    let fallback = resolved_colour(defaults.tab_color, None);
    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Tab colour",
            hint: "Theme default follows the app accent",
            toggle_label: "Theme default",
            hex: hex.tab,
            resolved: fallback,
        },
        &mut defaults.tab_color,
    );
    let fallback = resolved_colour(defaults.term_background, Some(palette.bg_term));
    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Terminal background",
            hint: "Theme default follows the app terminal background",
            toggle_label: "Theme default",
            hex: hex.background,
            resolved: fallback,
        },
        &mut defaults.term_background,
    );
    let fallback = resolved_colour(defaults.term_foreground, Some(palette.term_fg));
    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Terminal text",
            hint: "Theme default follows the app terminal text colour",
            toggle_label: "Theme default",
            hex: hex.foreground,
            resolved: fallback,
        },
        &mut defaults.term_foreground,
    );
    row(
        ui,
        palette,
        "Font family",
        Some("Default monospace family for hosts using the default"),
        |ui| {
            text_input(ui, palette, &mut defaults.font_family, "JetBrains Mono");
        },
    );
    row(ui, palette, "Font size", None, |ui| {
        let mut size = u32::from(defaults.font_size);
        if number_input(
            ui,
            palette,
            &mut size,
            MIN_FONT_SIZE_PX as u32,
            MAX_FONT_SIZE_PX as u32,
        ) {
            defaults.font_size = size as u16;
        }
    });
    row(ui, palette, "Scrollback lines", None, |ui| {
        number_input(
            ui,
            palette,
            &mut defaults.scrollback,
            MIN_SCROLLBACK_LINES,
            MAX_SCROLLBACK_LINES,
        );
    });
    row(ui, palette, "Bell", None, |ui| {
        let options = [
            (BellMode::System, "Default system sound"),
            (BellMode::InvertWindow, "Blink whole terminal"),
            (BellMode::InvertLine, "Blink current line"),
        ];
        if let Some(bell) = select(ui, "options-bell", &defaults.bell, &options) {
            defaults.bell = bell;
        }
    });
    row(ui, palette, "Cursor", None, |ui| {
        let options = [
            (CursorStyle::Block, "Block"),
            (CursorStyle::Underline, "Underline"),
            (CursorStyle::Bar, "Vertical line"),
        ];
        if let Some(cursor) = select(ui, "options-cursor", &defaults.cursor, &options) {
            defaults.cursor = cursor;
        }
    });
    row(ui, palette, "Cursor blink", None, |ui| {
        checkbox(ui, palette, &mut defaults.cursor_blink, "Blink");
    });
}

fn options_backup(ui: &mut Ui, palette: &Palette, path: &mut String, actions: &mut Vec<DialogAction>) {
    row_block(ui, palette, "Configuration backup", |ui| {
        banner(
            ui,
            palette,
            BannerKind::Info,
            "The archive holds every config file, including stored passwords — keep it safe.",
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Archive")
                .size(11.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(2.0);
        ui.add(
            egui::TextEdit::singleline(path)
                .font(egui::FontId::monospace(11.0))
                .desired_width(f32::INFINITY)
                .hint_text("~/wassh-backup.zip"),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if Button::new("Export backup").show(ui, palette).clicked() {
                actions.push(DialogAction::Backup {
                    import: false,
                    path: path.clone(),
                });
            }
            if Button::new("Restore backup").show(ui, palette).clicked() {
                actions.push(DialogAction::Backup {
                    import: true,
                    path: path.clone(),
                });
            }
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Restoring overwrites the current files; restart WaSSH afterwards to load them.",
            )
            .size(11.0)
            .color(to_color32(palette.text_muted)),
        );
    });
}

/// Height a plugin editor's body gets in a window `screen_height` tall.
///
/// Half the window, so the settings and provider cards get a real box to scroll in while
/// the dialog still reads as a dialog; a small window clamps to the minimum instead.
fn editor_room(screen_height: f32) -> f32 {
    (screen_height * EDITOR_HEIGHT_SHARE - EDITOR_CHROME).max(EDITOR_MIN_HEIGHT)
}

/// A plugin's own settings dialog, opened by a "Configure…" row.
///
/// Only plugins whose settings are one big record list use one (the MQTT analyser's saved
/// messages); everything else is drawn inline in its Options section.
fn options_plugin_editor(ctx: &Context, palette: &Palette, state: &mut OptionsState) {
    let Some(id) = state.editor else {
        return;
    };
    let schema = plugin_settings::app_schema(id);
    if schema.is_empty() {
        state.editor = None;
        return;
    }
    let manifest = id.manifest();
    // The values are taken out of the draft while the modal borrows them.
    let mut values = state
        .draft
        .plugin_settings
        .remove(id.as_str())
        .unwrap_or_default();
    let mut close = false;
    let room = editor_room(ctx.content_rect().height());
    let response = modal(
        ctx,
        ("plugin-settings-editor", id.as_str()),
        palette,
        &format!("{} settings", manifest.name),
        EDITOR_WIDTH,
        |ui| {
            // The modal measures itself from its content, so the tall body has to be
            // claimed explicitly. A scroll area only gets `min(content, available)` and
            // `available` collapses to nothing on egui's sizing pass, which used to leave
            // the AI agent's editor ~180 px tall however tall the window was.
            let width = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::Vec2::new(width, room),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("plugin-editor-scroll", id.as_str()))
                        .max_height(room)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            plugin_settings_rows(
                                ui,
                                palette,
                                schema,
                                &mut values,
                                &mut SecretEditing {
                                    id,
                                    secrets: &mut state.plugin_secrets,
                                },
                            );
                        });
                },
            );
            footer(ui, |ui| {
                if small_button(ui, palette, "Close") {
                    close = true;
                }
            });
        },
    );
    if !values.is_empty() {
        state
            .draft
            .plugin_settings
            .insert(id.as_str().to_owned(), values);
    }
    if response.dismissed || close {
        state.editor = None;
    }
}

/// The AI agent's "add or edit a provider" dialog.
///
/// The plugin owns the draft and its fields; the frame, the title, the buttons and
/// Escape are this app's, so the dialog behaves like every other one here and sits on
/// top of Options without either dialog knowing the other.
fn options_provider_dialog(ctx: &Context, palette: &Palette, state: &mut OptionsState) {
    if !state.ai_providers.dialog_open() {
        return;
    }
    let OptionsState {
        draft,
        ai_providers,
        plugin_secrets,
        ..
    } = state;
    let values = draft
        .plugin_settings
        .entry(PluginId::AiAgent.as_str().to_owned())
        .or_default();
    let title = ai_providers.dialog_title();
    let primary = ai_providers.dialog_primary();
    let can_commit = ai_providers.can_commit();
    let mut commit = false;
    let mut cancel = false;
    let response = modal(
        ctx,
        "ai-agent-provider",
        palette,
        title,
        wassh_plugins::ai_agent::PROVIDER_DIALOG_WIDTH,
        |ui| {
            ai_providers.dialog_body(ui, palette, values, plugin_secrets);
            footer(ui, |ui| {
                if small_button_enabled(ui, palette, primary, can_commit) {
                    commit = true;
                }
                if small_button(ui, palette, "Cancel") {
                    cancel = true;
                }
            });
        },
    );
    if commit {
        ai_providers.commit(values, plugin_secrets);
    } else if cancel || response.dismissed {
        ai_providers.cancel();
    }
}

/// One plugin section's content: its provider list (when it has one) and its settings.
///
/// Everything is drawn inline in the global dialog; the only dialog a plugin section opens
/// is the app's single-item editor, which the AI agent fills with an add/edit provider
/// draft (see [`options_provider_dialog`]).
fn plugin_section_rows(
    ui: &mut Ui,
    palette: &Palette,
    section: &PluginSection,
    values: &mut serde_json::Map<String, serde_json::Value>,
    editor: &mut wassh_plugins::ai_agent::ProviderEditor,
    secrets: &mut PluginSecretDrafts,
) {
    if plugin_settings::draws_provider_list(section.id) {
        row_block(ui, palette, "Providers", |ui| {
            ui.label(
                egui::RichText::new(
                    "Endpoints the AI agent can chat through. API keys are sealed in the \
                     credential vault.",
                )
                .size(11.0)
                .color(to_color32(palette.text_muted)),
            );
            ui.add_space(6.0);
            editor.show(ui, palette, values, secrets);
        });
    }
    plugin_settings_rows(
        ui,
        palette,
        section.schema,
        values,
        &mut SecretEditing {
            id: section.id,
            secrets,
        },
    );
}

/// One plugin-owned section of a settings dialog.
#[derive(Clone, Copy, Debug)]
struct PluginSection {
    id: PluginId,
    /// Rail title.
    heading: &'static str,
    /// Field schema drawn in the section (or in its editor).
    schema: &'static [SettingsField],
    /// Inline rows, or a "Configure…" row that opens the plugin's editor.
    presentation: plugin_settings::Presentation,
}

/// Sections Options shows: every enabled plugin that declares app-wide fields.
fn app_plugin_sections(settings: &AppSettings) -> Vec<PluginSection> {
    PluginId::UI_ORDER
        .into_iter()
        .filter(|id| settings.plugin_enabled(id.as_str()))
        .filter(|id| !plugin_settings::app_schema(*id).is_empty())
        .map(|id| PluginSection {
            id,
            heading: plugin_settings::app_heading(id),
            schema: plugin_settings::app_schema(id),
            presentation: plugin_settings::app_presentation(id),
        })
        .collect()
}

/// Sections Host settings shows: every enabled plugin that declares per-host
/// fields.
fn host_plugin_sections(enabled: &[String]) -> Vec<PluginSection> {
    PluginId::UI_ORDER
        .into_iter()
        .filter(|id| enabled.iter().any(|enabled| enabled == id.as_str()))
        .filter(|id| !plugin_settings::host_schema(*id).is_empty())
        .map(|id| PluginSection {
            id,
            heading: plugin_settings::host_heading(id),
            schema: plugin_settings::host_schema(id),
            presentation: plugin_settings::Presentation::Fields,
        })
        .collect()
}

fn options_plugins(ui: &mut Ui, palette: &Palette, settings: &mut AppSettings) {
    row_block(ui, palette, "Enable plugins", |ui| {
        ui.label(
            egui::RichText::new("Turn built-in plugins on or off.")
                .size(11.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(6.0);
        for plugin in PluginId::UI_ORDER {
            let mut enabled = settings.plugin_enabled(plugin.as_str());
            let manifest = plugin.manifest();
            row(ui, palette, manifest.name, Some(manifest.description), |ui| {
                checkbox(ui, palette, &mut enabled, "");
            });
            let is_on = settings.plugin_enabled(plugin.as_str());
            if enabled != is_on {
                settings
                    .enabled_plugins
                    .retain(|existing| existing != plugin.as_str());
                if enabled {
                    settings.enabled_plugins.push(plugin.as_str().to_owned());
                }
            }
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "External plugins: none loaded. Future versions can scan a plugins folder in \
                 the data directory.",
            )
            .size(11.0)
            .color(to_color32(palette.text_muted)),
        );
    });
}

/// Draw a plugin settings schema: one row per field, children of a group indented
/// and disabled while the group toggle is off.
fn plugin_settings_rows(
    ui: &mut Ui,
    palette: &Palette,
    schema: &[SettingsField],
    values: &mut serde_json::Map<String, serde_json::Value>,
    section: &mut SecretEditing<'_>,
) {
    for field in schema {
        if field.kind == FieldKind::Group {
            let mut on = setting_bool(values, field);
            row(ui, palette, field.label, Some(field.description), |ui| {
                checkbox(ui, palette, &mut on, "");
            });
            if on != setting_bool(values, field) {
                values.insert(field.key.to_owned(), serde_json::Value::Bool(on));
            }
            ui.indent(field.key, |ui| {
                if !on {
                    // Children stay visible but untouchable while the group is off.
                    ui.disable();
                }
                plugin_settings_rows(ui, palette, field.children, values, section);
            });
            continue;
        }
        match field.kind {
            FieldKind::Boolean | FieldKind::Group => {
                let mut on = setting_bool(values, field);
                row(ui, palette, field.label, Some(field.description), |ui| {
                    checkbox(ui, palette, &mut on, "");
                });
                if on != setting_bool(values, field) {
                    values.insert(field.key.to_owned(), serde_json::Value::Bool(on));
                }
            }
            FieldKind::Number => {
                let mut number = setting_i64(values, field).clamp(0, 1_000_000_000) as u32;
                let before = number;
                row(ui, palette, field.label, Some(field.description), |ui| {
                    number_input(ui, palette, &mut number, 0, 1_000_000_000);
                });
                if number != before {
                    values.insert(field.key.to_owned(), serde_json::Value::from(number));
                }
            }
            FieldKind::Select | FieldKind::Permission => {
                let options: Vec<&'static str> = if field.kind == FieldKind::Permission {
                    vec!["allow", "deny", "ask"]
                } else {
                    field.options.iter().map(|option| option.label).collect()
                };
                let labels: Vec<&'static str> = if field.kind == FieldKind::Permission {
                    vec!["Allow", "Deny", "Ask"]
                } else {
                    field.options.iter().map(|option| option.label).collect()
                };
                let default = setting_text(values, field);
                let selected = labels
                    .iter()
                    .position(|label| *label == default)
                    .unwrap_or(0);
                let mut chosen = selected;
                row(ui, palette, field.label, Some(field.description), |ui| {
                    if let Some(picked) =
                        select_str(ui, ("plugin-setting", field.key), labels[chosen], &labels)
                        && let Some(index) = labels.iter().position(|label| *label == picked)
                    {
                        chosen = index;
                    }
                });
                if chosen != selected {
                    let value = if field.kind == FieldKind::Permission {
                        options[chosen]
                    } else {
                        field.options[chosen].value
                    };
                    values.insert(field.key.to_owned(), serde_json::Value::from(value));
                }
            }
            FieldKind::StringList => {
                let mut text = setting_list_text(values, field);
                let before = text.clone();
                row_block(ui, palette, field.label, |ui| {
                    ui.label(
                        egui::RichText::new(field.description)
                            .size(11.0)
                            .color(to_color32(palette.text_muted)),
                    );
                    ui.add_space(4.0);
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 84.0),
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("One entry per line"),
                    );
                });
                if text != before {
                    let items = text
                        .lines()
                        .map(|line| serde_json::Value::from(line.trim_end()))
                        .collect();
                    values.insert(field.key.to_owned(), serde_json::Value::Array(items));
                }
            }
            FieldKind::ItemList => item_list_editor(ui, palette, field, values),
            FieldKind::TextArea => {
                let mut text = setting_text(values, field);
                let before = text.clone();
                row_block(ui, palette, field.label, |ui| {
                    ui.label(
                        egui::RichText::new(field.description)
                            .size(11.0)
                            .color(to_color32(palette.text_muted)),
                    );
                    ui.add_space(4.0);
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 96.0),
                        egui::TextEdit::multiline(&mut text),
                    );
                });
                if text != before {
                    values.insert(field.key.to_owned(), serde_json::Value::from(text));
                }
            }
            FieldKind::String | FieldKind::Directory => {
                let hint = match field.default {
                    wassh_plugins::settings::FieldDefault::Text(text) => text,
                    _ => "",
                };
                if field.secret {
                    // Sealed in the credential vault: the settings map never
                    // sees the plain text, so a saved host has no copy of it.
                    let key = secret_field_key(section.id.as_str(), field.key);
                    let draft = section.secrets.entry(key).or_default();
                    row(ui, palette, field.label, Some(field.description), |ui| {
                        password_input(
                            ui,
                            palette,
                            &mut draft.draft,
                            if draft.stored { "(stored)" } else { hint },
                        );
                        if draft.stored {
                            ui.add_space(2.0);
                            checkbox(ui, palette, &mut draft.clear, "Forget stored value");
                        }
                    });
                    continue;
                }
                let mut text = setting_text(values, field);
                let before = text.clone();
                row(ui, palette, field.label, Some(field.description), |ui| {
                    text_input(ui, palette, &mut text, hint);
                });
                if text != before {
                    values.insert(field.key.to_owned(), serde_json::Value::from(text));
                }
            }
        }
    }
}

/// Edit a list of records: one card per record with the fields of
/// `field.item_schema`, plus Remove/Add buttons.
fn item_list_editor(
    ui: &mut Ui,
    palette: &Palette,
    field: &SettingsField,
    values: &mut serde_json::Map<String, serde_json::Value>,
) {
    let muted = to_color32(palette.text_muted);
    ui.add_space(ROW_PAD);
    ui.label(
        egui::RichText::new(field.label)
            .size(13.0)
            .strong()
            .color(to_color32(palette.text)),
    );
    if !field.description.is_empty() {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(field.description).size(12.0).color(muted));
    }
    ui.add_space(6.0);

    let mut records = values
        .get(field.key)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    let mut remove = None;
    // The original renders pure text lists as one compact row per record (the
    // macro editor) and everything else as expandable cards.
    let compact = !field.item_schema.is_empty()
        && field
            .item_schema
            .iter()
            .all(|sub| sub.kind == FieldKind::String);
    if compact {
        for (index, record) in records.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let width = ((ui.available_width() - 90.0)
                    / field.item_schema.len().max(1) as f32)
                    .max(60.0);
                for sub in field.item_schema {
                    let mut text = record
                        .get(sub.key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let before = text.clone();
                    let response = ui.add_sized(
                        [width, 22.0],
                        egui::TextEdit::singleline(&mut text)
                            .id_salt(("plugin-item-text", field.key, index, sub.key))
                            .hint_text(sub.label),
                    );
                    let _ = response;
                    if text != before {
                        if !record.is_object() {
                            *record = serde_json::Value::Object(Default::default());
                        }
                        record[sub.key] = serde_json::Value::from(text);
                        changed = true;
                    }
                }
                if small_button(ui, palette, "Remove") {
                    remove = Some(index);
                }
            });
            ui.add_space(4.0);
        }
        if let Some(index) = remove {
            records.remove(index);
            changed = true;
        }
        let add = format!("Add {}", field.item_label);
        if small_button(ui, palette, &add) {
            records.push(blank_record(field, &records));
            changed = true;
        }
        if changed {
            values.insert(field.key.to_owned(), serde_json::Value::Array(records));
        }
        ui.add_space(ROW_PAD);
        let y = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().x_range(),
            y,
            Stroke::new(1.0, to_color32(palette.border).gamma_multiply(0.5)),
        );
        return;
    }
    for (index, record) in records.iter_mut().enumerate() {
        if !record.is_object() {
            *record = serde_json::Value::Object(Default::default());
        }
        ui.push_id(("plugin-item", field.key, index), |ui| {
            card(ui, palette, |ui| {
                for sub in field.item_schema {
                    edit_record_field(ui, palette, field.key, index, sub, record, &mut changed);
                }
                if small_button(ui, palette, "Remove") {
                    remove = Some(index);
                }
            });
        });
        ui.add_space(6.0);
    }
    if let Some(index) = remove {
        records.remove(index);
        changed = true;
    }
    let add = format!("Add {}", field.item_label);
    if small_button(ui, palette, &add) {
        records.push(blank_record(field, &records));
        changed = true;
    }
    if changed {
        values.insert(field.key.to_owned(), serde_json::Value::Array(records));
    }
    ui.add_space(ROW_PAD);
    let y = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        y,
        Stroke::new(1.0, to_color32(palette.border).gamma_multiply(0.5)),
    );
}

/// One field of one record.
fn edit_record_field(
    ui: &mut Ui,
    palette: &Palette,
    list_key: &str,
    index: usize,
    field: &SettingsField,
    record: &mut serde_json::Value,
    changed: &mut bool,
) {
    let current = record.get(field.key).cloned();
    let mut text = match &current {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => match field.default {
            wassh_plugins::settings::FieldDefault::Text(text) => text.to_owned(),
            _ => String::new(),
        },
    };
    let before = text.clone();
    match field.kind {
        FieldKind::TextArea => row_block(ui, palette, field.label, |ui| {
            ui.add_sized(
                Vec2::new(ui.available_width(), 60.0),
                egui::TextEdit::multiline(&mut text)
                    .id_salt(("plugin-item-field", list_key, index, field.key))
                    .hint_text(field.description),
            );
        }),
        FieldKind::Select => {
            let labels: Vec<&'static str> = field.options.iter().map(|option| option.label).collect();
            let selected = field
                .options
                .iter()
                .find(|option| option.value == text)
                .map(|option| option.value)
                .unwrap_or(text.as_str());
            let mut chosen = selected.to_owned();
            row(ui, palette, field.label, None, |ui| {
                if let Some(picked) = select_str(
                    ui,
                    ("plugin-item-select", list_key, index, field.key),
                    &chosen,
                    &labels,
                ) && let Some(option) = field
                    .options
                    .iter()
                    .find(|option| option.label == picked || option.value == picked)
                {
                    chosen = option.value.to_owned();
                }
            });
            text = chosen;
        }
        _ => row(ui, palette, field.label, None, |ui| {
            text_input(ui, palette, &mut text, field.description);
        }),
    }
    if text != before {
        if !record.is_object() {
            *record = serde_json::Value::Object(Default::default());
        }
        record[field.key] = serde_json::Value::from(text);
        *changed = true;
    }
}

/// A new record with fresh ids and the schema defaults.
fn blank_record(
    field: &SettingsField,
    existing: &[serde_json::Value],
) -> serde_json::Value {
    let mut record = serde_json::Map::new();
    for sub in field.item_schema {
        record.insert(sub.key.to_owned(), sub.default_value());
    }
    // Records are addressed by id; pick one that is not taken yet.
    let mut next = existing.len() + 1;
    let taken = |id: &str| {
        existing
            .iter()
            .any(|record| record.get("id").and_then(serde_json::Value::as_str) == Some(id))
    };
    while taken(&format!("{}-{next}", field.key)) {
        next += 1;
    }
    record.insert(
        "id".to_owned(),
        serde_json::Value::from(format!("{}-{next}", field.key)),
    );
    serde_json::Value::Object(record)
}

fn setting_bool(
    values: &serde_json::Map<String, serde_json::Value>,
    field: &SettingsField,
) -> bool {
    values
        .get(field.key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(matches!(
            field.default,
            wassh_plugins::settings::FieldDefault::Bool(true)
        ))
}

fn setting_i64(values: &serde_json::Map<String, serde_json::Value>, field: &SettingsField) -> i64 {
    values
        .get(field.key)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(match field.default {
            wassh_plugins::settings::FieldDefault::Number(number) => number,
            _ => 0,
        })
}

fn setting_text(values: &serde_json::Map<String, serde_json::Value>, field: &SettingsField) -> String {
    match values.get(field.key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => match field.default {
            wassh_plugins::settings::FieldDefault::Text(text) => text.to_owned(),
            _ => String::new(),
        },
    }
}

fn setting_list_text(
    values: &serde_json::Map<String, serde_json::Value>,
    field: &SettingsField,
) -> String {
    match values.get(field.key) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => match field.default {
            wassh_plugins::settings::FieldDefault::TextList(items) => items.join("\n"),
            _ => String::new(),
        },
    }
}

// --- Host / Session settings ---------------------------------------------

/// Sections shown for a connection type.
pub fn host_sections(kind: ConnectionType) -> Vec<&'static str> {
    let mut sections = vec!["Connection"];
    if kind == ConnectionType::Ssh {
        sections.push("Authentication");
        sections.push("Advanced");
        sections.push("Tunnels");
    }
    sections.push("Appearance");
    sections
}

fn host_dialog(
    ctx: &Context,
    palette: &Palette,
    state: &mut HostState,
    hosts: &[HostProfile],
    enabled_plugins: &[String],
) -> Vec<DialogAction> {
    let mut actions = Vec::new();
    let mut sections = host_sections(state.draft.connection.kind);
    let target = state.target.clone();
    // Enabled plugins with per-host fields add a section each, after Appearance. A session
    // dialog leaves them out: plugin settings are stored per host, and the session dialog
    // must not write the host.
    let builtin_sections = sections.len();
    let plugin_sections = match target {
        HostTarget::Host => host_plugin_sections(enabled_plugins),
        HostTarget::Session(_) => Vec::new(),
    };
    sections.extend(plugin_sections.iter().map(|plug| plug.heading));
    let title = match target {
        HostTarget::Host => "Host settings",
        HostTarget::Session(_) => "Session settings",
    };
    // Footer actions: a session dialog only saves to its tab, and only a saved host can
    // be applied to its sessions or deleted.
    let can_apply = state.can_apply_to_sessions();
    let can_delete = state.can_delete();
    // Borrow the fields one by one so the content closure can touch the credential
    // fields while the shell holds the rail.
    let HostState {
        nav,
        draft,
        tab_color_draft,
        term_background_draft,
        term_foreground_draft,
        tag_draft,
        plugin_secrets,
        password,
        passphrase,
        stored_password,
        stored_passphrase,
        clear_password,
        clear_passphrase,
        defaults: state_defaults,
        has_sessions: _,
        saved: _,
        target: _,
    } = state;

    let outcome = settings_shell(
        ctx,
        palette,
        nav,
        ShellSpec {
            id: "host-settings",
            title,
            sections: &sections,
            buttons: ShellButtons {
                primary: "Save",
                secondary: "Cancel",
                apply: can_apply.then_some("Apply to sessions"),
                danger: can_delete.then_some("Delete"),
            },
        },
        |ui, index| match sections[index] {
            // Plugin sections are the ones past the built-in list.
            _ if index >= builtin_sections => {
                if let Some(section) = plugin_sections.get(index - builtin_sections) {
                    let values = draft
                        .plugin_settings
                        .entry(section.id.as_str().to_owned())
                        .or_default();
                    plugin_settings_rows(
                        ui,
                        palette,
                        section.schema,
                        values,
                        &mut SecretEditing {
                            id: section.id,
                            secrets: plugin_secrets,
                        },
                    );
                }
            }
            "Connection" => host_connection(
                ui,
                palette,
                draft,
                tag_draft,
                target == HostTarget::Host,
            ),
            "Authentication" => host_authentication(
                ui,
                palette,
                &mut AuthFields {
                    draft,
                    password,
                    passphrase,
                    stored_password: *stored_password,
                    stored_passphrase: *stored_passphrase,
                    clear_password,
                    clear_passphrase,
                },
            ),
            "Advanced" => host_advanced(
                ui,
                palette,
                draft,
                hosts,
                target == HostTarget::Host,
            ),
            "Tunnels" => host_tunnels(ui, palette, draft),
            _ => host_appearance(
                ui,
                palette,
                &mut AppearanceFields {
                    style: &mut draft.connection.style,
                    tab_color: tab_color_draft,
                    term_background: term_background_draft,
                    term_foreground: term_foreground_draft,
                    defaults: state_defaults,
                },
            ),
        },
    );
    let save = || {
        let mut host = draft.clone();
        host.connection = host.connection.normalized();
        HostSave {
            host,
            password: SecretEdit::from_field(password, *clear_password),
            passphrase: SecretEdit::from_field(passphrase, *clear_passphrase),
            plugin_secrets: plugin_secrets
                .iter()
                .filter(|(_, draft)| draft_changes(draft))
                .map(|(key, draft)| (key.clone(), draft.clone()))
                .collect(),
        }
    };
    match outcome {
        ShellOutcome::Save => match target {
            HostTarget::Host => actions.push(DialogAction::SaveHost(Box::new(save()))),
            HostTarget::Session(session) => actions.push(DialogAction::SaveSession {
                session,
                save: Box::new(save()),
            }),
        },
        ShellOutcome::Apply => actions.push(DialogAction::ApplyHost(Box::new(save()))),
        ShellOutcome::Delete => actions.push(DialogAction::DeleteHost(draft.id.clone())),
        ShellOutcome::Cancel | ShellOutcome::Dismissed => actions.push(DialogAction::Close),
        ShellOutcome::Pending => {}
    }
    actions
}

fn host_connection(
    ui: &mut Ui,
    palette: &Palette,
    draft: &mut HostProfile,
    tag_draft: &mut String,
    host_fields: bool,
) {
    // Name and tags describe the saved host; a session has neither.
    if host_fields {
        row(ui, palette, "Name", Some("Shown in the sessions panel"), |ui| {
            text_input(ui, palette, &mut draft.name, "My server");
        });
        row(
            ui,
            palette,
            "Tags",
            Some("Labels for this host. Type a tag and press Enter or comma to add it."),
            |ui| {
                tag_input(ui, palette, "host-tags", &mut draft.tags, tag_draft, "Add tag…");
            },
        );
    }
    row(ui, palette, "Type", None, |ui| {
        let options = [
            (ConnectionType::Ssh, "SSH"),
            (ConnectionType::Telnet, "Telnet"),
            (ConnectionType::Serial, "Serial"),
            (ConnectionType::Local, "Local terminal"),
        ];
        if let Some(kind) = select(ui, "host-kind", &draft.connection.kind, &options) {
            draft.connection.kind = kind;
            if kind != ConnectionType::Serial && kind != ConnectionType::Local {
                draft.connection.port = kind.default_port();
            }
        }
    });

    match draft.connection.kind {
        ConnectionType::Ssh | ConnectionType::Telnet => {
            row(ui, palette, "Host", Some("Hostname or IP address"), |ui| {
                text_input(ui, palette, &mut draft.connection.host, "example.com");
            });
            row(ui, palette, "Port", None, |ui| {
                let mut port = u32::from(draft.connection.port);
                if number_input(ui, palette, &mut port, 1, 65_535) {
                    draft.connection.port = port as u16;
                }
            });
            if draft.connection.kind == ConnectionType::Ssh {
                row(ui, palette, "Reconnect", Some("Retry policy for drops"), |ui| {
                    reconnect_select(ui, &mut draft.connection.reconnect, "host-reconnect");
                });
            }
        }
        ConnectionType::Serial => {
            row(ui, palette, "Serial port", None, |ui| {
                text_input(ui, palette, &mut draft.connection.serial.path, "/dev/ttyUSB0");
            });
            row(ui, palette, "Baud rate", None, |ui| {
                number_input(ui, palette, &mut draft.connection.serial.baud, 300, 3_000_000);
            });
            row(ui, palette, "Data bits", None, |ui| {
                let options = [(5u8, "5"), (6, "6"), (7, "7"), (8, "8")];
                if let Some(bits) = select(ui, "host-data-bits", &draft.connection.serial.data_bits, &options)
                {
                    draft.connection.serial.data_bits = bits;
                }
            });
            row(ui, palette, "Parity", None, |ui| {
                let options = [
                    (Parity::None, "None"),
                    (Parity::Even, "Even"),
                    (Parity::Odd, "Odd"),
                    (Parity::Mark, "Mark"),
                    (Parity::Space, "Space"),
                ];
                if let Some(parity) = select(ui, "host-parity", &draft.connection.serial.parity, &options)
                {
                    draft.connection.serial.parity = parity;
                }
            });
            row(ui, palette, "Stop bits", None, |ui| {
                let options = [(StopBits::One, "1"), (StopBits::Two, "2")];
                if let Some(bits) = select(ui, "host-stop-bits", &draft.connection.serial.stop_bits, &options)
                {
                    draft.connection.serial.stop_bits = bits;
                }
            });
            row(ui, palette, "Flow control", None, |ui| {
                let options = [
                    (wassh_core::FlowControl::None, "None"),
                    (wassh_core::FlowControl::RtsCts, "Hardware (RTS/CTS)"),
                    (wassh_core::FlowControl::XonXoff, "Software (XON/XOFF)"),
                ];
                if let Some(flow) = select(ui, "host-flow", &draft.connection.serial.flow, &options) {
                    draft.connection.serial.flow = flow;
                }
            });
        }
        ConnectionType::Local => {
            row(ui, palette, "Shell", Some("Uses $SHELL as a login shell"), |ui| {
                ui.label(
                    egui::RichText::new("Local terminal")
                        .size(12.0)
                        .color(to_color32(palette.text_muted)),
                );
            });
        }
    }
}

fn reconnect_select(ui: &mut Ui, value: &mut ReconnectMode, id: &'static str) {
    let options = [
        (ReconnectMode::None, "Never"),
        (ReconnectMode::OnFocus, "When focused"),
        (ReconnectMode::Always, "Always"),
    ];
    if let Some(mode) = select(ui, id, value, &options) {
        *value = mode;
    }
}

/// Borrowed credential fields for the authentication section.
struct AuthFields<'a> {
    draft: &'a mut HostProfile,
    password: &'a mut String,
    passphrase: &'a mut String,
    stored_password: bool,
    stored_passphrase: bool,
    clear_password: &'a mut bool,
    clear_passphrase: &'a mut bool,
}

fn host_authentication(ui: &mut Ui, palette: &Palette, fields: &mut AuthFields<'_>) {
    let draft = &mut *fields.draft;
    row(ui, palette, "Username", Some("Blank uses your login name"), |ui| {
        text_input(ui, palette, &mut draft.connection.username, "username");
    });
    row(ui, palette, "Auth method", None, |ui| {
        let options = [
            (AuthMethod::Password, "Password"),
            (AuthMethod::PrivateKey, "Private key"),
            (AuthMethod::None, "ssh-agent"),
        ];
        if let Some(method) = select(ui, "host-auth", &draft.connection.auth, &options) {
            draft.connection.auth = method;
        }
    });
    if draft.connection.auth == AuthMethod::PrivateKey {
        row(ui, palette, "Private key", Some("Path to the key file"), |ui| {
            let mut path = draft
                .connection
                .private_key_path
                .clone()
                .unwrap_or_default();
            if text_input(ui, palette, &mut path, "~/.ssh/id_ed25519").changed() {
                draft.connection.private_key_path =
                    (!path.trim().is_empty()).then_some(path);
            }
        });
        secret_row(
            ui,
            palette,
            "Passphrase",
            "Unlocks the private key. Stored encrypted; empty asks when connecting.",
            fields.passphrase,
            fields.stored_passphrase,
            fields.clear_passphrase,
        );
    } else if draft.connection.auth == AuthMethod::Password {
        secret_row(
            ui,
            palette,
            "Password",
            "Stored encrypted on this machine; empty asks in the terminal.",
            fields.password,
            fields.stored_password,
            fields.clear_password,
        );
    }
}

/// A secret field with a hint about what is already stored.
fn secret_row(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    description: &str,
    value: &mut String,
    stored: bool,
    clear: &mut bool,
) {
    row(ui, palette, label, Some(description), |ui| {
        let placeholder = if stored { "(stored)" } else { "not saved" };
        password_input(ui, palette, value, placeholder);
        if stored {
            ui.add_space(4.0);
            let undo = *clear;
            let text = if undo {
                "Keep stored secret"
            } else {
                "Clear stored secret"
            };
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(text)
                            .size(11.0)
                            .color(to_color32(if undo {
                                palette.accent
                            } else {
                                palette.text_muted
                            })),
                    )
                    .frame(false),
                )
                .clicked()
            {
                *clear = !*clear;
            }
        }
    });
}

fn host_advanced(
    ui: &mut Ui,
    palette: &Palette,
    draft: &mut HostProfile,
    hosts: &[HostProfile],
    host_fields: bool,
) {
    row(ui, palette, "Jump host", Some("Reached with a direct-tcpip tunnel"), |ui| {
        let current = draft.connection.jump_host_id.clone();
        let label = current
            .as_deref()
            .and_then(|id| hosts.iter().find(|host| host.id == id))
            .map(|host| host.display_name().to_owned())
            .unwrap_or_else(|| "None".to_owned());
        egui::ComboBox::from_id_salt("host-jump")
            .selected_text(
                egui::RichText::new(label)
                    .size(12.0)
                    .color(to_color32(palette.text)),
            )
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "None").clicked() {
                    draft.connection.jump_host_id = None;
                }
                for host in hosts {
                    if host.id == draft.id {
                        continue;
                    }
                    let selected = current.as_deref() == Some(host.id.as_str());
                    if ui.selectable_label(selected, host.display_name()).clicked() {
                        draft.connection.jump_host_id = Some(host.id.clone());
                    }
                }
            });
    });
    // Attaching to tmux/screen is set up when a host opens a session, so a session
    // dialog leaves it out.
    if host_fields {
        row(
            ui,
            palette,
            "Open in remote session",
            Some("Attach to tmux or screen"),
            |ui| {
                checkbox(
                    ui,
                    palette,
                    &mut draft.connection.remote_session.enabled,
                    "Enabled",
                );
            },
        );
    }
    if host_fields && draft.connection.remote_session.enabled {
        row(ui, palette, "Session type", None, |ui| {
            let options = [(Multiplexer::Tmux, "tmux"), (Multiplexer::Screen, "screen")];
            if let Some(kind) = select(
                ui,
                "host-multiplexer",
                &draft.connection.remote_session.multiplexer,
                &options,
            ) {
                draft.connection.remote_session.multiplexer = kind;
            }
        });
        row(ui, palette, "Session name", None, |ui| {
            text_input(
                ui,
                palette,
                &mut draft.connection.remote_session.name,
                "WaSSH",
            );
        });
        row(ui, palette, "Session busy handling", None, |ui| {
            let options = [
                (BusyPolicy::DoNotAttach, "Do not attach"),
                (BusyPolicy::Share, "Share the session"),
                (BusyPolicy::ForceDetach, "Force detach others"),
            ];
            if let Some(policy) = select(
                ui,
                "host-busy",
                &draft.connection.remote_session.busy,
                &options,
            ) {
                draft.connection.remote_session.busy = policy;
            }
        });
    }
    row(ui, palette, "X11 forwarding", Some("Forward X11 to this machine"), |ui| {
        checkbox(ui, palette, &mut draft.connection.x11, "Enabled");
    });
}

fn host_tunnels(ui: &mut Ui, palette: &Palette, draft: &mut HostProfile) {
    row_block(ui, palette, "Tunnels", |ui| {
        ui.label(
            egui::RichText::new("Local (-L), remote (-R) and dynamic SOCKS (-D) forwards.")
                .size(11.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(8.0);

        let mut remove = None;
        for (index, tunnel) in draft.connection.tunnels.iter_mut().enumerate() {
            let frame = egui::Frame::NONE
                .fill(to_color32(palette.bg_panel_2))
                .stroke(Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(wassh_core::RADIUS)
                .inner_margin(egui::Margin::same(8));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Remove reserves its space first, so a long summary wraps instead of
                // shoving the button out of the card.
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Button::new("Remove")
                            .compact()
                            .danger()
                            .show(ui, palette)
                            .clicked()
                        {
                            remove = Some(index);
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            checkbox(ui, palette, &mut tunnel.enabled, "");
                            ui.label(
                                egui::RichText::new(tunnel.summary())
                                    .size(12.0)
                                    .color(to_color32(palette.text)),
                            );
                        });
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let kinds = [
                        (TunnelKind::Local, "Local (-L)"),
                        (TunnelKind::Remote, "Remote (-R)"),
                        (TunnelKind::Dynamic, "SOCKS (-D)"),
                    ];
                    if let Some(kind) = select(ui, ("tunnel-kind", index), &tunnel.kind, &kinds) {
                        tunnel.kind = kind;
                    }
                });
                ui.add_space(4.0);
                control_label(ui, palette, "Listen");
                ui.horizontal(|ui| {
                    text_input_before(
                        ui,
                        palette,
                        &mut tunnel.listen_host,
                        "127.0.0.1",
                        TUNNEL_PORT_WIDTH,
                    );
                    tunnel_port(ui, palette, &mut tunnel.listen_port);
                });
                if tunnel.kind != TunnelKind::Dynamic {
                    ui.add_space(4.0);
                    control_label(ui, palette, "Destination");
                    ui.horizontal(|ui| {
                        text_input_before(
                            ui,
                            palette,
                            &mut tunnel.dest_host,
                            "db.internal",
                            TUNNEL_PORT_WIDTH,
                        );
                        tunnel_port(ui, palette, &mut tunnel.dest_port);
                    });
                }
            });
            ui.add_space(6.0);
        }
        if let Some(index) = remove {
            draft.connection.tunnels.remove(index);
        }
        if small_button(ui, palette, "+ Add tunnel") {
            draft.connection.tunnels.push(Tunnel {
                id: format!("tunnel-{}", draft.connection.tunnels.len() + 1),
                kind: TunnelKind::Local,
                listen_host: "127.0.0.1".to_owned(),
                listen_port: 8080,
                dest_host: "localhost".to_owned(),
                dest_port: 80,
                ..Tunnel::default()
            });
        }
        if draft.connection.tunnels.is_empty() {
            ui.label(
                egui::RichText::new("No tunnels configured.")
                    .size(11.0)
                    .italics()
                    .color(to_color32(palette.text_muted)),
            );
        }
    });
}

/// Appearance fields, borrowed together so the section stays under clippy's argument cap.
struct AppearanceFields<'a> {
    style: &'a mut SessionStyle,
    tab_color: &'a mut String,
    term_background: &'a mut String,
    term_foreground: &'a mut String,
    defaults: &'a SessionStyleDefaults,
}

/// What the "Use default" checkbox did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultToggle {
    /// Untouched.
    None,
    /// Switched to "use the app default".
    On,
    /// Switched to an explicit override.
    Off,
}

/// The colour an override falls back to.
fn resolved_colour(override_value: Option<Rgba>, fallback: Option<Rgba>) -> Rgba {
    override_value
        .or(fallback)
        .unwrap_or(colors::GROUP_COLOR_FALLBACK)
}

fn host_appearance(ui: &mut Ui, palette: &Palette, fields: &mut AppearanceFields<'_>) {
    let defaults = fields.defaults.clone();
    let style = &mut *fields.style;

    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Tab colour",
            hint: "Accent for the tab and the sidebar row",
            toggle_label: "Use default",
            hex: fields.tab_color,
            resolved: resolved_colour(defaults.tab_color, None),
        },
        &mut style.tab_color,
    );
    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Terminal background",
            hint: "Behind the session output",
            toggle_label: "Use default",
            hex: fields.term_background,
            resolved: resolved_colour(defaults.term_background, Some(palette.bg_term)),
        },
        &mut style.term_background,
    );
    colour_row(
        ui,
        palette,
        ColourRow {
            label: "Terminal text",
            hint: "Default foreground for the session",
            toggle_label: "Use default",
            hex: fields.term_foreground,
            resolved: resolved_colour(defaults.term_foreground, Some(palette.term_fg)),
        },
        &mut style.term_foreground,
    );

    override_row(
        ui,
        palette,
        "Font family",
        "Monospace family used by the emulator",
        &mut style.font_family,
        defaults.font_family.clone(),
        |ui, family| {
            text_input(ui, palette, family, "default");
        },
    );
    override_row(
        ui,
        palette,
        "Font size",
        "Cell size in pixels",
        &mut style.font_size,
        defaults.font_size,
        |ui, size| {
            let mut pixels = u32::from(*size);
            if number_input(
                ui,
                palette,
                &mut pixels,
                MIN_FONT_SIZE_PX as u32,
                MAX_FONT_SIZE_PX as u32,
            ) {
                *size = pixels as u16;
            }
        },
    );
    override_row(
        ui,
        palette,
        "Scrollback lines",
        "Primary-buffer history kept for this session",
        &mut style.scrollback,
        defaults.scrollback,
        |ui, lines| {
            number_input(ui, palette, lines, MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        },
    );
    override_row(
        ui,
        palette,
        "Bell",
        "What a bell character does",
        &mut style.bell,
        defaults.bell,
        |ui, bell| {
            let options = [
                (BellMode::System, "Default system sound"),
                (BellMode::InvertWindow, "Blink whole terminal"),
                (BellMode::InvertLine, "Blink current line"),
            ];
            if let Some(chosen) = select(ui, "host-bell", bell, &options) {
                *bell = chosen;
            }
        },
    );
    override_row(
        ui,
        palette,
        "Cursor",
        "Shape of the terminal cursor",
        &mut style.cursor,
        defaults.cursor,
        |ui, cursor| {
            let options = [
                (CursorStyle::Block, "Block"),
                (CursorStyle::Underline, "Underline"),
                (CursorStyle::Bar, "Vertical line"),
            ];
            if let Some(chosen) = select(ui, "host-cursor", cursor, &options) {
                *cursor = chosen;
            }
        },
    );
    override_row(
        ui,
        palette,
        "Cursor blink",
        "Pulse the cursor while the session is idle",
        &mut style.cursor_blink,
        defaults.cursor_blink,
        |ui, blink| {
            checkbox(ui, palette, blink, "Blink");
        },
    );
}

/// A row with a "Use default" checkbox above its control, dimmed while inherited.
fn defaultable_row(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    hint: &str,
    toggle_label: &str,
    use_default: &mut bool,
    control: impl FnOnce(&mut Ui),
) -> DefaultToggle {
    let mut toggle = DefaultToggle::None;
    row(ui, palette, label, Some(hint), |ui| {
        let mut checked = *use_default;
        if checkbox(ui, palette, &mut checked, toggle_label).changed() {
            *use_default = checked;
            toggle = if checked {
                DefaultToggle::On
            } else {
                DefaultToggle::Off
            };
        }
        ui.add_space(6.0);
        ui.scope(|ui| {
            if *use_default {
                // Disabled controls also draw dimmed, like the faded default state.
                ui.disable();
            }
            control(ui);
        });
    });
    toggle
}

/// A row whose value may be overridden, cleared back to `fallback` by "Use default".
fn override_row<T: Clone + PartialEq>(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    hint: &str,
    current: &mut Option<T>,
    fallback: T,
    control: impl FnOnce(&mut Ui, &mut T),
) {
    let mut use_default = current.is_none();
    let toggle = defaultable_row(
        ui,
        palette,
        label,
        hint,
        "Use default",
        &mut use_default,
        |ui| {
        // Start from what is shown; only a real edit creates an override.
        let shown = current.clone().unwrap_or_else(|| fallback.clone());
        let mut value = shown.clone();
        control(ui, &mut value);
        if value != shown {
            *current = Some(value);
        }
        },
    );
    apply_toggle(current, fallback, toggle);
}

/// Apply a checkbox change: "use default" drops the override, "off" pins the default value.
fn apply_toggle<T>(current: &mut Option<T>, fallback: T, toggle: DefaultToggle) {
    match toggle {
        DefaultToggle::On => *current = None,
        DefaultToggle::Off => *current = Some(fallback),
        DefaultToggle::None => {}
    }
}

/// Description of one colour row.
struct ColourRow<'a> {
    label: &'a str,
    hint: &'a str,
    /// Wording of the checkbox that clears the override.
    toggle_label: &'a str,
    /// Hex text backing the field.
    hex: &'a mut String,
    /// Colour the row falls back to.
    resolved: Rgba,
}

/// A colour row: a swatch that opens the picker, plus a hex field.
fn colour_row(
    ui: &mut Ui,
    palette: &Palette,
    row_spec: ColourRow<'_>,
    current: &mut Option<Rgba>,
) {
    let ColourRow {
        label,
        hint,
        toggle_label,
        hex,
        resolved,
    } = row_spec;
    if hex.trim().is_empty() {
        *hex = resolved.to_hex();
    }
    let mut use_default = current.is_none();
    let toggle = defaultable_row(ui, palette, label, hint, toggle_label, &mut use_default, |ui| {
        let mut value = current.unwrap_or(resolved);
        let before = value;
        // The swatch opens the picker; the hex field commits typed values.
        hex_color_input(ui, palette, hex, &mut value);
        if value != before {
            *current = Some(value);
        }
    });
    match toggle {
        DefaultToggle::On => {
            *current = None;
            *hex = resolved.to_hex();
        }
        DefaultToggle::Off => {
            *current = Some(resolved);
            *hex = resolved.to_hex();
        }
        DefaultToggle::None => {}
    }
}

/// The edit-group dialog: name plus the same colour picker as the appearance
/// rows in Host settings, committed with OK and dropped with Cancel.
fn group_edit_dialog(
    ctx: &Context,
    palette: &Palette,
    state: &mut GroupEditState,
) -> Vec<DialogAction> {
    let mut actions = Vec::new();
    let mut ok = false;
    let mut cancel = false;
    let mut clear = false;
    let mut value = state.shown_color();
    let before = value;
    let response = modal(ctx, "group-edit", palette, "Edit group", 440.0, |ui| {
        row(
            ui,
            palette,
            "Name",
            Some("Heading of the group in the sidebar."),
            |ui| {
                text_input(ui, palette, &mut state.name, "Group name");
            },
        );
        row(
            ui,
            palette,
            "Colour",
            Some("Used for the group's tab colour and its rows in the sidebar."),
            |ui| {
                hex_color_input(ui, palette, &mut state.hex, &mut value);
            },
        );
        ui.add_space(8.0);
        if Button::new("No colour").compact().show(ui, palette).clicked() {
            clear = true;
        }
        wassh_ui::widgets::footer(ui, |ui| {
            if Button::new("Cancel").show(ui, palette).clicked() {
                cancel = true;
            }
            if Button::new("OK").primary().show(ui, palette).clicked() {
                ok = true;
            }
        });
    });
    if response.dismissed || cancel {
        actions.push(DialogAction::Close);
    } else if clear {
        actions.push(DialogAction::SaveGroup {
            group_id: state.group_id.clone(),
            name: state.committed_name().to_owned(),
            color: None,
        });
    } else if ok {
        // The swatch or hex field wins; an untouched dialog keeps the old colour.
        let color = if value != before || !state.hex.trim().is_empty() {
            Some(value)
        } else {
            state.color
        };
        actions.push(DialogAction::SaveGroup {
            group_id: state.group_id.clone(),
            name: state.committed_name().to_owned(),
            color,
        });
    }
    actions
}

// --- About, palette, find bar --------------------------------------------

fn about_dialog(ctx: &Context, palette: &Palette) -> Vec<DialogAction> {
    let mut actions = Vec::new();
    let response = modal(ctx, "about", palette, "About", 420.0, |ui| {
        ui.label(
            egui::RichText::new(APP_NAME)
                .size(20.0)
                .strong()
                .color(to_color32(palette.text)),
        );
        ui.label(
            egui::RichText::new(format!("Version {APP_VERSION}"))
                .size(12.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(
                "A native SSH, Telnet and serial client. Rewritten in Rust for speed.",
            )
            .size(12.0)
            .color(to_color32(palette.text)),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            badge(ui, palette, "MIT licence", Tone::Info);
        });
        ui.add_space(12.0);
        wassh_ui::widgets::footer(ui, |ui| {
            if Button::new("Close").primary().show(ui, palette).clicked() {
                actions.push(DialogAction::Close);
            }
        });
    });
    if response.dismissed {
        actions.push(DialogAction::Close);
    }
    actions
}

fn palette_dialog(ctx: &Context, palette: &Palette, state: &mut PaletteState) -> Vec<DialogAction> {
    let mut actions = Vec::new();
    let matches = state.matches();
    let mut chosen: Option<Command> = None;
    modal(ctx, "command-palette", palette, "Command palette", 460.0, |ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Type a command…")
                .desired_width(f32::INFINITY),
        );
        response.request_focus();
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
            for (index, (command, label)) in matches.iter().enumerate() {
                let selected = index == state.selected;
                let text = egui::RichText::new(*label).size(12.0).color(to_color32(
                    if selected {
                        palette.text
                    } else {
                        palette.text_muted
                    },
                ));
                let response = ui.add(
                    egui::Button::new(text)
                        .fill(if selected {
                            to_color32(palette.bg_panel_2)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .min_size(Vec2::new(ui.available_width(), 26.0))
                        .frame(false),
                );
                if response.clicked() {
                    chosen = Some(*command);
                }
            }
        });
        ui.add_space(6.0);
        if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            state.selected = (state.selected + 1).min(matches.len().saturating_sub(1));
        }
        if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            state.selected = state.selected.saturating_sub(1);
        }
        if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some((command, _)) = matches.get(state.selected)
        {
            chosen = Some(*command);
        }
    });
    if let Some(command) = chosen {
        actions.push(DialogAction::Command(command));
    }
    actions
}

fn find_bar(
    ctx: &Context,
    palette: &Palette,
    state: &mut FindState,
    matches: usize,
) -> Option<FindCommand> {
    let mut command = None;
    state.matches = matches;
    egui::Area::new(egui::Id::unique("wassh-find-bar"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-16.0, 80.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(to_color32(palette.bg_panel))
                .stroke(Stroke::new(1.0, to_color32(palette.border)))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut state.query)
                                .hint_text("Find")
                                .desired_width(180.0),
                        );
                        response.request_focus();
                        let count = if state.query.is_empty() {
                            String::new()
                        } else {
                            format!("{}/{}", state.index + 1, state.matches)
                        };
                        ui.label(
                            egui::RichText::new(count)
                                .size(11.0)
                                .color(to_color32(palette.text_muted)),
                        );
                        if Button::new("◀").compact().show(ui, palette).clicked() {
                            command = Some(FindCommand::Previous);
                        }
                        if Button::new("▶").compact().show(ui, palette).clicked() {
                            command = Some(FindCommand::Next);
                        }
                        if Button::new("✕").compact().show(ui, palette).clicked() {
                            command = Some(FindCommand::Close);
                        }
                    });
                });
        });
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        command = Some(FindCommand::Close);
    }
    command
}

/// Menu items for a tab context menu.
pub fn tab_menu_items() -> Vec<wassh_ui::widgets::MenuItem> {
    use wassh_ui::widgets::MenuItem;
    vec![
        MenuItem::new("Reconnect"),
        MenuItem::new("Session settings"),
        MenuItem::new("Save as host…"),
        MenuItem::separator(),
        MenuItem::new("Close tab"),
    ]
}

/// Map a tab menu index to a command.
pub fn tab_menu_command(index: usize) -> Option<Command> {
    match index {
        0 => Some(Command::Reconnect),
        1 => Some(Command::EditHost),
        2 => Some(Command::SaveAsHost),
        4 => Some(Command::CloseTab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wassh_core::{ConnectionParams, RemoteSession};

    /// Run one frame of the dialogs headlessly and count the painted shapes.
    fn run_frame(ctx: &Context, dialogs: &mut Dialogs) -> usize {
        let palette = Palette::dark();
        let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            dialogs.show(ctx, &palette, &[], 0);
        });
        let shapes = output.shapes.len();
        output.drop_without_applying_deltas();
        shapes
    }

    fn frame_with_screen(ctx: &Context, dialogs: &mut Dialogs, size: Vec2) -> usize {
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        };
        let output = ctx.run_pass(input, |ctx| {
            dialogs.show(ctx, &palette, &[], 0);
        });
        let shapes = output.shapes.len();
        output.drop_without_applying_deltas();
        shapes
    }

    #[test]
    fn the_host_dialog_stays_inside_its_panel() {
        // A row that overflows its column widens every row after it, which used to push
        // the whole dialog past its panel. Tags and the tunnel editor both did that.
        let size = Vec2::new(1175.0, 765.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let panel = Rect::from_center_size(
            screen.center(),
            Vec2::new(DIALOG_MAX_WIDTH, DIALOG_MAX_HEIGHT),
        );
        let host = wassh_core::HostProfile {
            id: "h1".to_owned(),
            name: "probe".to_owned(),
            tags: ["cryohab", "octopy", "plc", "beckhoff", "BSD"]
                .iter()
                .map(|tag| (*tag).to_owned())
                .collect(),
            connection: ConnectionParams {
                tunnels: vec![
                    wassh_core::Tunnel {
                        id: "t1".to_owned(),
                        kind: wassh_core::TunnelKind::Local,
                        listen_host: "127.0.0.1".to_owned(),
                        listen_port: 8080,
                        dest_host: "db.internal".to_owned(),
                        dest_port: 5432,
                        enabled: true,
                    },
                    wassh_core::Tunnel {
                        id: "t2".to_owned(),
                        kind: wassh_core::TunnelKind::Dynamic,
                        listen_host: "127.0.0.1".to_owned(),
                        listen_port: 1080,
                        dest_host: String::new(),
                        dest_port: 0,
                        enabled: false,
                    },
                ],
                ..ConnectionParams::default()
            },
            ..wassh_core::HostProfile::default()
        };
        let mut dialogs = Dialogs {
            host: Some(HostState::draft(host, false)),
            ..Dialogs::default()
        };
        let ctx = Context::default();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let output = ctx.run_pass(input, |ctx| {
                dialogs.show(ctx, &Palette::dark(), &[], 0);
            });
            for clipped in &output.shapes {
                let painted = clipped.shape.visual_bounding_rect();
                // The backdrop covers the whole screen; the panel itself ends at its edge.
                if painted.width() >= size.x - 1.0 || painted.max.x <= panel.right() + 0.5 {
                    continue;
                }
                if matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text().is_empty())
                {
                    continue;
                }
                assert!(
                    painted.max.x <= panel.right(),
                    "{:?} is painted at {painted:?}, past the panel edge {}",
                    clipped.shape,
                    panel.right()
                );
            }
            output.drop_without_applying_deltas();
        }
    }

    /// Run two frames (the first is egui's sizing pass) and collect the painted text.
    fn painted_rows(
        ctx: &Context,
        size: Vec2,
        mut draw: impl FnMut(&Context),
    ) -> Vec<(i32, i32, String)> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut rows = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_pass(input.clone(), |ctx| draw(ctx));
            let shapes = std::mem::take(&mut output.shapes);
            rows.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    rows.push((
                        text.pos.y as i32,
                        text.pos.x as i32,
                        text.galley.text().to_owned(),
                    ));
                }
            }
            output.drop_without_applying_deltas();
        }
        rows.sort();
        rows
    }

    #[test]
    fn the_options_shelf_order_matches_the_original_app() {
        assert_eq!(
            OPTION_SECTIONS,
            [
                "Appearance",
                "General",
                "Terminal",
                "Backup",
                "Host defaults",
                "Plugins"
            ]
        );
    }

    #[test]
    fn the_plugins_section_lists_every_plugin_with_its_description() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut settings = AppSettings {
            enabled_plugins: vec!["docker-manager".to_owned()],
            ..AppSettings::default()
        };
        let rows = painted_rows(&ctx, Vec2::new(1000.0, 3000.0), |ctx| {
            egui::Area::new(Id::unique("plugin-list-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    options_plugins(ui, &palette, &mut settings);
                });
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        assert!(has("Enable plugins"));
        assert!(has("Turn built-in plugins on or off."));
        for plugin in PluginId::ALL {
            let manifest = plugin.manifest();
            assert!(has(manifest.name), "{} should be listed", manifest.name);
            assert!(
                has(manifest.description),
                "{} should show its description",
                manifest.name
            );
        }
    }

    #[test]
    fn only_enabled_plugins_add_sections() {
        let mut settings = AppSettings::default();
        assert!(app_plugin_sections(&settings).is_empty());

        settings.enabled_plugins = vec!["sftp".to_owned(), "docker-manager".to_owned()];
        let sections = app_plugin_sections(&settings);
        // Files has no app-wide fields, so it contributes no section.
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].id, PluginId::DockerManager);
        assert_eq!(sections[0].heading, "Docker");

        settings.enabled_plugins.push("mqtt-analyser".to_owned());
        settings.enabled_plugins.push("connection-logger".to_owned());
        settings.enabled_plugins.push("macro-pad".to_owned());
        settings.enabled_plugins.push("ai-agent".to_owned());
        let sections = app_plugin_sections(&settings);
        let headings: Vec<_> = sections.iter().map(|section| section.heading).collect();
        // The original app's registration order, not the order plugins were
        // enabled in: macro-pad, mqtt-analyser, ai-agent, connection-logger,
        // docker-manager.
        assert_eq!(
            headings,
            [
                "Macro pad",
                "MQTT Analyser",
                "AI agent",
                "Connection logger",
                "Docker"
            ]
        );
        // And only the plugin that owns a settings editor shows a "Configure…" row.
        let editors: Vec<_> = sections
            .iter()
            .filter(|section| {
                section.presentation == plugin_settings::Presentation::Editor
            })
            .map(|section| section.heading)
            .collect();
        assert_eq!(editors, ["MQTT Analyser"]);
        // The AI agent draws its settings inline; only its providers get a button.
        assert!(
            sections
                .iter()
                .any(|section| section.id == PluginId::AiAgent
                    && section.presentation == plugin_settings::Presentation::Fields)
        );
    }

    #[test]
    fn host_sections_follow_the_enabled_plugins() {
        let enabled = ["server-monitor".to_owned(), "sftp".to_owned()];
        let sections = host_plugin_sections(&enabled);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "Server monitor");
        assert!(host_plugin_sections(&[]).is_empty());
    }

    #[test]
    fn a_plugin_section_paints_its_fields_and_keeps_edits() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut values = serde_json::Map::new();
        let mut secrets = PluginSecretDrafts::new();
        let rows = painted_rows(&ctx, Vec2::new(1000.0, 1200.0), |ctx| {
            egui::Area::new(Id::unique("plugin-fields-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    plugin_settings_rows(
                        ui,
                        &palette,
                        plugin_settings::host_schema(PluginId::DockerManager),
                        &mut values,
                        &mut SecretEditing {
                            id: PluginId::DockerManager,
                            secrets: &mut secrets,
                        },
                    );
                });
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        for label in [
            "Daemon socket path",
            "Show stopped containers",
            "List refresh interval (ms)",
            "Stats interval (ms)",
            "Log tail lines",
            "sudo password",
        ] {
            assert!(has(label), "{label:?} should be a Docker host setting");
        }
        // Drawing alone must not invent stored values.
        assert!(values.is_empty());
    }

    #[test]
    fn a_record_list_section_edits_its_records_in_place() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut values: serde_json::Map<String, serde_json::Value> = [(
            "macros".to_owned(),
            serde_json::json!([{"id":"m1","label":"Status","group":"Ops","text":"systemctl status"}]),
        )]
        .into_iter()
        .collect();
        let mut secrets = PluginSecretDrafts::new();
        let rows = painted_rows(&ctx, Vec2::new(1000.0, 1400.0), |ctx| {
            egui::Area::new(Id::unique("record-list-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    plugin_settings_rows(
                        ui,
                        &palette,
                        plugin_settings::app_schema(PluginId::MacroPad),
                        &mut values,
                        &mut SecretEditing {
                            id: PluginId::MacroPad,
                            secrets: &mut secrets,
                        },
                    );
                });
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        assert!(has("Buttons"));
        // The original renders macros as compact single-line rows, so the only
        // headings are the list title and the two buttons.
        assert!(has("Add button"));
        assert!(has("Remove"));
        // Drawing the editor must not rewrite the list.
        assert_eq!(
            values["macros"],
            serde_json::json!([{"id":"m1","label":"Status","group":"Ops","text":"systemctl status"}])
        );
    }

    #[test]
    fn a_new_record_gets_an_unused_id_and_the_field_defaults() {
        use wassh_plugins::settings::{item_list, text as text_field};
        const ITEM: &[SettingsField] = &[
            text_field("label", "Label", "", ""),
            text_field("topic", "Topic", "", ""),
        ];
        const LIST: &[SettingsField] =
            &[item_list("messages", "Messages", "", "message", ITEM)];
        let field = &LIST[0];
        let existing = vec![serde_json::json!({"id":"messages-1","label":"Old"})];
        let record = blank_record(field, &existing);
        assert_eq!(record["label"], serde_json::json!(""));
        assert_eq!(record["topic"], serde_json::json!(""));
        assert_eq!(record["id"], serde_json::json!("messages-2"));

        // A taken id is skipped rather than reused.
        let taken = vec![
            serde_json::json!({"id":"messages-2"}),
            serde_json::json!({"id":"messages-3"}),
        ];
        assert_eq!(blank_record(field, &taken)["id"], serde_json::json!("messages-4"));
        assert_eq!(
            blank_record(field, &[serde_json::json!({"id":"messages-3"})])["id"],
            serde_json::json!("messages-2")
        );
    }

    #[test]
    fn a_plugin_with_an_editor_opens_it_from_its_section() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut state = OptionsState::new(&AppSettings {
            enabled_plugins: vec!["mqtt-analyser".to_owned()],
            ..AppSettings::default()
        });
        // The section itself is a single "Configure…" row, like the original.
        let sections = app_plugin_sections(&state.draft);
        assert_eq!(sections[0].presentation, plugin_settings::Presentation::Editor);

        // The editor holds the saved-messages list.
        state.editor = Some(PluginId::MqttAnalyser);
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 900.0), |ctx| {
            options_plugin_editor(ctx, &palette, &mut state);
        });
        let texts: Vec<_> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&"MQTT Analyser settings"));
        assert!(texts.contains(&"Saved messages"));
        assert!(texts.contains(&"Add message"));
        // Nothing tells the user to go somewhere else.
        for punt in ["Edit in the plugin panel.", "host settings", "Host settings"] {
            assert!(
                !texts.iter().any(|text| text.contains(punt)),
                "the global panel should not mention {punt:?}"
            );
        }
        // A modal is not dismissible by drawing it.
        assert_eq!(state.editor, Some(PluginId::MqttAnalyser));
    }

    /// The plugin editor's panel rect (the widest dialog-sized rounded rect painted).
    fn editor_panel(
        ctx: &Context,
        size: Vec2,
        mut draw: impl FnMut(&Context),
    ) -> Rect {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut panel = Rect::NOTHING;
        for _ in 0..3 {
            let mut output = ctx.run_pass(input.clone(), |ctx| draw(ctx));
            let shapes = std::mem::take(&mut output.shapes);
            for clipped in shapes {
                if let egui::Shape::Rect(rect) = clipped.shape
                    && (rect.rect.width() - EDITOR_WIDTH - 2.0).abs() < 2.0
                {
                    panel = rect.rect;
                }
            }
            output.drop_without_applying_deltas();
        }
        panel
    }

    /// Settings with an AI agent section enabled, holding `count` providers.
    fn ai_agent_settings(count: usize) -> AppSettings {
        let providers: Vec<_> = (0..count)
            .map(|index| {
                serde_json::json!({
                    "id": format!("p{index}"),
                    "name": format!("Provider {index}"),
                    "protocol": "openai",
                    "baseUrl": format!("http://127.0.0.1:1143{index}/v1"),
                    "models": ["llama3.1"],
                })
            })
            .collect();
        let mut settings = AppSettings {
            enabled_plugins: vec!["ai-agent".to_owned()],
            ..AppSettings::default()
        };
        settings.plugin_settings.insert(
            "ai-agent".to_owned(),
            [(
                wassh_plugins::ai_agent::PROVIDERS_KEY.to_owned(),
                serde_json::Value::Array(providers),
            )]
            .into_iter()
            .collect(),
        );
        settings
    }

    #[test]
    fn a_short_editor_stays_compact() {
        // A plugin with little to show keeps a small dialog instead of a mostly empty one.
        let ctx = Context::default();
        let palette = Palette::dark();
        let settings = AppSettings {
            enabled_plugins: vec!["mqtt-analyser".to_owned()],
            ..AppSettings::default()
        };
        let mut state = OptionsState::new(&settings);
        state.editor = Some(PluginId::MqttAnalyser);
        let panel = editor_panel(&ctx, Vec2::new(1280.0, 1300.0), |ctx| {
            options_plugin_editor(ctx, &palette, &mut state);
        });
        assert!(
            panel.height() < 700.0,
            "the MQTT editor stretched to {} px",
            panel.height()
        );
        assert!(panel.height() > EDITOR_MIN_HEIGHT, "and it is not collapsed");
    }

    #[test]
    fn the_editor_takes_half_the_window() {
        // Half the window, less the header and footer, on a normal screen.
        assert!((editor_room(1300.0) - (650.0 - EDITOR_CHROME)).abs() < 0.01);
        assert!(editor_room(1400.0) > editor_room(900.0));
        // A short window keeps the minimum instead of collapsing.
        assert_eq!(editor_room(300.0), EDITOR_MIN_HEIGHT);
        // Never more than half the window, however tall it is.
        assert!(editor_room(2400.0) < 1200.0);
    }

    /// The AI agent's section, without the Options shell around it.
    ///
    /// `passes` exists because an `Area`'s first frame is a sizing pass that paints
    /// nothing: read with 2, send input in a single pass once the area is warm.
    fn draw_ai_agent_section(
        ctx: &Context,
        size: Vec2,
        state: &mut OptionsState,
        events: Vec<egui::Event>,
        passes: usize,
    ) -> Vec<(i32, i32, String)> {
        let mut rows = Vec::new();
        for pass in 0..passes.max(1) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                // Input belongs to the last pass, when the area is laid out.
                events: if pass + 1 == passes { events.clone() } else { Vec::new() },
                ..Default::default()
            };
            rows = run_section_pass(ctx, state, input);
        }
        rows
    }

    fn run_section_pass(
        ctx: &Context,
        state: &mut OptionsState,
        input: egui::RawInput,
    ) -> Vec<(i32, i32, String)> {
        let palette = Palette::dark();
        let section = PluginSection {
            id: PluginId::AiAgent,
            heading: "AI agent",
            schema: plugin_settings::app_schema(PluginId::AiAgent),
            presentation: plugin_settings::Presentation::Fields,
        };
        let mut output = ctx.run_pass(input, |ctx| {
            egui::Area::new(Id::unique("ai-agent-section-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    let mut values = state
                        .draft
                        .plugin_settings
                        .remove(PluginId::AiAgent.as_str())
                        .unwrap_or_default();
                    plugin_section_rows(
                        ui,
                        &palette,
                        &section,
                        &mut values,
                        &mut state.ai_providers,
                        &mut state.plugin_secrets,
                    );
                    state
                        .draft
                        .plugin_settings
                        .insert(PluginId::AiAgent.as_str().to_owned(), values);
                });
        });
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut rows: Vec<_> = shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.pos.y as i32,
                    text.pos.x as i32,
                    text.galley.text().to_owned(),
                )),
                _ => None,
            })
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn the_ai_agent_section_holds_its_providers_and_settings() {
        // Both live in the global dialog: the provider list and every agent setting are
        // rows of the section, so nothing needs a "Configure…" step.
        let ctx = Context::default();
        let size = Vec2::new(1000.0, 2000.0);
        let mut state = OptionsState::new(&ai_agent_settings(1));
        let rows = draw_ai_agent_section(&ctx, size, &mut state, Vec::new(), 2);
        let texts: Vec<_> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        // The provider list.
        assert!(texts.contains(&"Providers"));
        assert!(texts.contains(&"Fetch models"));
        assert!(texts.contains(&"+ Add provider"));
        assert!(texts.contains(&"Provider 0"), "the configured provider's name");
        // And every setting of the plugin.
        for label in [
            "Global prompt text",
            "Global knowledge",
            "Default allow rules",
            "Default deny rules",
            "Web search",
        ] {
            assert!(texts.contains(&label), "{label:?} should be in the section");
        }
        // Nothing points at a separate editor any more.
        assert!(!texts.contains(&"Configure…"));
    }

    /// The Options dialog with the AI agent section scrolled to the top, settled.
    fn settled_options(
        ctx: &Context,
        dialogs: &mut Dialogs,
        size: Vec2,
    ) -> Vec<(i32, i32, String)> {
        let mut rows = Vec::new();
        for step in 0..12 {
            if let Some(state) = dialogs.options.as_mut() {
                state.nav.scroll_to = Some(OPTION_SECTIONS.len());
            }
            rows = run_options(ctx, dialogs, size, f64::from(step) * 0.05, Vec::new());
        }
        rows
    }

    /// Click a painted row and return the frame that draws the result.
    fn click_options(
        ctx: &Context,
        dialogs: &mut Dialogs,
        size: Vec2,
        row: &(i32, i32, String),
    ) -> Vec<(i32, i32, String)> {
        let pos = Pos2::new(row.1 as f32 + 4.0, row.0 as f32 + 4.0);
        let click = [
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ];
        run_options(ctx, dialogs, size, 20.0, click.to_vec());
        // The dialog's area is sized on its first frame, so read the next one.
        run_options(ctx, dialogs, size, 20.05, Vec::new())
    }

    #[test]
    fn the_provider_dialog_is_one_of_the_apps_dialogs() {
        // Adding or editing a provider opens the same kind of dialog as everything else
        // here: the app draws its frame, its buttons and Escape.
        let ctx = Context::default();
        let size = Vec2::new(1280.0, 1300.0);
        let mut dialogs = Dialogs {
            options: Some(OptionsState::new(&ai_agent_settings(0))),
            ..Dialogs::default()
        };
        let rows = settled_options(&ctx, &mut dialogs, size);
        let add = rows
            .iter()
            .find(|(_, _, text)| text == "+ Add provider")
            .cloned()
            .expect("the provider list offers an add button");
        let rows = click_options(&ctx, &mut dialogs, size, &add);
        assert!(
            dialogs
                .options
                .as_ref()
                .is_some_and(|state| state.ai_providers.dialog_open()),
            "the draft is open"
        );
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&"Add a provider"), "the dialog is drawn");
        assert!(texts.contains(&"API key"), "with the plugin's fields");
        assert!(texts.contains(&"Providers"), "and the list stays put");

        // Escape closes the dialog on top, not the settings under it.
        let escape = [egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        run_options(&ctx, &mut dialogs, size, 30.0, escape.to_vec());
        let state = dialogs.options.as_ref().expect("Options stays open");
        assert!(!state.ai_providers.dialog_open(), "Escape closed the draft");
    }

    /// One Options dialog frame with a clock (the settings scroll area animates its
    /// offset, so a frozen clock never settles) and optional input events.
    fn run_options(
        ctx: &Context,
        dialogs: &mut Dialogs,
        size: Vec2,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<(i32, i32, String)> {
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_pass(input, |ctx| {
            let _ = dialogs.show(ctx, &palette, &[], 0);
        });
        let shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
        let mut rows: Vec<_> = shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.pos.y as i32,
                    text.pos.x as i32,
                    text.galley.text().to_owned(),
                )),
                _ => None,
            })
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn the_ai_agent_providers_are_inside_the_options_dialog() {
        // The list used to sit behind a "Configure…" row; it is part of the section now, so
        // there is no step between the Options dialog and the providers.
        let ctx = Context::default();
        let size = Vec2::new(1280.0, 1300.0);
        let mut dialogs = Dialogs {
            options: Some(OptionsState::new(&ai_agent_settings(1))),
            ..Dialogs::default()
        };
        // Bring the section to the top of the viewport and let the animated scroll settle.
        let mut rows = Vec::new();
        for step in 0..12 {
            if let Some(state) = dialogs.options.as_mut() {
                state.nav.scroll_to = Some(OPTION_SECTIONS.len());
            }
            rows = run_options(&ctx, &mut dialogs, size, f64::from(step) * 0.05, Vec::new());
        }
        let texts: Vec<_> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&"Providers"), "the provider list is inline");
        assert!(texts.contains(&"Fetch models"));
        assert!(texts.contains(&"+ Add provider"));
        assert!(
            !texts.contains(&"Configure…"),
            "nothing has to be opened to see the providers"
        );
        assert_eq!(
            dialogs.options.as_ref().and_then(|state| state.editor),
            None,
            "no plugin editor is involved"
        );
    }

    #[test]
    fn a_plugin_section_shows_the_stored_value() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut values: serde_json::Map<String, serde_json::Value> =
            [("socketPath".to_owned(), serde_json::json!("/run/docker.sock"))]
                .into_iter()
                .collect();
        let mut secrets = PluginSecretDrafts::new();
        let rows = painted_rows(&ctx, Vec2::new(1000.0, 1200.0), |ctx| {
            egui::Area::new(Id::unique("plugin-value-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    plugin_settings_rows(
                        ui,
                        &palette,
                        plugin_settings::host_schema(PluginId::DockerManager),
                        &mut values,
                        &mut SecretEditing {
                            id: PluginId::DockerManager,
                            secrets: &mut secrets,
                        },
                    );
                });
        });
        // The input paints its text as shapes, not as a Text shape, so assert on
        // the value surviving the draw instead.
        assert_eq!(values["socketPath"], serde_json::json!("/run/docker.sock"));
        assert!(rows.iter().any(|(_, _, text)| text == "Daemon socket path"));
    }


    #[test]
    fn a_plugin_password_row_is_masked_and_kept_out_of_the_settings_map() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut values = serde_json::Map::new();
        let mut secrets = PluginSecretDrafts::new();
        secrets.insert(
            secret_field_key("mqtt-analyser", "password"),
            SecretDraft::stored(),
        );
        let rows = painted_rows(&ctx, Vec2::new(1000.0, 1200.0), |ctx| {
            egui::Area::new(Id::unique("plugin-secret-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    plugin_settings_rows(
                        ui,
                        &palette,
                        plugin_settings::host_schema(PluginId::MqttAnalyser),
                        &mut values,
                        &mut SecretEditing {
                            id: PluginId::MqttAnalyser,
                            secrets: &mut secrets,
                        },
                    );
                });
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        assert!(has("MQTT password"), "password row missing: {rows:?}");
        // A stored secret is reported, never shown.
        assert!(has("(stored)"), "stored marker missing: {rows:?}");
        assert!(has("Forget stored value"), "clear toggle missing: {rows:?}");
        // The plain-text value never reaches the settings map, so nothing can
        // write it into hosts.json.
        assert!(
            !values.contains_key("password"),
            "secret leaked into the settings map: {values:?}"
        );
    }

    #[test]
    fn the_settings_gear_lands_on_the_plugins_own_section() {
        let settings = AppSettings {
            enabled_plugins: vec!["docker-manager".to_owned(), "macro-pad".to_owned()],
            ..AppSettings::default()
        };
        // Macro pad is registered before Docker, so it is the first plugin section.
        let state = OptionsState::new(&settings).focusing_plugin(PluginId::MacroPad);
        assert_eq!(state.nav.active, OPTION_SECTIONS.len());
        assert_eq!(state.nav.scroll_to, Some(OPTION_SECTIONS.len()));

        let state = OptionsState::new(&settings).focusing_plugin(PluginId::DockerManager);
        assert_eq!(state.nav.active, OPTION_SECTIONS.len() + 1);

        // A plugin with no settings section falls back to the switch list.
        let state = OptionsState::new(&settings).focusing_plugin(PluginId::Sftp);
        assert_eq!(state.nav.active, OPTION_SECTIONS.len() - 1);
        assert_eq!(OPTION_SECTIONS[OPTION_SECTIONS.len() - 1], "Plugins");
    }

    #[test]
    fn options_lists_a_section_for_each_enabled_plugin() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut state = OptionsState::new(&AppSettings {
            enabled_plugins: vec!["docker-manager".to_owned()],
            ..AppSettings::default()
        });
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 900.0), |ctx| {
            let actions = options_dialog(ctx, &palette, &mut state);
            assert!(actions.is_empty(), "drawing commits nothing");
        });
        // The rail grows an entry for the plugin; the fields of a section far
        // down the column are covered by the field-list test, since a headless
        // frame clips text below the ScrollArea fold.
        assert!(rows.iter().any(|(_, _, text)| text == "Docker"));
        assert!(rows.iter().any(|(_, _, text)| text == "Plugins"));

        let mut plain = OptionsState::new(&AppSettings::default());
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 900.0), |ctx| {
            let _ = options_dialog(ctx, &palette, &mut plain);
        });
        assert!(!rows.iter().any(|(_, _, text)| text == "Docker"));
        assert!(rows.iter().any(|(_, _, text)| text == "Plugins"));
    }

    #[test]
    fn the_host_dialog_gains_a_section_per_enabled_plugin() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut with_plugin = Dialogs {
            host: Some(HostState::draft(HostProfile::default(), true)),
            enabled_plugins: vec!["docker-manager".to_owned()],
            ..Dialogs::default()
        };
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 900.0), |ctx| {
            let actions = with_plugin.show(ctx, &palette, &[], 0);
            assert!(actions.is_empty(), "drawing a dialog commits nothing");
        });
        // The rail lists the built-in sections plus the plugin's.
        assert!(rows.iter().any(|(_, _, text)| text == "Docker"));

        let mut without_plugin = Dialogs {
            host: Some(HostState::draft(HostProfile::default(), true)),
            ..Dialogs::default()
        };
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 900.0), |ctx| {
            let _ = without_plugin.show(ctx, &palette, &[], 0);
        });
        assert!(!rows.iter().any(|(_, _, text)| text == "Docker"));
    }

    #[test]
    fn the_host_defaults_section_holds_the_style_defaults() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut draft = AppSettings::default();
        let (mut tab, mut background, mut foreground) =
            (String::new(), String::new(), String::new());
        let rows = painted_rows(&ctx, Vec2::new(1280.0, 1600.0), |ctx| {
            egui::Area::new(Id::unique("host-defaults-probe"))
                .fixed_pos(Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(700.0);
                    options_host_defaults(
                        ui,
                        &palette,
                        &mut draft,
                        &mut DefaultColourDrafts {
                            tab: &mut tab,
                            background: &mut background,
                            foreground: &mut foreground,
                        },
                    );
                });
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        for label in [
            "Tab colour",
            "Terminal background",
            "Terminal text",
            "Font family",
            "Font size",
            "Scrollback lines",
            "Bell",
            "Cursor",
            "Cursor blink",
        ] {
            assert!(has(label), "{label:?} should be a host default");
        }
        // TERM is a session setting, not a default.
        assert!(!has("TERM"));
    }

    #[test]
    fn the_edit_group_dialog_holds_the_name_and_the_settings_picker() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut state = GroupEditState {
            group_id: "g1".to_owned(),
            original_name: "Ops".to_owned(),
            name: "Ops".to_owned(),
            color: Some(Rgba::rgb(0x3d8bfd)),
            hex: "#3d8bfd".to_owned(),
        };
        let rows = painted_rows(&ctx, Vec2::new(900.0, 700.0), |ctx| {
            let actions = group_edit_dialog(ctx, &palette, &mut state);
            assert!(actions.is_empty(), "drawing commits nothing");
        });
        let has = |needle: &str| rows.iter().any(|(_, _, text)| text == needle);
        assert!(has("Edit group"), "rows: {rows:?}");
        // The same name row and colour picker as Host settings.
        assert!(has("Name"));
        assert!(has("Colour"));
        assert!(has("No colour"));
        assert!(has("Cancel"));
        assert!(has("OK"));
        assert_eq!(state.hex, "#3d8bfd");
        assert_eq!(state.name, "Ops");
    }

    #[test]
    fn the_group_edit_state_starts_from_the_saved_group() {
        let group = wassh_core::HostGroup {
            id: "g1".to_owned(),
            name: "Ops".to_owned(),
            color: Some(Rgba::rgb(0x16a34a)),
            collapsed: false,
        };
        let state = GroupEditState::new(&group);
        assert_eq!(state.color, Some(Rgba::rgb(0x16a34a)));
        assert_eq!(state.hex, "#16a34a");
        assert_eq!(state.name, "Ops");
        assert_eq!(state.committed_name(), "Ops");

        // An emptied name falls back to the stored one instead of blanking it.
        let blank = GroupEditState {
            name: "   ".to_owned(),
            ..GroupEditState::new(&group)
        };
        assert_eq!(blank.committed_name(), "Ops");
        // Surrounding whitespace never reaches hosts.json.
        let padded = GroupEditState {
            name: "  Ops 2  ".to_owned(),
            ..GroupEditState::new(&group)
        };
        assert_eq!(padded.committed_name(), "Ops 2");

        let plain = GroupEditState::new(&wassh_core::HostGroup {
            color: None,
            ..group
        });
        assert_eq!(plain.hex, "");
        assert_eq!(
            plain.shown_color(),
            wassh_core::colors::GROUP_COLOR_FALLBACK,
            "an uncoloured group starts from the default swatch"
        );
    }

    #[test]
    fn the_host_dialog_lands_inside_the_window() {
        let ctx = Context::default();
        let mut dialogs = Dialogs {
            host: Some(HostState::draft(wassh_core::HostProfile::default(), true)),
            ..Dialogs::default()
        };
        let screen = Vec2::new(1280.0, 800.0);
        let _ = frame_with_screen(&ctx, &mut dialogs, screen);
        let _ = frame_with_screen(&ctx, &mut dialogs, screen);
        let id = Id::unique(("wassh-settings", "host-settings"));
        let rect = ctx.memory(|memory| memory.area_rect(id));
        let rect = rect.expect("the dialog area exists");
        let screen_rect = Rect::from_min_size(Pos2::ZERO, screen);
        assert!(
            screen_rect.contains_rect(rect),
            "dialog rect {rect:?} is outside the window {screen_rect:?}"
        );
        assert!(dialogs.host.is_some(), "still open after drawing");
    }

    #[test]
    fn the_default_checkbox_restores_and_releases_the_override() {
        // Ticking "Use default" drops the override so the app default applies again.
        let mut size = Some(20u16);
        apply_toggle(&mut size, 14, DefaultToggle::On);
        assert_eq!(size, None);

        // Unticking it pins the value that was being shown.
        apply_toggle(&mut size, 14, DefaultToggle::Off);
        assert_eq!(size, Some(14));

        // A changed control keeps its own value.
        let mut size = Some(18u16);
        apply_toggle(&mut size, 14, DefaultToggle::None);
        assert_eq!(size, Some(18));
    }

    #[test]
    fn appearance_drafts_show_the_app_defaults_until_overridden() {
        let defaults = SessionStyleDefaults {
            tab_color: Some(Rgba::rgb(0x3d8bfd)),
            term_background: Some(Rgba::rgb(0x101010)),
            term_foreground: Some(Rgba::rgb(0xf0f0f0)),
            font_size: 12,
            ..SessionStyleDefaults::default()
        };
        let mut host = HostProfile::default();
        let state = HostState::draft(host.clone(), false).with_defaults(&defaults);
        assert_eq!(state.tab_color_draft, Rgba::rgb(0x3d8bfd).to_hex());
        assert_eq!(state.term_background_draft, Rgba::rgb(0x101010).to_hex());
        assert_eq!(state.term_foreground_draft, Rgba::rgb(0xf0f0f0).to_hex());
        assert_eq!(state.defaults.font_size, 12);

        // An override keeps its own colour in the field.
        host.connection.style.tab_color = Some(Rgba::rgb(0xe35d6a));
        let state = HostState::draft(host, false).with_defaults(&defaults);
        assert_eq!(state.tab_color_draft, Rgba::rgb(0xe35d6a).to_hex());
    }

    #[test]
    fn colours_resolve_to_the_session_palette_when_nothing_is_set() {
        let palette = Palette::dark();
        assert_eq!(resolved_colour(None, Some(palette.bg_term)), palette.bg_term);
        let custom = Rgba::rgb(0x123456);
        assert_eq!(resolved_colour(Some(custom), Some(palette.bg_term)), custom);
        assert_eq!(
            resolved_colour(None, None),
            colors::GROUP_COLOR_FALLBACK
        );
    }

    #[test]
    fn the_appearance_section_paints_every_row() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let defaults = SessionStyleDefaults::default();
        let mut host = HostProfile::default();
        let mut tab = String::new();
        let mut background = String::new();
        let mut foreground = String::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))),
            ..Default::default()
        };
        let mut painted = 0;
        for _ in 0..2 {
            let output = ctx.run_ui(input.clone(), |ui| {
                host_appearance(
                    ui,
                    &palette,
                    &mut AppearanceFields {
                        style: &mut host.connection.style,
                        tab_color: &mut tab,
                        term_background: &mut background,
                        term_foreground: &mut foreground,
                        defaults: &defaults,
                    },
                );
            });
            painted = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        assert!(painted > 40, "the section painted only {painted} shapes");
        assert_eq!(tab, colors::GROUP_COLOR_FALLBACK.to_hex());
        assert!(host.connection.style.is_empty(), "nothing overridden yet");
    }

    #[test]
    fn secret_edits_follow_the_dialog_field() {
        assert_eq!(SecretEdit::from_field("", false), SecretEdit::Unchanged);
        assert_eq!(
            SecretEdit::from_field("hunter2", false),
            SecretEdit::Set("hunter2".to_owned())
        );
        assert_eq!(SecretEdit::from_field("", true), SecretEdit::Clear);
        // "Clear" wins over anything typed.
        assert_eq!(SecretEdit::from_field("hunter2", true), SecretEdit::Clear);
    }

    #[test]
    fn a_host_dialog_starts_without_secrets_but_can_be_told_about_them() {
        let host = HostProfile {
            connection: ConnectionParams {
                auth: AuthMethod::Password,
                ..ConnectionParams::default()
            },
            ..HostProfile::default()
        };
        let state = HostState::draft(host.clone(), false);
        assert!(!state.stored_password);
        assert!(state.password.is_empty());

        let state = state.with_stored_secrets(true, false);
        assert!(state.stored_password);
        assert!(!state.stored_passphrase);
    }

    #[test]
    fn the_password_field_paints_for_password_auth() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut state = HostState::draft(
            HostProfile {
                connection: ConnectionParams {
                    auth: AuthMethod::Password,
                    ..ConnectionParams::default()
                },
                ..HostProfile::default()
            },
            false,
        )
        .with_stored_secrets(true, false);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
            ..Default::default()
        };
        let mut painted = 0;
        for _ in 0..2 {
            let output = ctx.run_ui(input.clone(), |ui| {
                host_authentication(
                    ui,
                    &palette,
                    &mut AuthFields {
                        draft: &mut state.draft,
                        password: &mut state.password,
                        passphrase: &mut state.passphrase,
                        stored_password: state.stored_password,
                        stored_passphrase: state.stored_passphrase,
                        clear_password: &mut state.clear_password,
                        clear_passphrase: &mut state.clear_passphrase,
                    },
                );
            });
            painted = output.shapes.len();
            output.drop_without_applying_deltas();
        }
        assert!(painted > 5, "the section painted {painted} shapes");
    }

    #[test]
    fn rows_line_up_their_controls() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut starts = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let mut short = String::new();
            let mut long = String::new();
            row(ui, &palette, "Port", None, |ui| {
                starts.push(text_input(ui, &palette, &mut short, "22").rect.left());
            });
            row(
                ui,
                &palette,
                "A considerably longer label",
                Some("With a description under it"),
                |ui| {
                    starts.push(text_input(ui, &palette, &mut long, "22").rect.left());
                },
            );
        });
        output.drop_without_applying_deltas();
        assert_eq!(starts.len(), 2);
        assert!(
            (starts[0] - starts[1]).abs() < 0.5,
            "controls start at different x positions: {starts:?}"
        );
    }

    #[test]
    fn the_host_dialog_paints_a_window() {
        let mut dialogs = Dialogs {
            host: Some(HostState::draft(wassh_core::HostProfile::default(), true)),
            ..Dialogs::default()
        };
        let ctx = Context::default();
        let _ = run_frame(&ctx, &mut dialogs);
        let painted = run_frame(&ctx, &mut dialogs);
        assert!(
            painted > 10,
            "the dialog painted only {painted} shapes; expected the backdrop, panel, rail and footer"
        );
        assert!(dialogs.host.is_some(), "the dialog stays open");
    }

    #[test]
    fn the_tags_row_paints_one_chip_per_tag() {
        let host = HostProfile {
            id: "h1".to_owned(),
            name: "Prod".to_owned(),
            tags: vec!["work".to_owned(), "db".to_owned()],
            ..HostProfile::default()
        };
        let mut dialogs = Dialogs {
            host: Some(HostState::draft(host, false)),
            ..Dialogs::default()
        };
        let ctx = Context::default();
        let rows = painted_rows(&ctx, Vec2::new(900.0, 700.0), |ctx| {
            dialogs.show(ctx, &Palette::dark(), &[], 0);
        });
        let texts: Vec<&str> = rows.iter().map(|(_, _, text)| text.as_str()).collect();
        assert!(texts.contains(&"work"), "chip missing: {texts:?}");
        assert!(texts.contains(&"db"), "chip missing: {texts:?}");
        // A plain comma-separated field would paint the joined string instead.
        assert!(
            !texts.iter().any(|text| text.contains("work, db")),
            "the row is still a comma-separated text field: {texts:?}"
        );
        // The chips' ✕ on top of the dialog's own close button.
        assert!(
            texts.iter().filter(|text| **text == "✕").count() >= 3,
            "each chip needs a remove button: {texts:?}"
        );
    }

    #[test]
    fn palette_filters_by_substring() {
        let mut state = PaletteState::new();
        assert_eq!(state.matches().len(), Command::ALL.len());
        state.query = "tab".to_owned();
        let labels: Vec<_> = state.matches().iter().map(|(_, label)| *label).collect();
        assert!(labels.iter().any(|label| label.contains("local terminal")));
        assert!(!labels.iter().any(|label| label.contains("About")));
    }

    #[test]
    fn command_labels_round_trip() {
        for (command, label) in Command::ALL {
            assert_eq!(command.label(), label);
        }
    }

    #[test]
    fn host_state_prefills_the_colour_draft_the_tags_stay_in_the_draft() {
        let host = HostProfile {
            id: "h1".to_owned(),
            name: "Prod".to_owned(),
            tags: vec!["work".to_owned(), "db".to_owned()],
            connection: ConnectionParams {
                style: SessionStyle {
                    tab_color: Some(colors::GROUP_COLOR_FALLBACK),
                    ..SessionStyle::default()
                },
                ..ConnectionParams::default()
            },
            ..HostProfile::default()
        };
        let state = HostState::draft(host.clone(), false);
        assert_eq!(state.tab_color_draft, colors::GROUP_COLOR_FALLBACK.to_hex());
        // The tags live in the draft; the field starts empty, like the reference.
        assert_eq!(state.draft.tags, ["work", "db"]);
        assert!(state.tag_draft.is_empty());
        assert!(state.saved);
    }

    #[test]
    fn only_a_saved_host_can_be_applied_or_deleted() {
        let host = HostProfile {
            id: "host-9".to_owned(),
            ..HostProfile::default()
        };
        // A unsaved session, even with a tab open, has nothing to apply to.
        let session = HostState::draft(host.clone(), true)
            .with_open_sessions(true)
            .with_saved(false);
        assert!(!session.can_apply_to_sessions());
        assert!(!session.can_delete());

        // A saved host with no sessions cannot be applied either.
        let saved_idle = HostState::draft(host.clone(), false)
            .with_open_sessions(false)
            .with_saved(true);
        assert!(!saved_idle.can_apply_to_sessions());
        assert!(saved_idle.can_delete());

        // A saved host with sessions open offers both.
        let saved_live = HostState::draft(host.clone(), false)
            .with_open_sessions(true)
            .with_saved(true);
        assert!(saved_live.can_apply_to_sessions());
        assert!(saved_live.can_delete());

        // The session dialog for that same host offers neither: Save goes to its tab.
        let session_dialog = HostState::draft(host, false)
            .with_open_sessions(true)
            .with_saved(true)
            .with_target(HostTarget::Session("tab-1".to_owned()));
        assert!(!session_dialog.can_apply_to_sessions());
        assert!(!session_dialog.can_delete());
    }

    #[test]
    fn the_tab_menu_can_save_a_session_as_a_host() {
        assert_eq!(tab_menu_command(2), Some(Command::SaveAsHost));
    }

    #[test]
    fn sections_follow_the_connection_type() {
        assert_eq!(
            host_sections(ConnectionType::Ssh),
            vec!["Connection", "Authentication", "Advanced", "Tunnels", "Appearance"]
        );
        assert_eq!(
            host_sections(ConnectionType::Serial),
            vec!["Connection", "Appearance"]
        );
        assert_eq!(
            host_sections(ConnectionType::Local),
            vec!["Connection", "Appearance"]
        );
    }

    #[test]
    fn nav_jump_sets_both_active_and_scroll() {
        let mut nav = SettingsNav::default();
        nav.jump_to(3);
        assert_eq!(nav.active, 3);
        assert_eq!(nav.scroll_to, Some(3));
    }

    #[test]
    fn remote_session_defaults_are_off() {
        assert!(!RemoteSession::default().enabled);
    }
}

#[cfg(test)]
mod layout_probe {
    use super::*;
    use wassh_core::ConnectionParams;

    #[test]
    fn dump_host_dialog() {
        let ctx = Context::default();
        let palette = Palette::dark();
        let mut state = HostState::draft(
            HostProfile {
                name: "Dev".to_owned(),
                connection: ConnectionParams {
                    kind: ConnectionType::Ssh,
                    host: "192.168.42.7".to_owned(),
                    username: "brainchild".to_owned(),
                    auth: AuthMethod::Password,
                    ..ConnectionParams::default()
                },
                ..HostProfile::default()
            },
            false,
        );
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            ..Default::default()
        };
        let mut lines = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_pass(input.clone(), |ctx| {
                host_dialog(ctx, &palette, &mut state, &[], &["docker-manager".to_owned()]);
            });
            lines.clear();
            let shapes = std::mem::take(&mut output.shapes);
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    let pos = text.pos;
                    lines.push((pos.y as i32, pos.x as i32, text.galley.text().to_owned()));
                }
            }
            output.drop_without_applying_deltas();
        }
        lines.sort();
        for (y, x, text) in lines {
            println!("PROBE y={y:4} x={x:4} {text}");
        }
    }
}

