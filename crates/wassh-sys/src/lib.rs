//! Native facts about this machine.
//!
//! On Windows every number comes straight from the Win32 API — no shell, no WMI, no
//! helper process. Other platforms report nothing, and their callers use their own
//! probes.

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
    cfg!(windows)
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    cpu, disk, memory, network, process_details, processes, stop_process, system,
};

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{
    cpu, disk, memory, network, process_details, processes, stop_process, system,
};
