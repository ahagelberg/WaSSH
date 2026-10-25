//! The Files plugin: a remote directory browser over SFTP.
//!
//! The pane mirrors the reference file manager: a connection status bar, an
//! action toolbar, breadcrumbs, a sortable table with mode and date columns,
//! multi-selection, a row context menu and the dialogs that change the remote
//! filesystem (new folder, rename, permissions, delete).

use egui::Ui;
use wassh_core::colors;
use wassh_core::{Palette, SessionStatus};
use wassh_ui::dock::HeaderHealth;
use wassh_ui::widgets::{
    Button, ButtonKind, MenuItem, context_menu, faded, footer, modal, small_button,
    small_button_enabled, text_input,
};

use crate::color::to_color32;
use crate::plugin::{Plugin, PluginContext, SessionAccess};
use crate::{PluginId, PluginManifest};

/// One row of the remote directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpEntry {
    /// File name without its directory.
    pub name: String,
    /// Full remote path.
    pub path: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether this entry is a symbolic link.
    pub is_link: bool,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in seconds since the Unix epoch.
    pub modified: Option<u64>,
    /// POSIX permission bits, when the server reported them.
    pub permissions: Option<u32>,
}

/// A finished directory listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SftpListing {
    /// Directory that was listed.
    pub path: String,
    /// Its entries.
    pub entries: Vec<SftpEntry>,
}

/// Remote file access handed to the plugin by the session.
pub trait SftpAccess {
    /// Ask for `path` to be listed. Cheap to call every frame: repeats are ignored once a
    /// request for the same path has been made.
    fn request_listing(&mut self, path: &str);

    /// List `path` again, even if it was already listed.
    fn refresh(&mut self, path: &str);

    /// The most recent completed listing, if any.
    fn listing(&self) -> Option<&SftpListing>;

    /// Whether a listing is in flight.
    fn is_loading(&self) -> bool;

    /// The last error, if the previous request failed.
    fn error(&self) -> Option<&str>;

    /// Copy the local file at `local` to the remote file at `remote`.
    fn upload(&mut self, local: &str, remote: &str);

    /// Copy the remote file at `remote` to the local file at `local`.
    fn download(&mut self, remote: &str, local: &str);

    /// Whether a transfer is in flight.
    fn is_transferring(&self) -> bool;

    /// The result line of the last finished transfer.
    fn transfer_status(&self) -> Option<&str>;

    /// Record a problem the browser found before it could start a request.
    fn report_problem(&mut self, message: &str);

    /// Resolve `path` to an absolute one — `"."` is the login directory.
    ///
    /// Starts the request and returns the answer once it has arrived, so a pane can call
    /// it every frame.
    fn resolve(&mut self, path: &str) -> Option<String>;

    /// Create a directory at `path`.
    fn mkdir(&mut self, path: &str);

    /// Rename `from` to `to`.
    fn rename(&mut self, from: &str, to: &str);

    /// Delete `path`, and everything under it when it is a directory.
    fn remove(&mut self, path: &str, is_dir: bool);

    /// Change the mode bits of `path`.
    fn chmod(&mut self, path: &str, mode: u32);

    /// Start reading the first `max_bytes` of `path`, answering on a later frame.
    fn read(&mut self, path: &str, max_bytes: u64) -> Option<FileContent>;

    /// Upload several local files in order.
    fn upload_many(&mut self, items: Vec<(String, String)>);

    /// Download several remote files in order.
    fn download_many(&mut self, items: Vec<(String, String)>);

    /// Copy remote files to other remote paths, or move them when `move_items`.
    fn copy_many(&mut self, items: Vec<(String, String)>, move_items: bool);

    /// How far the request in flight has got.
    fn progress(&self) -> Option<(u64, Option<u64>)>;

    /// Fetch each path (file or whole directory) into `dir` on this machine.
    fn download_tree(&mut self, items: Vec<String>, dir: &str);

    /// Fetch `items` into a temporary directory and write `archive` from it.
    fn download_zip(&mut self, items: Vec<String>, archive: &str);

    /// The transfer list: finished, running, then queued.
    fn transfers(&self) -> Vec<TransferRow>;

    /// Run the failed transfer again, continuing where it stopped.
    fn retry(&mut self);
}

/// A file prefix, as the viewer shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileContent {
    /// Bytes that were read.
    pub data: Vec<u8>,
    /// Whether the file continues past them.
    pub truncated: bool,
}

/// How a queued transfer is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferState {
    /// Waiting for the one before it.
    #[default]
    Queued,
    /// Moving bytes right now.
    Running,
    /// Finished.
    Done,
    /// Stopped with an error; it can be resumed.
    Failed,
}

/// Which way a transfer moves bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferDirection {
    /// Local file to the remote folder.
    #[default]
    Upload,
    /// Remote file to the local disk.
    Download,
    /// Remote tree into a local archive.
    Zip,
    /// Remote file to another remote path.
    Copy,
}

impl TransferDirection {
    /// The label the transfer list shows, as the reference writes it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Upload => "⬆ Upload",
            Self::Download => "⬇ Download",
            Self::Zip => "⬇ Download ZIP",
            Self::Copy => "⇄ Copy",
        }
    }
}

/// One line of the transfer list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferRow {
    /// Which way the bytes move.
    pub direction: TransferDirection,
    /// Remote path the transfer is about.
    pub path: String,
    /// Bytes moved so far.
    pub done: u64,
    /// Bytes in total, when known.
    pub total: Option<u64>,
    /// Where it is in its life.
    pub state: TransferState,
}

impl TransferRow {
    /// Completion from 0 to 1, when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        match self.total {
            Some(total) if total > 0 => Some((self.done as f64 / total as f64).clamp(0.0, 1.0) as f32),
            _ => None,
        }
    }
}

/// One entry of a ZIP archive, read from its central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    /// Path inside the archive.
    pub name: String,
    /// Uncompressed size.
    pub size: u64,
    /// Whether it is a directory entry.
    pub is_dir: bool,
}

/// Whether `name` looks like an archive the viewer can list.
pub fn is_zip(name: &str) -> bool {
    name.to_lowercase().ends_with(".zip")
}

/// Entry names of a ZIP archive.
///
/// The central directory is plain, so this needs no decompression and works for
/// any ZIP (deflated entries included). ZIP64 archives are refused rather than
/// mis-read.
pub fn zip_entries(bytes: &[u8]) -> Option<Vec<ZipEntry>> {
    const EOCD: &[u8] = b"PK\x05\x06";
    const CENTRAL: &[u8] = b"PK\x01\x02";
    if bytes.len() < 22 {
        return None;
    }
    // The end-of-central-directory record sits in the last 64 KB.
    let window = bytes.len().saturating_sub(65_557);
    let eocd = bytes[window..]
        .windows(4)
        .rposition(|window| window == EOCD)
        .map(|offset| window + offset)?;
    let read = |at: usize, width: usize| -> Option<u64> {
        let slice = bytes.get(at..at + width)?;
        let mut value = 0u64;
        for byte in slice.iter().rev() {
            value = (value << 8) | u64::from(*byte);
        }
        Some(value)
    };
    let count = read(eocd + 10, 2)?;
    let directory_at = read(eocd + 16, 4)? as usize;
    if read(eocd + 12, 4)? as usize == 0 && count == 0xffff {
        // ZIP64: the real offsets live in an extra record.
        return None;
    }

    let mut entries = Vec::new();
    let mut at = directory_at;
    for _ in 0..count {
        if bytes.get(at..at + 4)? != CENTRAL {
            break;
        }
        let size = read(at + 24, 4)?;
        let name_len = read(at + 28, 2)? as usize;
        let extra_len = read(at + 30, 2)? as usize;
        let comment_len = read(at + 32, 2)? as usize;
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + name_len)?).into_owned();
        entries.push(ZipEntry {
            is_dir: name.ends_with('/'),
            name,
            size,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Some(entries)
}

/// How much of a file the viewer pulls down.
pub const VIEW_BYTES: u64 = 256 * 1024;
/// Archives are read whole (the central directory sits at the end), so they get
/// a far bigger cap than a text preview.
pub const ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
/// Files up to this many lines are drawn without a scroll view.
const VIEW_INLINE_LINES: usize = 30;

/// Settings key for the directory the save dialog starts in.
const SAVE_DIR_KEY: &str = "saveDir";

/// A dialog that changes the remote filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FileDialog {
    NewFolder {
        name: String,
    },
    Rename {
        path: String,
        name: String,
    },
    Permissions {
        path: String,
        mode: String,
    },
    Delete {
        /// Path and whether it is a directory.
        targets: Vec<(String, bool)>,
    },
}

/// The file viewer's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    /// Remote path being shown.
    pub path: String,
    /// Name shown in the title.
    pub name: String,
    /// Size the file reports, so the header can say "text · 1.0 KB of 4.0 KB".
    pub total: Option<u64>,
    /// Whether the hex dump is shown instead of the text.
    pub hex: bool,
}

/// What a click in the pane asked for.
enum Intent {
    Navigate(String),
    Refresh,
    /// Choose local files to send into the folder on screen.
    PickUpload,
    Download,
    View(String),
    Copy,
    Paste,
    Zip,
}

/// What a context-menu item does.
#[derive(Clone, Debug)]
enum MenuAction {
    Open(String),
    View(String),
    Copy,
    Download,
    /// ZIP of the selection, or of the row the menu was opened on.
    ZipSelection,
    Rename,
    Permissions,
    Delete,
    Paste,
    NewFolder,
    Upload,
    /// ZIP of the folder on screen.
    ZipFolder,
    Refresh,
}

/// Which OS dialog is open.
#[derive(Clone, Debug)]
enum NativeDialog {
    /// `⬆ Upload…`: pick local files to send into the folder on screen.
    Upload,
    /// Save one remote file.
    Save {
        /// Remote path being saved.
        remote: String,
    },
    /// Save the archive being built.
    Archive {
        /// Remote paths that go into it.
        items: Vec<String>,
    },
}

/// What a dialog thread answered.
struct Answer {
    /// Files chosen by an open dialog.
    files: Vec<std::path::PathBuf>,
    /// Path chosen by a save dialog.
    path: Option<std::path::PathBuf>,
    /// How long the dialog was up.
    elapsed: std::time::Duration,
}

/// An OS dialog running on its own thread, polled once a frame.
struct PendingDialog {
    answers: std::sync::mpsc::Receiver<Answer>,
    kind: NativeDialog,
    started: std::time::Instant,
}

/// A OS dialog that comes back faster than this was never really shown: the
/// portal (and its zenity fallback) are missing, so the pane uses its own
/// browser instead of leaving the button dead.
const NATIVE_MIN_INTERACTION: std::time::Duration = std::time::Duration::from_millis(200);

/// Whether this build may open OS dialogs.
///
/// Tests never do: a `cargo test` run must not pop a real window, so they take
/// the in-pane fallback every time.
const fn native_dialogs_enabled() -> bool {
    !cfg!(test)
}

/// A download that is waiting for the user to say where it lands.
///
/// There is no default destination: nothing is written until the save dialog is
/// confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SaveRequest {
    /// Remote paths being saved.
    items: Vec<String>,
    /// Save `items` as one ZIP instead of copying them one by one.
    zip: bool,
    /// Directory the browser shows.
    dir: String,
    /// Name the file (or archive) gets.
    name: String,
    /// Folders that were selected but cannot be copied file by file.
    skipped: Vec<String>,
}

/// Where the browser is looking, and what it remembers between frames.
pub struct Files {
    /// Directory currently shown.
    path: String,
    /// Path typed into the address bar, before Enter.
    draft: String,
    /// Rows the user selected, by path.
    selected: Vec<String>,
    /// Row a shift-click range starts from.
    anchor: Option<String>,
    /// Open dialog, if any.
    dialog: Option<FileDialog>,
    /// Where the context menu is open, and the row it was opened on (`None`
    /// for the empty space under the rows).
    menu: Option<(egui::Pos2, Option<String>)>,
    /// One-line result of the last action, and whether it failed.
    notice: Option<(String, bool)>,
    /// When the notice appeared, so it can fade out like the reference.
    notice_at: f64,
    /// Notice this pane has already timed, so a new one restarts the clock.
    notice_seen: Option<(String, bool)>,
    /// Entries of the listing on screen, for lookups while a menu is open.
    entries: Vec<SftpEntry>,
    /// Paths copied with Copy/Cut, and whether Paste should move them.
    clipboard: Vec<String>,
    clipboard_cut: bool,
    /// File shown in the viewer, if any.
    viewer: Option<Viewer>,
    /// Local browser used to choose the files to upload, and its directory.
    picker: Option<String>,
    /// Archive the next ZIP request should write, plus what goes in it.
    pending_zip: Option<(Vec<String>, String)>,
    /// Download waiting for the user to choose a destination (the in-pane
    /// fallback for machines without a desktop portal).
    save: Option<SaveRequest>,
    /// Native dialog in flight.
    native: Option<PendingDialog>,
    /// Remote files still waiting for their own save dialog, as (remote, name).
    save_queue: std::collections::VecDeque<(String, String)>,
    /// Folders that were selected but cannot be copied file by file.
    skipped_folders: Vec<String>,
    /// Directory the save dialog last used, remembered between saves.
    save_dir: String,
    /// Local files chosen in the picker, ready to upload.
    picker_selected: Vec<String>,
    /// State line the pane header shows: the text and how it reads.
    caption: (String, HeaderHealth),
}

impl Default for Files {
    fn default() -> Self {
        Self {
            path: String::new(),
            draft: String::new(),
            selected: Vec::new(),
            anchor: None,
            dialog: None,
            menu: None,
            notice: None,
            notice_at: 0.0,
            notice_seen: None,
            entries: Vec::new(),
            clipboard: Vec::new(),
            clipboard_cut: false,
            viewer: None,
            picker: None,
            pending_zip: None,
            save: None,
            native: None,
            save_queue: std::collections::VecDeque::new(),
            skipped_folders: Vec::new(),
            save_dir: String::new(),
            picker_selected: Vec::new(),
            caption: (String::new(), HeaderHealth::Bad),
        }
    }
}

impl Files {
    /// Read the persisted switches back into this pane.
    fn apply_settings(&mut self, settings: &serde_json::Map<String, serde_json::Value>) {
        if let Some(dir) = settings
            .get(SAVE_DIR_KEY)
            .and_then(serde_json::Value::as_str)
        {
            dir.clone_into(&mut self.save_dir);
        }
    }

    /// Directory the browser shows.
    pub fn current_path(&self) -> &str {
        &self.path
    }

    /// Paths currently selected.
    pub fn selection(&self) -> &[String] {
        &self.selected
    }

    /// Whether `path` is selected.
    fn is_selected(&self, path: &str) -> bool {
        self.selected.iter().any(|selected| selected == path)
    }

    fn select_only(&mut self, path: &str) {
        self.selected = vec![path.to_owned()];
        self.anchor = Some(path.to_owned());
    }

    fn toggle(&mut self, path: &str) {
        if let Some(index) = self.selected.iter().position(|entry| entry == path) {
            self.selected.remove(index);
        } else {
            self.selected.push(path.to_owned());
        }
        self.anchor = Some(path.to_owned());
    }

    /// Select everything between the anchor and `path`, in visible order.
    fn select_range(&mut self, visible: &[&str], path: &str) {
        let Some(anchor) = self.anchor.clone() else {
            self.select_only(path);
            return;
        };
        let from = visible.iter().position(|entry| *entry == anchor);
        let to = visible.iter().position(|entry| *entry == path);
        match (from, to) {
            (Some(from), Some(to)) => {
                let (low, high) = if from <= to { (from, to) } else { (to, from) };
                self.selected = visible[low..=high].iter().map(|entry| (*entry).to_owned()).collect();
            }
            _ => self.select_only(path),
        }
    }
}

/// The parent of `path`, or `None` at the root.
pub fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_owned()),
        Some(index) => Some(trimmed[..index].to_owned()),
        None => None,
    }
}

