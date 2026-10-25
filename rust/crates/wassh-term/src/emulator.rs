//! VT emulation, wrapping `alacritty_terminal`.
//!
//! The parser and grid come from
//! `alacritty_terminal`; this module exposes a small, renderer-friendly surface:
//! feed bytes, resize, read a snapshot, and receive semantic events (bell, title).

use std::sync::mpsc::Sender;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode, point_to_viewport};
use alacritty_terminal::vte::ansi;
use wassh_core::{Palette, Rgba};

use crate::grid::TerminalSize;

/// Events raised by the emulator while parsing output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Bell,
    Title(String),
    ClipboardStore(String),
    Wakeup,
}

/// `alacritty_terminal` asks for a grid description through this trait.
struct TermDims {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDims {
    fn columns(&self) -> usize {
        self.cols
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn total_lines(&self) -> usize {
        self.rows
    }
}

#[derive(Clone)]
struct EventProxy {
    tx: Sender<TerminalEvent>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let mapped = match event {
            Event::Bell => Some(TerminalEvent::Bell),
            Event::Title(title) => Some(TerminalEvent::Title(title)),
            Event::PtyWrite(text) => Some(TerminalEvent::ClipboardStore(text)),
            Event::Wakeup => Some(TerminalEvent::Wakeup),
            // Colour queries, clipboard requests and cursor-change notifications are wired
            // up in Phase 2 when the terminal owns a real clipboard.
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = self.tx.send(event);
        }
    }
}

/// The sixteen ANSI colours plus defaults, resolved from the app palette.
#[derive(Clone, Copy, Debug)]
pub struct AnsiPalette {
    colors: [Rgba; 16],
    pub foreground: Rgba,
    pub background: Rgba,
}

impl AnsiPalette {
    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            colors: [
                Rgba::rgb(0x1b1d21),
                Rgba::rgb(0xe35d6a),
                Rgba::rgb(0x3dd68c),
                Rgba::rgb(0xf0b429),
                Rgba::rgb(0x3d8bfd),
                Rgba::rgb(0x8b5cf6),
                Rgba::rgb(0x14b8a6),
                Rgba::rgb(0xe8eaed),
                Rgba::rgb(0x5f6368),
                Rgba::rgb(0xff7b86),
                Rgba::rgb(0x6ee7b7),
                Rgba::rgb(0xffd166),
                Rgba::rgb(0x5a9dff),
                Rgba::rgb(0xa78bfa),
                Rgba::rgb(0x2dd4bf),
                Rgba::rgb(0xffffff),
            ],
            foreground: palette.term_fg,
            background: palette.bg_term,
        }
    }

    fn resolve(&self, color: ansi::Color) -> Rgba {
        match color {
            ansi::Color::Spec(rgb) => Rgba::rgb(
                u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b),
            ),
            ansi::Color::Indexed(index) => self.indexed(index),
            ansi::Color::Named(named) => self.named(named),
        }
    }

    fn indexed(&self, index: u8) -> Rgba {
        match index {
            0..=15 => self.colors[usize::from(index)],
            16..=231 => {
                let i = usize::from(index) - 16;
                let steps = [0u8, 95, 135, 175, 215, 255];
                let r = steps[(i / 36) % 6];
                let g = steps[(i / 6) % 6];
                let b = steps[i % 6];
                Rgba::rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
            }
            _ => {
                let level = 8 + (u32::from(index) - 232) * 10;
                Rgba::rgb(level << 16 | level << 8 | level)
            }
        }
    }

    fn named(&self, named: ansi::NamedColor) -> Rgba {
        use ansi::NamedColor as N;
        match named {
            N::Foreground | N::BrightForeground | N::DimForeground | N::Cursor => self.foreground,
            N::Background => self.background,
            N::Black | N::DimBlack => self.colors[0],
            N::Red | N::DimRed => self.colors[1],
            N::Green | N::DimGreen => self.colors[2],
            N::Yellow | N::DimYellow => self.colors[3],
            N::Blue | N::DimBlue => self.colors[4],
            N::Magenta | N::DimMagenta => self.colors[5],
            N::Cyan | N::DimCyan => self.colors[6],
            N::White | N::DimWhite => self.colors[7],
            N::BrightBlack => self.colors[8],
            N::BrightRed => self.colors[9],
            N::BrightGreen => self.colors[10],
            N::BrightYellow => self.colors[11],
            N::BrightBlue => self.colors[12],
            N::BrightMagenta => self.colors[13],
            N::BrightCyan => self.colors[14],
            N::BrightWhite => self.colors[15],
        }
    }
}

