//! The Linux implementation: CPU, memory, disk, network and process facts read from
//! `/proc`, `/sys` and one `statvfs`, so a sample is a handful of small file reads
//! instead of a shell round trip.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::{Cpu, CpuTimes, Disk, Interface, Memory, Process, ProcessDetails, System};

/// Where the kernel exposes its counters.
const PROC: &str = "/proc";
/// Where the kernel publishes the network interfaces.
const NET_CLASS: &str = "/sys/class/net";
/// Where the kernel publishes the whole block devices.
const BLOCK_CLASS: &str = "/sys/block";
/// The mount point the disk gauge reports on, like the probe's `df -kP /`.
const ROOT: &[u8] = b"/\0";
/// Bytes in one KiB, the unit `/proc/meminfo` counts.
const KIB_BYTES: u64 = 1024;
/// Bytes in one `/proc/diskstats` sector.
const SECTOR_BYTES: u64 = 512;
/// Longest command line read from `/proc/<pid>/cmdline`; a table shows a fraction.
const COMMAND_BYTES: usize = 512;
/// Interface loopback traffic is counted on; the pane leaves it out.
const LOOPBACK: &str = "lo";
/// What `/sys/class/net/<iface>/speed` answers when the driver does not know the speed.
const UNKNOWN_SPEED: &str = "-1";
/// Heading lines `/proc/net/dev` puts before the counters.
const NET_HEADINGS: usize = 2;
/// Index of the receive counter in a `/proc/net/dev` row.
const NET_RX: usize = 0;
/// Index of the transmit counter in a `/proc/net/dev` row.
const NET_TX: usize = 8;
/// Index of the device name in a `/proc/diskstats` row.
const DISKSTATS_NAME: usize = 2;
/// Index of the sectors read in a `/proc/diskstats` row.
const DISKSTATS_READ: usize = 5;
/// Index of the sectors written in a `/proc/diskstats` row.
const DISKSTATS_WRITE: usize = 9;
/// Index of the idle counter in a `/proc/stat` CPU line.
const STAT_IDLE: usize = 3;
/// Index of the idle-with-I/O-wait counter in a `/proc/stat` CPU line.
const STAT_IOWAIT: usize = 4;
/// CPU line fields that make up the total; the guest counters repeat the user ones.
const STAT_TOTAL_FIELDS: usize = 8;
/// Indices into the `/proc/<pid>/stat` fields that follow the process name.
const PID_PARENT: usize = 1;
const PID_UTIME: usize = 11;
const PID_STIME: usize = 12;
const PID_THREADS: usize = 17;
const PID_VSIZE: usize = 20;
const PID_RSS: usize = 21;
/// Devices that only mirror another device's traffic, or hold removable media.
const VIRTUAL_DEVICES: [&str; 5] = ["loop", "ram", "zram", "dm-", "sr"];
/// Documentation-only address (RFC 5737) that makes the routing table pick this
/// machine's own address; connecting a UDP socket sends no packet.
const ROUTE_PROBE: std::net::Ipv4Addr = std::net::Ipv4Addr::new(192, 0, 2, 1);
/// Port of the route probe; nothing listens there and nothing is sent.
const ROUTE_PROBE_PORT: u16 = 9;

/// Clock ticks per second, the unit `/proc/<pid>/stat` counts CPU time in.
fn clock_ticks() -> f64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as f64 } else { 100.0 }
}

/// Bytes in a memory page, the unit `/proc/<pid>/stat` counts residency in.
fn page_bytes() -> u64 {
    let bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if bytes > 0 { bytes as u64 } else { 4096 }
}

pub fn cpu() -> Option<Cpu> {
    let text = fs::read_to_string(format!("{PROC}/stat")).ok()?;
    let mut lines = text.lines();
    let total = cpu_line(lines.next()?)?;
    Some(Cpu {
        total,
        cores: lines.filter_map(cpu_line).collect(),
    })
}

/// One `cpu`/`cpuN` line of `/proc/stat`; idle plus I/O wait is the idle share.
fn cpu_line(line: &str) -> Option<CpuTimes> {
    let mut fields = line.split_whitespace();
    if !fields.next()?.starts_with("cpu") {
        return None;
    }
    let values: Vec<u64> = fields.map(|field| field.parse().unwrap_or(0)).collect();
    let idle = values
        .get(STAT_IDLE)?
        .saturating_add(*values.get(STAT_IOWAIT)?);
    Some(CpuTimes {
        total: values.iter().take(STAT_TOTAL_FIELDS).sum(),
        idle,
    })
}