/// Join a directory and a file name with exactly one `/` between them.
pub fn join_remote(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return format!("/{name}");
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The final component of a path, ignoring trailing separators.
pub fn file_name(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
}

/// Human-readable size for the transfer lines.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Size as the table shows it: no fraction from ten upwards.
pub fn format_entry_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if size == 0 {
        // The reference's `formatBytes` shows a dash for nothing.
        return "—".to_owned();
    }
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 || unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Unix mode as three octal digits, e.g. `644`.
pub fn octal_mode(mode: u32) -> String {
    format!("{:03o}", mode & 0o777)
}

/// Parse a typed octal mode; `None` unless it is one to four octal digits.
pub fn parse_mode(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    if trimmed.is_empty()
        || trimmed.len() > 4
        || !trimmed.chars().all(|digit| ('0'..='7').contains(&digit))
    {
        return None;
    }
    u32::from_str_radix(trimmed, 8).ok().map(|mode| mode & 0o7777)
}

/// A name a file may be given: not empty, no separator, not a relative link.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\u{0}'])
}

/// Icon the table shows for `entry`.
pub fn type_icon(entry: &SftpEntry) -> &'static str {
    if entry.is_dir {
        "📁"
    } else if entry.is_link {
        "🔗"
    } else {
        "📄"
    }
}

/// Indices of `entries` in table order.
///
/// This is the reference's `compareListEntries`: folders first, then names
/// compared case-insensitively and numerically, so `file2` sorts before
/// `file10`. There is no column sorting and no hidden-file filter.
pub fn sort_entries(entries: &[SftpEntry]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|left, right| {
        let left = &entries[*left];
        let right = &entries[*right];
        let folders = right.is_dir.cmp(&left.is_dir);
        if folders != std::cmp::Ordering::Equal {
            return folders;
        }
        compare_names(&left.name, &right.name)
    });
    order
}

/// Case-insensitive, numeric-aware name comparison.
fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let left = left.to_lowercase();
    let right = right.to_lowercase();
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();
    loop {
        match (left_chars.peek(), right_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) => {
                if a.is_ascii_digit() && b.is_ascii_digit() {
                    // Compare whole runs of digits as numbers.
                    let mut left_number = String::new();
                    while let Some(digit) = left_chars.peek().copied().filter(char::is_ascii_digit) {
                        left_number.push(digit);
                        left_chars.next();
                    }
                    let mut right_number = String::new();
                    while let Some(digit) = right_chars.peek().copied().filter(char::is_ascii_digit) {
                        right_number.push(digit);
                        right_chars.next();
                    }
                    let left_value = left_number.trim_start_matches('0');
                    let right_value = right_number.trim_start_matches('0');
                    let ordering = left_value
                        .len()
                        .cmp(&right_value.len())
                        .then_with(|| left_value.cmp(right_value));
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                } else {
                    let ordering = a.cmp(b);
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                    left_chars.next();
                    right_chars.next();
                }
            }
        }
    }
}

impl Plugin for Files {
    fn manifest(&self) -> PluginManifest {
        PluginId::Sftp.manifest()
    }

    fn header_status(&self) -> Option<(String, HeaderHealth)> {
        (!self.caption.0.is_empty()).then(|| self.caption.clone())
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        self.apply_settings(context.settings);
        let muted = to_color32(palette.text_muted);
        let now = ui.ctx().input(|input| input.time);

        let Some(access) = context.session.as_deref_mut() else {
            ui.label(
                egui::RichText::new("Remote files are available for SSH sessions.")
                    .size(12.0)
                    .color(muted),
            );
            return;
        };

        // The browser opens in the login directory, like the reference.
        if self.path.is_empty() {
            match access.resolve(".") {
                Some(path) => {
                    self.path = path.clone();
                    self.draft = path;
                }
                None => {
                    ui.label(
                        egui::RichText::new("Opening the login directory…")
                            .size(12.0)
                            .color(muted),
                    );
                    return;
                }
            }
        }
        access.request_listing(&self.path);

        let connected = context.status == SessionStatus::Connected;
        self.caption = match context.status {
            SessionStatus::Connected => ("Connected".to_owned(), HeaderHealth::Ok),
            SessionStatus::Connecting
            | SessionStatus::Authenticating
            | SessionStatus::AwaitingHostKey
            | SessionStatus::Reconnecting => ("Connecting…".to_owned(), HeaderHealth::Warn),
            _ => ("Files not connected".to_owned(), HeaderHealth::Bad),
        };
        let listing = access.listing().cloned().unwrap_or_default();
        let loading = access.is_loading();
        self.entries.clone_from(&listing.entries);
        let error = access.error().map(str::to_owned);

        // Files dropped on the pane are uploaded into the directory on screen.
        let dropped: Vec<String> = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_string_lossy().into_owned())
                .collect()
        });
        if !dropped.is_empty() && connected {
            let items: Vec<(String, String)> = dropped
                .iter()
                .filter_map(|local| {
                    file_name(local).map(|name| (local.clone(), join_remote(&self.path, name)))
                })
                .collect();
            if !items.is_empty() {
                let count = items.len();
                access.upload_many(items);
                self.set_notice(format!("Uploading {count} file{}", plural(count)), false, now);
            }
        }

        self.poll_dialog(access, now);

        let mut intent = self.toolbar(ui, &palette, connected);
        if let Some(path) = self.path_bar(ui, &palette) {
            intent = Some(Intent::Navigate(path));
        }

        // A notice fades after a few seconds, like the reference's toast.
        if self.notice_seen != self.notice {
            self.notice_seen = self.notice.clone();
            self.notice_at = now;
        }
        let notice = self
            .notice
            .clone()
            .filter(|_| now - self.notice_at <= NOTICE_SECONDS);

        // The transfer list sits under the table and takes it own room.
        let transfers = access.transfers();
        let reserved = transfers_area_height(transfers.len());
        let table_height = (ui.available_height() - reserved).max(60.0);
        let table_top = ui.cursor().min;
        egui::ScrollArea::vertical()
            .id_salt("wassh-files")
            .max_height(table_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(clicked) = self.table(ui, &palette, &listing, loading, error.as_deref()) {
                    intent = Some(clicked);
                }
            });
        let table_rect = egui::Rect::from_min_size(
            table_top,
            egui::Vec2::new(ui.available_width(), table_height),
        );
        if let Some((message, failed)) = &notice {
            paint_notice(ui, &palette, table_rect, message, *failed);
        }
        // The reference binds Delete and Ctrl+C inside the file view. Ours fire
        // while the pointer is over the table, and the key is consumed so the
        // terminal behind the pane never sees it (`forward_input` runs after the
        // panes are drawn).
        if connected && !self.selected.is_empty() && ui.rect_contains_pointer(table_rect) {
            if ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
            {
                let targets = self.selection_with_kind();
                self.dialog = Some(FileDialog::Delete { targets });
            }
            if ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::C))
            {
                intent = Some(Intent::Copy);
            }
        }
        let dragging = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        if dragging && connected {
            paint_drop_hint(ui, &palette, table_rect, &self.path);
        }
        self.transfers(ui, &palette, &transfers, access);

        if let Some(clicked) = self.context_menu(ui, &palette, &listing) {
            intent = Some(clicked);
        }
        if let Some(dialog) = self.dialog.clone() {
            self.show_dialog(ui, &palette, access, dialog);
        }
        if self.viewer.is_some() {
            self.show_viewer(ui, &palette, access);
        }
        if let Some(directory) = self.picker.clone() {
            self.show_picker(ui, &palette, access, directory);
        }
        if self.save.is_some() {
            self.show_save(ui, &palette, access);
        }
        if let Some((items, archive)) = self.pending_zip.take() {
            access.download_zip(items, &archive);
        }

        match intent {
            Some(Intent::Navigate(path)) => {
                self.path = path.clone();
                self.draft = path.clone();
                self.selected.clear();
                self.anchor = None;
                self.notice = None;
                access.refresh(&path);
            }
            Some(Intent::Refresh) => access.refresh(&self.path.clone()),
            Some(Intent::Download) => self.begin_download(access, &listing),
            Some(Intent::View(path)) => {
                let total = self
                    .entries
                    .iter()
                    .find(|entry| entry.path == path)
                    .map(|entry| entry.size);
                self.viewer = Some(Viewer {
                    name: file_name(&path).unwrap_or(&path).to_owned(),
                    path,
                    total,
                    hex: false,
                });
            }
            Some(Intent::Copy) => {
                self.clipboard = self.selected.clone();
                self.clipboard_cut = false;
                let count = self.clipboard.len();
                self.set_notice(format!("Copied {count} item{}", plural(count)), false, now);
            }
            Some(Intent::Paste) => self.paste(access),
            Some(Intent::Zip) => {
                let items = if self.selected.is_empty() {
                    vec![self.path.clone()]
                } else {
                    self.selected.clone()
                };
                let root = self.zip_root();
                self.begin_zip(items, root);
            }
            Some(Intent::PickUpload) => {
                if native_dialogs_enabled() {
                    self.spawn_upload();
                } else {
                    self.open_picker();
                }
            }
            None => {}
        }

        context.set_value(SAVE_DIR_KEY, serde_json::Value::from(self.save_dir.clone()));
    }
}

/// How long a notice stays on screen.
const NOTICE_SECONDS: f64 = 3.5;

impl Files {
    /// Show a one-line result for a few seconds.
    fn set_notice(&mut self, text: String, failed: bool, now: f64) {
        self.notice = Some((text, failed));
        self.notice_at = now;
        self.notice_seen = self.notice.clone();
    }

    /// The action toolbar, in the reference's order.
    fn toolbar(&mut self, ui: &mut Ui, palette: &Palette, connected: bool) -> Option<Intent> {
        let mut intent = None;
        let has_selection = !self.selected.is_empty();
        let single = self.selected.len() == 1;
        let files: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| !entry.is_dir && self.is_selected(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        let at_root = parent_path(&self.path).is_none();
        let add = |intent: &mut Option<Intent>, wanted: Intent| {
            *intent = Some(wanted);
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if small_button_enabled(ui, palette, "⬆ Up", connected && !at_root)
                && let Some(parent) = parent_path(&self.path)
            {
                add(&mut intent, Intent::Navigate(parent));
            }
            if small_button_enabled(ui, palette, "⟳ Refresh", connected) {
                add(&mut intent, Intent::Refresh);
            }
            toolbar_separator(ui, palette);
            if small_button_enabled(ui, palette, "+ New folder", connected) {
                self.dialog = Some(FileDialog::NewFolder {
                    name: String::new(),
                });
            }
            if small_button_enabled(ui, palette, "⬆ Upload…", connected) {
                add(&mut intent, Intent::PickUpload);
            }
            if small_button_enabled(ui, palette, "⧉ Copy", connected && has_selection) {
                add(&mut intent, Intent::Copy);
            }
            let paste = if self.clipboard.is_empty() {
                "▣ Paste".to_owned()
            } else {
                format!("▣ Paste ({})", self.clipboard.len())
            };
            if small_button_enabled(ui, palette, &paste, connected && !self.clipboard.is_empty()) {
                add(&mut intent, Intent::Paste);
            }
            toolbar_separator(ui, palette);
            if small_button_enabled(ui, palette, "✎ Rename", connected && single)
                && let Some(path) = self.selected.first().cloned()
            {
                self.dialog = Some(FileDialog::Rename {
                    name: file_name(&path).unwrap_or_default().to_owned(),
                    path,
                });
            }
            if small_button_enabled(ui, palette, "🔒 Permissions", connected && single)
                && let Some(path) = self.selected.first().cloned()
            {
                let mode = self.mode_of(&path);
                self.dialog = Some(FileDialog::Permissions { path, mode });
            }
            if Button::new("🗑 Delete")
                .compact()
                .kind(ButtonKind::Danger)
                .enabled(connected && has_selection)
                .show(ui, palette)
                .clicked()
            {
                let targets = self.selection_with_kind();
                self.dialog = Some(FileDialog::Delete { targets });
            }
            toolbar_separator(ui, palette);
            if small_button_enabled(ui, palette, "⬇ Download", connected && !files.is_empty()) {
                add(&mut intent, Intent::Download);
            }
            // The reference calls this "⬇ Download ZIP" and it covers the
            // selection, or the folder on screen when nothing is selected.
            if small_button_enabled(ui, palette, "⬇ Download ZIP", connected) {
                add(&mut intent, Intent::Zip);
            }
        });
        intent
    }

