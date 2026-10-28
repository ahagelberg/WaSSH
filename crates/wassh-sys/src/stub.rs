//! The platforms without a native reader report nothing, so callers fall back to their
//! own shell probes.

use crate::{Cpu, Disk, Interface, Memory, Process, ProcessDetails, System};

pub fn cpu() -> Option<Cpu> {
    None
}

pub fn memory() -> Option<Memory> {
    None
}

pub fn disk() -> Option<Disk> {
    None
}

pub fn network() -> Vec<Interface> {
    Vec::new()
}

pub fn processes() -> Vec<Process> {
    Vec::new()
}

pub fn system() -> Option<System> {
    None
}

pub fn process_details(_pid: u32) -> Option<ProcessDetails> {
    None
}

pub fn stop_process(_pid: u32) -> Result<(), String> {
    Err("stopping processes needs the Windows API".to_owned())
}
