//! SSH transport.
//!
//! Built on `russh` (pure Rust, async). This crate owns:
//!
//! * proxy/jump chains (each hop multiplexing the next via `direct-tcpip`),
//! * host-key verification and the prompt flow,
//! * password, public-key, agent and keyboard-interactive authentication,
//! * session channels (shell, exec, extra shells for plugins),
//! * local/remote/dynamic forwarding and X11 channels.

#![forbid(unsafe_code)]

pub mod exec;
pub mod hostkey;
pub mod known_hosts;
pub mod remote_session;
pub mod session;
pub mod sftp;
pub mod tunnels;
pub mod x11;

pub use hostkey::{HostKeyFingerprint, fingerprint_sha256};
pub use known_hosts::KnownHosts;
pub use remote_session::{Presence, RemotePlan, plan as plan_remote_session};
pub use session::{
    AuthPromptField, ExecOutput, HostKeyPolicy, JumpHop, SessionEnd, SftpJob, SshAuth, SshEvent,
    SshOptions, SshSession, capture as capture_command,
};
pub use sftp::{SftpEntry, SftpOutcome, SftpProgress, SftpRequest, format_size};
pub use tunnels::{RemoteForwards, TunnelRequest, remote_forwards};
pub use x11::X11Config;