    /// `Path / › srv › www`: a root link plus the rest of the path as text.
    fn path_bar(&mut self, ui: &mut Ui, palette: &Palette) -> Option<String> {
        let muted = to_color32(palette.text_muted);
        let mut intent = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new("PATH")
                    .size(11.0)
                    .color(muted)
                    .strong(),
            );
            if small_button(ui, palette, "/") {
                intent = Some("/".to_owned());
            }
            if !self.path.is_empty() && self.path != "/" {
                ui.label(egui::RichText::new("›").size(12.0).color(muted));
                let room = (ui.available_width() - 4.0).max(40.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(self.path.trim_start_matches('/')).size(12.0),
                    )
                    .truncate(),
                );
                let _ = room;
            }
        });
        intent
    }

    fn table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        listing: &SftpListing,
        loading: bool,
        error: Option<&str>,
    ) -> Option<Intent> {
        let content = ui.available_width();
        let muted = to_color32(palette.text_muted);
        let text = to_color32(palette.text);
        let heading_font = egui::FontId::proportional(12.0);
        let cell_font = egui::FontId::proportional(13.0);
        let mono_font = egui::FontId::monospace(11.5);
        let painter = ui.painter().clone();
        let measure = |font: &egui::FontId, value: &str| {
            painter
                .layout_no_wrap(value.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };

        // Every column asks for what its widest value needs; the name asks for
        // its longest row. The priorities decide who gives way: name first,
        // then the size, then the permissions and the date.
        let icon_width = measure(&cell_font, "📁 ");
        let mut name_need = icon_width + measure(&heading_font, "Name");
        let mut narrow = [
            measure(&heading_font, "Size"),
            measure(&heading_font, "Permissions"),
            measure(&heading_font, "Modified"),
        ];
        for entry in &listing.entries {
            let link_mark = if entry.is_link { " →" } else { "" };
            name_need = name_need.max(
                icon_width + measure(&cell_font, &format!("{}{link_mark}", entry.name)) + 8.0,
            );
            narrow[0] = narrow[0].max(measure(&mono_font, &size_cell(entry)));
            narrow[1] = narrow[1].max(measure(&mono_font, &mode_symbolic(entry)));
            narrow[2] = narrow[2].max(measure(&mono_font, &date_cell(entry)));
        }
        let widths = column_widths(content, name_need, &narrow);
        let mut offsets = [0.0_f32; 4];
        let mut left = 0.0;
        for (index, width) in widths.iter().enumerate() {
            offsets[index] = left;
            left += width;
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        let heading_top = ui.cursor().top();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, (label, right_aligned)) in
                [
                    ("Name", false),
                    ("Size", true),
                    ("Permissions", false),
                    ("Modified", false),
                ]
                    .into_iter()
                    .enumerate()
            {
                if widths[index] <= 0.0 {
                    continue;
                }
                let (rect, _) =
                    ui.allocate_exact_size(egui::Vec2::new(widths[index], HEADING_HEIGHT), egui::Sense::hover());
                let painter = ui.painter();
                let caption = ellipsize(painter, label, heading_font.clone(), widths[index] - 8.0);
                let used = measure(&heading_font, &caption);
                let x = if right_aligned {
                    rect.right() - 4.0 - used
                } else {
                    rect.left() + 4.0
                };
                painter.text(
                    egui::pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    caption,
                    heading_font.clone(),
                    muted,
                );
            }
        });
        ui.painter().hline(
            0.0..=content,
            heading_top + HEADING_HEIGHT,
            egui::Stroke::new(1.0, faded(palette.border, 0.7)),
        );

        let mut intent = None;
        if let Some(parent) = parent_path(&self.path) {
            let (rect, response) =
                ui.allocate_exact_size(egui::Vec2::new(content, ROW_HEIGHT), egui::Sense::click());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 0.0, faded(palette.bg_input, 0.8));
            }
            let painter = ui.painter();
            painter.text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                "📁 ..",
                cell_font.clone(),
                to_color32(palette.accent),
            );
            for (index, right_aligned) in [(1, true), (3, false)] {
                if widths[index] <= 0.0 {
                    continue;
                }
                cell_paint(
                    painter,
                    rect,
                    offsets[index],
                    widths[index],
                    "—",
                    &mono_font,
                    muted,
                    right_aligned,
                    content,
                );
            }
            // Every row opens with a double click, `..` included.
            if response.double_clicked() || response.triple_clicked() {
                intent = Some(Intent::Navigate(parent));
            }
        }

        let order = sort_entries(&listing.entries);
        let visible: Vec<&str> = order
            .iter()
            .map(|index| listing.entries[*index].path.as_str())
            .collect();
        for index in order {
            let entry = &listing.entries[index];
            let (rect, response) =
                ui.allocate_exact_size(egui::Vec2::new(content, ROW_HEIGHT), egui::Sense::click());
            let selected = self.is_selected(&entry.path);
            let menu_open = self
                .menu
                .as_ref()
                .is_some_and(|(_, path)| path.as_deref() == Some(entry.path.as_str()));
            let painter = ui.painter();
            if selected {
                painter.rect_filled(
                    rect,
                    0.0,
                    to_color32(colors::tint(palette.accent, palette.bg_input, 0.18)),
                );
            } else if menu_open {
                painter.rect_filled(rect, 0.0, faded(palette.accent, 0.14));
            } else if response.hovered() {
                painter.rect_filled(rect, 0.0, faded(palette.bg_input, 0.8));
            }

            let modifiers = ui.input(|input| input.modifiers);
            let opens = (response.double_clicked() || response.triple_clicked()) && selected;
            if opens {
                if entry.is_dir {
                    intent = Some(Intent::Navigate(entry.path.clone()));
                } else {
                    self.select_only(&entry.path);
                    intent = Some(Intent::View(entry.path.clone()));
                }
            } else if response.secondary_clicked() {
                if !selected {
                    self.select_only(&entry.path);
                }
                let position = response
                    .interact_pointer_pos()
                    .unwrap_or_else(|| rect.center());
                self.menu = Some((position, Some(entry.path.clone())));
            } else if response.clicked() {
                if modifiers.shift {
                    self.select_range(&visible, &entry.path);
                } else if modifiers.command || modifiers.ctrl {
                    self.toggle(&entry.path);
                } else {
                    self.select_only(&entry.path);
                }
                self.menu = None;
            }

            let painter = ui.painter();
            let colour = if entry.is_dir {
                to_color32(palette.accent)
            } else {
                text
            };
            painter.text(
                egui::pos2(rect.left() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                type_icon(entry),
                cell_font.clone(),
                colour,
            );
            let icon_width = measure(&cell_font, "📁 ");
            let name_room = widths[0] - icon_width - 8.0;
            let link_mark = if entry.is_link { " →" } else { "" };
            // The name owns the whole column: the permission string has its own.
            painter.text(
                egui::pos2(rect.left() + 4.0 + icon_width, rect.center().y),
                egui::Align2::LEFT_CENTER,
                ellipsize(
                    painter,
                    &format!("{}{link_mark}", entry.name),
                    cell_font.clone(),
                    name_room.max(24.0),
                ),
                cell_font.clone(),
                colour,
            );
            let cells = [
                (1, size_cell(entry), true),
                (2, mode_symbolic(entry), false),
                (3, date_cell(entry), false),
            ];
            for (index, value, right_aligned) in cells {
                if widths[index] <= 0.0 {
                    continue;
                }
                cell_paint(
                    painter,
                    rect,
                    offsets[index],
                    widths[index],
                    &value,
                    &mono_font,
                    muted,
                    right_aligned,
                    content,
                );
            }
        }

        // Loading, empty and failed states sit over the table like the
        // reference's centred `.sftp-empty` block.
        let body = egui::Rect::from_min_size(
            egui::pos2(ui.min_rect().left(), heading_top + HEADING_HEIGHT),
            egui::Vec2::new(content, (ui.min_rect().bottom() - heading_top).max(0.0)),
        );
        if loading && listing.entries.is_empty() {
            centred_message(ui, palette, body, "Loading…", muted);
        } else if let Some(error) = error {
            centred_message(
                ui,
                palette,
                body,
                &format!("⚠ {error}"),
                to_color32(palette.danger),
            );
        } else if listing.entries.is_empty() {
            centred_message(ui, palette, body, "Empty folder", muted);
        }
        intent
    }

    /// Context menu, with the reference's items for a row or the empty space.
    fn context_menu(&mut self, ui: &mut Ui, palette: &Palette, listing: &SftpListing) -> Option<Intent> {
        let (position, path) = self.menu.clone()?;
        let entry = path
            .as_ref()
            .and_then(|path| listing.entries.iter().find(|entry| &entry.path == path).cloned());

        // One action per item, `None` for a separator, so the chosen index maps
        // straight onto what to do.
        let mut items = Vec::new();
        let mut actions: Vec<Option<MenuAction>> = Vec::new();
        fn push(
            items: &mut Vec<MenuItem>,
            actions: &mut Vec<Option<MenuAction>>,
            label: &'static str,
            action: MenuAction,
        ) {
            items.push(MenuItem::new(label));
            actions.push(Some(action));
        }
        match &entry {
            Some(row) => {
                if row.is_dir {
                    push(&mut items, &mut actions, "📂 Open", MenuAction::Open(row.path.clone()));
                }
                push(&mut items, &mut actions, "⧉ Copy", MenuAction::Copy);
                if !row.is_dir {
                    push(&mut items, &mut actions, "👁 View", MenuAction::View(row.path.clone()));
                    push(&mut items, &mut actions, "⬇ Download", MenuAction::Download);
                } else {
                    push(&mut items, &mut actions, "⬇ Download as ZIP", MenuAction::ZipSelection);
                }
                push(&mut items, &mut actions, "✎ Rename", MenuAction::Rename);
                push(&mut items, &mut actions, "🔒 Permissions", MenuAction::Permissions);
                items.push(MenuItem::Separator);
                actions.push(None);
                items.push(MenuItem::danger("🗑 Delete"));
                actions.push(Some(MenuAction::Delete));
            }
            None => {
                if !self.clipboard.is_empty() {
                    push(&mut items, &mut actions, "▣ Paste", MenuAction::Paste);
                }
                push(&mut items, &mut actions, "+ New folder", MenuAction::NewFolder);
                push(&mut items, &mut actions, "⬆ Upload files…", MenuAction::Upload);
                push(
                    &mut items,
                    &mut actions,
                    "⬇ Download current folder as ZIP",
                    MenuAction::ZipFolder,
                );
                push(&mut items, &mut actions, "⟳ Refresh", MenuAction::Refresh);
            }
        }

        let chosen = context_menu(ui, palette, position, &items);
        let dismissed = ui.input(|input| {
            input.pointer.primary_clicked() || input.key_pressed(egui::Key::Escape)
        });
        let Some(choice) = chosen else {
            if dismissed {
                self.menu = None;
            }
            return None;
        };
        self.menu = None;
        let action = actions.get(choice).cloned().flatten();
        let row = entry;
        match action {
            Some(MenuAction::Open(path)) => Some(Intent::Navigate(path)),
            Some(MenuAction::View(path)) => Some(Intent::View(path)),
            Some(MenuAction::Copy) => {
                let targets = match &row {
                    Some(row) if self.is_selected(&row.path) => self.selected.clone(),
                    Some(row) => vec![row.path.clone()],
                    None => self.selected.clone(),
                };
                self.clipboard = targets;
                self.clipboard_cut = false;
                None
            }
            Some(MenuAction::Download) => {
                if let Some(row) = &row {
                    self.begin_download_one(row.path.clone());
                }
                None
            }
            Some(MenuAction::ZipSelection) => {
                if let Some(row) = &row {
                    let items = if self.is_selected(&row.path) {
                        self.selected.clone()
                    } else {
                        vec![row.path.clone()]
                    };
                    let root = file_name(&row.path).unwrap_or("archive").to_owned();
                    self.begin_zip(items, root);
                }
                None
            }
            Some(MenuAction::Rename) => {
                if let Some(row) = &row {
                    self.dialog = Some(FileDialog::Rename {
                        name: row.name.clone(),
                        path: row.path.clone(),
                    });
                }
                None
            }
            Some(MenuAction::Permissions) => {
                if let Some(row) = &row {
                    self.dialog = Some(FileDialog::Permissions {
                        path: row.path.clone(),
                        mode: octal_mode(row.permissions.unwrap_or(0)),
                    });
                }
                None
            }
            Some(MenuAction::Delete) => {
                let targets = match &row {
                    Some(row) if self.is_selected(&row.path) => self.selection_with_kind(),
                    Some(row) => vec![(row.path.clone(), row.is_dir)],
                    None => self.selection_with_kind(),
                };
                if !targets.is_empty() {
                    self.dialog = Some(FileDialog::Delete { targets });
                }
                None
            }
            Some(MenuAction::Paste) => Some(Intent::Paste),
            Some(MenuAction::NewFolder) => {
                self.dialog = Some(FileDialog::NewFolder {
                    name: String::new(),
                });
                None
            }
            Some(MenuAction::Upload) => Some(Intent::PickUpload),
            Some(MenuAction::ZipFolder) => {
                let root = file_name(&self.path).unwrap_or("archive").to_owned();
                self.begin_zip(vec![self.path.clone()], root);
                None
            }
            Some(MenuAction::Refresh) => Some(Intent::Refresh),
            None => None,
        }
    }

    /// The transfer list under the table: direction, path, amount and a bar.
    fn transfers(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        rows: &[TransferRow],
        access: &mut dyn SessionAccess,
    ) {
        if rows.is_empty() {
            return;
        }
        let muted = to_color32(palette.text_muted);
        let mut retry = false;
        egui::Frame::NONE
            .fill(to_color32(palette.bg_panel_2))
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top(),
                    egui::Stroke::new(1.0, to_color32(palette.border)),
                );
                ui.spacing_mut().item_spacing.y = 6.0;
                for row in rows.iter().take(TRANSFER_ROWS) {
                    let colour = match row.state {
                        TransferState::Failed => to_color32(palette.danger),
                        _ => to_color32(palette.text),
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(row.direction.label())
                                .size(11.0)
                                .color(colour),
                        );
                        let amount = match (row.state, row.total) {
                            (TransferState::Failed, _) => "failed".to_owned(),
                            (TransferState::Queued, _) => "queued".to_owned(),
                            (_, Some(total)) if total > 0 => format!(
                                "{} / {}",
                                format_entry_size(row.done),
                                format_entry_size(total)
                            ),
                            _ => format_entry_size(row.done),
                        };
                        let font = egui::FontId::proportional(11.0);
                        let amount_width = ui
                            .painter()
                            .layout_no_wrap(amount.clone(), font, muted)
                            .size()
                            .x;
                        let used = ui.cursor().min.x;
                        let room = (ui.clip_rect().right() - used - amount_width - CELL_GAP)
                            .max(24.0);
                        ui.allocate_ui_with_layout(
                            egui::Vec2::new(room, 0.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&row.path).size(11.0).color(muted),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        ui.label(egui::RichText::new(amount).size(11.0).color(muted));
                        if row.state == TransferState::Failed
                            && small_button(ui, palette, "Retry")
                        {
                            retry = true;
                        }
                    });
                    let track = ui.available_width();
                    let (rect, _) = ui.allocate_exact_size(
                        egui::Vec2::new(track, TRANSFER_TRACK_HEIGHT),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(
                        rect,
                        TRANSFER_TRACK_HEIGHT / 2.0,
                        faded(palette.border, 0.55),
                    );
                    let fraction = row.fraction().unwrap_or(0.0);
                    if fraction > 0.0 {
                        let filled = egui::Rect::from_min_size(
                            rect.min,
                            egui::Vec2::new(rect.width() * fraction, rect.height()),
                        );
                        ui.painter().rect_filled(
                            filled,
                            TRANSFER_TRACK_HEIGHT / 2.0,
                            if row.state == TransferState::Failed {
                                to_color32(palette.danger)
                            } else {
                                to_color32(palette.accent)
                            },
                        );
                    }
                }
            });
        if retry {
            access.retry();
        }
    }

    /// Send every chosen local file into `dir`, one row each.
    fn send_uploads(&mut self, access: &mut dyn SessionAccess, files: Vec<String>, now: f64) {
        let items: Vec<(String, String)> = files
            .iter()
            .filter_map(|local| {
                file_name(local).map(|name| (local.clone(), join_remote(&self.path, name)))
            })
            .collect();
        if items.is_empty() {
            access.report_problem("Choose a file to upload.");
            return;
        }
        let count = items.len();
        let dir = self.path.clone();
        access.upload_many(items);
        self.set_notice(
            format!("Uploading {count} file{} → {dir}", plural(count)),
            false,
            now,
        );
    }

    /// Ask the desktop for the files to upload.
    fn spawn_upload(&mut self) {
        if self.native.is_some() {
            return;
        }
        let (sender, answers) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new()
                .set_title("Upload files")
                .pick_files()
                .unwrap_or_default();
            let _ = sender.send(Answer {
                files,
                path: None,
                elapsed: started.elapsed(),
            });
        });
        self.native = Some(PendingDialog {
            answers,
            kind: NativeDialog::Upload,
            started,
        });
    }

    /// Ask the desktop where one remote file should land.
    fn spawn_save(&mut self, remote: String, name: String) {
        if self.native.is_some() {
            return;
        }
        let (sender, answers) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        let dir = self.save_dir.clone();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title("Save downloaded file")
                .set_file_name(&name);
            if !dir.is_empty() && std::path::Path::new(&dir).is_dir() {
                dialog = dialog.set_directory(&dir);
            }
            let path = dialog.save_file();
            let _ = sender.send(Answer {
                files: Vec::new(),
                path,
                elapsed: started.elapsed(),
            });
        });
        self.native = Some(PendingDialog {
            answers,
            kind: NativeDialog::Save { remote },
            started,
        });
    }

    /// Ask the desktop where the archive should be written.
    fn spawn_archive(&mut self, items: Vec<String>, name: String, title: &'static str) {
        if self.native.is_some() {
            return;
        }
        let (sender, answers) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        let dir = self.save_dir.clone();
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new()
                .set_title(title)
                .set_file_name(&name)
                .add_filter("ZIP archive (*.zip)", &["zip"]);
            if !dir.is_empty() && std::path::Path::new(&dir).is_dir() {
                dialog = dialog.set_directory(&dir);
            }
            let path = dialog.save_file();
            let _ = sender.send(Answer {
                files: Vec::new(),
                path,
                elapsed: started.elapsed(),
            });
        });
        self.native = Some(PendingDialog {
            answers,
            kind: NativeDialog::Archive { items },
            started,
        });
    }

    /// Collect the answer of the open dialog, if it has arrived.
    fn poll_dialog(&mut self, access: &mut dyn SessionAccess, now: f64) {
        let Some(pending) = self.native.take() else {
            return;
        };
        let answer = match pending.answers.try_recv() {
            Ok(answer) => answer,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.native = Some(pending);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Answer {
                files: Vec::new(),
                path: None,
                elapsed: pending.started.elapsed(),
            },
        };
        // A dialog that never appeared means there is no desktop portal here.
        let no_portal = answer.elapsed < NATIVE_MIN_INTERACTION;

        match pending.kind {
            NativeDialog::Upload => {
                if !answer.files.is_empty() {
                    let files = answer
                        .files
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect();
                    self.send_uploads(access, files, now);
                } else if no_portal {
                    self.open_picker();
                }
            }
            NativeDialog::Save { remote } => {
                if let Some(path) = &answer.path {
                    remember_dir(&mut self.save_dir, path);
                    access.download(&remote, &path.to_string_lossy());
                } else if no_portal {
                    // Fall back to the pane's own dialog for whatever is left.
                    self.save_queue.push_front((
                        remote.clone(),
                        file_name(&remote).unwrap_or("download").to_owned(),
                    ));
                    self.start_save_fallback();
                    return;
                }
                self.next_save();
            }
            NativeDialog::Archive { items } => {
                if let Some(path) = answer.path {
                    remember_dir(&mut self.save_dir, &path);
                    self.pending_zip = Some((items, with_zip_suffix(path)));
                } else if no_portal {
                    let name = with_zip_suffix(std::path::PathBuf::from(archive_name(&self.path)));
                    self.save = Some(SaveRequest {
                        items,
                        zip: true,
                        dir: self.save_start(),
                        name: file_name(&name).unwrap_or("archive.zip").to_owned(),
                        skipped: Vec::new(),
                    });
                }
            }
        }
    }

    /// Start the save dialog for the next queued file, if any.
    fn next_save(&mut self) {
        let Some((remote, name)) = self.save_queue.pop_front() else {
            return;
        };
        if native_dialogs_enabled() {
            self.spawn_save(remote, name);
        } else {
            self.save_queue.push_front((remote, name));
            self.start_save_fallback();
        }
    }

    /// Hand the rest of the batch to the pane's own dialog.
    fn start_save_fallback(&mut self) {
        let items: Vec<String> = self
            .save_queue
            .iter()
            .map(|(remote, _)| remote.clone())
            .collect();
        self.save_queue.clear();
        if items.is_empty() {
            return;
        }
        let name = if items.len() == 1 {
            file_name(&items[0]).unwrap_or("download").to_owned()
        } else {
            String::new()
        };
        self.save = Some(SaveRequest {
            items,
            zip: false,
            dir: self.save_start(),
            name,
            skipped: std::mem::take(&mut self.skipped_folders),
        });
    }

    /// Ask where the selected files should be saved.
    ///
    /// One dialog per file, like the reference's download loop; nothing is
    /// written until a path is chosen.
    fn begin_download(&mut self, access: &mut dyn SessionAccess, listing: &SftpListing) {
        if self.selected.is_empty() {
            access.report_problem("Select a file to download.");
            return;
        }
        let (files, skipped) = download_plan(&listing.entries, &self.selected);
        self.skipped_folders = skipped;
        if files.is_empty() {
            // Nothing but folders: an archive is the only way to save them.
            let root = self.zip_root();
            self.begin_zip(self.selected.clone(), root);
            return;
        }
        self.save_queue = files.into();
        self.next_save();
    }

    /// Ask where a single remote path should be saved.
    fn begin_download_one(&mut self, path: String) {
        let is_dir = self
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .is_some_and(|entry| entry.is_dir);
        if is_dir {
            let root = file_name(&path).unwrap_or("archive").to_owned();
            self.begin_zip(vec![path], root);
            return;
        }
        self.save_queue.clear();
        self.skipped_folders.clear();
        if native_dialogs_enabled() {
            self.spawn_save(path.clone(), file_name(&path).unwrap_or("download").to_owned());
        } else {
            self.save_queue
                .push_back((path.clone(), file_name(&path).unwrap_or("download").to_owned()));
            self.start_save_fallback();
        }
    }

    /// Directory the save dialog opens in.
    ///
    /// The last folder that was saved into, or the home folder the first time —
    /// a place to browse from, never a destination of its own.
    fn save_start(&self) -> String {
        if !self.save_dir.is_empty() {
            return self.save_dir.clone();
        }
        std::env::var("HOME").unwrap_or_else(|_| "/".to_owned())
    }

    /// Name a ZIP of the current selection, or of the folder on screen.
    fn zip_root(&self) -> String {
        if self.selected.is_empty() {
            // The whole folder on screen.
            return file_name(&self.path).unwrap_or("archive").to_owned();
        }
        if self.selected.len() == 1 {
            // One item keeps its own name, folder or file.
            return file_name(&self.selected[0]).unwrap_or("archive").to_owned();
        }
        // Several items: the folder on screen plus how many were picked, so
        // `docs` with three files becomes `docs-3-files.zip`.
        let folder = file_name(&self.path).unwrap_or("archive");
        format!("{folder}-{}-files", self.selected.len())
    }

    /// Ask where a ZIP of `items` should be written.
    fn begin_zip(&mut self, items: Vec<String>, root: String) {
        let title = if items.len() == 1 && items[0] == self.path {
            "Save folder as ZIP"
        } else {
            "Save selection as ZIP"
        };
        let name = format!("{root}.zip");
        if native_dialogs_enabled() {
            self.spawn_archive(items, name, title);
        } else {
            self.save = Some(SaveRequest {
                items,
                zip: true,
                dir: self.save_start(),
                name,
                skipped: Vec::new(),
            });
        }
    }

    /// Start the transfer the save dialog asked for.
    fn confirm_save(&mut self, access: &mut dyn SessionAccess) {
        let Some(request) = self.save.take() else {
            return;
        };
        let dir = request.dir.trim().to_owned();
        if dir.is_empty() {
            access.report_problem("Choose where the download should be saved.");
            return;
        }
        self.save_dir.clone_from(&dir);
        if request.zip {
            let mut name = request.name.trim().to_owned();
            if name.is_empty() {
                name = "archive.zip".to_owned();
            }
            if !name.to_lowercase().ends_with(".zip") {
                name.push_str(".zip");
            }
            let archive = join_local(&dir, &name);
            self.notice = Some((format!("Building {archive}…"), false));
            self.pending_zip = Some((request.items, archive));
            return;
        }

        let single = request.items.len() == 1;
        let mut queued = 0;
        for remote in &request.items {
            let name = if single {
                request.name.trim().to_owned()
            } else {
                file_name(remote).unwrap_or_default().to_owned()
            };
            if name.is_empty() {
                continue;
            }
            let local = join_local(&dir, &name);
            access.download(remote, &local);
            queued += 1;
        }
        if queued > 0 {
            self.notice = Some((
                format!("Downloading {queued} file{} → {dir}", plural(queued)),
                false,
            ));
        }
    }

    /// The save dialog: a local folder browser plus the name to write.
    fn show_save(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn SessionAccess) {
        let Some(mut request) = self.save.clone() else {
            return;
        };
        let muted = to_color32(palette.text_muted);
        let mut go: Option<String> = None;
        let mut close = false;
        let mut confirm = false;
        let single = request.items.len() == 1;
        let mut folders = local_entries(&request.dir);
        folders.retain(|entry| entry.is_dir);
        let title = if request.zip {
            "Save as ZIP"
        } else {
            "Save download"
        };
        let response = modal(
            ui.ctx(),
            ("files-save", 11),
            palette,
            title,
            dialog_width(ui, 560.0),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if request.dir.is_empty() {
                            "Choose a folder".to_owned()
                        } else {
                            request.dir.clone()
                        })
                        .size(11.0)
                        .color(muted),
                    )
                    .truncate(),
                );
                ui.add_space(4.0);
                ui.set_min_height(DIALOG_LIST_HEIGHT);
                egui::ScrollArea::vertical()
                    .id_salt("wassh-save-dir")
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(parent) = parent_path(&request.dir)
                            && local_row(ui, palette, "📁 ..", false) == RowClick::Double
                        {
                            go = Some(parent);
                        }
                        for entry in &folders {
                            let label = format!("📁 {}", entry.name);
                            if local_row(ui, palette, &label, false) == RowClick::Double {
                                go = Some(entry.path.clone());
                            }
                        }
                        if folders.is_empty() {
                            ui.label(
                                egui::RichText::new("No folders in here")
                                    .size(12.0)
                                    .color(muted),
                            );
                        }
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Name").size(11.0).color(muted));
                    if request.zip || single {
                        text_input(ui, palette, &mut request.name, "file name");
                    } else {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("each file keeps its own name")
                                    .size(11.0)
                                    .color(muted),
                            )
                            .truncate(),
                        );
                    }
                });
                if !request.skipped.is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "Folders are saved as an archive, not copied: {}",
                            request.skipped.join(", ")
                        ))
                        .size(11.0)
                        .color(to_color32(palette.text_muted)),
                    );
                }
                footer(ui, |ui| {
                    if small_button_enabled(ui, palette, "Cancel", true) {
                        close = true;
                    }
                    let ready = !request.dir.trim().is_empty()
                        && (!request.name.trim().is_empty() || (!request.zip && !single));
                    if small_button_enabled(ui, palette, "Save", ready) {
                        confirm = true;
                    }
                });
            },
        );
        if close || response.dismissed {
            self.save = None;
            return;
        }
        if let Some(path) = go {
            request.dir = path;
        }
        self.save = Some(request);
        if confirm {
            self.confirm_save(access);
        }
    }

    /// Copy (or move) the clipboard into the directory on screen.
    fn paste(&mut self, access: &mut dyn SessionAccess) {
        if self.clipboard.is_empty() {
            access.report_problem("Nothing has been copied.");
            return;
        }
        let items: Vec<(String, String)> = self
            .clipboard
            .iter()
            .filter_map(|from| file_name(from).map(|name| (from.clone(), join_remote(&self.path, name))))
            .collect();
        if items.is_empty() {
            access.report_problem("Nothing has been copied.");
            return;
        }
        let count = items.len();
        access.copy_many(items, self.clipboard_cut);
        self.notice = Some((
            format!(
                "{} {} item{}",
                if self.clipboard_cut { "Moving" } else { "Copying" },
                count,
                plural(count)
            ),
            false,
        ));
        if self.clipboard_cut {
            self.clipboard.clear();
            self.clipboard_cut = false;
        }
    }

    /// Open the local browser that chooses the files to upload.
    fn open_picker(&mut self) {
        self.picker_selected.clear();
        self.picker = Some(
            std::env::var("HOME").unwrap_or_else(|_| "/".to_owned()),
        );
    }

    /// The local file browser behind "⬆ Upload…".
    ///
    /// Single click toggles a file, a double click on a folder walks into it,
    /// and the footer sends everything chosen into the folder on screen.
    fn show_picker(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        access: &mut dyn SessionAccess,
        directory: String,
    ) {
        let muted = to_color32(palette.text_muted);
        let now = ui.ctx().input(|input| input.time);
        let mut go: Option<String> = None;
        let mut close = false;
        let mut upload = false;
        let mut chosen = self.picker_selected.clone();
        let mut entries = local_entries(&directory);
        entries.sort_by(|left, right| {
            right.is_dir.cmp(&left.is_dir).then_with(|| left.name.cmp(&right.name))
        });
        let response = modal(
            ui.ctx(),
            ("files-picker", 10),
            palette,
            "Upload files",
            dialog_width(ui, 560.0),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&directory).size(11.0).color(muted),
                    )
                    .truncate(),
                );
                ui.add_space(4.0);
                // Without a minimum the centred dialog hands the list only a
                // sliver of height, and every row below it stops responding.
                ui.set_min_height(DIALOG_LIST_HEIGHT);
                egui::ScrollArea::vertical()
                    .id_salt("wassh-local-picker")
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(parent) = parent_path(&directory)
                            && local_row(ui, palette, "📁 ..", false) == RowClick::Double
                        {
                            go = Some(parent);
                        }
                        for entry in &entries {
                            let label = format!(
                                "{} {}",
                                if entry.is_dir { "📁" } else { "📄" },
                                entry.name
                            );
                            let is_chosen = chosen.iter().any(|path| path == &entry.path);
                            match local_row(ui, palette, &label, is_chosen) {
                                RowClick::None => {}
                                RowClick::Single => {
                                    if entry.is_dir {
                                        go = Some(entry.path.clone());
                                    } else if let Some(at) =
                                        chosen.iter().position(|path| path == &entry.path)
                                    {
                                        chosen.remove(at);
                                    } else {
                                        chosen.push(entry.path.clone());
                                    }
                                }
                                RowClick::Double => {
                                    if entry.is_dir {
                                        go = Some(entry.path.clone());
                                    }
                                }
                            }
                        }
                        if entries.is_empty() {
                            ui.label(
                                egui::RichText::new("Empty folder").size(12.0).color(muted),
                            );
                        }
                    });
                ui.add_space(6.0);
                // The line keeps its height whether or not something is
                // chosen: a dialog that grows under the pointer moves the rows.
                let summary = if chosen.is_empty() {
                    String::new()
                } else {
                    format!("{} file{} chosen", chosen.len(), plural(chosen.len()))
                };
                ui.label(egui::RichText::new(summary).size(11.0).color(muted));
                footer(ui, |ui| {
                    if small_button_enabled(ui, palette, "Cancel", true) {
                        close = true;
                    }
                    let label = if chosen.is_empty() {
                        "Upload".to_owned()
                    } else {
                        format!("Upload ({})", chosen.len())
                    };
                    if small_button_enabled(ui, palette, &label, !chosen.is_empty()) {
                        upload = true;
                    }
                });
            },
        );
        self.picker_selected.clone_from(&chosen);
        if close || response.dismissed {
            self.picker = None;
            self.picker_selected.clear();
            return;
        }
        if let Some(directory) = go {
            self.picker = Some(directory);
            return;
        }
        if upload {
            self.picker = None;
            let files = std::mem::take(&mut self.picker_selected);
            self.send_uploads(access, files, now);
        }
    }

    /// The file viewer: text or hex dump, with a download button.
    fn show_viewer(&mut self, ui: &mut Ui, palette: &Palette, access: &mut dyn SessionAccess) {
        let Some(viewer) = self.viewer.clone() else {
            return;
        };
        let muted = to_color32(palette.text_muted);
        let cap = if is_zip(&viewer.name) {
            ARCHIVE_BYTES
        } else {
            VIEW_BYTES
        };
        let content = access.read(&viewer.path, cap);
        let mut close = false;
        let mut flip = false;
        let mut download = false;
        let response = modal(
            ui.ctx(),
            ("files-viewer", 9),
            palette,
            &viewer.name,
            dialog_width(ui, 720.0),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&viewer.path).size(11.0).color(muted),
                    )
                    .truncate(),
                );
                if let Some(content) = &content {
                    let kind = if looks_binary(&content.data) {
                        "binary"
                    } else {
                        "text"
                    };
                    let mut meta = format!("{kind} · {}", format_entry_size(content.data.len() as u64));
                    if let Some(total) = viewer.total
                        && total > content.data.len() as u64
                    {
                        meta.push_str(&format!(" of {}", format_entry_size(total)));
                    }
                    ui.label(egui::RichText::new(meta).size(11.0).color(muted));
                }
                ui.add_space(4.0);
                match &content {
                    None => {
                        ui.label(egui::RichText::new("Loading…").size(12.0).color(muted));
                    }
                    Some(content) => {
                        let binary = looks_binary(&content.data);
                        if content.truncated {
                            let mut banner = format!(
                                "Showing first {}",
                                format_entry_size(content.data.len() as u64)
                            );
                            if let Some(total) = viewer.total {
                                banner.push_str(&format!(" of {}", format_entry_size(total)));
                            }
                            banner.push_str(&format!(
                                " — file larger than the {} view limit.",
                                format_entry_size(VIEW_BYTES)
                            ));
                            ui.label(egui::RichText::new(banner).size(11.0).color(muted));
                        }
                        // An archive shows its contents instead of a hex dump.
                        let entries = if is_zip(&viewer.name) && !content.truncated {
                            zip_entries(&content.data)
                        } else {
                            None
                        };
                        if let Some(entries) = entries {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} entr{}",
                                    entries.len(),
                                    if entries.len() == 1 { "y" } else { "ies" }
                                ))
                                .size(11.0)
                                .color(muted),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("wassh-archive-viewer")
                                .max_height(360.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for entry in &entries {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(if entry.is_dir {
                                                    "📁"
                                                } else {
                                                    "📄"
                                                })
                                                .size(11.0),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&entry.name)
                                                        .size(11.5)
                                                        .color(to_color32(palette.text)),
                                                )
                                                .truncate(),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(if entry.is_dir {
                                                            "—".to_owned()
                                                        } else {
                                                            format_entry_size(entry.size)
                                                        })
                                                        .size(10.0)
                                                        .color(muted),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                });
                        } else if content.data.is_empty() {
                            ui.label(egui::RichText::new("File is empty").size(12.0).color(muted));
                        } else {
                            let body = if viewer.hex || binary {
                                hex_dump(&content.data)
                            } else {
                                viewer_text(&content.data)
                            };
                            let label = |ui: &mut Ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&body)
                                            .monospace()
                                            .size(11.0)
                                            .color(to_color32(palette.text)),
                                    )
                                    .selectable(true),
                                );
                            };
                            // A short file is drawn in place; only a long one
                            // needs a scroll view around it.
                            if body.lines().count() <= VIEW_INLINE_LINES {
                                label(ui);
                            } else {
                                egui::ScrollArea::both()
                                    .id_salt("wassh-file-viewer")
                                    .max_height(360.0)
                                    .auto_shrink([false, false])
                                    .show(ui, label);
                            }
                            if binary && !viewer.hex {
                                ui.label(
                                    egui::RichText::new("Binary file, shown as a hex dump")
                                        .size(11.0)
                                        .color(muted),
                                );
                            }
                        }
                    }
                }
                footer(ui, |ui| {
                    if small_button_enabled(ui, palette, "Close", true) {
                        close = true;
                    }
                    if small_button_enabled(ui, palette, "Download", true) {
                        download = true;
                    }
                    if small_button_enabled(ui, palette, if viewer.hex { "Text" } else { "Hex" }, true)
                    {
                        flip = true;
                    }
                });
            },
        );
        if close || response.dismissed {
            self.viewer = None;
            return;
        }
        if flip && let Some(viewer) = self.viewer.as_mut() {
            viewer.hex = !viewer.hex;
        }
        if download {
            self.begin_download_one(viewer.path.clone());
        }
    }

    /// Draw the open dialog and act on its buttons.
    fn show_dialog(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        access: &mut dyn SessionAccess,
        dialog: FileDialog,
    ) {
        let muted = to_color32(palette.text_muted);
        let id = match &dialog {
            FileDialog::NewFolder { .. } => 1_u8,
            FileDialog::Rename { .. } => 2,
            FileDialog::Permissions { .. } => 3,
            FileDialog::Delete { .. } => 4,
        };
        let title = match &dialog {
            FileDialog::NewFolder { .. } => "New folder",
            FileDialog::Rename { .. } => "Rename",
            FileDialog::Permissions { .. } => "Permissions",
            FileDialog::Delete { .. } => "Delete",
        };
        let mut draft = dialog;
        let mut cancel = false;
        let mut confirm = false;
        let response = modal(
            ui.ctx(),
            ("files-dialog", id),
            palette,
            title,
            dialog_width(ui, 460.0),
            |ui| {
            match &mut draft {
                FileDialog::NewFolder { name } => {
                    ui.label(egui::RichText::new("NAME").size(11.0).color(muted));
                    ui.add_space(4.0);
                    text_input(ui, palette, name, "new folder");
                }
                FileDialog::Rename { path, name } => {
                    ui.label(egui::RichText::new(path.as_str()).size(11.0).color(muted));
                    ui.add_space(4.0);
                    text_input(ui, palette, name, "new name");
                }
                FileDialog::Permissions { path, mode } => {
                    ui.label(egui::RichText::new(path.as_str()).size(11.0).color(muted));
                    ui.add_space(4.0);
                    text_input(ui, palette, mode, "644");
                    ui.label(
                        egui::RichText::new("Octal mode, e.g. 644, 755, 700")
                            .size(11.0)
                            .color(muted),
                    );
                }
                FileDialog::Delete { targets } => {
                    let count = targets.len();
                    ui.label(
                        egui::RichText::new(format!(
                            "Delete {count} item{}? This cannot be undone.",
                            plural(count)
                        ))
                        .size(12.0),
                    );
                    ui.add_space(4.0);
                    for (path, _) in targets.iter().take(12) {
                        ui.label(
                            egui::RichText::new(file_name(path).unwrap_or(path))
                                .size(11.0)
                                .color(muted),
                        );
                    }
                }
            }
            footer(ui, |ui| {
                if small_button_enabled(ui, palette, "Cancel", true) {
                    cancel = true;
                }
                let ready = match &draft {
                    FileDialog::NewFolder { name } => valid_name(name),
                    FileDialog::Rename { name, .. } => valid_name(name),
                    FileDialog::Permissions { mode, .. } => parse_mode(mode).is_some(),
                    FileDialog::Delete { .. } => true,
                };
                let label = match &draft {
                    FileDialog::Delete { .. } => "Delete",
                    _ => "OK",
                };
                if small_button_enabled(ui, palette, label, ready) {
                    confirm = true;
                }
            });
            },
        );

        if response.dismissed || cancel {
            self.dialog = None;
            return;
        }
        if !confirm {
            // Keep whatever was typed.
            self.dialog = Some(draft);
            return;
        }
        self.dialog = None;
        match draft {
            FileDialog::NewFolder { name } => {
                let path = join_remote(&self.path.clone(), name.trim());
                access.mkdir(&path);
                self.notice = Some((format!("Created {path}"), false));
            }
            FileDialog::Rename { path, name } => {
                let trimmed = name.trim().to_owned();
                let parent = parent_path(&path).unwrap_or_default();
                let to = join_remote(&parent, &trimmed);
                access.rename(&path, &to);
                self.notice = Some((format!("Renamed to {trimmed}"), false));
                self.selected.clear();
            }
            FileDialog::Permissions { path, mode } => {
                if let Some(bits) = parse_mode(&mode) {
                    access.chmod(&path, bits);
                    self.notice = Some((
                        format!(
                            "Mode {bits:03o} on {}",
                            file_name(&path).unwrap_or(&path)
                        ),
                        false,
                    ));
                }
            }
            FileDialog::Delete { targets } => {
                for (path, is_dir) in &targets {
                    access.remove(path, *is_dir);
                }
                self.notice = Some((
                    format!("Deleted {} item{}", targets.len(), plural(targets.len())),
                    false,
                ));
                self.selected.clear();
            }
        }
    }

    /// Mode the permissions dialog starts with for `path`.
    fn mode_of(&self, path: &str) -> String {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.permissions)
            .map(octal_mode)
            .unwrap_or_else(|| "644".to_owned())
    }

    /// The selection with the kind of each item, for the delete dialog.
    fn selection_with_kind(&self) -> Vec<(String, bool)> {
        self.selected
            .iter()
            .map(|path| {
                let is_dir = self
                    .entries
                    .iter()
                    .find(|entry| &entry.path == path)
                    .is_some_and(|entry| entry.is_dir);
                (path.clone(), is_dir)
            })
            .collect()
    }
}