/// Visual attributes of one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: Rgba,
    pub bg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub dim: bool,
    pub hidden: bool,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character; renders background only.
    pub wide_spacer: bool,
}

impl CellStyle {
    pub fn blank(palette: &AnsiPalette) -> Self {
        Self {
            fg: palette.foreground,
            bg: palette.background,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            dim: false,
            hidden: false,
            wide: false,
            wide_spacer: false,
        }
    }

    /// True when two cells can be drawn as one text run.
    fn continues(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.strikeout == other.strikeout
            && self.dim == other.dim
            && self.hidden == other.hidden
    }
}

/// One grid cell as the renderer needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSnapshot {
    pub ch: char,
    pub style: CellStyle,
}

/// Cursor position and visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

/// Terminal modes the renderer and input encoder care about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnapshotModes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub show_cursor: bool,
}

/// The selected part of the visible grid, in viewport coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionSpan {
    /// First selected cell, top left.
    pub start: (usize, usize),
    /// Last selected cell, bottom right.
    pub end: (usize, usize),
}

impl SelectionSpan {
    /// Whether `(row, col)` is inside the selection.
    ///
    /// The selection runs from the start column of the first line to the end column of the
    /// last one; the lines in between are selected from edge to edge.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if row < self.start.0 || row > self.end.0 {
            return false;
        }
        if self.start.0 == self.end.0 {
            return col >= self.start.1 && col <= self.end.1;
        }
        if row == self.start.0 {
            return col >= self.start.1;
        }
        if row == self.end.0 {
            return col <= self.end.1;
        }
        true
    }
}

/// A complete frame's worth of terminal state.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenSnapshot {
    pub size: TerminalSize,
    pub cells: Vec<CellSnapshot>,
    pub cursor: CursorSnapshot,
    pub modes: SnapshotModes,
    pub base_style: CellStyle,
    /// Selected cells, when the user has dragged across the grid.
    pub selection: Option<SelectionSpan>,
}

impl ScreenSnapshot {
    pub fn cell(&self, row: usize, col: usize) -> Option<&CellSnapshot> {
        if row >= usize::from(self.size.rows) || col >= usize::from(self.size.cols) {
            return None;
        }
        self.cells.get(row * usize::from(self.size.cols) + col)
    }

    /// Whether `(row, col)` is selected.
    pub fn is_selected(&self, row: usize, col: usize) -> bool {
        self.selection
            .is_some_and(|selection| selection.contains(row, col))
    }
}

/// Owns the emulator grid for one session.
pub struct Terminal {
    term: Term<EventProxy>,
    processor: ansi::Processor,
    size: TerminalSize,
    palette: AnsiPalette,
    base_style: CellStyle,
}

