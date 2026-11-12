//! Native facts about this machine.
//!
//! On Windows every number comes straight from the Win32 API — no shell, no WMI, no
//! helper process. On Linux they come from `/proc`, `/sys` and `statvfs`. Other
//! platforms report nothing, and their callers use their own probes.

/// CPU time counters: a total and its idle share, in 100 ns ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub total: u64,
    pub idle: u64,
}

/// The aggregate CPU counters and one pair per core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cpu {
    pub total: CpuTimes,
    pub cores: Vec<CpuTimes>,
}

/// Physical memory and page file, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_available: u64,
}

/// The system drive, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    pub total: u64,
    pub free: u64,
}

/// One network interface's cumulative counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    /// Bytes received since boot.
    pub rx: u64,
    /// Bytes sent since boot.
    pub tx: u64,
    /// Link speed in megabits per second, when the adapter reports one.
    pub speed_mbps: Option<u64>,
}

/// One row of the process table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    /// Executable name, or the command line where the platform reports one.
    pub name: String,
    pub threads: u32,
    /// CPU seconds used since the process started.
    pub cpu_s: f64,
    pub working_set: u64,
    pub private_bytes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Facts about the machine itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct System {
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub ip: Option<String>,
    pub uptime_s: u64,
}

/// What the details dialog shows for one process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessDetails {
    pub executable: Option<String>,
    pub parent_pid: u32,
    pub owner: Option<String>,
    /// Creation time in local time, ready to print.
    pub started: Option<String>,
    pub elapsed_s: u64,
    pub threads: u32,
    pub working_set: u64,
    pub peak_working_set: u64,
    pub private_bytes: u64,
    pub handles: u32,
}

/// Whether this build reads its numbers from the operating system.
pub const fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// Bytes the whole machine read and wrote since boot.
///
/// `None` means the platform counts IO per process instead, so the caller sums the
/// counters of [`processes`].
pub fn disk_io() -> Option<(u64, u64)> {
    platform::disk_io()
}

/// Give a GUI-subsystem process the console it was started from, so its CLI flags print.
///
/// A binary built with `windows_subsystem = "windows"` gets no console when launched from a
/// terminal; attaching to the parent's is what lets `--version` and friends write where the
/// user can read it.
#[cfg(windows)]
pub fn attach_console() {
    windows::attach_console();
}

/// No-op where the console is inherent to the process.
#[cfg(not(windows))]
pub fn attach_console() {}

/// Nothing to hide where a child process has no window of its own.
#[cfg(not(windows))]
pub fn hide_console(_command: &mut std::process::Command) {}

/// Whether Windows has a change staged that only a restart can finish.
///
/// Enabling an optional component (as `wsl --install` does) lands in the component store and
/// stays unusable until the machine restarts.
#[cfg(windows)]
pub fn reboot_pending() -> bool {
    windows::reboot_pending()
}

/// No restart bookkeeping to read on other platforms.
#[cfg(not(windows))]
pub fn reboot_pending() -> bool {
    false
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use crate::windows as platform;
#[cfg(windows)]
pub use windows::{
    cpu, disk, hide_console, memory, network, process_details, processes, stop_process, system,
    unprotect_user_data,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use crate::linux as platform;
#[cfg(target_os = "linux")]
pub use linux::{
    cpu, disk, memory, network, process_details, processes, stop_process, system,
};

#[cfg(not(any(windows, target_os = "linux")))]
mod stub;
#[cfg(not(any(windows, target_os = "linux")))]
use crate::stub as platform;
#[cfg(not(any(windows, target_os = "linux")))]
pub use stub::{
    cpu, disk, memory, network, process_details, processes, stop_process, system,
};