/// Gap between two table columns.
const CELL_GAP: f32 = 10.0;
/// Height of the table headings.
const HEADING_HEIGHT: f32 = 19.0;
/// Height of one row.
const ROW_HEIGHT: f32 = 18.0;
/// Below this a column cannot show anything useful, so it is dropped and its
/// width goes back to the name.
const MIN_COLUMN_WIDTH: f32 = 44.0;

/// Lay the four columns out in priority order.
///
/// The name comes first, then the size, then the permissions and finally the
/// date: when the pane is too narrow the date gives way before the permissions,
/// the permissions before the size, and the size before the name. Whatever is
/// left over goes back to the name, so the row always fills the pane.
pub fn column_widths(content: f32, name_need: f32, narrow: &[f32; 3]) -> [f32; 4] {
    let mut widths = [
        name_need.max(MIN_COLUMN_WIDTH),
        narrow[0] + CELL_GAP,
        narrow[1] + CELL_GAP,
        narrow[2] + CELL_GAP,
    ];
    let mut total: f32 = widths.iter().sum();

    // Take from the least important column first.
    for index in [3, 2, 1, 0] {
        if total <= content {
            break;
        }
        let give = (total - content).min(widths[index]);
        widths[index] -= give;
        total -= give;
    }
    // A column squeezed past usefulness is dropped entirely.
    for index in [3, 2, 1] {
        if widths[index] > 0.0 && widths[index] < MIN_COLUMN_WIDTH {
            widths[0] += widths[index];
            widths[index] = 0.0;
        }
    }
    // The name takes the slack.
    let used: f32 = widths.iter().sum();
    widths[0] += (content - used).max(0.0);
    widths
}
/// Height the file browser's list asks for inside a dialog.
const DIALOG_LIST_HEIGHT: f32 = 320.0;
/// Height of a transfer's progress track.
const TRANSFER_TRACK_HEIGHT: f32 = 4.0;
/// Height one transfer row takes, including its track and the gap after it.
const TRANSFER_ROW_HEIGHT: f32 = 28.0;
/// Transfer rows shown at once; the list scrolls past this.
const TRANSFER_ROWS: usize = 5;
/// Padding above and below the transfer list.
const TRANSFERS_PADDING: f32 = 12.0;

