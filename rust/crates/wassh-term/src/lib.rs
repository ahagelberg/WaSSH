//! Terminal emulation and transport.
//!
//! `alacritty_terminal` provides the VT parser and grid; this crate adds the pieces the
//! This crate provides: PTY transport, key encoding, selection
//! and search, plus a renderer-friendly snapshot type.

#![forbid(unsafe_code)]

pub mod emulator;
pub mod grid;
pub mod input;
pub mod pty;

pub use emulator::{
    AnsiPalette, CellSnapshot, CellStyle, CursorSnapshot, ScreenSnapshot, SnapshotModes,
    Terminal, TerminalEvent,
};
pub use grid::{CellSize, TerminalSize, fit_cells};
pub use input::{Key, KeyStroke, encode};
pub use pty::PtySession;