pub fn memory() -> Option<Memory> {
    let text = fs::read_to_string(format!("{PROC}/meminfo")).ok()?;
    let mut total = 0;
    let mut free = 0;
    let mut available = 0;
    let mut swap_total = 0;
    let mut swap_free = 0;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Ok(bytes) = value.parse::<u64>() else {
            continue;
        };
        let bytes = bytes.saturating_mul(KIB_BYTES);
        match name {
            "MemTotal:" => total = bytes,
            "MemFree:" => free = bytes,
            "MemAvailable:" => available = bytes,
            "SwapTotal:" => swap_total = bytes,
            "SwapFree:" => swap_free = bytes,
            _ => {}
        }
    }
    Some(Memory {
        total,
        // A kernel without `MemAvailable` (before 3.14) only knows the free pages.
        available: if available > 0 { available } else { free },
        swap_total,
        swap_available: swap_free,
    })
}

// `c_ulong` is only 32 bits wide on a 32-bit target, so the casts stay.
#[allow(clippy::unnecessary_cast)]
pub fn disk() -> Option<Disk> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `statvfs` only writes the buffer it is handed, and `ROOT` is a
    // NUL-terminated path.
    if unsafe { libc::statvfs(ROOT.as_ptr().cast(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let unit = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    } as u64;
    Some(Disk {
        total: (stat.f_blocks as u64).saturating_mul(unit),
        free: (stat.f_bavail as u64).saturating_mul(unit),
    })
}

pub fn network() -> Vec<Interface> {
    let Ok(text) = fs::read_to_string(format!("{PROC}/net/dev")) else {
        return Vec::new();
    };
    text.lines()
        .skip(NET_HEADINGS)
        .filter_map(interface)
        .collect()
}

/// One `/proc/net/dev` row, with the speed the driver reports for it.
fn interface(line: &str) -> Option<Interface> {
    let (name, counters) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name == LOOPBACK {
        return None;
    }
    let fields: Vec<&str> = counters.split_whitespace().collect();
    Some(Interface {
        name: name.to_owned(),
        rx: fields.get(NET_RX)?.parse().ok()?,
        tx: fields.get(NET_TX)?.parse().ok()?,
        speed_mbps: link_speed(name),
    })
}

/// Link speed in megabits per second, when the driver knows it. Virtual devices
/// answer with an error or `-1`.
fn link_speed(interface: &str) -> Option<u64> {
    let text = fs::read_to_string(format!("{NET_CLASS}/{interface}/speed")).ok()?;
    let speed = text.trim();
    if speed.is_empty() || speed == UNKNOWN_SPEED {
        return None;
    }
    speed.parse::<u64>().ok().filter(|speed| *speed > 0)
}

/// Bytes every device read and wrote since boot.
///
/// Partitions repeat their device's counters and stacked devices repeat them again, so
/// only a name `/sys/block` lists is counted; virtual devices are left out.
pub fn disk_io() -> Option<(u64, u64)> {
    let text = fs::read_to_string(format!("{PROC}/diskstats")).ok()?;
    let mut read = 0u64;
    let mut written = 0u64;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = fields.get(DISKSTATS_NAME) else {
            continue;
        };
        if VIRTUAL_DEVICES
            .iter()
            .any(|prefix| name.starts_with(prefix))
            || !Path::new(BLOCK_CLASS).join(name).exists()
        {
            continue;
        }
        let sectors = |index: usize| {
            fields
                .get(index)
                .and_then(|field| field.parse::<u64>().ok())
                .unwrap_or(0)
        };
        read = read.saturating_add(sectors(DISKSTATS_READ).saturating_mul(SECTOR_BYTES));
        written = written.saturating_add(sectors(DISKSTATS_WRITE).saturating_mul(SECTOR_BYTES));
    }
    Some((read, written))
}

pub fn processes() -> Vec<Process> {
    let Ok(entries) = fs::read_dir(PROC) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(process)
        .collect()
}