/// Room the transfer list needs for `count` rows.
fn transfers_area_height(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count.min(TRANSFER_ROWS) as f32 * TRANSFER_ROW_HEIGHT + TRANSFERS_PADDING
    }
}

/// One table cell: ellipsised, right-aligned for the size column.
#[allow(clippy::too_many_arguments)]
fn cell_paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    offset: f32,
    width: f32,
    value: &str,
    font: &egui::FontId,
    colour: egui::Color32,
    right_aligned: bool,
    content: f32,
) {
    let room = (width - 8.0).max(8.0);
    let caption = ellipsize(painter, value, font.clone(), room);
    let used = painter
        .layout_no_wrap(caption.clone(), font.clone(), colour)
        .size()
        .x;
    let x = if right_aligned {
        rect.left() + offset + width - 4.0 - used
    } else {
        rect.left() + offset + 4.0
    };
    painter.text(
        egui::pos2(x.min(rect.left() + content - 2.0), rect.center().y),
        egui::Align2::LEFT_CENTER,
        caption,
        font.clone(),
        colour,
    );
}

/// The `sftp-toolbar-sep` divider between toolbar groups.
fn toolbar_separator(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(1.0, 18.0), egui::Sense::hover());
    ui.painter()
        .vline(rect.center().x, rect.y_range(), egui::Stroke::new(1.0, to_color32(palette.border)));
    ui.add_space(4.0);
}

/// The reference's `.sftp-drop-hint`: a dashed box over the table while files
/// are dragged in.
fn paint_drop_hint(ui: &mut Ui, palette: &Palette, area: egui::Rect, path: &str) {
    let painter = ui.painter();
    painter.rect_filled(area, 0.0, faded(palette.bg_panel, 0.8));
    let box_rect = egui::Rect::from_center_size(area.center(), egui::Vec2::new(240.0, 96.0));
    let stroke = egui::Stroke::new(2.0, to_color32(palette.accent));
    let corners = [
        box_rect.left_top(),
        box_rect.right_top(),
        box_rect.right_bottom(),
        box_rect.left_bottom(),
        box_rect.left_top(),
    ];
    painter.add(egui::Shape::dashed_line(&corners, stroke, 6.0, 4.0));
    painter.text(
        egui::pos2(box_rect.center().x, box_rect.top() + 26.0),
        egui::Align2::CENTER_CENTER,
        "⬆",
        egui::FontId::proportional(26.0),
        to_color32(palette.accent),
    );
    painter.text(
        egui::pos2(box_rect.center().x, box_rect.top() + 54.0),
        egui::Align2::CENTER_CENTER,
        "Drop files to upload",
        egui::FontId::proportional(13.0),
        to_color32(palette.text),
    );
    painter.text(
        egui::pos2(box_rect.center().x, box_rect.top() + 74.0),
        egui::Align2::CENTER_CENTER,
        format!("into {}", if path.is_empty() { "/" } else { path }),
        egui::FontId::proportional(12.0),
        to_color32(palette.text_muted),
    );
}

/// The reference's `.sftp-notice`: a pill over the bottom of the table.
fn paint_notice(ui: &mut Ui, palette: &Palette, area: egui::Rect, message: &str, failed: bool) {
    let font = egui::FontId::proportional(12.0);
    let colour = if failed {
        to_color32(palette.danger)
    } else {
        to_color32(palette.text)
    };
    let galley = {
        let mut job = egui::text::LayoutJob::simple_singleline(
            message.to_owned(),
            font,
            colour,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((area.width() - 40.0).max(40.0));
        ui.painter().layout_job(job)
    };
    let size = galley.size() + egui::Vec2::new(24.0, 12.0);
    let rect = egui::Rect::from_center_size(
        egui::pos2(area.center().x, area.bottom() - 10.0 - size.y / 2.0),
        size,
    );
    ui.painter().rect(
        rect,
        4.0,
        to_color32(palette.bg_input),
        egui::Stroke::new(1.0, to_color32(palette.border)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        colour,
    );
}

/// A message centred in `area` — the reference's `.sftp-empty` block.
fn centred_message(
    ui: &mut Ui,
    _palette: &Palette,
    area: egui::Rect,
    message: &str,
    colour: egui::Color32,
) {
    ui.painter().text(
        egui::pos2(area.center().x, area.top() + 28.0),
        egui::Align2::CENTER_CENTER,
        message,
        egui::FontId::proportional(12.0),
        colour,
    );
}

/// `-rw-r--r--`-style mode, the reference's `modeSymbolic`.
pub fn mode_symbolic(entry: &SftpEntry) -> String {
    let kind = if entry.is_link {
        'l'
    } else if entry.is_dir {
        'd'
    } else {
        '-'
    };
    let mut text = String::with_capacity(10);
    text.push(kind);
    let mode = entry.permissions.unwrap_or(0);
    for (bit, letter) in [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        text.push(if mode & bit != 0 { letter } else { '-' });
    }
    text
}


/// `—` for a directory, otherwise the size.
fn size_cell(entry: &SftpEntry) -> String {
    if entry.is_dir {
        "—".to_owned()
    } else {
        format_entry_size(entry.size)
    }
}


/// `YYYY-MM-DD HH:MM`, or `—`.
fn date_cell(entry: &SftpEntry) -> String {
    entry
        .modified
        .map(format_timestamp)
        .unwrap_or_else(|| "—".to_owned())
}

/// Files the selection should save, as (remote path, suggested name), plus the
/// folders that cannot be copied file by file.
pub fn download_plan(entries: &[SftpEntry], selected: &[String]) -> (Vec<(String, String)>, Vec<String>) {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for path in selected {
        let is_dir = entries
            .iter()
            .find(|entry| &entry.path == path)
            .is_some_and(|entry| entry.is_dir);
        if is_dir {
            skipped.push(file_name(path).unwrap_or(path).to_owned());
        } else {
            files.push((
                path.clone(),
                file_name(path).unwrap_or("download").to_owned(),
            ));
        }
    }
    (files, skipped)
}

/// `<root>.zip` for a folder or selection.
fn archive_name(root: &str) -> String {
    format!("{}.zip", file_name(root).unwrap_or("archive"))
}

/// Add the `.zip` suffix a save dialog may have dropped.
fn with_zip_suffix(path: std::path::PathBuf) -> String {
    let text = path.to_string_lossy().into_owned();
    if text.to_lowercase().ends_with(".zip") {
        text
    } else {
        format!("{text}.zip")
    }
}

/// Remember the folder a file was saved into.
fn remember_dir(save_dir: &mut String, path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        *save_dir = parent.to_string_lossy().into_owned();
    }
}

/// Whether `bytes` look like text rather than a binary file.
pub fn looks_binary(bytes: &[u8]) -> bool {
    // The reference's `isLikelyTextView`: text when no control byte shows up,
    // tab / LF / FF / CR excepted. High-bit bytes are allowed so UTF-8 and
    // latin-1 text stay readable.
    bytes
        .iter()
        .any(|byte| !matches!(byte, 0x09 | 0x0a | 0x0c | 0x0d) && (*byte < 0x20 || *byte == 0x7f))
}

/// Text of the viewer, with the trailing newline dropped.
pub fn viewer_text(bytes: &[u8]) -> String {
    // UTF-8 when it is valid, otherwise byte-per-char (latin-1), which is what
    // the reference's `decodeViewText` does.
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => bytes.iter().map(|byte| *byte as char).collect(),
    };
    text.trim_end_matches(['\n', '\r']).to_owned()
}