impl Terminal {
    pub fn new(
        size: TerminalSize,
        scrollback: usize,
        events: Sender<TerminalEvent>,
        palette: &Palette,
    ) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let dims = TermDims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        };
        let term = Term::new(config, &dims, EventProxy { tx: events });
        let ansi_palette = AnsiPalette::from_palette(palette);
        Self {
            term,
            processor: ansi::Processor::new(),
            size,
            palette: ansi_palette,
            base_style: CellStyle::blank(&ansi_palette),
        }
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn palette(&self) -> &AnsiPalette {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: &Palette) {
        self.palette = AnsiPalette::from_palette(palette);
        self.base_style = CellStyle::blank(&self.palette);
    }

    /// Parse bytes coming from the transport.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, size: TerminalSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.resize(TermDims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        });
    }

    /// Scroll the viewport (negative scrolls up into history).
    pub fn scroll(&mut self, lines: i32) {
        use alacritty_terminal::grid::Scroll;
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Start a fresh selection at a viewport cell.
    pub fn begin_selection(&mut self, row: usize, col: usize) {
        let Some(point) = self.grid_point(row, col) else {
            return;
        };
        self.term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
    }

    /// Extend the current selection to a viewport cell.
    pub fn extend_selection(&mut self, row: usize, col: usize) {
        let Some(point) = self.grid_point(row, col) else {
            return;
        };
        let Some(selection) = self.term.selection.as_mut() else {
            return;
        };
        selection.update(point, Side::Right);
    }

    /// Drop the selection.
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Whether anything is selected.
    pub fn has_selection(&self) -> bool {
        self.term.selection.is_some()
    }

    /// The selected text, ready for the clipboard.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    /// Map a viewport cell to a grid point, allowing for the scrollback offset.
    fn grid_point(&self, row: usize, col: usize) -> Option<Point> {
        // A drag that leaves the grid keeps selecting to the edge instead of stopping.
        let rows = usize::from(self.size.rows);
        let cols = usize::from(self.size.cols);
        if rows == 0 || cols == 0 {
            return None;
        }
        let row = row.min(rows - 1);
        let col = col.min(cols - 1);
        let offset = self.term.grid().display_offset() as i32;
        Some(Point::new(Line(row as i32 - offset), Column(col)))
    }

    /// The selection as viewport coordinates, clipped to the visible grid.
    fn selection_span(&self) -> Option<SelectionSpan> {
        let selection = self.term.selection.as_ref()?;
        let range = selection.to_range(&self.term)?;
        let offset = self.term.grid().display_offset();
        let rows = usize::from(self.size.rows);
        let cols = usize::from(self.size.cols);

        let to_viewport = |point: Point| -> Option<(usize, usize)> {
            let row = (point.line.0 + offset as i32).max(0) as usize;
            if row >= rows {
                return None;
            }
            Some((row, point.column.0.min(cols.saturating_sub(1))))
        };
        let start = to_viewport(range.start)?;
        let end = to_viewport(range.end)?;
        Some(SelectionSpan { start, end })
    }

    pub fn modes(&self) -> SnapshotModes {
        let mode = self.term.mode();
        SnapshotModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            show_cursor: mode.contains(TermMode::SHOW_CURSOR),
        }
    }

    /// Copy the visible grid into an owned snapshot.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        let mut cells = vec![
            CellSnapshot {
                ch: ' ',
                style: self.base_style,
            };
            cols * rows
        ];

        let content = self.term.renderable_content();
        let mut row = 0usize;
        let mut col = 0usize;
        for indexed in content.display_iter {
            if col >= cols {
                col = 0;
                row += 1;
            }
            if row >= rows {
                break;
            }
            let cell = indexed.cell;
            let flags = cell.flags;
            let mut style = self.base_style;
            let inverse = flags.contains(Flags::INVERSE);
            let fg = if inverse {
                self.palette.resolve(cell.bg)
            } else {
                self.palette.resolve(cell.fg)
            };
            let bg = if inverse {
                self.palette.resolve(cell.fg)
            } else {
                self.palette.resolve(cell.bg)
            };
            style.fg = fg;
            style.bg = bg;
            style.bold = flags.contains(Flags::BOLD);
            style.italic = flags.contains(Flags::ITALIC);
            style.underline = flags.intersects(Flags::ALL_UNDERLINES);
            style.strikeout = flags.contains(Flags::STRIKEOUT);
            style.dim = flags.contains(Flags::DIM);
            style.hidden = flags.contains(Flags::HIDDEN);
            style.wide = flags.contains(Flags::WIDE_CHAR);
            style.wide_spacer = flags.contains(Flags::WIDE_CHAR_SPACER);

            cells[row * cols + col] = CellSnapshot {
                ch: cell.c,
                style,
            };
            col += 1;
        }

        let cursor_point = content.cursor.point;
        let display_offset = self.term.grid().display_offset();
        let cursor_row = point_to_viewport(display_offset, cursor_point)
            .map(|viewport| viewport.line)
            .unwrap_or(usize::MAX);
        let modes = self.modes();

        ScreenSnapshot {
            size: self.size,
            selection: self.selection_span(),
            cells,
            cursor: CursorSnapshot {
                row: cursor_row,
                col: cursor_point.column.0,
                visible: modes.show_cursor,
            },
            modes,
            base_style: self.base_style,
        }
    }
}