/// One process from its `/proc/<pid>/stat`: parent, name, threads, CPU time and
/// residency. The IO counters stay empty because [`disk_io`] counts the devices.
fn process(pid: u32) -> Option<Process> {
    let text = fs::read_to_string(format!("{PROC}/{pid}/stat")).ok()?;
    // The name sits in parentheses and may hold spaces and parentheses of its own.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text.get(open + 1..close)?;
    let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let field = |index: usize| {
        fields
            .get(index)
            .and_then(|field| field.parse::<u64>().ok())
            .unwrap_or(0)
    };
    Some(Process {
        pid,
        parent: field(PID_PARENT) as u32,
        name: command_line(pid, name),
        threads: field(PID_THREADS) as u32,
        cpu_s: field(PID_UTIME).saturating_add(field(PID_STIME)) as f64 / clock_ticks(),
        working_set: field(PID_RSS).saturating_mul(page_bytes()),
        private_bytes: field(PID_VSIZE),
        read_bytes: 0,
        write_bytes: 0,
    })
}

/// The first [`COMMAND_BYTES`] of `/proc/<pid>/cmdline` as one line, or the name from
/// `stat` for a process without an argv (a kernel thread, or one we may not read).
fn command_line(pid: u32, name: &str) -> String {
    let Ok(file) = fs::File::open(format!("{PROC}/{pid}/cmdline")) else {
        return name.to_owned();
    };
    let mut bytes = Vec::with_capacity(COMMAND_BYTES);
    if file
        .take(COMMAND_BYTES as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return name.to_owned();
    }
    let command = String::from_utf8_lossy(&bytes).replace('\0', " ");
    let command = command.trim();
    if command.is_empty() {
        name.to_owned()
    } else {
        command.to_owned()
    }
}

pub fn system() -> Option<System> {
    let kernel = fs::read_to_string(format!("{PROC}/sys/kernel/osrelease")).ok()?;
    Some(System {
        os: os_name(),
        kernel: kernel.trim().to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        ip: ipv4_address(),
        uptime_s: uptime_s(),
    })
}

/// `PRETTY_NAME` of `/etc/os-release`, the line the shell probe prints.
fn os_name() -> String {
    let Ok(text) = fs::read_to_string("/etc/os-release") else {
        return "Linux".to_owned();
    };
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Linux".to_owned())
}

fn uptime_s() -> u64 {
    fs::read_to_string(format!("{PROC}/uptime"))
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
        .map(|seconds| seconds as u64)
        .unwrap_or(0)
}

/// The address the routing table would use for an outside connection.
fn ipv4_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((ROUTE_PROBE, ROUTE_PROBE_PORT)).ok()?;
    let address = socket.local_addr().ok()?;
    (!address.ip().is_loopback()).then(|| address.ip().to_string())
}

/// Linux keeps the process dialog on the host probe: `/proc/<pid>` holds far more than
/// this record does, and the probe reads it all in one pass.
pub fn process_details(_pid: u32) -> Option<ProcessDetails> {
    None
}

/// Signals go out through the host probe on Linux.
pub fn stop_process(_pid: u32) -> Result<(), String> {
    Err("stopping processes through the OS is a Windows call".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_reports_its_own_numbers() {
        let memory = memory().expect("memory");
        assert!(memory.total > 0);
        assert!(memory.available <= memory.total);
        assert!(disk().is_some(), "the root filesystem is readable");
        let cpu = cpu().expect("cpu");
        assert!(cpu.total.total > 0);
        assert!(!cpu.cores.is_empty(), "one counter per core");
        let system = system().expect("system");
        assert!(!system.os.is_empty());
        assert!(!system.kernel.is_empty());
        assert!(system.uptime_s > 0);
    }

    #[test]
    fn this_process_is_listed_with_its_own_residency() {
        let me = std::process::id();
        let process = processes()
            .into_iter()
            .find(|process| process.pid == me)
            .expect("this process");
        assert!(process.working_set > 0, "{}", process.working_set);
        assert!(process.threads >= 1);
        assert!(process.cpu_s >= 0.0);
    }

    #[test]
    fn the_devices_count_their_own_traffic_once() {
        let (read, written) = disk_io().expect("disk stats");
        assert!(read > 0 || written > 0, "{read} {written}");
    }

    #[test]
    fn the_loopback_interface_is_not_counted() {
        assert!(network().iter().all(|interface| interface.name != LOOPBACK));
    }
}