/// A `hexdump -C`-style dump: offset, sixteen bytes, then their characters.
pub fn hex_dump(bytes: &[u8]) -> String {
    const PER_ROW: usize = 16;
    let mut out = String::new();
    for (row, chunk) in bytes.chunks(PER_ROW).enumerate() {
        let mut hex = String::new();
        let mut text = String::new();
        for (index, byte) in chunk.iter().enumerate() {
            if index == PER_ROW / 2 {
                hex.push(' ');
            }
            hex.push_str(&format!("{byte:02x} "));
            text.push(if (0x20..0x7f).contains(byte) {
                *byte as char
            } else {
                '.'
            });
        }
        while hex.len() < PER_ROW * 3 + 1 {
            hex.push(' ');
        }
        out.push_str(&format!("{:08x}  {hex} |{text}|\n", row * PER_ROW));
    }
    out
}

/// One entry of the local file picker.
pub struct LocalEntry {
    /// File or directory name.
    pub name: String,
    /// Full local path.
    pub path: String,
    /// Whether it is a directory.
    pub is_dir: bool,
}

/// Entries of a local directory, folders first; unreadable ones read as empty.
pub fn local_entries(directory: &str) -> Vec<LocalEntry> {
    let Ok(read) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries: Vec<LocalEntry> = read
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name != "." && name != ".." {
                return None;
            }
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            Some(LocalEntry {
                name,
                path: entry.path().to_string_lossy().into_owned(),
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    entries
}

/// Join two local path parts with one separator.
pub fn join_local(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// What a clickable row received this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowClick {
    None,
    Single,
    Double,
}

/// One clickable row of the local picker.
fn local_row(ui: &mut Ui, palette: &Palette, label: &str, selected: bool) -> RowClick {
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, faded(palette.accent, 0.22));
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, faded(palette.accent, 0.08));
    }
    ui.painter().text(
        egui::pos2(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        ellipsize(
            ui.painter(),
            label,
            egui::FontId::proportional(12.0),
            rect.width() - 4.0,
        ),
        egui::FontId::proportional(12.0),
        to_color32(palette.text),
    );
    // A double click is also a click, so it has to be checked first — and a
    // third click in quick succession still means "open this".
    if response.double_clicked() || response.triple_clicked() {
        RowClick::Double
    } else if response.clicked() {
        RowClick::Single
    } else {
        RowClick::None
    }
}

/// Width of a dialog: `wanted`, but never wider than the window it opens in.
fn dialog_width(ui: &Ui, wanted: f32) -> f32 {
    let screen = ui.ctx().content_rect().width();
    (screen - 80.0).clamp(320.0, wanted.max(320.0))
}

