//! Telnet with IAC option negotiation.
//!
//! The state machine
//! and the negotiation rules are ported here one-for-one so servers that only work with
//! the old client keep working.

#![forbid(unsafe_code)]

pub mod codec;
pub mod session;

pub use codec::TelnetFilter;
pub use session::{TelnetEvent, TelnetOptions, TelnetSession};