impl CellStyle {
    /// True when `self` and `other` can share one text run.
    pub fn same_run(&self, other: &Self) -> bool {
        self.continues(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use wassh_core::{Palette, ThemeMode};

    fn terminal(cols: u16, rows: u16) -> (Terminal, std::sync::mpsc::Receiver<TerminalEvent>) {
        let (tx, rx) = channel();
        let term = Terminal::new(
            TerminalSize::new(cols, rows),
            100,
            tx,
            &Palette::for_mode(ThemeMode::Dark),
        );
        (term, rx)
    }

    #[test]
    fn dragging_across_the_grid_selects_text() {
        let (mut term, _rx) = terminal(20, 4);
        term.feed(b"hello\r\nworld\r\n");

        assert!(!term.has_selection());
        assert_eq!(term.selection_text(), None);

        term.begin_selection(0, 0);
        term.extend_selection(1, 4);
        assert!(term.has_selection());
        assert_eq!(term.selection_text().as_deref(), Some("hello\nworld"));

        // The snapshot marks the selected cells for the renderer.
        let snapshot = term.snapshot();
        let span = snapshot.selection.expect("a span");
        assert_eq!(span.start, (0, 0));
        assert_eq!(span.end, (1, 4));
        assert!(snapshot.is_selected(0, 0));
        assert!(snapshot.is_selected(1, 4));
        assert!(!snapshot.is_selected(1, 5));
        assert!(!snapshot.is_selected(2, 0));

        term.clear_selection();
        assert!(!term.has_selection());
        assert!(term.snapshot().selection.is_none());
    }

    #[test]
    fn a_selection_that_leaves_the_grid_clamps_to_the_edge() {
        let (mut term, _rx) = terminal(10, 3);
        term.feed(b"abcdefghij");
        term.begin_selection(0, 2);
        // Dragging past the bottom-right corner keeps the last reachable cell selected.
        term.extend_selection(99, 99);
        let snapshot = term.snapshot();
        let span = snapshot.selection.expect("a span");
        assert_eq!(span.end, (2, 9));
    }

    #[test]
    fn a_span_knows_the_cells_inside_it() {
        let single = SelectionSpan {
            start: (1, 3),
            end: (1, 6),
        };
        assert!(!single.contains(1, 2));
        assert!(single.contains(1, 3));
        assert!(single.contains(1, 6));
        assert!(!single.contains(1, 7));
        assert!(!single.contains(0, 4));

        let multi = SelectionSpan {
            start: (1, 3),
            end: (3, 2),
        };
        assert!(multi.contains(1, 9), "the first line runs to its end");
        assert!(multi.contains(2, 0), "middle lines are fully selected");
        assert!(multi.contains(3, 2));
        assert!(!multi.contains(3, 3));
        assert!(!multi.contains(4, 0));
    }

    fn row_text(snapshot: &ScreenSnapshot, row: usize) -> String {
        (0..usize::from(snapshot.size.cols))
            .map(|col| snapshot.cell(row, col).map(|c| c.ch).unwrap_or(' '))
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn prints_plain_text() {
        let (mut term, _) = terminal(20, 4);
        term.feed(b"hello");
        let snapshot = term.snapshot();
        assert_eq!(row_text(&snapshot, 0), "hello");
        assert_eq!(snapshot.cursor.row, 0);
        assert_eq!(snapshot.cursor.col, 5);
    }

    #[test]
    fn handles_carriage_return_and_line_feed() {
        let (mut term, _) = terminal(10, 4);
        term.feed(b"one\r\ntwo\r\n");
        let snapshot = term.snapshot();
        assert_eq!(row_text(&snapshot, 0), "one");
        assert_eq!(row_text(&snapshot, 1), "two");
        assert_eq!(snapshot.cursor.row, 2);
    }

    #[test]
    fn alternate_screen_is_isolated() {
        let (mut term, _) = terminal(10, 3);
        term.feed(b"primary");
        term.feed(b"\x1b[?1049h");
        term.feed(b"alt");
        let snapshot = term.snapshot();
        assert!(snapshot.modes.alternate_screen);
        // 1049h preserves the cursor column, so "alt" starts after "primary".
        assert_eq!(row_text(&snapshot, 0), "       alt");
        term.feed(b"\x1b[?1049l");
        let snapshot = term.snapshot();
        assert!(!snapshot.modes.alternate_screen);
        assert_eq!(row_text(&snapshot, 0), "primary");
    }

    #[test]
    fn wide_characters_occupy_two_cells() {
        let (mut term, _) = terminal(10, 2);
        term.feed("你好".as_bytes());
        let snapshot = term.snapshot();
        let first = snapshot.cell(0, 0).unwrap();
        let second = snapshot.cell(0, 1).unwrap();
        assert_eq!(first.ch, '你');
        assert!(first.style.wide);
        assert!(second.style.wide_spacer);
        assert_eq!(snapshot.cursor.col, 4);
    }

    #[test]
    fn sgr_colors_and_flags_are_reported() {
        let (mut term, _) = terminal(20, 2);
        term.feed(b"\x1b[1;31mA\x1b[0mB");
        let snapshot = term.snapshot();
        let a = snapshot.cell(0, 0).unwrap();
        let b = snapshot.cell(0, 1).unwrap();
        assert!(a.style.bold);
        assert_eq!(a.style.fg, Rgba::rgb(0xe35d6a));
        assert!(!b.style.bold);
        assert_eq!(b.style.fg, Rgba::rgb(0xe8eaed));
    }

    #[test]
    fn reverse_video_swaps_colors() {
        let (mut term, _) = terminal(10, 2);
        term.feed(b"\x1b[7mX\x1b[0m");
        let snapshot = term.snapshot();
        let cell = snapshot.cell(0, 0).unwrap();
        assert_eq!(cell.style.fg, Rgba::rgb(0x0c0d0f));
        assert_eq!(cell.style.bg, Rgba::rgb(0xe8eaed));
    }

    #[test]
    fn bell_and_title_events_are_forwarded() {
        let (mut term, rx) = terminal(10, 2);
        term.feed(b"\x07");
        term.feed(b"\x1b]0;my title\x07");
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events.contains(&TerminalEvent::Bell));
        assert!(events.contains(&TerminalEvent::Title("my title".to_owned())));
    }

    #[test]
    fn resize_preserves_content_and_reports_new_size() {
        let (mut term, _) = terminal(10, 2);
        term.feed(b"abc");
        term.resize(TerminalSize::new(40, 10));
        let snapshot = term.snapshot();
        assert_eq!(snapshot.size, TerminalSize::new(40, 10));
        assert_eq!(row_text(&snapshot, 0), "abc");
    }

    #[test]
    fn bracketed_paste_mode_is_tracked() {
        let (mut term, _) = terminal(10, 2);
        term.feed(b"\x1b[?2004h");
        assert!(term.modes().bracketed_paste);
        term.feed(b"\x1b[?2004l");
        assert!(!term.modes().bracketed_paste);
    }
}