/// `text`, shortened with an ellipsis when it is wider than `width`.
fn ellipsize(painter: &egui::Painter, text: &str, font: egui::FontId, width: f32) -> String {
    if width <= 0.0 {
        return String::new();
    }
    let measure = |candidate: &str| {
        painter
            .layout_no_wrap(candidate.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    if measure(text) <= width {
        return text.to_owned();
    }
    let mut shortened: Vec<char> = text.chars().collect();
    while !shortened.is_empty() {
        shortened.pop();
        let candidate = format!("{}…", shortened.iter().collect::<String>());
        if measure(&candidate) <= width {
            return candidate;
        }
    }
    String::new()
}

/// `""` for one item, `"s"` for several.
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// `YYYY-MM-DD HH:MM` for an epoch second count.
pub fn format_timestamp(seconds: u64) -> String {
    crate::time::format_local_seconds(seconds)
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    use super::*;

    /// Access that answers with one fixed listing and records what it was asked.
    #[derive(Default)]
    struct FakeAccess {
        listing: Option<SftpListing>,
        requested: Vec<String>,
        refreshed: Vec<String>,
        resolved: Vec<String>,
        loading: bool,
        error: Option<String>,
        uploads: Vec<(String, String)>,
        downloads: Vec<(String, String)>,
        transfer: Option<String>,
        made: Vec<String>,
        renamed: Vec<(String, String)>,
        removed: Vec<(String, bool)>,
        modes: Vec<(String, u32)>,
        contents: std::collections::HashMap<String, FileContent>,
        upload_batches: Vec<Vec<(String, String)>>,
        download_batches: Vec<Vec<(String, String)>>,
        copies: Vec<(Vec<(String, String)>, bool)>,
        trees: Vec<(Vec<String>, String)>,
        zips: Vec<(Vec<String>, String)>,
        progress_state: Option<(u64, Option<u64>)>,
        transferring: bool,
        rows: Vec<TransferRow>,
        retried: usize,
        screen: String,
        typed: String,
    }

    impl SftpAccess for FakeAccess {
        fn request_listing(&mut self, path: &str) {
            self.requested.push(path.to_owned());
        }

        fn refresh(&mut self, path: &str) {
            self.refreshed.push(path.to_owned());
        }

        fn listing(&self) -> Option<&SftpListing> {
            self.listing.as_ref()
        }

        fn is_loading(&self) -> bool {
            self.loading
        }

        fn error(&self) -> Option<&str> {
            self.error.as_deref()
        }

        fn upload(&mut self, local: &str, remote: &str) {
            self.uploads.push((local.to_owned(), remote.to_owned()));
        }

        fn download(&mut self, remote: &str, local: &str) {
            self.downloads.push((remote.to_owned(), local.to_owned()));
        }

        fn is_transferring(&self) -> bool {
            self.transferring
        }

        fn transfer_status(&self) -> Option<&str> {
            self.transfer.as_deref()
        }

        fn report_problem(&mut self, message: &str) {
            self.error = Some(message.to_owned());
        }

        fn resolve(&mut self, path: &str) -> Option<String> {
            self.resolved.push(path.to_owned());
            Some("/home/dev".to_owned())
        }

        fn mkdir(&mut self, path: &str) {
            self.made.push(path.to_owned());
        }

        fn rename(&mut self, from: &str, to: &str) {
            self.renamed.push((from.to_owned(), to.to_owned()));
        }

        fn remove(&mut self, path: &str, is_dir: bool) {
            self.removed.push((path.to_owned(), is_dir));
        }

        fn chmod(&mut self, path: &str, mode: u32) {
            self.modes.push((path.to_owned(), mode));
        }

        fn read(&mut self, path: &str, _max_bytes: u64) -> Option<FileContent> {
            self.contents.get(path).cloned()
        }

        fn upload_many(&mut self, items: Vec<(String, String)>) {
            self.upload_batches.push(items);
        }

        fn download_many(&mut self, items: Vec<(String, String)>) {
            self.download_batches.push(items);
        }

        fn copy_many(&mut self, items: Vec<(String, String)>, move_items: bool) {
            self.copies.push((items, move_items));
        }

        fn progress(&self) -> Option<(u64, Option<u64>)> {
            self.progress_state
        }

        fn download_tree(&mut self, items: Vec<String>, dir: &str) {
            self.trees.push((items, dir.to_owned()));
        }

        fn download_zip(&mut self, items: Vec<String>, archive: &str) {
            self.zips.push((items, archive.to_owned()));
        }

        fn transfers(&self) -> Vec<TransferRow> {
            self.rows.clone()
        }

        fn retry(&mut self) {
            self.retried += 1;
        }
    }

    // The pane is handed one access object for every capability.
    impl crate::monitor::MonitorAccess for FakeAccess {
        fn request_sample(&mut self) {}

        fn sample(&self) -> Option<&crate::monitor::Snapshot> {
            None
        }

        fn is_sampling(&self) -> bool {
            false
        }

        fn last_error(&self) -> Option<&str> {
            self.error.as_deref()
        }
    }

    impl crate::command::CommandAccess for FakeAccess {
        fn exec(&mut self, _key: &str, _command: &str) {}

        fn output(&self, _key: &str) -> Option<&crate::command::CommandOutput> {
            None
        }

        fn failure(&self, _key: &str) -> Option<&str> {
            None
        }

        fn is_running(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::command::TcpAccess for FakeAccess {
        fn open_forward(&mut self, _key: &str, _host: &str, _port: u16) {}

        fn forward_address(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_error(&self, _key: &str) -> Option<&str> {
            None
        }

        fn forward_pending(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::plugin::TerminalAccess for FakeAccess {
        fn screen_text(&self) -> String {
            self.screen.clone()
        }

        fn type_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }

        fn is_closed(&self) -> bool {
            false
        }
    }


    /// A click and release at `pos`.
    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
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
        ]
    }

    /// Runs frames against one context, so hover and double-click state carry
    /// over between clicks the way they do on screen.
    struct PointerHarness {
        ctx: egui::Context,
        time: f64,
    }

    impl PointerHarness {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 1.0,
            }
        }

        /// Draw one frame, returning where every text ended up.
        fn frame(
            &mut self,
            plugin: &mut Files,
            access: &mut FakeAccess,
            events: Vec<egui::Event>,
        ) -> Vec<(String, egui::Pos2)> {
            self.frame_with(plugin, access, events, 0)
        }

        /// One frame drawn into a pane `width` wide.
        fn frame_sized(
            &mut self,
            plugin: &mut Files,
            access: &mut FakeAccess,
            events: Vec<egui::Event>,
            width: f32,
        ) -> Vec<(String, egui::Pos2)> {
            let mut texts = Vec::new();
            for pass in 0..2 {
                self.time += if events.is_empty() { 0.5 } else { 0.05 };
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(width, 900.0),
                    )),
                    time: Some(self.time),
                    events: if pass == 0 { Vec::new() } else { events.clone() },
                    ..Default::default()
                };
                let mut settings = serde_json::Map::new();
                let mut output = self.ctx.run_ui(input, |ui| {
                    let mut context = PluginContext {
                        palette: Palette::dark(),
                        host_label: "dev".to_owned(),
                        status: SessionStatus::Connected,
                        settings: &mut settings,
                        scope: "host-1".to_owned(),
                        session: Some(&mut *access),
                        events: &[],
                        pending_input: Vec::new(),
                        secrets: &EMPTY_SECRETS,
                        secret_writes: Vec::new(),
                    };
                    plugin.show(ui, &mut context);
                });
                texts.clear();
                for clipped in std::mem::take(&mut output.shapes) {
                    if let egui::Shape::Text(text) = clipped.shape {
                        texts.push((text.galley.text().to_owned(), text.pos));
                    }
                }
                output.drop_without_applying_deltas();
            }
            texts
        }

        /// One frame with `hovered` files being dragged over the pane.
        fn frame_with(
            &mut self,
            plugin: &mut Files,
            access: &mut FakeAccess,
            events: Vec<egui::Event>,
            hovered: usize,
        ) -> Vec<(String, egui::Pos2)> {
            // egui attributes a click to a widget that already existed, so the
            // first pass paints the pane and only the second one gets the event.
            let mut texts = Vec::new();
            for pass in 0..2 {
                // Frames without events move the clock well past the
                // double-click window, so only clicks sent back to back chain.
                self.time += if events.is_empty() { 0.5 } else { 0.05 };
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(520.0, 900.0),
                    )),
                    time: Some(self.time),
                    events: if pass == 0 { Vec::new() } else { events.clone() },
                    hovered_files: (0..hovered)
                        .map(|index| egui::HoveredFile {
                            mime: "text/plain".to_owned(),
                            path: Some(std::path::PathBuf::from(format!("/tmp/{index}.txt"))),
                        })
                        .collect(),
                    ..Default::default()
                };
                let mut settings = serde_json::Map::new();
                let mut output = self.ctx.run_ui(input, |ui| {
                    let mut context = PluginContext {
                        palette: Palette::dark(),
                        host_label: "dev".to_owned(),
                        status: SessionStatus::Connected,
                        settings: &mut settings,
                        scope: "host-1".to_owned(),
                        session: Some(&mut *access),
                        events: &[],
                        pending_input: Vec::new(),
                        secrets: &EMPTY_SECRETS,
                        secret_writes: Vec::new(),
                    };
                    plugin.show(ui, &mut context);
                });
                texts.clear();
                for clipped in std::mem::take(&mut output.shapes) {
                    if let egui::Shape::Text(text) = clipped.shape {
                        texts.push((text.galley.text().to_owned(), text.pos));
                    }
                }
                output.drop_without_applying_deltas();
            }
            texts
        }

        /// Where `label` was painted.
        ///
        /// The last match wins: a modal is painted over the pane, and both can
        /// show a row with the same label.
        fn find(&mut self, plugin: &mut Files, access: &mut FakeAccess, label: &str) -> egui::Pos2 {
            let texts = self.frame(plugin, access, Vec::new());
            match texts.iter().rev().find(|(text, _)| text == label) {
                Some((_, pos)) => *pos,
                None => panic!(
                    "{label} was not painted; the pane shows {:?}",
                    texts.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>()
                ),
            }
        }

        fn click(&mut self, plugin: &mut Files, access: &mut FakeAccess, pos: egui::Pos2) {
            self.frame(plugin, access, press(pos));
        }
    }

    /// Draw two frames and return the painted text.
    fn draw(plugin: &mut Files, access: &mut FakeAccess) -> Vec<String> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let mut settings = serde_json::Map::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(520.0, 900.0),
            )),
            ..Default::default()
        };
        let mut painted = Vec::new();
        for _ in 0..2 {
            let mut context = PluginContext {
                palette,
                host_label: "dev".to_owned(),
                status: SessionStatus::Connected,
                settings: &mut settings,
                scope: "host-1".to_owned(),
                session: Some(access),
                events: &[],
                pending_input: Vec::new(),
                secrets: &EMPTY_SECRETS,
                secret_writes: Vec::new(),
            };
            let mut output = ctx.run_ui(input.clone(), |ui| plugin.show(ui, &mut context));
            let shapes = std::mem::take(&mut output.shapes);
            painted.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    painted.push(text.galley.text().to_owned());
                }
            }
            output.drop_without_applying_deltas();
        }
        painted
    }

    fn entry(name: &str, is_dir: bool, size: u64, mode: u32, modified: u64) -> SftpEntry {
        SftpEntry {
            name: name.to_owned(),
            path: format!("/home/dev/{name}"),
            is_dir,
            is_link: false,
            size,
            modified: Some(modified),
            permissions: Some(mode),
        }
    }

    fn listing() -> SftpListing {
        SftpListing {
            path: "/home/dev".to_owned(),
            entries: vec![
                entry("docs", true, 0, 0o755, 1_700_000_000),
                entry("readme.txt", false, 2048, 0o644, 1_700_000_100),
                entry(".hidden", false, 10, 0o600, 1_700_000_200),
            ],
        }
    }

    #[test]
    fn the_browser_opens_the_login_directory() {
        let mut plugin = Files::default();
        let mut access = FakeAccess::default();
        draw(&mut plugin, &mut access);
        assert_eq!(access.resolved, ["."]);
        assert_eq!(plugin.current_path(), "/home/dev");
        assert_eq!(access.requested.first().map(String::as_str), Some("/home/dev"));
    }

    #[test]
    fn a_listing_is_painted_with_its_mode_and_date() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let painted = draw(&mut plugin, &mut access);
        let has = |needle: &str| painted.iter().any(|text| text == needle);
        assert!(has("Name"), "headings missing from {painted:?}");
        assert!(has("Size"));
        assert!(has("Permissions"), "the mode column is the permission string");
        assert!(has("Modified"), "the reference calls the last column Modified");
        assert!(has("2.0 KB"));
        // One column, one spelling: the symbolic permissions, never the octal.
        assert!(!has("755"), "no octal duplicate: {painted:?}");
        assert!(!has("644"), "no octal duplicate: {painted:?}");
        // The date is the local wall clock, so only the day is fixed here.
        assert!(
            painted
                .iter()
                .any(|text| text.starts_with("2023-11-14") || text.starts_with("2023-11-15")),
            "{painted:?}"
        );
        assert!(has("drwxr-xr-x"), "{painted:?}");
        assert!(has("-rw-r--r--"), "{painted:?}");
        // Directories show a dash for their size.
        assert!(painted.iter().filter(|text| *text == "—").count() >= 2);
        // There is no hidden-file filter: dot-files are listed.
        assert!(
            painted.iter().any(|text| text.contains(".hidden")),
            "{painted:?}"
        );
    }

    #[test]
    fn ctrl_c_over_the_table_copies_the_selection_as_paths() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let row = pointer.find(&mut plugin, &mut access, "readme.txt");
        plugin.selected = vec!["/home/dev/readme.txt".to_owned()];

        pointer.frame(&mut plugin, &mut access, vec![egui::Event::PointerMoved(row)]);
        pointer.frame(
            &mut plugin,
            &mut access,
            vec![egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        assert_eq!(plugin.clipboard, ["/home/dev/readme.txt"]);
        assert!(!plugin.clipboard_cut, "a copy, not a cut");
        assert_eq!(plugin.selected, ["/home/dev/readme.txt"]);
        assert!(plugin.dialog.is_none());
    }

    #[test]
    fn files_dragged_over_the_pane_show_the_drop_hint() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let texts = pointer.frame_with(&mut plugin, &mut access, Vec::new(), 1);
        let painted: Vec<&str> = texts.iter().map(|(text, _)| text.as_str()).collect();
        assert!(painted.contains(&"Drop files to upload"), "{painted:?}");
        assert!(
            painted.iter().any(|text| text.contains("into /home/dev")),
            "{painted:?}"
        );
    }

    #[test]
    fn the_delete_key_asks_before_removing_the_selection() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let row = pointer.find(&mut plugin, &mut access, "readme.txt");
        plugin.selected = vec!["/home/dev/readme.txt".to_owned()];

        // Hover the row, then press Delete: the dialog opens, nothing is deleted.
        pointer.frame(&mut plugin, &mut access, vec![egui::Event::PointerMoved(row)]);
        pointer.frame(
            &mut plugin,
            &mut access,
            vec![egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        match plugin.dialog.clone() {
            Some(FileDialog::Delete { targets }) => {
                assert_eq!(targets, [("/home/dev/readme.txt".to_owned(), false)]);
            }
            other => panic!("expected the delete dialog, got {other:?}"),
        }
        assert!(access.removed.is_empty(), "nothing is removed yet");
    }

    #[test]
    fn the_up_row_needs_a_double_click() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let up = pointer.find(&mut plugin, &mut access, "📁 ..");
        assert_eq!(plugin.path, "/home/dev");

        // A single click on `..` must do nothing at all.
        pointer.click(&mut plugin, &mut access, up);
        assert_eq!(plugin.path, "/home/dev", "one click must not climb");
        assert!(access.refreshed.is_empty());

        // The second click completes the double click and climbs one level.
        pointer.click(&mut plugin, &mut access, up);
        assert_eq!(plugin.path, "/home", "a double click goes up");
        assert!(access.refreshed.contains(&"/home".to_owned()));
        assert!(plugin.selected.is_empty());
        assert!(plugin.viewer.is_none(), "and it must not open a row");
    }

    #[test]
    fn a_folder_needs_a_double_click() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let docs = pointer.find(&mut plugin, &mut access, "docs");

        pointer.click(&mut plugin, &mut access, docs);
        assert_eq!(plugin.path, "/home/dev", "a single click only selects");
        assert_eq!(plugin.selected, vec!["/home/dev/docs".to_owned()]);

        // The second click of the pair lands inside the double-click window.
        pointer.click(&mut plugin, &mut access, docs);
        assert_eq!(plugin.path, "/home/dev/docs", "a double click opens");
        assert!(plugin.selected.is_empty(), "opening clears the selection");
    }

    #[test]
    fn a_file_needs_a_double_click_to_open_the_viewer() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            contents: [("/home/dev/readme.txt".to_owned(), FileContent {
                data: b"hello".to_vec(),
                truncated: false,
            })]
            .into_iter()
            .collect(),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let file = pointer.find(&mut plugin, &mut access, "readme.txt");
        pointer.click(&mut plugin, &mut access, file);
        assert!(plugin.viewer.is_none(), "a single click must not open it");
        pointer.click(&mut plugin, &mut access, file);
        assert!(plugin.viewer.is_some(), "a double click opens the viewer");
    }

    #[test]
    fn the_upload_browser_sends_the_chosen_files_into_the_folder_on_screen() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        draw(&mut plugin, &mut access);
        plugin.path = "/home/dev/docs".to_owned();
        plugin.draft = plugin.path.clone();

        let dir = std::env::temp_dir().join("wassh-upload-browser");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("b.txt"), b"b").unwrap();

        plugin.open_picker();
        plugin.picker = Some(dir.display().to_string());

        // A single click chooses a file; two files can be chosen at once.
        let a = pointer.find(&mut plugin, &mut access, "📄 a.txt");
        pointer.click(&mut plugin, &mut access, a);
        let b = pointer.find(&mut plugin, &mut access, "📄 b.txt");
        pointer.click(&mut plugin, &mut access, b);
        assert_eq!(plugin.picker_selected.len(), 2, "{:?}", plugin.picker_selected);

        // The footer button sends them into the current folder.
        let upload = pointer.find(&mut plugin, &mut access, "Upload (2)");
        pointer.click(&mut plugin, &mut access, upload);
        assert!(plugin.picker.is_none(), "the browser closes");
        assert_eq!(
            access.upload_batches.last().cloned().unwrap_or_default(),
            [
                (
                    dir.join("a.txt").display().to_string(),
                    "/home/dev/docs/a.txt".to_owned()
                ),
                (
                    dir.join("b.txt").display().to_string(),
                    "/home/dev/docs/b.txt".to_owned()
                ),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_in_the_upload_browser_is_entered_with_a_double_click() {
        let mut plugin = Files::default();
        let mut access = FakeAccess::default();
        let mut pointer = PointerHarness::new();
        let dir = std::env::temp_dir().join("wassh-upload-nested");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/inner.txt"), b"x").unwrap();

        plugin.open_picker();
        plugin.picker = Some(dir.display().to_string());
        let nested = pointer.find(&mut plugin, &mut access, "📁 nested");
        pointer.click(&mut plugin, &mut access, nested);
        pointer.click(&mut plugin, &mut access, nested);
        assert_eq!(
            plugin.picker.as_deref(),
            Some(dir.join("nested").display().to_string().as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_connection_state_is_handed_to_the_pane_header() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        // Nothing to show before the first frame has resolved the login dir.
        assert!(plugin.header_status().is_none());
        draw(&mut plugin, &mut access);
        let (text, health) = plugin.header_status().expect("a header line");
        assert_eq!(text, "Connected", "the header carries no path hint");
        assert_eq!(health, HeaderHealth::Ok);
        // The pane itself no longer paints a status row.
        let painted = draw(&mut plugin, &mut access);
        assert!(!painted.iter().any(|text| text.contains("uploads go to")));
    }

    #[test]
    fn parent_paths_climb_to_the_root() {
        assert_eq!(parent_path("/home/alice/file"), Some("/home/alice".to_owned()));
        assert_eq!(parent_path("/home"), Some("/".to_owned()));
        assert_eq!(parent_path("/"), None);
        assert_eq!(parent_path(""), None);
        assert_eq!(parent_path("/home/alice/"), Some("/home".to_owned()));
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
        // The table drops the fraction once the number is wide.
        assert_eq!(format_entry_size(2048), "2.0 KB");
        assert_eq!(format_entry_size(20 * 1024), "20 KB");
        // Zero reads as a dash, like the reference's `formatBytes`.
        assert_eq!(format_entry_size(0), "—");
    }

    #[test]
    fn remote_paths_join_with_one_separator() {
        assert_eq!(join_remote("/home/alice", "report.txt"), "/home/alice/report.txt");
        assert_eq!(join_remote("/home/alice/", "report.txt"), "/home/alice/report.txt");
        assert_eq!(join_remote("", "report.txt"), "/report.txt");
        assert_eq!(join_remote("/", "report.txt"), "/report.txt");
    }

    #[test]
    fn file_names_survive_trailing_separators() {
        assert_eq!(file_name("/home/alice/report.txt"), Some("report.txt"));
        assert_eq!(file_name("/home/alice/"), Some("alice"));
        assert_eq!(file_name("report.txt"), Some("report.txt"));
        assert_eq!(file_name("C:\\Users\\alice\\notes.txt"), Some("notes.txt"));
        assert_eq!(file_name("/"), None);
        assert_eq!(file_name(""), None);
    }

    #[test]
    fn modes_are_octal_and_validated() {
        assert_eq!(octal_mode(0o644), "644");
        assert_eq!(octal_mode(0o100_644), "644", "the file-type bits are dropped");
        assert_eq!(parse_mode("644"), Some(0o644));
        assert_eq!(parse_mode(" 755 "), Some(0o755));
        assert_eq!(parse_mode("0o700"), Some(0o700));
        assert_eq!(parse_mode(""), None);
        assert_eq!(parse_mode("789"), None, "8 and 9 are not octal");
        assert_eq!(parse_mode("0644"), Some(0o644));
        assert_eq!(parse_mode("abc"), None);
    }

    #[test]
    fn names_that_could_escape_the_directory_are_refused() {
        assert!(valid_name("report.txt"));
        assert!(valid_name(" spaced "));
        assert!(!valid_name(""));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
    }

    #[test]
    fn timestamps_show_the_local_wall_clock() {
        // 1970-01-01T00:00Z, shifted by the machine's zone.
        let epoch = format_timestamp(0);
        assert_eq!(epoch.len(), 16, "{epoch}");
        assert!(epoch.starts_with("1970-01-01"), "{epoch}");
        // Whatever the zone, the value must be a real minute of that day.
        let value = format_timestamp(1_700_000_000);
        assert_eq!(value.len(), 16, "{value}");
        assert!(value.starts_with("2023-11-14") || value.starts_with("2023-11-15"));
    }

    #[test]
    fn icons_say_what_a_row_is() {
        let mut file = entry("a.txt", false, 1, 0o644, 0);
        assert_eq!(type_icon(&file), "📄");
        file.is_link = true;
        assert_eq!(type_icon(&file), "🔗");
        assert_eq!(type_icon(&entry("docs", true, 0, 0o755, 0)), "📁");
    }

    #[test]
    fn local_paths_join_and_list() {
        assert_eq!(join_local("/home/dev", "a.txt"), "/home/dev/a.txt");
        assert_eq!(join_local("/home/dev/", "a.txt"), "/home/dev/a.txt");

        let dir = std::env::temp_dir().join("wassh-local-listing");
        std::fs::create_dir_all(dir.join("nested")).expect("temp dir");
        std::fs::write(dir.join("b.txt"), b"x").expect("write");
        std::fs::write(dir.join(".hidden"), b"x").expect("write");
        let entries = local_entries(&dir.to_string_lossy());
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        // Folders first, dot-files hidden.
        assert_eq!(names, ["nested", "b.txt"]);
        assert!(entries[0].is_dir);
        assert!(!entries[1].is_dir);
    }

    #[test]
    fn zip_asks_where_the_archive_should_be_saved() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.begin_zip(vec![plugin.path.clone()], plugin.zip_root());
        // Nothing is written until the dialog is confirmed.
        assert!(access.zips.is_empty(), "no archive before a destination");
        let request = plugin.save.clone().expect("a save dialog");
        assert!(request.zip);
        assert_eq!(request.name, "dev.zip");

        plugin.save.as_mut().unwrap().dir = "/tmp".to_owned();
        plugin.confirm_save(&mut access);
        let (items, archive) = plugin.pending_zip.clone().expect("an archive path");
        assert_eq!(archive, "/tmp/dev.zip");
        draw(&mut plugin, &mut access);
        assert_eq!(access.zips.len(), 1);
        assert_eq!(access.zips[0].0, items);
        assert_eq!(access.zips[0].1, archive);
    }

    #[test]
    fn a_zip_is_named_after_the_folder_it_covers() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        // Nothing selected: the folder on screen.
        assert_eq!(plugin.zip_root(), "dev");
        // One folder selected: that folder's own name.
        plugin.selected = vec!["/home/dev/docs".to_owned()];
        assert_eq!(plugin.zip_root(), "docs");
        // One file selected: the file's name.
        plugin.selected = vec!["/home/dev/readme.txt".to_owned()];
        assert_eq!(plugin.zip_root(), "readme.txt");
    }

    #[test]
    fn a_zip_of_several_items_is_named_after_the_folder_and_the_count() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.selected = vec![
            "/home/dev/docs".to_owned(),
            "/home/dev/readme.txt".to_owned(),
            "/home/dev/.hidden".to_owned(),
        ];
        plugin.begin_zip(plugin.selected.clone(), plugin.zip_root());
        // The folder on screen (`dev`) plus how many were picked.
        assert_eq!(plugin.save.as_ref().unwrap().name, "dev-3-files.zip");
        plugin.save.as_mut().unwrap().dir = "/tmp".to_owned();
        plugin.confirm_save(&mut access);
        draw(&mut plugin, &mut access);
        assert_eq!(
            access.zips[0].0,
            [
                "/home/dev/docs",
                "/home/dev/readme.txt",
                "/home/dev/.hidden"
            ]
        );
    }

    #[test]
    fn a_typed_archive_name_gets_its_extension() {
        let mut plugin = Files::default();
        let mut access = FakeAccess::default();
        plugin.begin_zip(vec!["/home/dev".to_owned()], "dev".to_owned());
        let request = plugin.save.as_mut().unwrap();
        request.dir = "/tmp".to_owned();
        request.name = "backup".to_owned();
        plugin.confirm_save(&mut access);
        assert_eq!(
            plugin.pending_zip.clone().expect("an archive path").1,
            "/tmp/backup.zip"
        );
    }

    #[test]
    fn a_running_transfer_shows_how_far_it_has_got() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        // The list only draws rows while a transfer is in flight.
        access.transferring = true;
        access.rows = vec![TransferRow {
            direction: TransferDirection::Download,
            path: "/srv/big.iso".to_owned(),
            done: 1024,
            total: Some(2048),
            state: TransferState::Running,
        }];
        let painted = draw(&mut plugin, &mut access);
        assert!(
            painted.iter().any(|text| text == "1.0 KB / 2.0 KB"),
            "{painted:?}"
        );
        assert!(
            painted.iter().any(|text| text == "/srv/big.iso"),
            "{painted:?}"
        );
    }

    #[test]
    fn zip_archives_list_their_entries() {
        // A tiny store-only archive: one directory, two files.
        let mut writer = ZipForTest::default();
        writer.add("docs/", b"");
        writer.add("docs/a.txt", b"hello");
        writer.add("b.bin", &[0u8, 1, 2, 3]);
        let entries = zip_entries(&writer.finish()).expect("a readable archive");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["docs/", "docs/a.txt", "b.bin"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 5);
        assert_eq!(entries[2].size, 4);
        assert!(zip_entries(b"not an archive").is_none());
        assert!(is_zip("photos.ZIP"));
        assert!(!is_zip("notes.txt"));
    }

    /// Minimal store-only ZIP writer, so the reader is tested against a real
    /// central directory rather than a hand-written blob.
    #[derive(Default)]
    struct ZipForTest {
        entries: Vec<(String, Vec<u8>)>,
    }

    impl ZipForTest {
        fn add(&mut self, name: &str, data: &[u8]) {
            self.entries.push((name.to_owned(), data.to_vec()));
        }

        fn finish(self) -> Vec<u8> {
            let mut out = Vec::new();
            let mut directory = Vec::new();
            for (name, data) in &self.entries {
                let offset = out.len() as u32;
                let crc = crc_of(data);
                out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
                out.extend_from_slice(&20u16.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&crc.to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(data);

                directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
                directory.extend_from_slice(&20u16.to_le_bytes());
                directory.extend_from_slice(&20u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&crc.to_le_bytes());
                directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
                directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
                directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u16.to_le_bytes());
                directory.extend_from_slice(&0u32.to_le_bytes());
                directory.extend_from_slice(&offset.to_le_bytes());
                directory.extend_from_slice(name.as_bytes());
            }
            let start = out.len() as u32;
            out.extend_from_slice(&directory);
            out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
            out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
            out.extend_from_slice(&start.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out
        }
    }

    fn crc_of(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn the_transfer_list_shows_each_item_and_offers_a_resume() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            rows: vec![
                TransferRow {
                    direction: TransferDirection::Upload,
                    path: "/srv/a.txt".to_owned(),
                    done: 2048,
                    total: Some(2048),
                    state: TransferState::Done,
                },
                TransferRow {
                    direction: TransferDirection::Upload,
                    path: "/srv/b.txt".to_owned(),
                    done: 512,
                    total: Some(4096),
                    state: TransferState::Running,
                },
                TransferRow {
                    direction: TransferDirection::Download,
                    path: "/srv/c.txt".to_owned(),
                    done: 0,
                    total: None,
                    state: TransferState::Failed,
                },
            ],
            ..FakeAccess::default()
        };
        let painted = draw(&mut plugin, &mut access);
        let has = |needle: &str| painted.iter().any(|text| text.contains(needle));
        // Direction, remote path and amount, like the reference's list.
        assert!(painted.iter().any(|text| text == "⬆ Upload"), "{painted:?}");
        assert!(painted.iter().any(|text| text == "⬇ Download"), "{painted:?}");
        assert!(painted.iter().any(|text| text == "/srv/a.txt"), "{painted:?}");
        assert!(has("512 B / 4.0 KB"), "{painted:?}");
        assert!(has("failed"), "{painted:?}");
        assert!(painted.iter().any(|text| text == "Retry"), "{painted:?}");
    }

    #[test]
    fn a_transfer_row_reports_its_completion() {
        let mut row = TransferRow {
            done: 512,
            total: Some(2048),
            ..TransferRow::default()
        };
        assert_eq!(row.fraction(), Some(0.25));
        row.total = None;
        assert_eq!(row.fraction(), None);
        row.total = Some(0);
        assert_eq!(row.fraction(), None);
    }

    #[test]
    fn hex_dumps_look_like_hexdump_c() {
        let dump = hex_dump(b"hello world");
        assert!(dump.starts_with("00000000  68 65 6c 6c 6f 20 77 6f  72 6c 64  "), "{dump}");
        assert!(dump.contains("|hello world|"), "{dump}");
        // Non-printing bytes become dots in the character column.
        assert!(hex_dump(&[0x00, 0x41]).contains("|.A|"));
        assert!(hex_dump(b"").is_empty());
    }

    #[test]
    fn binary_files_are_told_apart_from_text() {
        assert!(!looks_binary(b"plain log line\n"));
        assert!(!looks_binary(b"tab\there\r\nform \x0c feed\n"));
        assert!(looks_binary(b"\x00\x01\x02"));
        // The reference's sniff: one escape byte is enough to call it binary.
        assert!(looks_binary(b"line\x1b[0m done\n"));
        assert!(looks_binary(b"bell\x07\n"));
        assert_eq!(viewer_text(b"hello\r\n"), "hello");
        // Not UTF-8: fall back to byte-per-char instead of losing characters.
        assert_eq!(viewer_text(&[0xE9, b'u']), "éu");
    }

    #[test]
    fn pasting_copies_into_the_directory_on_screen() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);

        plugin.clipboard = vec!["/srv/report.txt".to_owned()];
        plugin.paste(&mut access);
        assert_eq!(
            access.copies,
            [(
                vec![("/srv/report.txt".to_owned(), "/home/dev/report.txt".to_owned())],
                false
            )]
        );
        // A copy leaves the clipboard alone.
        assert_eq!(plugin.clipboard.len(), 1);
    }

    #[test]
    fn cutting_moves_and_empties_the_clipboard() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);

        plugin.clipboard = vec!["/srv/report.txt".to_owned()];
        plugin.clipboard_cut = true;
        plugin.paste(&mut access);
        assert_eq!(
            access.copies,
            [(
                vec![("/srv/report.txt".to_owned(), "/home/dev/report.txt".to_owned())],
                true
            )]
        );
        assert!(plugin.clipboard.is_empty());
        assert!(!plugin.clipboard_cut);
    }

    #[test]
    fn the_viewer_shows_a_text_file() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        access.contents.insert(
            "/home/dev/readme.txt".to_owned(),
            FileContent {
                data: b"hello\nworld\n".to_vec(),
                truncated: false,
            },
        );
        plugin.viewer = Some(Viewer {
            name: "readme.txt".to_owned(),
            path: "/home/dev/readme.txt".to_owned(),
            total: Some(2048),
            hex: false,
        });
        let painted = draw(&mut plugin, &mut access);
        assert!(
            painted.iter().any(|text| text == "hello\nworld"),
            "{painted:?}"
        );
    }

    #[test]
    fn the_viewer_dumps_binary_files() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        access.contents.insert(
            "/home/dev/blob.bin".to_owned(),
            FileContent {
                data: vec![0x00, 0x41, 0x42],
                truncated: true,
            },
        );
        plugin.viewer = Some(Viewer {
            name: "blob.bin".to_owned(),
            path: "/home/dev/blob.bin".to_owned(),
            total: None,
            hex: false,
        });
        let painted = draw(&mut plugin, &mut access);
        assert!(
            painted.iter().any(|text| text.contains("00000000")),
            "{painted:?}"
        );
        assert!(painted.iter().any(|text| text.contains("done as a hex dump") || text.contains("hex dump")), "{painted:?}");
    }

    #[test]
    fn uploading_lands_in_the_folder_on_screen() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        // Walk into a sub-folder, then upload: the file must land there.
        plugin.path = "/home/dev/docs".to_owned();
        plugin.draft = plugin.path.clone();
        plugin.send_uploads(&mut access, vec!["/tmp/notes.txt".to_owned()], 0.0);
        assert_eq!(
            access.upload_batches.last().cloned().unwrap_or_default(),
            [("/tmp/notes.txt".to_owned(), "/home/dev/docs/notes.txt".to_owned())]
        );
        // Dropping files behaves the same way.
        let items: Vec<(String, String)> = ["/tmp/a.txt", "/tmp/b.txt"]
            .iter()
            .filter_map(|local| {
                file_name(local).map(|name| (local.to_string(), join_remote(&plugin.path, name)))
            })
            .collect();
        access.upload_many(items);
        assert_eq!(
            access.upload_batches.last().cloned().unwrap_or_default(),
            [
                ("/tmp/a.txt".to_owned(), "/home/dev/docs/a.txt".to_owned()),
                ("/tmp/b.txt".to_owned(), "/home/dev/docs/b.txt".to_owned()),
            ]
        );
    }

    #[test]
    fn uploading_sends_the_local_file_into_the_current_directory() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.send_uploads(&mut access, vec!["/tmp/notes.txt".to_owned()], 0.0);
        assert_eq!(
            access.upload_batches.last().cloned().unwrap_or_default(),
            [("/tmp/notes.txt".to_owned(), "/home/dev/notes.txt".to_owned())]
        );
        assert!(plugin.notice.is_some());

        // Without a file there is nothing to send.
        plugin.send_uploads(&mut access, Vec::new(), 0.0);
        assert_eq!(access.upload_batches.len(), 1);
        assert!(access.error.is_some());
    }

    #[test]
    fn columns_give_way_in_priority_order() {
        // Roomy: everyone gets what it asked for, the name takes the slack.
        let wide = column_widths(1000.0, 300.0, &[60.0, 90.0, 130.0]);
        assert_eq!(wide[0], 1000.0 - (60.0 + 90.0 + 130.0) - 3.0 * CELL_GAP);
        assert!(wide[1] > 0.0 && wide[2] > 0.0 && wide[3] > 0.0);

        // Tight: the date goes first, then the permissions, then the size.
        let tight = column_widths(420.0, 300.0, &[60.0, 90.0, 130.0]);
        assert_eq!(tight[3], 0.0, "the date is the first to go");
        assert!(tight[2] > 0.0, "the permissions survive the date");
        assert_eq!(tight[0] + tight[1] + tight[2], 420.0);

        // Tighter: only the name and the size are left.
        let tighter = column_widths(360.0, 280.0, &[60.0, 90.0, 130.0]);
        assert_eq!((tighter[2], tighter[3]), (0.0, 0.0));
        assert!(tighter[1] > 0.0, "the size outlasts the permissions");

        // Too narrow for anything but the name.
        let tiny = column_widths(200.0, 280.0, &[60.0, 90.0, 130.0]);
        assert_eq!((tiny[1], tiny[2], tiny[3]), (0.0, 0.0, 0.0));
        assert_eq!(tiny[0], 200.0, "the name keeps the whole row");
    }

    #[test]
    fn the_name_gets_its_room_before_the_other_columns() {
        // As long as the pane allows once the date has given way.
        let long = "a-long-file-name-that-needs-its-space.txt";
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(SftpListing {
                path: "/home/dev".to_owned(),
                entries: vec![SftpEntry {
                    name: long.to_owned(),
                    path: format!("/home/dev/{long}"),
                    is_dir: false,
                    is_link: false,
                    size: 2 * 1024 * 1024,
                    modified: Some(1_700_000_000),
                    permissions: Some(0o644),
                }],
            }),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        let texts = pointer.frame(&mut plugin, &mut access, Vec::new());
        let find = |needle: &str| {
            texts
                .iter()
                .find(|(text, _)| text == needle)
                .map(|(_, pos)| *pos)
        };

        // Priority one: the name is painted whole, nothing elided.
        assert!(find(long).is_some(), "the name was cut: {texts:?}");
        // Priority two: the size column is still there.
        assert!(find("Size").is_some(), "{texts:?}");
        let size = find("Size").expect("the size heading");
        assert!(
            size.x > 520.0 * 0.46,
            "the name column ends at {} — it did not get the spare room",
            size.x
        );
    }

    #[test]
    fn a_narrow_pane_still_shows_a_readable_name_column() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        // The narrowest pane the dock allows must not squeeze the name away.
        let mut pointer = PointerHarness::new();
        let texts = pointer.frame_sized(&mut plugin, &mut access, Vec::new(), 300.0);
        let name = texts
            .iter()
            .find(|(text, _)| text.contains("readme"))
            .map(|(_, pos)| *pos)
            .unwrap_or_else(|| panic!("no name painted: {texts:?}"));
        assert!(name.x < 60.0, "the name starts at the left edge");
    }

    #[test]
    fn a_download_plan_names_every_file_and_reports_folders() {
        let entries = listing().entries;
        let (files, skipped) = download_plan(
            &entries,
            &[
                "/home/dev/docs".to_owned(),
                "/home/dev/readme.txt".to_owned(),
                "/srv/blob.bin".to_owned(),
            ],
        );
        assert_eq!(
            files,
            [
                ("/home/dev/readme.txt".to_owned(), "readme.txt".to_owned()),
                ("/srv/blob.bin".to_owned(), "blob.bin".to_owned()),
            ]
        );
        assert_eq!(skipped, ["docs"]);
    }

    #[test]
    fn every_file_in_a_batch_gets_its_own_save_step() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.selected = vec![
            "/home/dev/readme.txt".to_owned(),
            "/home/dev/docs".to_owned(),
        ];
        let listing = listing();
        plugin.begin_download(&mut access, &listing);
        // Without OS dialogs the pane asks once for the whole batch, but the
        // plan itself is one entry per file (checked above).
        let request = plugin.save.clone().expect("the fallback dialog");
        assert_eq!(request.items, ["/home/dev/readme.txt"]);
        assert_eq!(request.skipped, ["docs"]);
        assert!(plugin.save_queue.is_empty(), "the queue moved into the dialog");
    }

    #[test]
    fn downloading_asks_where_to_save_and_skips_folders() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.selected = vec![
            "/home/dev/docs".to_owned(),
            "/home/dev/readme.txt".to_owned(),
        ];
        let listing = listing();
        plugin.begin_download(&mut access, &listing);

        // A destination has to be chosen: nothing moves before that.
        assert!(access.downloads.is_empty(), "no default download path");
        let request = plugin.save.clone().expect("a save dialog");
        assert_eq!(request.items, ["/home/dev/readme.txt"]);
        assert_eq!(request.name, "readme.txt", "the name is filled in");
        assert_eq!(request.skipped, ["docs"], "folders are not copied");

        plugin.save.as_mut().unwrap().dir = "/tmp".to_owned();
        plugin.confirm_save(&mut access);
        assert_eq!(
            access.downloads,
            [("/home/dev/readme.txt".to_owned(), "/tmp/readme.txt".to_owned())]
        );
        let (notice, failed) = plugin.notice.clone().expect("a notice");
        assert!(notice.contains("Downloading 1 file"), "{notice}");
        assert!(!failed);
        assert_eq!(plugin.save_dir, "/tmp", "the folder is remembered");
    }

    #[test]
    fn the_save_dialog_paints_the_folder_the_name_and_save() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        let mut pointer = PointerHarness::new();
        plugin.selected = vec!["/home/dev/readme.txt".to_owned()];
        let listing = listing();
        plugin.begin_download(&mut access, &listing);
        plugin.save.as_mut().unwrap().dir = "/tmp".to_owned();

        let texts = pointer.frame(&mut plugin, &mut access, Vec::new());
        let painted: Vec<&str> = texts.iter().map(|(text, _)| text.as_str()).collect();
        assert!(painted.contains(&"Save download"), "{painted:?}");
        assert!(painted.contains(&"Name"), "{painted:?}");
        assert!(painted.contains(&"readme.txt"), "the name is prefilled: {painted:?}");
        assert!(painted.iter().any(|text| text.contains("/tmp")), "{painted:?}");
        assert!(painted.contains(&"Save"), "{painted:?}");
        assert!(access.downloads.is_empty());

        // Clicking Save starts the download in the folder on screen.
        let save = pointer.find(&mut plugin, &mut access, "Save");
        pointer.click(&mut plugin, &mut access, save);
        assert_eq!(
            access.downloads,
            [("/home/dev/readme.txt".to_owned(), "/tmp/readme.txt".to_owned())]
        );
        assert!(plugin.save.is_none(), "the dialog closes");
    }

    #[test]
    fn cancelling_the_save_dialog_downloads_nothing() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        plugin.selected = vec!["/home/dev/readme.txt".to_owned()];
        let listing = listing();
        plugin.begin_download(&mut access, &listing);
        plugin.save = None;
        assert!(access.downloads.is_empty());
        assert!(plugin.notice.is_none());
    }

    #[test]
    fn a_single_download_keeps_the_name_the_dialog_was_given() {
        let mut plugin = Files::default();
        let mut access = FakeAccess::default();
        plugin.begin_download_one("/srv/report.bin".to_owned());
        let request = plugin.save.as_mut().expect("a save dialog");
        request.dir = "/tmp".to_owned();
        request.name = "renamed.bin".to_owned();
        plugin.confirm_save(&mut access);
        assert_eq!(
            access.downloads,
            [("/srv/report.bin".to_owned(), "/tmp/renamed.bin".to_owned())]
        );
    }

    #[test]
    fn several_downloads_keep_their_own_names() {
        let mut plugin = Files::default();
        let mut access = FakeAccess::default();
        plugin.selected = vec!["/srv/a.txt".to_owned(), "/srv/b.txt".to_owned()];
        plugin.begin_download(&mut access, &SftpListing {
            path: "/srv".to_owned(),
            entries: vec![
                SftpEntry {
                    name: "a.txt".to_owned(),
                    path: "/srv/a.txt".to_owned(),
                    is_dir: false,
                    is_link: false,
                    size: 1,
                    modified: None,
                    permissions: None,
                },
                SftpEntry {
                    name: "b.txt".to_owned(),
                    path: "/srv/b.txt".to_owned(),
                    is_dir: false,
                    is_link: false,
                    size: 1,
                    modified: None,
                    permissions: None,
                },
            ],
        });
        plugin.save.as_mut().unwrap().dir = "/tmp".to_owned();
        plugin.confirm_save(&mut access);
        assert_eq!(
            access.downloads,
            [
                ("/srv/a.txt".to_owned(), "/tmp/a.txt".to_owned()),
                ("/srv/b.txt".to_owned(), "/tmp/b.txt".to_owned()),
            ]
        );
    }

    #[test]
    fn a_folder_only_selection_falls_back_to_a_zip() {
        let mut plugin = Files::default();
        let mut access = FakeAccess {
            listing: Some(listing()),
            ..FakeAccess::default()
        };
        draw(&mut plugin, &mut access);
        plugin.selected = vec!["/home/dev/docs".to_owned()];
        let listing = listing();
        plugin.begin_download(&mut access, &listing);
        let request = plugin.save.expect("a save dialog");
        assert!(request.zip, "there is no file to copy a folder to");
        assert_eq!(request.items, ["/home/dev/docs"]);
        assert_eq!(request.name, "docs.zip");
        assert!(access.downloads.is_empty());
    }
}
