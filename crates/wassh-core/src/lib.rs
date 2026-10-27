//! Domain model shared by every WaSSH crate.
//!
//! This crate is deliberately free of UI, I/O and platform dependencies so that the
//! rules it encodes can be unit-tested in isolation.

#![forbid(unsafe_code)]

pub mod app;
pub mod colors;
pub mod connection;
pub mod hosts;
pub mod layout;
pub mod session;
pub mod settings;
pub mod tabs;
pub mod theme;
pub mod time;

pub use app::*;
pub use colors::*;
pub use connection::*;
pub use hosts::*;
pub use layout::*;
pub use session::*;
pub use settings::*;
pub use tabs::*;
pub use theme::*;
