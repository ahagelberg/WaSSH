//! The Server Monitor plugin: CPU, memory, disk and load for the host.
//!
//! The session runs one small shell command per poll — over SSH, or on this
//! machine for a Local tab — and hands the raw text to [`parse_report`];
//! everything here is pure so the parsing can be tested without a host.

use egui::Ui;
use serde::Deserialize;
use wassh_core::Palette;
use wassh_ui::widgets::{
    BannerKind, MenuItem, SLIDER_WIDTH, bar_meter, banner, chip, context_menu, empty_state, faded,
    fixed_inset_card, footer, line_chart, modal, ring_gauge, segmented_bar, select_str,
    slider_field, small_button, stat_row,
};

use crate::color::to_color32;
use crate::plugin::{Plugin, PluginContext};
use crate::time;
use crate::{PluginId, PluginManifest};

/// The probe run on the remote host. `@@` separates the sections and is unlikely to appear in
/// any of them.
pub const MONITOR_COMMAND: &str = "cat /proc/loadavg; echo @@; cat /proc/stat; \
echo @@; cat /proc/meminfo; echo @@; df -kP /; echo @@; cat /proc/uptime; \
echo @@; cat /proc/net/dev; echo @@; for i in $(ls /sys/class/net); do \
printf '%s %s\\n' \"$i\" \"$(cat /sys/class/net/$i/speed 2>/dev/null)\"; done; \
echo @@; cat /proc/diskstats; echo @@; \
ps -eo pid=,user=,stat=,ni=,nlwp=,pcpu=,pmem=,args= --sort=-pcpu 2>/dev/null | head -n 30; \
echo @@; (. /etc/os-release 2>/dev/null; echo \"$PRETTY_NAME\"); uname -r; uname -m; \
hostname -I 2>/dev/null";

/// A Linux build of `df` reports 1024-byte blocks.
const DF_BLOCK_BYTES: u64 = 1024;

/// Raw `/proc/stat` CPU counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    /// All jiffies except idle ones.
    pub total: u64,
    /// Idle plus iowait jiffies.
    pub idle: u64,
}

/// Memory in bytes, as `/proc/meminfo` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: Option<u64>,
    pub free: Option<u64>,
    pub available: Option<u64>,
    pub buffers: Option<u64>,
    pub cached: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_free: Option<u64>,
}

impl Memory {
    /// Bytes applications use: everything not free, not buffers and not cache.
    pub fn used(&self) -> Option<u64> {
        let total = self.total?;
        let available = self.available.unwrap_or(self.free?);
        Some(total.saturating_sub(available))
    }

    /// Bytes the kernel holds as buffers and page cache.
    pub fn cached_total(&self) -> Option<u64> {
        Some(self.buffers.unwrap_or(0) + self.cached.unwrap_or(0))
    }

    /// Bytes used by applications themselves (used minus buffers and cache).
    pub fn app(&self) -> Option<u64> {
        let used = self.used()?;
        Some(used.saturating_sub(self.cached_total().unwrap_or(0)))
    }
}

/// One network interface's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetIface {
    pub name: String,
    /// Bytes received since boot.
    pub rx_total: u64,
    /// Bytes sent since boot.
    pub tx_total: u64,
    /// Bytes per second since the previous sample.
    pub rx_rate: Option<u64>,
    pub tx_rate: Option<u64>,
    /// Reported link speed in megabits per second, when the host says.
    pub speed: Option<u64>,
}

impl NetIface {
    /// The fastest rate seen on this interface in either direction.
    pub fn peak_rate(&self) -> u64 {
        self.rx_rate.unwrap_or(0).max(self.tx_rate.unwrap_or(0))
    }

    /// Link speed in Mbit/s: what the host reports, else the smallest assumed
    /// speed that still covers the traffic seen. A host that does not report
    /// `/sys/class/net/<iface>/speed` gets 100 Mbit/s, stepped up to 1, 2.5 and
    /// 10 Gbit/s when the measured rate would not fit.
    pub fn link_speed_mbps(&self) -> (u64, bool) {
        match self.speed {
            Some(reported) => (reported, true),
            None => (assumed_speed_mbps(self.peak_rate()), false),
        }
    }
}

/// Link speeds assumed when the host reports none, in Mbit/s.
pub const ASSUMED_SPEEDS_MBPS: [u64; 4] = [100, 1_000, 2_500, 10_000];

/// The smallest assumed link speed that still covers `bytes_per_second`.
pub fn assumed_speed_mbps(bytes_per_second: u64) -> u64 {
    let needed = bytes_per_second.saturating_mul(8) / 1_000_000;
    ASSUMED_SPEEDS_MBPS
        .into_iter()
        .find(|speed| *speed >= needed.saturating_add(1))
        .unwrap_or(ASSUMED_SPEEDS_MBPS[ASSUMED_SPEEDS_MBPS.len() - 1])
}

/// `100 Mbit`, `1 Gbit`, `2.5 Gbit`.
pub fn format_speed(mbps: u64) -> String {
    if mbps >= 1_000 {
        let gbit = mbps as f64 / 1_000.0;
        if (gbit - gbit.round()).abs() < 0.05 {
            format!("{gbit:.0} Gbit")
        } else {
            format!("{gbit:.1} Gbit")
        }
    } else {
        format!("{mbps} Mbit")
    }
}

/// One row of the process table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub user: String,
    /// One-letter state from `ps` (R, S, D, Z, …).
    pub state: String,
    pub nice: i32,
    pub threads: u32,
    /// CPU percentage as `ps` reports it.
    pub cpu: f32,
    /// Resident memory percentage as `ps` reports it.
    pub mem: f32,
    pub command: String,
}

/// A column of the process table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcessColumn {
    Pid,
    User,
    State,
    Nice,
    Threads,
    #[default]
    Cpu,
    Mem,
    Command,
}

/// The process table's columns, left to right: column, heading, numeric.
pub const PROCESS_COLUMNS: [(ProcessColumn, &str, bool); 8] = [
    (ProcessColumn::Pid, "PID", true),
    (ProcessColumn::User, "User", false),
    (ProcessColumn::State, "S", false),
    (ProcessColumn::Nice, "NI", true),
    (ProcessColumn::Threads, "THR", true),
    (ProcessColumn::Cpu, "CPU", true),
    (ProcessColumn::Mem, "Mem", true),
    (ProcessColumn::Command, "Command", false),
];

impl ProcessColumn {
    /// Name used in the plugin's own data.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pid => "pid",
            Self::User => "user",
            Self::State => "state",
            Self::Nice => "nice",
            Self::Threads => "threads",
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Command => "command",
        }
    }

    /// The column a stored `key` names, if any.
    pub fn parse(key: &str) -> Option<Self> {
        PROCESS_COLUMNS
            .iter()
            .find(|(column, _, _)| column.key() == key)
            .map(|(column, _, _)| *column)
    }
}

/// The direction the first click on `column` sorts in, like the reference:
/// pressure columns read highest-first.
pub fn default_descending(column: ProcessColumn) -> bool {
    matches!(
        column,
        ProcessColumn::Cpu | ProcessColumn::Mem | ProcessColumn::Threads | ProcessColumn::Nice
    )
}

/// Indices into `processes`, ordered for `column`.
pub fn sort_processes(
    processes: &[Process],
    column: ProcessColumn,
    descending: bool,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..processes.len()).collect();
    order.sort_by(|left, right| {
        let left = &processes[*left];
        let right = &processes[*right];
        let ordering = match column {
            ProcessColumn::Pid => left.pid.cmp(&right.pid),
            ProcessColumn::User => left.user.to_lowercase().cmp(&right.user.to_lowercase()),
            ProcessColumn::State => left.state.cmp(&right.state),
            ProcessColumn::Nice => left.nice.cmp(&right.nice),
            ProcessColumn::Threads => left.threads.cmp(&right.threads),
            ProcessColumn::Cpu => left.cpu.total_cmp(&right.cpu),
            ProcessColumn::Mem => left.mem.total_cmp(&right.mem),
            ProcessColumn::Command => left
                .command
                .to_lowercase()
                .cmp(&right.command.to_lowercase()),
        };
        // Ties keep a stable, recognisable order rather than the arrival order.
        let ordering = ordering.then_with(|| left.pid.cmp(&right.pid));
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    order
}

/// Signals the details dialog can send, numbered like `kill -s`.
pub const SIGNALS: [(&str, u8); 15] = [
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("ILL", 4),
    ("TRAP", 5),
    ("ABRT", 6),
    ("BUS", 7),
    ("FPE", 8),
    ("KILL", 9),
    ("USR1", 10),
    ("SEGV", 11),
    ("USR2", 12),
    ("PIPE", 13),
    ("ALRM", 14),
    ("TERM", 15),
];

/// Signal offered by default.
pub const SIGNAL_DEFAULT: &str = "TERM";

/// Whether `signal` is one of the names a `kill -s` may be given.
///
/// The name goes into a shell command, so anything outside this list is refused
/// rather than quoted.
pub fn valid_signal(signal: &str) -> bool {
    let upper = signal.trim().to_uppercase();
    let upper = upper.strip_prefix("SIG").unwrap_or(&upper);
    SIGNALS.iter().any(|(name, _)| *name == upper)
}

/// The `kill` command that sends `signal` to `pid`, or `None` for anything the
/// host could not understand.
pub fn signal_command(pid: u32, signal: &str) -> Option<String> {
    if pid == 0 || !valid_signal(signal) {
        return None;
    }
    let upper = signal.trim().to_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper).to_owned();
    Some(format!("kill -s {name} {pid}"))
}

/// The signals a Windows host understands. It has no POSIX signals, so both of these
/// stop the process; `KILL` skips the request to close first.
pub const WINDOWS_SIGNALS: [&str; 2] = ["TERM", "KILL"];

/// The `Stop-Process` command that stops `pid` on a Windows host, or `None` for a
/// signal Windows cannot deliver.
pub fn windows_signal_command(pid: u32, signal: &str) -> Option<String> {
    if pid == 0 || !valid_signal(signal) {
        return None;
    }
    let upper = signal.trim().to_uppercase();
    match upper.strip_prefix("SIG").unwrap_or(&upper) {
        "TERM" => Some(format!("Stop-Process -Id {pid}")),
        "KILL" => Some(format!("Stop-Process -Id {pid} -Force")),
        _ => None,
    }
}

/// Field separator of the details probe: a unit separator, which cannot appear
/// in `/proc` text.
const DETAILS_SEP: char = '\u{1f}';

/// The one exec that reads everything the details dialog shows.
///
/// Every probe is best-effort: a missing file (permissions, kernel thread,
/// non-Linux host) simply leaves the field out.
pub fn details_command(pid: u32) -> String {
    let sep = DETAILS_SEP;
    let probes = [
        ("STATUS", format!("cat /proc/{pid}/status 2>/dev/null")),
        ("CMDLINE", format!("tr '\\0' ' ' < /proc/{pid}/cmdline 2>/dev/null")),
        ("EXE", format!("readlink /proc/{pid}/exe 2>/dev/null")),
        ("CWD", format!("readlink /proc/{pid}/cwd 2>/dev/null")),
        ("ROOT", format!("readlink /proc/{pid}/root 2>/dev/null")),
        (
            "PARENT",
            format!("ps -o ppid=,user=,lstart=,etime= -p {pid} 2>/dev/null"),
        ),
        ("FDS", format!("ls /proc/{pid}/fd 2>/dev/null | wc -l")),
        ("LIMITS", format!("cat /proc/{pid}/limits 2>/dev/null")),
        ("CGROUP", format!("cat /proc/{pid}/cgroup 2>/dev/null")),
        (
            "ENVIRON",
            format!("tr '\\0' '\\n' < /proc/{pid}/environ 2>/dev/null | head -n 40"),
        ),
    ];
    probes
        .iter()
        .map(|(label, command)| format!("echo \"{label}{sep}$({command})\""))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The details probe for a Windows host: the same `LABEL␟value` rows, read from CIM.
///
/// Windows has no `/proc`, so the dialog's groups are filled from `Win32_Process` and
/// `Get-Process`; the sections that only exist on Unix (working directory, root,
/// limits, cgroups, environment) are left out.
pub fn windows_details_command(pid: u32) -> String {
    format!(
        r#"$ErrorActionPreference = 'SilentlyContinue'
$sep = [char]{sep}
$process = Get-CimInstance Win32_Process | Where-Object {{ $_.ProcessId -eq {pid} }}
$owner = Invoke-CimMethod -InputObject $process -MethodName GetOwner
$live = Get-Process -Id {pid}
$parent = Get-CimInstance Win32_Process | Where-Object {{ $_.ProcessId -eq $process.ParentProcessId }}
$parentOwner = Invoke-CimMethod -InputObject $parent -MethodName GetOwner
$elapsed = [TimeSpan]((Get-Date) - $parent.CreationDate)
$status = @(
    'State: ' + $(if ($live.Responding) {{ 'Running' }} else {{ 'Not responding' }})
    'Threads: ' + $process.ThreadCount
    'Uid: ' + $owner.User
    'VmRSS: ' + $process.WorkingSetSize + ' B'
    'VmSize: ' + $process.VirtualSize + ' B'
) -join [char]10
'STATUS' + $sep + $status
'CMDLINE' + $sep + $process.CommandLine
'EXE' + $sep + $process.ExecutablePath
'FDS' + $sep + $process.HandleCount
$(if ($parent -and $parentOwner.User) {{
    'PARENT' + $sep + $parent.ProcessId + ' ' + $parentOwner.User + ' ' + $parent.CreationDate.ToString('yyyy-MM-dd HH:mm:ss') + ' ' + $elapsed.ToString('dd\.hh\:mm\:ss')
}})
"#,
        sep = DETAILS_SEP as u32,
        pid = pid
    )
}

/// Labelled rows the details dialog shows, parsed from the probe output.
///
/// The probe prints multi-line values (`/proc/PID/status`, limits, environ), so
/// only a line that carries the separator starts a group; the rest continue it.
pub fn parse_details(output: &str) -> Vec<(String, String)> {
    let mut groups: Vec<(String, String)> = Vec::new();
    for line in output.lines() {
        match line.find(DETAILS_SEP) {
            Some(index) => groups.push((
                line[..index].to_owned(),
                line[index + DETAILS_SEP.len_utf8()..].to_owned(),
            )),
            None => {
                if let Some((_, value)) = groups.last_mut() {
                    value.push('\n');
                    value.push_str(line);
                }
            }
        }
    }
    let group = |name: &str| {
        groups
            .iter()
            .find(|(label, _)| label == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or("")
    };

    let status = key_value_block(group("STATUS"));
    let parent = parent_line(group("PARENT"));
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut add = |label: &str, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            rows.push((label.to_owned(), value.to_owned()));
        }
    };
    add("Command line", group("CMDLINE"));
    add("Executable", group("EXE"));
    add("Working dir", group("CWD"));
    add("Root", group("ROOT"));
    add("Parent PID", parent.0.as_str());
    add("Owner", parent.1.as_str());
    add("Started", parent.2.as_str());
    add("Elapsed", parent.3.as_str());
    for (label, key) in [
        ("State", "State"),
        ("Threads", "Threads"),
        ("Owner (real)", "Uid"),
        ("Group", "Gid"),
        ("Virtual memory", "VmSize"),
        ("Resident memory", "VmRSS"),
        ("Peak memory", "VmPeak"),
    ] {
        add(label, status.get(key).map(String::as_str).unwrap_or(""));
    }
    add("Open file descriptors", group("FDS"));
    for (label, key) in [
        ("Voluntary ctx switches", "voluntary_ctxt_switches"),
        ("Involuntary ctx switches", "nonvoluntary_ctxt_switches"),
    ] {
        add(label, status.get(key).map(String::as_str).unwrap_or(""));
    }
    add("Cgroup", group("CGROUP").lines().next().unwrap_or(""));
    for (label, value) in limits(group("LIMITS")) {
        add(&label, &value);
    }
    rows
}

/// `Key:\tValue` lines of a `/proc/PID/status`-style block.
fn key_value_block(block: &str) -> std::collections::BTreeMap<String, String> {
    block
        .lines()
        .filter_map(|line| {
            let index = line.find(':')?;
            if index == 0 {
                return None;
            }
            Some((
                line[..index].trim().to_owned(),
                line[index + 1..].trim().to_owned(),
            ))
        })
        .collect()
}

/// `ppid user start… elapsed` columns from `ps -o ppid=,user=,lstart=,etime=`, or
/// from the shorter line a Windows host writes.
fn parent_line(line: &str) -> (String, String, String, String) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return (String::new(), String::new(), String::new(), String::new());
    }
    (
        parts[0].to_owned(),
        parts[1].to_owned(),
        parts[2..parts.len() - 1].join(" "),
        parts[parts.len() - 1].to_owned(),
    )
}

/// The interesting `limits` rows, as `label → soft value`.
fn limits(block: &str) -> Vec<(String, String)> {
    const WANTED: [&str; 6] = [
        "Max open files",
        "Max processes",
        "Max locked memory",
        "Max address space",
        "Max file size",
        "Max stack size",
    ];
    let mut rows = Vec::new();
    for line in block.lines() {
        let Some(index) = line.find("  ") else {
            continue;
        };
        let label = line[..index].trim();
        if !WANTED.contains(&label) {
            continue;
        }
        let parts: Vec<&str> = line[index..].split_whitespace().collect();
        let value = parts
            .first()
            .map(|soft| match parts.get(2) {
                Some(units) => format!("{soft} {units}"),
                None => (*soft).to_owned(),
            })
            .unwrap_or_default();
        rows.push((label.to_owned(), value));
    }
    rows
}

/// Static facts about the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub ip: Option<String>,
}

/// One poll of the host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// When the sample was taken, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Busy CPU percentage since the previous sample.
    pub cpu: Option<f32>,
    /// Busy percentage per core, since the previous sample.
    pub cores: Vec<f32>,
    /// One, five and fifteen minute load averages.
    pub load: [Option<f32>; 3],
    /// Processes in the run queue and in total, from `/proc/loadavg`.
    pub tasks_running: Option<u32>,
    pub tasks_total: Option<u32>,
    pub memory: Memory,
    /// Size of the sampled filesystem in bytes.
    pub disk_total: Option<u64>,
    /// Used bytes on the sampled filesystem.
    pub disk_used: Option<u64>,
    /// Free bytes on the sampled filesystem.
    pub disk_free: Option<u64>,
    /// Bytes read and written per second since the previous sample.
    pub disk_read_rate: Option<u64>,
    pub disk_write_rate: Option<u64>,
    /// Bytes read and written since boot.
    pub disk_read_total: Option<u64>,
    pub disk_write_total: Option<u64>,
    /// Interfaces, loopback excluded.
    pub net: Vec<NetIface>,
    pub processes: Vec<Process>,
    pub system: SystemInfo,
    /// Seconds since boot.
    pub uptime_s: Option<u64>,
}

impl Snapshot {
    /// Memory in use as a percentage.
    pub fn mem_percent(&self) -> Option<f32> {
        percent(self.memory.used()?, self.memory.total?)
    }

    /// Swap in use as a percentage.
    pub fn swap_percent(&self) -> Option<f32> {
        let total = self.memory.swap_total?;
        let free = self.memory.swap_free?;
        percent(total.saturating_sub(free), total)
    }

    /// Disk in use as a percentage.
    pub fn disk_percent(&self) -> Option<f32> {
        percent(self.disk_used?, self.disk_total?)
    }

    /// Receive rate across every interface.
    pub fn net_rx_rate(&self) -> u64 {
        self.net.iter().filter_map(|iface| iface.rx_rate).sum()
    }

    /// Send rate across every interface.
    pub fn net_tx_rate(&self) -> u64 {
        self.net.iter().filter_map(|iface| iface.tx_rate).sum()
    }

    /// Receive bytes across every interface since boot.
    pub fn net_rx_total(&self) -> u64 {
        self.net.iter().map(|iface| iface.rx_total).sum()
    }

    /// Send bytes across every interface since boot.
    pub fn net_tx_total(&self) -> u64 {
        self.net.iter().map(|iface| iface.tx_total).sum()
    }
}

/// Counters kept between samples, needed because every rate is a delta.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counters {
    /// When these counters were read, so rates use the real interval.
    pub at_ms: u64,
    pub cpu: CpuTimes,
    pub cores: Vec<CpuTimes>,
    pub net: Vec<(String, u64, u64)>,
    pub disk_read: u64,
    pub disk_write: u64,
    /// Cumulative CPU seconds per process, for hosts that report times rather than a rate.
    pub process_cpu: Vec<(u32, f64)>,
}

/// Space kept free on the right for the pane's scrollbar.
const SCROLLBAR_GUTTER: f32 = 14.0;
/// Gap between two cards in a row.
const CARD_GAP: f32 = 8.0;
/// The gauge ring is a fixed donut, exactly like the reference: it does not
/// grow with the card, so three of them fit without squeezing their labels.
const GAUGE_SIZE: f32 = 64.0;
/// Ring size when the dock is too narrow for a 64 px ring.
const GAUGE_SIZE_COMPACT: f32 = 48.0;
/// Narrowest a gauge card may get before the ring shrinks.
const GAUGE_MIN_CARD: f32 = 132.0;
/// Padding of a gauge card: `padding: 8px 6px` in the reference.
const GAUGE_MARGIN: (i8, i8) = (6, 8);
/// Smallest per-core bar; the row spreads the bars evenly like `auto-fill`.
const CORE_BAR_WIDTH: f32 = 14.0;
/// Gap between the per-core bars.
const CORE_BAR_GAP: f32 = 4.0;
/// Height of a per-core bar.
const CORE_BAR_HEIGHT: f32 = 48.0;
/// Room under a bar for its core index.
const CORE_LABEL_HEIGHT: f32 = 13.0;
/// Below this width the gauge cards stack instead of sitting three across.
const GAUGE_ROW_MIN: f32 = 420.0;

/// The history windows the pane offers, shortest first: label and seconds.
///
/// These are the reference's slider steps; the slider snaps to one of them, so
/// the window is always one the history ladder can answer in full.
pub const TIME_RANGES: [(&str, u64); 9] = [
    ("1m", 60),
    ("10m", 600),
    ("30m", 1_800),
    ("1h", 3_600),
    ("2h", 7_200),
    ("6h", 21_600),
    ("24h", 86_400),
    ("2d", 172_800),
    ("7d", 604_800),
];

/// Bucket width a window is read at, matching the ladder tier each step uses:
/// 1 s for a minute, 15 s up to an hour, 60 s up to a day, 10 min beyond.
pub fn bucket_seconds(range_s: u64) -> u64 {
    match range_s {
        0..=60 => 1,
        61..=3_600 => 15,
        3_601..=86_400 => 60,
        _ => 600,
    }
}

/// Buckets retained per width. Together they cover the seven-day window.
fn tier_capacity(bucket_s: u64) -> usize {
    match bucket_s {
        1 => 300,
        15 => 240,
        60 => 1_440,
        _ => 1_008,
    }
}

/// One charted value set: every series the charts draw, averaged over a bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub cpu: Option<f32>,
    pub mem: Option<f32>,
    /// Disk read bytes per second.
    pub read: Option<f32>,
    /// Disk write bytes per second.
    pub write: Option<f32>,
}

impl Point {
    /// The series of a sample.
    pub fn of(sample: &Snapshot) -> Self {
        Self {
            cpu: sample.cpu,
            mem: sample.mem_percent(),
            read: sample.disk_read_rate.map(|rate| rate as f32),
            write: sample.disk_write_rate.map(|rate| rate as f32),
        }
    }

    /// Fold another sample into this one's running mean. `samples` is how many
    /// samples the bucket already holds.
    fn merge(&mut self, other: Point, samples: u32) {
        fn blend(slot: &mut Option<f32>, value: Option<f32>, samples: u32) {
            let Some(value) = value else {
                return;
            };
            *slot = Some(match *slot {
                Some(current) => (current * samples as f32 + value) / (samples + 1) as f32,
                None => value,
            });
        }
        blend(&mut self.cpu, other.cpu, samples);
        blend(&mut self.mem, other.mem, samples);
        blend(&mut self.read, other.read, samples);
        blend(&mut self.write, other.write, samples);
    }
}

/// One completed bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    /// Bucket start, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub point: Point,
    /// Samples folded into it.
    pub count: u32,
}

/// One resolution of the history ladder.
#[derive(Clone, Debug, PartialEq)]
struct Tier {
    bucket_ms: u64,
    capacity: usize,
    buckets: std::collections::VecDeque<Bucket>,
    open: Option<Bucket>,
}

/// Multi-resolution history: coarse tiers answer the long windows, the 1 s tier
/// answers the short ones, so a seven-day window costs a few thousand buckets
/// instead of half a million samples.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    tiers: Vec<Tier>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        let widths: Vec<u64> = TIME_RANGES
            .iter()
            .map(|(_, seconds)| bucket_seconds(*seconds))
            .collect();
        let mut tiers = Vec::new();
        for width in widths {
            if tiers.iter().any(|tier: &Tier| tier.bucket_ms == width * 1_000) {
                continue;
            }
            tiers.push(Tier {
                bucket_ms: width * 1_000,
                capacity: tier_capacity(width),
                buckets: std::collections::VecDeque::new(),
                open: None,
            });
        }
        Self { tiers }
    }

    /// Fold one sample into every tier.
    pub fn push(&mut self, sample: &Snapshot) {
        let point = Point::of(sample);
        for tier in &mut self.tiers {
            let start = sample.at_ms - sample.at_ms % tier.bucket_ms;
            match &mut tier.open {
                Some(open) if open.at_ms == start => {
                    let count = open.count;
                    open.point.merge(point, count);
                    open.count = count + 1;
                }
                _ => {
                    if let Some(finished) = tier.open.take() {
                        tier.buckets.push_back(finished);
                        while tier.buckets.len() > tier.capacity {
                            tier.buckets.pop_front();
                        }
                    }
                    tier.open = Some(Bucket {
                        at_ms: start,
                        point,
                        count: 1,
                    });
                }
            }
        }
    }

    /// Points covering the last `range_s` seconds, oldest first.
    pub fn series(&self, range_s: u64, now_ms: u64) -> Vec<Point> {
        let width = bucket_seconds(range_s) * 1_000;
        let Some(tier) = self.tiers.iter().find(|tier| tier.bucket_ms == width) else {
            return Vec::new();
        };
        let from = now_ms.saturating_sub(range_s * 1_000);
        tier.buckets
            .iter()
            .chain(tier.open.iter())
            .filter(|bucket| bucket.at_ms + tier.bucket_ms > from)
            .map(|bucket| bucket.point)
            .collect()
    }

    /// How many points a window currently holds.
    pub fn len(&self, range_s: u64) -> usize {
        let width = bucket_seconds(range_s) * 1_000;
        self.tiers
            .iter()
            .find(|tier| tier.bucket_ms == width)
            .map(|tier| tier.buckets.len() + usize::from(tier.open.is_some()))
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.tiers.iter().all(|tier| tier.buckets.is_empty() && tier.open.is_none())
    }
}

/// Remote monitoring access handed to the plugin by the session.
/// System statistics for one session.
///
/// Extends [`CommandAccess`] because the process table runs its own probes
/// (details, signals) on the host without going through the sampler.
pub trait MonitorAccess: crate::command::CommandAccess {
    /// Ask for a fresh sample. Calling this every frame is fine; the session enforces the
    /// polling cadence.
    fn request_sample(&mut self);

    /// The newest sample, once one has arrived.
    fn sample(&self) -> Option<&Snapshot>;

    /// Whether a sample is in flight.
    fn is_sampling(&self) -> bool;

    /// The last failure, if the previous poll failed.
    ///
    /// Named apart from [`crate::files::SftpAccess::error`] so a pane holding both
    /// capabilities can call either one.
    fn last_error(&self) -> Option<&str>;
}

/// `used` as a percentage of `total`, clamped to 0..=100.
pub fn percent(used: u64, total: u64) -> Option<f32> {
    if total == 0 {
        return None;
    }
    Some(((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32)
}

/// Busy CPU percentage between two `/proc/stat` samples.
pub fn cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<f32> {
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle.min(total));
    Some(((busy as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32)
}

/// Parse `/proc/loadavg`, including the task counts in its fourth field.
pub fn parse_loadavg(text: &str) -> ([Option<f32>; 3], Option<u32>, Option<u32>) {
    let mut loads = [None, None, None];
    let Some(line) = text.lines().find(|line| !line.trim().is_empty()) else {
        return (loads, None, None);
    };
    for (slot, field) in loads.iter_mut().zip(line.split_whitespace()) {
        *slot = field.parse().ok();
    }
    let tasks = line
        .split_whitespace()
        .nth(3)
        .and_then(|field| field.split_once('/'))
        .and_then(|(running, total)| {
            Some((running.parse::<u32>().ok()?, total.parse::<u32>().ok()?))
        });
    match tasks {
        Some((running, total)) => (loads, Some(running), Some(total)),
        None => (loads, None, None),
    }
}

/// Parse `/proc/loadavg` load averages only.
pub fn parse_load(text: &str) -> [Option<f32>; 3] {
    parse_loadavg(text).0
}

/// Parse one `/proc/stat` line into counters.
fn cpu_line(line: &str) -> Option<CpuTimes> {
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|field| field.parse().ok())
        .collect();
    if values.len() < 5 {
        return None;
    }
    let idle = values[3] + values[4];
    let total: u64 = values.iter().take(8).sum();
    Some(CpuTimes { total, idle })
}

/// Parse the aggregate `cpu` line of `/proc/stat`.
pub fn parse_cpu(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|line| line.starts_with("cpu"))?;
    cpu_line(line)
}

/// Parse the per-core `cpuN` lines of `/proc/stat`.
pub fn parse_cores(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter(|line| line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .filter_map(cpu_line)
        .collect()
}

/// Busy percentage of every core between two samples.
pub fn core_percents(previous: &[CpuTimes], current: &[CpuTimes]) -> Vec<f32> {
    previous
        .iter()
        .zip(current)
        .filter_map(|(previous, current)| cpu_percent(*previous, *current))
        .collect()
}

/// Parse `/proc/meminfo` into byte counts.
pub fn parse_memory(text: &str) -> Memory {
    let mut memory = Memory::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(value) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };
        let bytes = value.saturating_mul(1024);
        match name {
            "MemTotal:" => memory.total = Some(bytes),
            "MemFree:" => memory.free = Some(bytes),
            "MemAvailable:" => memory.available = Some(bytes),
            "Buffers:" => memory.buffers = Some(bytes),
            "Cached:" => memory.cached = Some(bytes),
            "SwapTotal:" => memory.swap_total = Some(bytes),
            "SwapFree:" => memory.swap_free = Some(bytes),
            _ => {}
        }
    }
    memory
}

/// Parse the first data row of `df -kP`: total, used and free bytes.
pub fn parse_disk(text: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let total = fields[1].parse::<u64>().ok();
        let used = fields[2].parse::<u64>().ok();
        let free = fields[3].parse::<u64>().ok();
        if total.is_some() || used.is_some() {
            return (
                total.map(|value| value.saturating_mul(DF_BLOCK_BYTES)),
                used.map(|value| value.saturating_mul(DF_BLOCK_BYTES)),
                free.map(|value| value.saturating_mul(DF_BLOCK_BYTES)),
            );
        }
    }
    (None, None, None)
}

/// Parse `/proc/uptime`, in whole seconds.
pub fn parse_uptime(text: &str) -> Option<u64> {
    let field = text.split_whitespace().next()?;
    field.parse::<f64>().ok().map(|seconds| seconds as u64)
}

/// Parse `/proc/net/dev`, dropping loopback and interfaces with no traffic.
pub fn parse_net(text: &str) -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for line in text.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let fields: Vec<&str> = counters.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        let rx = fields[0].parse::<u64>().unwrap_or(0);
        let tx = fields[8].parse::<u64>().unwrap_or(0);
        out.push((name.to_owned(), rx, tx));
    }
    out
}

/// Parse the `<iface> <speed>` lines read from `/sys/class/net`.
///
/// Values of zero, negative numbers and the `4294967295` placeholder mean the
/// host does not know the speed.
pub fn parse_speeds(text: &str) -> Vec<(String, Option<u64>)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            let value = fields.next();
            let mbps = value
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value > 0 && *value != u32::MAX as i64)
                .map(|value| value as u64);
            Some((name.to_owned(), mbps))
        })
        .collect()
}

/// Sum the read and write bytes of `/proc/diskstats`, skipping loop, ram and
/// optical devices and their partitions.
pub fn parse_diskstats(text: &str) -> (u64, u64) {
    let mut read = 0u64;
    let mut written = 0u64;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        let name = fields[2];
        if name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("sr")
            || name.starts_with("dm-")
            || name.starts_with("zram")
        {
            continue;
        }
        // Partitions end in a digit (`sda1`, `nvme0n1p2`); whole devices do not,
        // except NVMe names such as `nvme0n1`.
        let partition = name
            .strip_prefix("nvme")
            .map(|rest| rest.contains('p'))
            .unwrap_or_else(|| name.chars().last().is_some_and(|c| c.is_ascii_digit()));
        if partition && !name.starts_with("nvme") {
            continue;
        }
        let sectors_read = fields[5].parse::<u64>().unwrap_or(0);
        let sectors_written = fields[9].parse::<u64>().unwrap_or(0);
        read += sectors_read.saturating_mul(512);
        written += sectors_written.saturating_mul(512);
    }
    (read, written)
}

/// Parse the `ps -eo pid=,user=,stat=,ni=,nlwp=,pcpu=,pmem=,args=` output.
pub fn parse_processes(text: &str) -> Vec<Process> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        let command_start = line
            .match_indices(fields[7])
            .next()
            .map(|(index, _)| index)
            .unwrap_or(0);
        out.push(Process {
            pid,
            user: fields[1].to_owned(),
            state: fields[2].chars().next().map(String::from).unwrap_or_default(),
            nice: fields[3].parse().unwrap_or(0),
            threads: fields[4].parse().unwrap_or_default(),
            cpu: fields[5].parse().unwrap_or(0.0),
            mem: fields[6].parse().unwrap_or(0.0),
            command: line[command_start..].trim().to_owned(),
        });
    }
    out
}

/// Parse the trailing section: OS name, kernel release, machine and IP.
pub fn parse_system(text: &str) -> SystemInfo {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let os = lines.next().filter(|line| !line.is_empty()).map(str::to_owned);
    let kernel = lines.next().map(str::to_owned);
    let arch = lines.next().map(str::to_owned);
    let ip = lines
        .next()
        .and_then(|line| line.split_whitespace().next().map(str::to_owned));
    SystemInfo {
        os,
        kernel,
        arch,
        ip,
    }
}

/// Everything one probe reported, before rates are worked out from the previous counters.
#[derive(Default)]
struct Sample {
    cpu: Option<CpuTimes>,
    cores: Vec<CpuTimes>,
    load: [Option<f32>; 3],
    tasks_running: Option<u32>,
    tasks_total: Option<u32>,
    memory: Memory,
    disk: (Option<u64>, Option<u64>, Option<u64>),
    net: Vec<(String, u64, u64)>,
    speeds: Vec<(String, Option<u64>)>,
    disk_io: (u64, u64),
    processes: Vec<Process>,
    /// Cumulative CPU seconds per process, kept so the next sample can tell the rate.
    process_cpu: Vec<(u32, f64)>,
    system: SystemInfo,
    uptime_s: Option<u64>,
}

/// Turn one probe's output into a snapshot. `previous` is the sample counters,
/// needed because every rate is a delta.
pub fn parse_report(text: &str, previous: Option<Counters>, at_ms: u64) -> (Snapshot, Option<Counters>) {
    let mut sections = text.split("@@");
    let loadavg = parse_loadavg(sections.next().unwrap_or_default());
    let stat_text = sections.next().unwrap_or_default();
    let memory_text = sections.next().unwrap_or_default();
    let disk_text = sections.next().unwrap_or_default();
    let uptime_text = sections.next().unwrap_or_default();
    let net_text = sections.next().unwrap_or_default();
    let speed_text = sections.next().unwrap_or_default();
    let diskstats_text = sections.next().unwrap_or_default();
    let processes_text = sections.next().unwrap_or_default();
    let system_text = sections.next().unwrap_or_default();

    let sample = Sample {
        cpu: parse_cpu(stat_text),
        cores: parse_cores(stat_text),
        load: loadavg.0,
        tasks_running: loadavg.1,
        tasks_total: loadavg.2,
        memory: parse_memory(memory_text),
        disk: parse_disk(disk_text),
        net: parse_net(net_text),
        speeds: parse_speeds(speed_text),
        disk_io: parse_diskstats(diskstats_text),
        processes: parse_processes(processes_text),
        process_cpu: Vec::new(),
        system: parse_system(system_text),
        uptime_s: parse_uptime(uptime_text),
    };
    finish(sample, previous, at_ms)
}

/// The probe a Local tab runs on Windows.
///
/// Windows has neither `/proc` nor `ps`, so PowerShell reads the same numbers from CIM
/// and answers with one JSON document for [`parse_windows_report`].
pub const WINDOWS_MONITOR_COMMAND: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$os = Get-CimInstance Win32_OperatingSystem
$cpu = @(Get-CimInstance Win32_PerfRawData_PerfOS_Processor)
$total = $cpu | Where-Object { $_.Name -eq '_Total' } | Select-Object -First 1
$cores = @($cpu | Where-Object { $_.Name -ne '_Total' } | ForEach-Object {
    [pscustomobject]@{
        idle = [double]$_.PercentIdleTime
        total = [double]$_.PercentProcessorTime + [double]$_.PercentIdleTime
    }
})
$memTotal = [double]$os.TotalVisibleMemorySize * 1024
$memFree = [double]$os.FreePhysicalMemory * 1024
$drive = Get-CimInstance Win32_LogicalDisk | Where-Object { $_.DeviceID -eq $env:SystemDrive } | Select-Object -First 1
$io = @(Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk | Where-Object { $_.Name -ne '_Total' })
$net = @(Get-CimInstance Win32_PerfRawData_Tcpip_NetworkInterface |
    Where-Object { $_.Name -notmatch 'Loopback|isatap|Teredo|Pseudo' } |
    ForEach-Object {
        [pscustomobject]@{
            name = $_.Name
            rx = [double]$_.BytesReceivedPersec
            tx = [double]$_.BytesSentPersec
            speedMbps = [double]$_.CurrentBandwidth / 1000000
        }
    })
$processes = @(Get-Process | Where-Object { $_.Id -ne 0 } | ForEach-Object {
    [pscustomobject]@{
        pid = $_.Id
        cpuS = $_.TotalProcessorTime.TotalSeconds
        mem = [double]$_.WorkingSet64
        threads = @($_.Threads).Count
        command = $_.ProcessName
    }
})
$ips = @(Get-CimInstance Win32_NetworkAdapterConfiguration |
    Where-Object { $_.IPEnabled } |
    ForEach-Object { $_.IPAddress } |
    Where-Object { $_ -notmatch ':' } |
    Select-Object -First 1)
[pscustomobject]@{
    cpu = [pscustomobject]@{
        idle = [double]$total.PercentIdleTime
        total = [double]$total.PercentProcessorTime + [double]$total.PercentIdleTime
        cores = $cores
    }
    memory = [pscustomobject]@{
        total = $memTotal
        free = $memFree
        available = $memFree
        swapTotal = [Math]::Max(0.0, ([double]$os.TotalVirtualMemorySize - [double]$os.TotalVisibleMemorySize) * 1024)
        swapFree = [Math]::Max(0.0, ([double]$os.FreeVirtualMemory - [double]$os.FreePhysicalMemory) * 1024)
    }
    disk = $(if ($drive) {
        [pscustomobject]@{
            total = [double]$drive.Size
            free = [double]$drive.FreeSpace
            used = [double]$drive.Size - [double]$drive.FreeSpace
        }
    } else { $null })
    uptimeS = ((Get-Date) - $os.LastBootUpTime).TotalSeconds
    net = $net
    diskIo = [pscustomobject]@{
        read = ($io | Measure-Object DiskReadBytesPersec -Sum).Sum
        write = ($io | Measure-Object DiskWriteBytesPersec -Sum).Sum
    }
    processes = $processes
    system = [pscustomobject]@{
        os = [string]$os.Caption
        kernel = [string]$os.Version
        arch = $env:PROCESSOR_ARCHITECTURE
        ip = $(if ($ips.Count -gt 0) { [string]$ips[0] } else { $null })
    }
} | ConvertTo-Json -Depth 5 -Compress
"#;

/// The probe to send. `windows` asks for the JSON report a local Windows host can answer.
pub fn probe_command(windows: bool) -> &'static str {
    if windows {
        WINDOWS_MONITOR_COMMAND
    } else {
        MONITOR_COMMAND
    }
}

/// Rows the process table keeps.
const PROCESS_ROWS: usize = 30;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsReport {
    cpu: WindowsCpu,
    memory: WindowsMemory,
    disk: Option<WindowsDisk>,
    uptime_s: Option<f64>,
    #[serde(default)]
    net: Vec<WindowsNet>,
    disk_io: Option<WindowsDiskIo>,
    #[serde(default)]
    processes: Vec<WindowsProcess>,
    system: WindowsSystem,
}

#[derive(Deserialize)]
struct WindowsCpu {
    idle: f64,
    total: f64,
    #[serde(default)]
    cores: Vec<WindowsCore>,
}

#[derive(Deserialize)]
struct WindowsCore {
    idle: f64,
    total: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsMemory {
    total: Option<f64>,
    free: Option<f64>,
    available: Option<f64>,
    swap_total: Option<f64>,
    swap_free: Option<f64>,
}

#[derive(Deserialize)]
struct WindowsDisk {
    total: Option<f64>,
    used: Option<f64>,
    free: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsNet {
    name: String,
    rx: f64,
    tx: f64,
    speed_mbps: Option<f64>,
}

#[derive(Deserialize)]
struct WindowsDiskIo {
    read: Option<f64>,
    write: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsProcess {
    pid: u32,
    cpu_s: Option<f64>,
    mem: Option<f64>,
    #[serde(default)]
    threads: u32,
    command: String,
}

#[derive(Deserialize)]
struct WindowsSystem {
    os: Option<String>,
    kernel: Option<String>,
    arch: Option<String>,
    ip: Option<String>,
}

/// Bytes from a JSON number; the probe never sends a negative count.
fn bytes(value: f64) -> u64 {
    value.max(0.0) as u64
}

/// Turn the Windows probe's JSON into a snapshot, with the same counters and rates as
/// the POSIX report.
pub fn parse_windows_report(
    text: &str,
    previous: Option<Counters>,
    at_ms: u64,
) -> (Snapshot, Option<Counters>) {
    let Ok(report) = serde_json::from_str::<WindowsReport>(text.trim()) else {
        return (
            Snapshot {
                at_ms,
                ..Snapshot::default()
            },
            None,
        );
    };
    let uptime_s = report.uptime_s.map(|seconds| seconds.max(0.0) as u64);
    let memory = Memory {
        total: report.memory.total.map(bytes),
        free: report.memory.free.map(bytes),
        available: report.memory.available.map(bytes),
        buffers: None,
        cached: None,
        swap_total: report.memory.swap_total.map(bytes),
        swap_free: report.memory.swap_free.map(bytes),
    };
    let process_cpu: Vec<(u32, f64)> = report
        .processes
        .iter()
        .map(|process| (process.pid, process.cpu_s.unwrap_or(0.0).max(0.0)))
        .collect();
    let elapsed_s = previous
        .as_ref()
        .map(|previous| at_ms.saturating_sub(previous.at_ms) as f64 / 1000.0)
        .filter(|seconds| *seconds > 0.0);
    let mut processes: Vec<Process> = report
        .processes
        .iter()
        .map(|process| {
            let seconds = process.cpu_s.unwrap_or(0.0).max(0.0);
            // A process's CPU share is the growth of its own CPU time; the first sample
            // has nothing to subtract, so it falls back to the average since start.
            let cpu = previous
                .as_ref()
                .zip(elapsed_s)
                .and_then(|(previous, elapsed)| {
                    previous
                        .process_cpu
                        .iter()
                        .find(|(pid, _)| *pid == process.pid)
                        .map(|(_, before)| ((seconds - before) / elapsed * 100.0) as f32)
                })
                .or_else(|| {
                    uptime_s
                        .filter(|uptime| *uptime > 0)
                        .map(|uptime| (seconds / uptime as f64 * 100.0) as f32)
                })
                .unwrap_or(0.0)
                .max(0.0);
            Process {
                pid: process.pid,
                user: "-".to_owned(),
                state: if cpu > 0.0 { "R" } else { "S" }.to_owned(),
                nice: 0,
                threads: process.threads,
                cpu,
                mem: memory
                    .total
                    .and_then(|total| percent(bytes(process.mem.unwrap_or(0.0)), total))
                    .unwrap_or(0.0),
                command: process.command.clone(),
            }
        })
        .collect();
    let running = processes.iter().filter(|process| process.cpu > 0.0).count() as u32;
    let total = processes.len() as u32;
    processes.sort_by(|left, right| right.cpu.total_cmp(&left.cpu));
    processes.truncate(PROCESS_ROWS);

    let sample = Sample {
        cpu: Some(CpuTimes {
            total: bytes(report.cpu.total),
            idle: bytes(report.cpu.idle),
        }),
        cores: report
            .cpu
            .cores
            .iter()
            .map(|core| CpuTimes {
                total: bytes(core.total),
                idle: bytes(core.idle),
            })
            .collect(),
        load: [None; 3],
        tasks_running: Some(running),
        tasks_total: Some(total),
        memory,
        disk: match report.disk {
            Some(disk) => (
                disk.total.map(bytes),
                disk.used.map(bytes),
                disk.free.map(bytes),
            ),
            None => (None, None, None),
        },
        net: report
            .net
            .iter()
            .map(|iface| (iface.name.clone(), bytes(iface.rx), bytes(iface.tx)))
            .collect(),
        speeds: report
            .net
            .iter()
            .map(|iface| {
                let speed = iface
                    .speed_mbps
                    .filter(|speed| *speed > 0.0)
                    .map(|speed| speed as u64);
                (iface.name.clone(), speed)
            })
            .collect(),
        disk_io: report
            .disk_io
            .map(|io| (bytes(io.read.unwrap_or(0.0)), bytes(io.write.unwrap_or(0.0))))
            .unwrap_or_default(),
        processes,
        process_cpu,
        system: SystemInfo {
            os: report.system.os,
            kernel: report.system.kernel,
            arch: report.system.arch,
            ip: report.system.ip,
        },
        uptime_s,
    };
    finish(sample, previous, at_ms)
}

/// Build the snapshot and the counters the next sample compares against.
fn finish(
    sample: Sample,
    previous: Option<Counters>,
    at_ms: u64,
) -> (Snapshot, Option<Counters>) {
    let elapsed_s = previous
        .as_ref()
        .map(|previous| at_ms.saturating_sub(previous.at_ms) as f32 / 1000.0)
        .filter(|seconds| *seconds > 0.0);

    // Rates are per second between this sample and the previous counters.
    let (cpu, core_rates, net, read_rate, write_rate) = match (&previous, sample.cpu) {
        (Some(previous), Some(current)) => {
            let cpu = cpu_percent(previous.cpu, current);
            let core_rates = core_percents(&previous.cores, &sample.cores);
            let net = net_rates(&sample.net, &previous.net, elapsed_s);
            let read_rate = rate(previous.disk_read, sample.disk_io.0, elapsed_s);
            let write_rate = rate(previous.disk_write, sample.disk_io.1, elapsed_s);
            (cpu, core_rates, net, read_rate, write_rate)
        }
        _ => (
            None,
            Vec::new(),
            net_rates(&sample.net, &[], None),
            None,
            None,
        ),
    };

    let counters = sample.cpu.map(|cpu| Counters {
        at_ms,
        cpu,
        cores: sample.cores,
        net: sample.net,
        disk_read: sample.disk_io.0,
        disk_write: sample.disk_io.1,
        process_cpu: sample.process_cpu,
    });
    let snapshot = Snapshot {
        at_ms,
        cpu,
        cores: core_rates,
        load: sample.load,
        tasks_running: sample.tasks_running,
        tasks_total: sample.tasks_total,
        memory: sample.memory,
        disk_total: sample.disk.0,
        disk_used: sample.disk.1,
        disk_free: sample.disk.2,
        disk_read_rate: read_rate,
        disk_write_rate: write_rate,
        disk_read_total: Some(sample.disk_io.0),
        disk_write_total: Some(sample.disk_io.1),
        net: net
            .into_iter()
            .map(|iface| attach_speed(iface, &sample.speeds))
            .collect(),
        processes: sample.processes,
        system: sample.system,
        uptime_s: sample.uptime_s,
    };
    (snapshot, counters)
}

/// Attach the reported link speed to an interface.
fn attach_speed(mut iface: NetIface, speeds: &[(String, Option<u64>)]) -> NetIface {
    iface.speed = speeds
        .iter()
        .find(|(name, _)| *name == iface.name)
        .and_then(|(_, speed)| *speed);
    iface
}

/// Per-second rate between two cumulative counter values.
pub fn rate(previous: u64, current: u64, elapsed_s: Option<f32>) -> Option<u64> {
    let elapsed = elapsed_s?;
    let delta = current.saturating_sub(previous);
    Some((delta as f32 / elapsed) as u64)
}

/// Attach rates to the interface counters.
fn net_rates(
    current: &[(String, u64, u64)],
    previous: &[(String, u64, u64)],
    elapsed_s: Option<f32>,
) -> Vec<NetIface> {
    current
        .iter()
        .map(|(name, rx, tx)| {
            let before = previous.iter().find(|(other, _, _)| other == name);
            NetIface {
                name: name.clone(),
                rx_total: *rx,
                tx_total: *tx,
                rx_rate: before.and_then(|(_, rx_before, _)| rate(*rx_before, *rx, elapsed_s)),
                tx_rate: before.and_then(|(_, _, tx_before)| rate(*tx_before, *tx, elapsed_s)),
                speed: None,
            }
        })
        .collect()
}

/// `3.4 / 7.8 GB`.
pub fn format_bytes(bytes: u64) -> String {
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

/// `12d 4h`, `3h 12m`, `4m 05s`.
pub fn format_uptime(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        3600..=86_399 => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
        _ => format!(
            "{}d {}h",
            seconds / 86_400,
            (seconds % 86_400) / 3600
        ),
    }
}

/// Data key the selected time range is kept under in the plugin's own data.
pub const RANGE_KEY: &str = "timeRange";
/// Data key of the process table's sort column.
pub const PROCESS_SORT_KEY: &str = "processSort";
/// Data key of the process table's sort direction.
pub const PROCESS_SORT_DESC_KEY: &str = "processSortDesc";

/// How many CPU samples per process the details dialog can draw.
const PROCESS_GRAPH_SAMPLES: usize = 120;

/// The open details dialog.
#[derive(Clone, Debug)]
struct Details {
    pid: u32,
    command: String,
    /// Rows from the probe, once it answered.
    fields: Option<Vec<(String, String)>>,
    /// Why the probe showed nothing.
    error: Option<String>,
    /// Signal the send button will use.
    signal: &'static str,
    /// A probe or a signal command is in flight.
    loading: bool,
}

/// What a click on the process table asked for.
enum ProcIntent {
    Sort(ProcessColumn),
    Details(u32, String),
    Signal(u32, &'static str),
}

/// What the pane remembers between frames.
#[derive(Clone, Debug)]
struct PaneState {
    history: History,
    /// Selected window, in seconds.
    range_s: u64,
    /// Timestamp of the last sample folded into the history.
    last_at: u64,
    /// Process table sort column and direction.
    sort: ProcessColumn,
    descending: bool,
    /// Row whose context menu is open, and where it was opened.
    menu: Option<(egui::Pos2, u32)>,
    /// Details dialog, when open.
    details: Option<Details>,
    /// One-line result of the last action, and whether it failed.
    status: Option<(String, bool)>,
    /// Recent CPU percentages per pid, for the details graph.
    proc_history: std::collections::HashMap<u32, std::collections::VecDeque<(u64, f32)>>,
    /// Whether the sampled host is Windows, where the process probes are PowerShell.
    windows: bool,
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            history: History::new(),
            // The same window the reference app starts on.
            range_s: 600,
            last_at: 0,
            sort: ProcessColumn::default(),
            descending: default_descending(ProcessColumn::default()),
            menu: None,
            details: None,
            status: None,
            proc_history: std::collections::HashMap::new(),
            windows: false,
        }
    }
}

impl PaneState {
    /// A pane with `settings` already applied; used by the tests to check that
    /// the stored range and sort order come back.
    #[cfg(test)]
    fn load(settings: &serde_json::Map<String, serde_json::Value>) -> Self {
        let mut state = Self::default();
        state.apply_settings(settings);
        state
    }

    /// Read the persisted range and sort order back into this pane.
    ///
    /// Everything the pane owns (history, dialogs, the open menu) is kept: a
    /// host settings change must not close a dialog or drop the ladder.
    fn apply_settings(&mut self, settings: &serde_json::Map<String, serde_json::Value>) {
        if let Some(label) = settings.get(RANGE_KEY).and_then(serde_json::Value::as_str)
            && let Some((_, seconds)) = TIME_RANGES.iter().find(|(name, _)| *name == label)
        {
            self.range_s = *seconds;
        }
        if let Some(column) = settings
            .get(PROCESS_SORT_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(ProcessColumn::parse)
        {
            self.sort = column;
        }
        if let Some(descending) = settings
            .get(PROCESS_SORT_DESC_KEY)
            .and_then(serde_json::Value::as_bool)
        {
            self.descending = descending;
        }
    }

    /// Keep one CPU sample per process, for the details graph.
    fn record_processes(&mut self, sample: &Snapshot) {
        self.proc_history
            .retain(|pid, _| sample.processes.iter().any(|process| process.pid == *pid));
        for process in &sample.processes {
            let samples = self.proc_history.entry(process.pid).or_default();
            samples.push_back((sample.at_ms, process.cpu));
            while samples.len() > PROCESS_GRAPH_SAMPLES {
                samples.pop_front();
            }
        }
    }

    /// Position of the sorted column in [`PROCESS_COLUMNS`].
    fn sort_index(&self) -> usize {
        PROCESS_COLUMNS
            .iter()
            .position(|(column, _, _)| *column == self.sort)
            .unwrap_or(0)
    }

    /// Click a column heading: same column flips the direction, a new column
    /// starts in its natural direction.
    fn sort_by(&mut self, column: ProcessColumn) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            self.descending = default_descending(column);
        }
    }

    /// CPU percentages recorded for `pid`, oldest first.
    fn process_series(&self, pid: u32) -> Vec<f32> {
        self.proc_history
            .get(&pid)
            .map(|samples| samples.iter().map(|(_, cpu)| *cpu).collect())
            .unwrap_or_default()
    }

    fn label(&self) -> &'static str {
        TIME_RANGES
            .iter()
            .find(|(_, seconds)| *seconds == self.range_s)
            .map(|(label, _)| *label)
            .unwrap_or("10m")
    }
}

/// The Server Monitor pane.
#[derive(Default)]
pub struct ServerMonitor {
    state: PaneState,
}

impl Plugin for ServerMonitor {
    fn manifest(&self) -> PluginManifest {
        PluginId::ServerMonitor.manifest()
    }

    fn show(&mut self, ui: &mut Ui, context: &mut PluginContext<'_>) {
        let palette = context.palette;
        let Some(access) = context.session.as_deref_mut() else {
            empty_state(
                ui,
                &palette,
                "No statistics",
                "System statistics need an SSH or local session.",
            );
            return;
        };
        let host_label = context.host_label.clone();
        // The section switches are the plugin's per-host settings, so the chips
        // write straight back into the dialog that owns them.
        let mut view = View::from_settings(context.settings);
        self.state.apply_settings(context.settings);
        draw(ui, &palette, &host_label, access, &mut view, &mut self.state);
        view.persist(context);
        context.set_value(
            RANGE_KEY,
            serde_json::Value::from(self.state.label()),
        );
        context.set_value(
            PROCESS_SORT_KEY,
            serde_json::Value::from(self.state.sort.key()),
        );
        context.set_value(
            PROCESS_SORT_DESC_KEY,
            serde_json::Value::Bool(self.state.descending),
        );
    }
}

/// Which sections the pane shows; mirrors the plugin's host settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    gauges: bool,
    history: bool,
    status: bool,
    processes: bool,
    network: bool,
}

impl View {
    fn from_settings(settings: &serde_json::Map<String, serde_json::Value>) -> Self {
        let flag = |key: &str, fallback: bool| {
            settings
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(fallback)
        };
        Self {
            gauges: flag("showGauges", true),
            history: flag("showSparks", true),
            status: flag("showStatus", true),
            processes: flag("showProcesses", true),
            network: flag("showNetwork", true),
        }
    }

    fn persist(&self, context: &mut PluginContext<'_>) {
        for (key, value) in [
            ("showGauges", self.gauges),
            ("showSparks", self.history),
            ("showStatus", self.status),
            ("showProcesses", self.processes),
            ("showNetwork", self.network),
        ] {
            context.set_value(key, serde_json::Value::Bool(value));
        }
    }
}

/// Colours the monitor uses for its sections.
struct MeterColors {
    cpu: egui::Color32,
    memory: egui::Color32,
    swap: egui::Color32,
    disk: egui::Color32,
    network: egui::Color32,
    cache: egui::Color32,
    buffers: egui::Color32,
}

impl MeterColors {
    fn new(_palette: &Palette) -> Self {
        // The reference pins these in CSS (`--monitor-*`), so they are the same
        // in both themes and the sections read the same way everywhere.
        Self {
            cpu: egui::Color32::from_rgb(0x3d, 0x8b, 0xfd),
            memory: egui::Color32::from_rgb(0x3d, 0xd6, 0x8c),
            swap: egui::Color32::from_rgb(0xf0, 0x71, 0x78),
            disk: egui::Color32::from_rgb(0xf0, 0xb4, 0x29),
            network: egui::Color32::from_rgb(0x2d, 0xd4, 0xbf),
            cache: egui::Color32::from_rgb(0x86, 0xef, 0xac),
            buffers: egui::Color32::from_rgb(0x5e, 0xea, 0xd4),
        }
    }
}

/// Draw the statistics for `access`.
fn draw(
    ui: &mut Ui,
    palette: &Palette,
    host_label: &str,
    access: &mut dyn MonitorAccess,
    view: &mut View,
    state: &mut PaneState,
) {
    let muted = to_color32(palette.text_muted);
    let colours = MeterColors::new(palette);
    access.request_sample();
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));

    // Section chips, then the sample itself.
    ui.horizontal_wrapped(|ui| {
        let chips = [
            ("Gauges", &mut view.gauges),
            ("History", &mut view.history),
            ("Status", &mut view.status),
            ("Procs", &mut view.processes),
            ("Net", &mut view.network),
        ];
        for (label, flag) in chips {
            if chip(ui, palette, label, *flag) {
                *flag = !*flag;
            }
        }
    });
    ui.add_space(6.0);
    // The range is a snapped slider, like the reference: one step per window,
    // with the chosen window spelled out beside it.
    if let Some(current) = TIME_RANGES.iter().position(|(_, seconds)| *seconds == state.range_s)
    {
        let mut index = current;
        let changed = slider_field(
            ui,
            palette,
            "TIME RANGE",
            TIME_RANGES[index].0,
            SLIDER_WIDTH,
            TIME_RANGES.len(),
            &mut index,
        );
        if changed {
            state.range_s = TIME_RANGES[index].1;
        }
    }
    ui.add_space(6.0);

    if let Some(error) = access.last_error() {
        banner(ui, palette, BannerKind::Warn, error);
        ui.add_space(6.0);
    }

    let Some(sample) = access.sample().cloned() else {
        let text = if access.is_sampling() {
            "Sampling…"
        } else {
            "Waiting for the first sample."
        };
        ui.label(egui::RichText::new(text).size(11.0).color(muted));
        return;
    };
    // One ladder entry per sample, whatever the window is set to.
    if sample.at_ms != state.last_at {
        state.history.push(&sample);
        state.record_processes(&sample);
        state.last_at = sample.at_ms;
    }

    // Non-Linux hosts have no /proc, so every section comes back empty.
    if sample.load.iter().all(Option::is_none)
        && sample.memory.total.is_none()
        && sample.disk_total.is_none()
        && sample.uptime_s.is_none()
    {
        empty_state(
            ui,
            palette,
            "No /proc statistics",
            "This host does not report system statistics.",
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("wassh-monitor")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The scrollbar is painted over the right edge once the content is
            // taller than the pane, so the cards stop short of it.
            let width = (ui.available_width() - SCROLLBAR_GUTTER).max(140.0);
            ui.set_width(width);
            if view.gauges {
                gauges(ui, palette, &sample, &colours, width);
                ui.add_space(8.0);
                if !sample.cores.is_empty() {
                    cores_card(ui, palette, &sample, &colours, width);
                    ui.add_space(8.0);
                }
                memory_card(ui, palette, &sample, &colours, width);
                ui.add_space(8.0);
            }
            if view.history {
                let points = state.history.series(state.range_s, sample.at_ms);
                chart_row(ui, palette, &points, &colours, width);
                ui.add_space(8.0);
            }
            if view.network {
                network_card(ui, palette, &sample, &colours, width);
                ui.add_space(8.0);
            }
            let points = state.history.series(state.range_s, sample.at_ms);
            disk_card(ui, palette, &points, &sample, &colours, width);
            ui.add_space(8.0);
            if view.status {
                status_card(ui, palette, &sample, width);
                ui.add_space(8.0);
            }
            if view.processes {
                processes_card(ui, palette, access, &sample, state, width);
                ui.add_space(8.0);
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} · updated {}",
                    host_label,
                    time::format_millis(sample.at_ms)
                ))
                .size(10.0)
                .color(muted),
            );
        });
}

/// A card with an explicit width.
///
/// `card()` alone sizes itself to its content, which lets a wide row (a
/// right-aligned value, a long label) grow past the pane. Reserving the width up
/// front keeps every card inside the dock.
fn sized_card(ui: &mut Ui, palette: &Palette, width: f32, add: impl FnOnce(&mut Ui)) {
    fixed_inset_card(ui, palette, width.max(48.0), egui::Margin::same(12), add);
}

/// [`sized_card`] with a custom inner margin (the gauge cards are tighter).
fn tight_card(
    ui: &mut Ui,
    palette: &Palette,
    width: f32,
    (horizontal, vertical): (i8, i8),
    add: impl FnOnce(&mut Ui),
) {
    fixed_inset_card(
        ui,
        palette,
        width.max(48.0),
        egui::Margin::symmetric(horizontal, vertical),
        add,
    );
}

/// `label  [bar]  value` on one line, painted to an exact width.
///
/// Laying this out by hand instead of with nested `ui.horizontal` calls keeps
/// the row exactly inside the card: egui adds its item spacing between every
/// child, which is what pushed these rows past the right edge before.
fn meter_row(
    ui: &mut Ui,
    palette: &Palette,
    row_width: f32,
    label: &str,
    fraction: f32,
    colour: egui::Color32,
    value: &str,
) {
    let height = 14.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(row_width, height), egui::Sense::hover());
    let painter = ui.painter();
    let font = egui::FontId::proportional(10.0);
    let muted = to_color32(palette.text_muted);
    let text = to_color32(palette.text);

    let label = painter.layout_no_wrap(label.to_owned(), font.clone(), muted);
    let value = painter.layout_no_wrap(value.to_owned(), font.clone(), text);
    let left = rect.left();
    let middle = rect.center().y;
    let label_top = middle - label.size().y / 2.0;
    let value_top = middle - value.size().y / 2.0;
    let label_width = label.size().x;
    let value_width = value.size().x;
    painter.galley(egui::pos2(left, label_top), label, muted);

    let bar_left = left + label_width + 6.0;
    let bar_right = (rect.right() - value_width - 6.0).max(bar_left + 12.0);
    let bar = egui::Rect::from_center_size(
        egui::pos2((bar_left + bar_right) / 2.0, middle),
        egui::Vec2::new((bar_right - bar_left).max(12.0), 6.0),
    );
    painter.rect_filled(bar, egui::CornerRadius::same(3), faded(palette.border, 0.55));
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(
                bar.min,
                egui::Vec2::new((bar.width() * fraction).max(6.0), bar.height()),
            ),
            egui::CornerRadius::same(3),
            colour,
        );
    }
    painter.galley(
        egui::pos2(bar_right + 6.0, value_top),
        value,
        text,
    );
}

/// Width of one glyph of `font`.
fn glyph_width(ui: &Ui, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap("0".to_owned(), font.clone(), egui::Color32::WHITE)
        .size()
        .x
}

/// A monospace font small enough that `chars` of them fit in `width`.
///
/// The tables are fixed-width text, so on a narrow dock they would otherwise
/// widen the whole card; shrinking the font keeps the columns aligned.
fn mono_fit(ui: &Ui, chars: usize, width: f32, size: f32, floor: f32) -> egui::FontId {
    let mut size = size;
    loop {
        let font = egui::FontId::monospace(size);
        if glyph_width(ui, &font) * chars as f32 <= width || size <= floor {
            return font;
        }
        size -= 0.5;
    }
}

/// The three donut gauges: CPU, memory and swap.
fn gauges(
    ui: &mut Ui,
    palette: &Palette,
    sample: &Snapshot,
    colours: &MeterColors,
    width: f32,
) {
    // Three across a wide pane, one per row when the dock is narrow.
    let columns = if width < GAUGE_ROW_MIN { 1 } else { 3 };
    let card_width = (width - (columns as f32 - 1.0) * CARD_GAP) / columns as f32;
    // The ring keeps its reference size; it only shrinks when the card itself
    // is too narrow to hold it.
    let ring = if card_width < GAUGE_MIN_CARD {
        GAUGE_SIZE_COMPACT
    } else {
        GAUGE_SIZE
    };

    let load = sample
        .load
        .iter()
        .map(|load| format_load(*load))
        .collect::<Vec<_>>()
        .join(" · ");
    let memory_detail = match (sample.memory.used(), sample.memory.total) {
        (Some(used), Some(total)) => format!("{} / {}", format_bytes(used), format_bytes(total)),
        _ => "—".to_owned(),
    };
    let swap_detail = match (sample.memory.swap_total, sample.memory.swap_free) {
        (Some(total), Some(free)) => format!(
            "{} / {}",
            format_bytes(total.saturating_sub(free)),
            format_bytes(total)
        ),
        _ => "—".to_owned(),
    };

    let panels = [
        (
            "CPU",
            sample.cpu.map(|cpu| cpu / 100.0),
            colours.cpu,
            format!("Load {load}"),
        ),
        (
            "Memory",
            sample.mem_percent().map(|value| value / 100.0),
            colours.memory,
            memory_detail,
        ),
        (
            "Swap",
            sample.swap_percent().map(|value| value / 100.0),
            colours.swap,
            swap_detail,
        ),
    ]
    .to_vec();

    // `chunks` keeps the rows exact: a wrapped row would let a card overflow.
    for (row, chunk) in panels.chunks(columns.max(1)).enumerate() {
        if row > 0 {
            ui.add_space(CARD_GAP);
        }
        // `horizontal_top`: a centered row would push each card further down
        // than the one before it, because the cards are allocated with an
        // unknown height.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for (caption, fraction, colour, detail) in chunk {
                tight_card(ui, palette, card_width, GAUGE_MARGIN, |ui| {
                    ui.vertical_centered(|ui| {
                        ring_gauge(ui, palette, ring, *fraction, *colour, caption, detail);
                    });
                });
            }
        });
    }
}

/// Per-core utilisation bars.
fn cores_card(
    ui: &mut Ui,
    palette: &Palette,
    sample: &Snapshot,
    colours: &MeterColors,
    width: f32,
) {
    sized_card(ui, palette, width, |ui| {
        ui.label(
            egui::RichText::new("CPU CORES")
                .size(10.0)
                .color(to_color32(palette.text_muted)),
        );
        ui.add_space(6.0);
        // The reference lays the bars out in an `auto-fill` grid: as many 14 px
        // columns as the card holds, each bar taking exactly one column, so the
        // gaps stay tight instead of growing with the pane.
        let content = ui.available_width().max(64.0);
        let columns = (((content + CORE_BAR_GAP) / (CORE_BAR_WIDTH + CORE_BAR_GAP))
            .floor() as usize)
            .max(1);
        let bar_width =
            ((content - (columns as f32 - 1.0) * CORE_BAR_GAP) / columns as f32).max(4.0);
        let font = egui::FontId::proportional(9.0);
        let track = faded(palette.border, 0.55);
        for (row, cores) in sample.cores.chunks(columns).enumerate() {
            if row > 0 {
                ui.add_space(CORE_BAR_GAP);
            }
            // `horizontal_top`, and a real height for the column: a centred
            // row with zero-height children walks each bar further down than
            // the one before it.
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = CORE_BAR_GAP;
                for (offset, core) in cores.iter().enumerate() {
                    let index = row * columns + offset;
                    ui.allocate_ui_with_layout(
                        egui::Vec2::new(bar_width, CORE_BAR_HEIGHT + CORE_LABEL_HEIGHT),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.set_width(bar_width);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::Vec2::new(bar_width, CORE_BAR_HEIGHT),
                                egui::Sense::hover(),
                            );
                            ui.painter()
                                .rect_filled(rect, egui::CornerRadius::same(3), track);
                            let filled =
                                (rect.height() * (core / 100.0).clamp(0.0, 1.0)).max(2.0);
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(rect.left(), rect.bottom() - filled),
                                    egui::Vec2::new(rect.width(), filled),
                                ),
                                egui::CornerRadius::same(3),
                                colours.cpu,
                            );
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(index.to_string())
                                    .font(font.clone())
                                    .color(to_color32(palette.text_muted)),
                            );
                        },
                    );
                }
            });
        }
    });
}

/// The memory breakdown bar with its legend.
fn memory_card(
    ui: &mut Ui,
    palette: &Palette,
    sample: &Snapshot,
    colours: &MeterColors,
    width: f32,
) {
    let muted = to_color32(palette.text_muted);
    let text = to_color32(palette.text);
    sized_card(ui, palette, width, |ui| {
        ui.label(egui::RichText::new("MEMORY").size(10.0).color(muted));
        ui.add_space(6.0);
        let total = sample.memory.total.unwrap_or(0).max(1) as f32;
        let app = sample.memory.app().unwrap_or(0) as f32;
        let buffers = sample.memory.buffers.unwrap_or(0) as f32;
        let cache = sample.memory.cached.unwrap_or(0) as f32;
        let free = sample.memory.free.unwrap_or(0) as f32;
        segmented_bar(
            ui,
            palette,
            &[
                (app / total, colours.cpu),
                (buffers / total, colours.buffers),
                (cache / total, colours.cache),
                (free / total, faded(palette.border, 0.55)),
            ],
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (label, value, colour) in [
                ("App", sample.memory.app(), colours.cpu),
                ("Buf", sample.memory.buffers, colours.buffers),
                ("Cache", sample.memory.cached, colours.cache),
                ("Free", sample.memory.free, faded(palette.border, 0.55)),
            ] {
                let (rect, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(7.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, colour);
                ui.label(
                    egui::RichText::new(format!(
                        "{label} {}",
                        value.map(format_bytes).unwrap_or_else(|| "—".to_owned())
                    ))
                    .size(10.0)
                    .color(text),
                );
                ui.add_space(8.0);
            }
        });
    });
}

/// CPU and memory history cards.
fn chart_row(
    ui: &mut Ui,
    palette: &Palette,
    points: &[Point],
    colours: &MeterColors,
    width: f32,
) {
    ui.horizontal_top(|ui| {
        // `horizontal` inserts its own item spacing; setting it to the card gap
        // and not adding another keeps the two cards inside the pane.
        ui.spacing_mut().item_spacing.x = CARD_GAP;
        let card_width = (width - CARD_GAP) / 2.0;
        for (label, pick, colour) in [
            (
                "CPU",
                Box::new(|point: &Point| point.cpu) as Box<dyn Fn(&Point) -> Option<f32>>,
                colours.cpu,
            ),
            (
                "Memory",
                Box::new(|point: &Point| point.mem),
                colours.memory,
            ),
        ] {
            sized_card(ui, palette, card_width, |ui| {
                let value = points
                    .last()
                    .and_then(&pick)
                    .map(|value| format!("{value:.0}%"))
                    .unwrap_or_else(|| "—".to_owned());
                stat_row(ui, palette, label, &value);
                ui.add_space(4.0);
                let values: Vec<f32> = points.iter().filter_map(&pick).collect();
                line_chart(ui, palette, &values, 100.0, colour);
            });
        }
    });
}

/// Network rates, totals and the interface table.
fn network_card(
    ui: &mut Ui,
    palette: &Palette,
    sample: &Snapshot,
    colours: &MeterColors,
    width: f32,
) {
    let muted = to_color32(palette.text_muted);
    let text = to_color32(palette.text);
    sized_card(ui, palette, width, |ui| {
        // The card's own content width: the frame's padding and border are
        // already taken off, so nothing measured from here can leave the card.
        let content = ui.available_width();
        ui.label(egui::RichText::new("NETWORK").size(10.0).color(muted));
        ui.add_space(6.0);

        let rx = sample.net_rx_rate();
        let tx = sample.net_tx_rate();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            // Two pixels shy of an exact half: a frame reports its border on
            // top of the content it was given.
            let tile_width = (content - CARD_GAP) / 2.0 - 2.0;
            for (label, rate) in [("Net ↓", rx), ("Net ↑", tx)] {
                egui::Frame::NONE
                    .fill(to_color32(palette.bg_app))
                    .stroke(egui::Stroke::new(1.0, faded(palette.border, 0.7)))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width((tile_width - 18.0).max(40.0));
                        stat_row(ui, palette, label, &format!("{}/s", format_bytes(rate)));
                    });
            }
        });
        ui.add_space(6.0);

        let peak = rx.max(tx).max(1) as f32;
        for (label, rate) in [("↓ RX", rx), ("↑ TX", tx)] {
            meter_row(
                ui,
                palette,
                content,
                label,
                rate as f32 / peak,
                colours.network,
                &format!("{}/s", format_bytes(rate)),
            );
        }
        ui.add_space(6.0);

        let (fastest, reported) = sample
            .net
            .iter()
            .map(NetIface::link_speed_mbps)
            .max_by_key(|(speed, _)| *speed)
            .unwrap_or((ASSUMED_SPEEDS_MBPS[0], false));
        let capacity = format_bytes(fastest.saturating_mul(1_000_000) / 8);
        // Two short lines: one wide line overflows a narrow dock pane.
        ui.label(
            egui::RichText::new(format!(
                "Total RX {}   Total TX {}",
                format_bytes(sample.net_rx_total()),
                format_bytes(sample.net_tx_total())
            ))
            .size(10.0)
            .color(muted),
        );
        ui.label(
            egui::RichText::new(format!(
                "Cap {}{}/s",
                if reported { "" } else { "~" },
                capacity
            ))
            .size(10.0)
            .color(muted),
        );

        if sample.net.is_empty() {
            return;
        }
        ui.add_space(6.0);
        // 6 columns of 10 plus the five spaces between them.
        let font = mono_fit(ui, 65, content, 10.0, 8.0);
        ui.label(
            egui::RichText::new(format!(
                "{:<10} {:>10} {:>10} {:>10} {:>10} {:>10}",
                "Iface", "Speed", "RX", "TX", "Total RX", "Total TX"
            ))
            .font(font.clone())
            .color(muted),
        );
        for iface in &sample.net {
            let (speed, reported) = iface.link_speed_mbps();
            ui.label(
                egui::RichText::new(format!(
                    "{:<10} {:>10} {:>10} {:>10} {:>10} {:>10}",
                    iface.name,
                    format!("{}{}", if reported { "" } else { "~" }, format_speed(speed)),
                    format!("{}/s", format_bytes(iface.rx_rate.unwrap_or(0))),
                    format!("{}/s", format_bytes(iface.tx_rate.unwrap_or(0))),
                    format_bytes(iface.rx_total),
                    format_bytes(iface.tx_total)
                ))
                .font(font.clone())
                .color(text),
            );
        }
    });
}

/// The disk gauge, usage bar, read/write charts and totals.
fn disk_card(
    ui: &mut Ui,
    palette: &Palette,
    points: &[Point],
    sample: &Snapshot,
    colours: &MeterColors,
    width: f32,
) {
    let muted = to_color32(palette.text_muted);
    let text = to_color32(palette.text);
    sized_card(ui, palette, width, |ui| {
        ui.label(egui::RichText::new("DISK").size(10.0).color(muted));
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ring_gauge(
                ui,
                palette,
                GAUGE_SIZE,
                sample.disk_percent().map(|value| value / 100.0),
                colours.disk,
                "Disk /",
                &match (sample.disk_used, sample.disk_total) {
                    (Some(used), Some(total)) => {
                        format!("{} / {}", format_bytes(used), format_bytes(total))
                    }
                    _ => "—".to_owned(),
                },
            );
        });
        ui.add_space(6.0);
        stat_row(
            ui,
            palette,
            "Used",
            &sample
                .disk_percent()
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "—".to_owned()),
        );
        bar_meter(
            ui,
            palette,
            sample.disk_percent().map(|value| value / 100.0),
            colours.disk,
        );
        ui.add_space(8.0);
        for (label, pick, colour) in [
            (
                "Read",
                Box::new(|point: &Point| point.read) as Box<dyn Fn(&Point) -> Option<f32>>,
                colours.memory,
            ),
            (
                "Write",
                Box::new(|point: &Point| point.write),
                colours.disk,
            ),
        ] {
            let current = pick(&Point::of(sample)).unwrap_or(0.0) as u64;
            stat_row(ui, palette, label, &format!("{}/s", format_bytes(current)));
            let values: Vec<f32> = points.iter().map(|point| pick(point).unwrap_or(0.0)).collect();
            line_chart(ui, palette, &values, 0.0, colour);
            ui.add_space(6.0);
        }
        ui.add_space(2.0);
        ui.horizontal_top(|ui| {
            for (label, value) in [
                ("Free", sample.disk_free.map(format_bytes)),
                ("Read total", sample.disk_read_total.map(format_bytes)),
                ("Write total", sample.disk_write_total.map(format_bytes)),
            ] {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(label).size(10.0).color(muted));
                    ui.label(
                        egui::RichText::new(value.unwrap_or_else(|| "—".to_owned()))
                            .size(11.0)
                            .strong()
                            .color(text),
                    );
                });
                ui.add_space(14.0);
            }
        });
    });
}

/// The status grid.
fn status_card(ui: &mut Ui, palette: &Palette, sample: &Snapshot, width: f32) {
    sized_card(ui, palette, width, |ui| {
        for (label, value) in [
            (
                "Uptime",
                sample.uptime_s.map(format_uptime).unwrap_or_else(|| "—".to_owned()),
            ),
            (
                "Load avg",
                sample
                    .load
                    .iter()
                    .map(|load| format_load(*load))
                    .collect::<Vec<_>>()
                    .join("   "),
            ),
            (
                "RAM free",
                sample.memory.free.map(format_bytes).unwrap_or_else(|| "—".to_owned()),
            ),
            (
                "Tasks",
                match (sample.tasks_running, sample.tasks_total) {
                    (Some(running), Some(total)) => format!("{running} / {total}"),
                    _ => "—".to_owned(),
                },
            ),
            (
                "CPUs",
                if sample.cores.is_empty() {
                    "—".to_owned()
                } else {
                    sample.cores.len().to_string()
                },
            ),
            (
                "OS",
                sample.system.os.clone().unwrap_or_else(|| "—".to_owned()),
            ),
            (
                "Kernel",
                match (&sample.system.kernel, &sample.system.arch) {
                    (Some(kernel), Some(arch)) => format!("{kernel} {arch}"),
                    (Some(kernel), None) => kernel.clone(),
                    _ => "—".to_owned(),
                },
            ),
            (
                "IP address",
                sample.system.ip.clone().unwrap_or_else(|| "—".to_owned()),
            ),
        ] {
            stat_row(ui, palette, label, &value);
        }
    });
}

/// Space a cell keeps on the right of its text.
const CELL_PAD: f32 = 6.0;
/// Narrowest the command column may get before the whole table is scaled down.
const COMMAND_MIN: f32 = 60.0;
/// Space reserved beside the active heading for its sort arrow.
const SORT_ARROW_WIDTH: f32 = 12.0;

/// The eight cells of one process row, in column order.
fn process_cells(process: &Process) -> [String; 8] {
    [
        process.pid.to_string(),
        process.user.clone(),
        process.state.to_uppercase(),
        process.nice.to_string(),
        process.threads.to_string(),
        format!("{:.1}%", process.cpu),
        format!("{:.1}%", process.mem),
        process.command.clone(),
    ]
}

/// Left edge and width of every process-table column.
///
/// `measured` holds the width each column's widest cell needs, so a PID such as
/// `4194304` or a long user name is never clipped; the command column takes
/// whatever is left. On a narrow dock everything is scaled down together, so
/// the table still fits instead of cutting a column off.
fn column_layout(width: f32, measured: &[f32; 7]) -> Vec<(f32, f32)> {
    let width = width.max(120.0);
    let mut widths: Vec<f32> = measured.iter().map(|cell| cell + CELL_PAD).collect();
    let fixed: f32 = widths.iter().sum();
    // Only when the short columns and the narrowest command column no longer
    // fit does anything shrink, so a measured column is never clipped.
    let command = (width - fixed).max(COMMAND_MIN);
    widths.push(command);
    let total: f32 = widths.iter().sum();
    let scale = if total > width { width / total } else { 1.0 };
    let mut left = 0.0;
    widths
        .into_iter()
        .map(|column_width| {
            let column_width = column_width * scale;
            let start = left;
            left += column_width;
            (start, column_width)
        })
        .collect()
}

/// Height of the process table headings.
const HEADING_HEIGHT: f32 = 18.0;
/// Height of one process row.
const ROW_HEIGHT: f32 = 15.0;
/// Command key of the details probe.
const DETAILS_COMMAND_KEY: &str = "proc.details";
/// Command key of a signal.
const SIGNAL_COMMAND_KEY: &str = "proc.signal";

/// The process table: sortable headings, row actions and the details dialog.
fn processes_card(
    ui: &mut Ui,
    palette: &Palette,
    access: &mut dyn MonitorAccess,
    sample: &Snapshot,
    state: &mut PaneState,
    width: f32,
) {
    let mut intent: Option<ProcIntent> = None;
    state.windows = sample
        .system
        .os
        .as_deref()
        .is_some_and(|os| os.contains("Windows"));
    sized_card(ui, palette, width, |ui| {
        let content = ui.available_width();
        let muted = to_color32(palette.text_muted);
        let text = to_color32(palette.text);
        let danger = to_color32(palette.danger);
        let rule = faded(palette.border, 0.7);

        ui.horizontal_top(|ui| {
            ui.label(egui::RichText::new("PROCESSES").size(10.0).color(muted));
            if let Some((message, failed)) = &state.status {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(message)
                            .size(10.0)
                            .color(if *failed { danger } else { muted }),
                    );
                });
            }
        });
        ui.add_space(4.0);

        if sample.processes.is_empty() {
            ui.label(egui::RichText::new("No process data").size(11.0).color(muted));
            return;
        }

        let heading_font = egui::FontId::proportional(10.0);
        let mono_font = egui::FontId::monospace(10.0);
        let cell_font = egui::FontId::proportional(10.5);
        let cell_font_for = |index: usize| {
            if PROCESS_COLUMNS[index].2 {
                mono_font.clone()
            } else {
                cell_font.clone()
            }
        };
        // An auto table: every column is as wide as its widest cell, so a long
        // PID or user name is shown in full.
        let measure = |font: &egui::FontId, text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let mut measured = [0.0_f32; 7];
        for (index, (_, label, _)) in PROCESS_COLUMNS.iter().enumerate().take(7) {
            measured[index] = measure(&heading_font, label);
        }
        // The sort arrow sits beside the active heading. The command column has
        // no measured width (it takes the remainder), so it is skipped.
        if let Some(slot) = measured.get_mut(state.sort_index()) {
            *slot += SORT_ARROW_WIDTH;
        }
        for process in &sample.processes {
            for (index, cell) in process_cells(process).iter().enumerate().take(7) {
                measured[index] = measured[index].max(measure(&cell_font_for(index), cell));
            }
        }
        let columns = column_layout(content, &measured);
        let heading_top = ui.cursor().top();
        // A table has no gaps: rows and their hover fill sit flush.
        ui.spacing_mut().item_spacing.y = 0.0;

        // Headings are the sort control; the active one shows its direction.
        // They share one row, and the item spacing is zeroed so each heading's
        // rect starts exactly where `columns` says the column starts.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, (column, label, _)) in PROCESS_COLUMNS.iter().enumerate() {
                let (_, column_width) = columns[index];
                let (rect, response) = ui.allocate_exact_size(
                    egui::Vec2::new(column_width, HEADING_HEIGHT),
                    egui::Sense::click(),
                );
                let active = state.sort == *column;
                let colour = if active || response.hovered() {
                    text
                } else {
                    muted
                };
                let painter = ui.painter();
                // Room for the arrow is reserved before the label is clipped,
                // so "CPU ▼" never turns into "CPU…".
                let arrow = if active {
                    if state.descending { "▼" } else { "▲" }
                } else {
                    ""
                };
                let arrow_width = if active { SORT_ARROW_WIDTH } else { 0.0 };
                let caption = ellipsize(
                    painter,
                    label,
                    heading_font.clone(),
                    column_width - 4.0 - arrow_width,
                );
                let caption_width = painter
                    .layout_no_wrap(caption.clone(), heading_font.clone(), egui::Color32::WHITE)
                    .size()
                    .x;
                painter.text(
                    egui::pos2(rect.left(), rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    caption,
                    heading_font.clone(),
                    colour,
                );
                if active {
                    painter.text(
                        egui::pos2(rect.left() + caption_width + 2.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        arrow,
                        egui::FontId::proportional(8.0),
                        colour,
                    );
                }
                if response.clicked() {
                    intent = Some(ProcIntent::Sort(*column));
                }
            }
        });
        ui.painter()
            .hline(0.0..=content, heading_top + HEADING_HEIGHT, egui::Stroke::new(1.0, rule));

        let order = sort_processes(&sample.processes, state.sort, state.descending);
        for index in order {
            let process = &sample.processes[index];
            let (rect, response) = ui.allocate_exact_size(
                egui::Vec2::new(content, ROW_HEIGHT),
                egui::Sense::click(),
            );
            let menu_open = matches!(state.menu, Some((_, pid)) if pid == process.pid);
            let painter = ui.painter();
            if menu_open {
                painter.rect_filled(rect, 0.0, faded(palette.accent, 0.14));
            } else if response.hovered() {
                painter.rect_filled(rect, 0.0, faded(palette.accent, 0.08));
            }
            // `double_clicked` first: a double click is also reported as a click.
            if response.double_clicked() {
                state.menu = None;
                intent = Some(ProcIntent::Details(process.pid, process.command.clone()));
            } else if response.secondary_clicked() {
                let position = response
                    .interact_pointer_pos()
                    .unwrap_or_else(|| rect.center());
                state.menu = Some((position, process.pid));
            } else if response.clicked() {
                state.menu = None;
            }
            let cells = process_cells(process);
            for (position, cell) in cells.iter().enumerate() {
                let (left, column_width) = columns[position];
                let font = cell_font_for(position);
                // The state column reads as a qualifier, like the reference.
                let colour = if position == 2 { muted } else { text };
                painter.text(
                    egui::pos2(rect.left() + left, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    ellipsize(painter, cell, font.clone(), column_width - 4.0),
                    font,
                    colour,
                );
            }
        }
    });

    if let Some((position, pid)) = state.menu {
        let items = [
            MenuItem::new("Details…"),
            MenuItem::new("Terminate (SIGTERM)"),
            MenuItem::danger("Kill (SIGKILL)"),
        ];
        let chosen = context_menu(ui, palette, position, &items);
        let dismissed = ui.input(|input| {
            input.pointer.primary_clicked() || input.key_pressed(egui::Key::Escape)
        });
        if let Some(choice) = chosen {
            state.menu = None;
            let command = sample
                .processes
                .iter()
                .find(|process| process.pid == pid)
                .map(|process| process.command.clone())
                .unwrap_or_default();
            intent = Some(match choice {
                0 => ProcIntent::Details(pid, command),
                1 => ProcIntent::Signal(pid, "TERM"),
                _ => ProcIntent::Signal(pid, "KILL"),
            });
        } else if dismissed {
            state.menu = None;
        }
    }

    match intent {
        Some(ProcIntent::Sort(column)) => state.sort_by(column),
        Some(ProcIntent::Details(pid, command)) => open_details(access, state, pid, command),
        Some(ProcIntent::Signal(pid, signal)) => send_signal(access, state, pid, signal),
        None => {}
    }

    if state.details.is_some() {
        poll_details(access, state);
        process_details_dialog(ui, palette, access, state);
    }
}

/// Ask the host for one process's details.
fn open_details(access: &mut dyn MonitorAccess, state: &mut PaneState, pid: u32, command: String) {
    let probe = if state.windows {
        windows_details_command(pid)
    } else {
        details_command(pid)
    };
    access.exec(DETAILS_COMMAND_KEY, &probe);
    state.details = Some(Details {
        pid,
        command,
        fields: None,
        error: None,
        signal: SIGNAL_DEFAULT,
        loading: true,
    });
}

/// Send `signal` to `pid` and say how it went.
fn send_signal(access: &mut dyn MonitorAccess, state: &mut PaneState, pid: u32, signal: &str) {
    let windows = state.windows;
    let command = if windows {
        windows_signal_command(pid, signal)
    } else {
        signal_command(pid, signal)
    };
    let Some(command) = command else {
        state.status = Some((format!("Cannot send {signal} to {pid}"), true));
        return;
    };
    access.exec(SIGNAL_COMMAND_KEY, &command);
    state.status = Some((
        if windows {
            format!("Stopping {pid}…")
        } else {
            format!("Sending SIG{signal} to {pid}…")
        },
        false,
    ));
    if let Some(details) = state.details.as_mut() {
        details.loading = true;
    }
}

/// Collect the answers to the details probe and the signal command.
fn poll_details(access: &mut dyn MonitorAccess, state: &mut PaneState) {
    // A signal replaces the dialog's busy state with its own result line.
    if let Some(details) = state.details.as_mut()
        && details.loading
        && !access.is_running(DETAILS_COMMAND_KEY)
        && !access.is_running(SIGNAL_COMMAND_KEY)
    {
        if let Some(failure) = access.failure(SIGNAL_COMMAND_KEY) {
            state.status = Some((failure.to_owned(), true));
        } else if let Some(output) = access.output(SIGNAL_COMMAND_KEY) {
            let pid = details.pid;
            if output.succeeded() {
                state.status = Some((
                    if state.windows {
                        format!("Stopped {pid}")
                    } else {
                        format!("Signal sent to {pid}")
                    },
                    false,
                ));
            } else {
                let detail = output
                    .error_line()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("kill failed (exit {:?})", output.code));
                state.status = Some((detail, true));
            }
        }
        state.details.as_mut().expect("checked above").loading = false;
    }

    let Some(details) = state.details.as_mut() else {
        return;
    };
    if details.fields.is_some() || details.error.is_some() {
        return;
    }
    if access.is_running(DETAILS_COMMAND_KEY) {
        return;
    }
    if let Some(failure) = access.failure(DETAILS_COMMAND_KEY) {
        details.error = Some(failure.to_owned());
        return;
    }
    if let Some(output) = access.output(DETAILS_COMMAND_KEY) {
        let fields = parse_details(&output.stdout);
        if fields.is_empty() {
            details.error = Some(
                output
                    .error_line()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "The host reported no details".to_owned()),
            );
        } else {
            details.fields = Some(fields);
        }
    }
}

/// The details dialog: what the host knows about one process, and a signal
/// selector.
fn process_details_dialog(
    ui: &mut Ui,
    palette: &Palette,
    access: &mut dyn MonitorAccess,
    state: &mut PaneState,
) {
    let Some(details) = state.details.clone() else {
        return;
    };
    let title = format!("PID {}", details.pid);
    let response = modal(ui.ctx(), ("process-details", details.pid), palette, &title, 560.0, |ui| {
        let muted = to_color32(palette.text_muted);
        let text = to_color32(palette.text);
        ui.label(
            egui::RichText::new(&details.command)
                .size(11.0)
                .color(muted),
        );
        ui.add_space(6.0);

        let series = state.process_series(details.pid);
        if series.len() >= 2 {
            let peak = series.iter().copied().fold(0.0_f32, f32::max);
            let latest = series.last().copied().unwrap_or(0.0);
            stat_row(ui, palette, "CPU", &format!("{latest:.1}%"));
            line_chart(ui, palette, &series, 100.0, to_color32(palette.accent));
            ui.label(
                egui::RichText::new(format!(
                    "peak {peak:.1}% · {} samples",
                    series.len()
                ))
                .size(10.0)
                .color(muted),
            );
            ui.add_space(6.0);
        }

        if let Some(error) = &details.error {
            ui.label(egui::RichText::new(error).size(11.0).color(to_color32(palette.danger)));
        } else if let Some(fields) = &details.fields {
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (label, value) in fields {
                        ui.horizontal_top(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(150.0, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(150.0);
                                    ui.label(
                                        egui::RichText::new(label).size(10.0).color(muted),
                                    );
                                },
                            );
                            ui.label(egui::RichText::new(value).size(10.5).color(text));
                        });
                        ui.add_space(2.0);
                    }
                });
        } else {
            ui.label(egui::RichText::new("Loading…").size(11.0).color(muted));
        }

        footer(ui, |ui| {
            if small_button(ui, palette, "Close") {
                state.details = None;
            }
            if small_button(ui, palette, "Send") {
                send_signal(access, state, details.pid, details.signal);
            }
            let options: Vec<&'static str> = if state.windows {
                WINDOWS_SIGNALS.to_vec()
            } else {
                SIGNALS.iter().map(|(name, _)| *name).collect()
            };
            if let Some(chosen) = select_str(
                ui,
                ("process-signal", details.pid),
                details.signal,
                &options,
            ) && let Some(details) = state.details.as_mut()
            {
                details.signal = SIGNALS
                    .iter()
                    .find(|(name, _)| *name == chosen)
                    .map(|(name, _)| *name)
                    .unwrap_or(SIGNAL_DEFAULT);
            }
            ui.label(egui::RichText::new("Signal").size(10.0).color(muted));
        });
    });
    if response.dismissed {
        state.details = None;
    }
}

/// `text`, shortened with an ellipsis when it is wider than `width`.
fn ellipsize(painter: &egui::Painter, text: &str, font: egui::FontId, width: f32) -> String {
    let measure = |candidate: &str| {
        painter
            .layout_no_wrap(candidate.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    if width <= 0.0 {
        return String::new();
    }
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

/// `0.42`, or `—` when the host did not report it.
fn format_load(load: Option<f32>) -> String {
    match load {
        Some(value) => format!("{value:.2}"),
        None => "—".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    /// No secret settings in these tests: panes read them through the context.
    static EMPTY_SECRETS: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    use super::*;

    const REPORT: &str = "0.42 0.51 0.63 2/345 6789\n@@\n\
cpu  3357 0 3065 123456 500 0 12 0 0 0\n\
cpu0 1678 0 1532 61728 250 0 6 0 0 0\n\
cpu1 1679 0 1533 61728 250 0 6 0 0 0\n@@\n\
MemTotal:       16316304 kB\nMemFree:         1234567 kB\nMemAvailable:    8148152 kB\n\
Buffers:          132096 kB\nCached:          2580480 kB\nSwapTotal:       2097148 kB\n\
SwapFree:        1048574 kB\n@@\n\
Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
/dev/sda1        1024000    614400    409600      60% /\n@@\n\
45678.90 123456.78\n@@\n\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo:    1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0\n\
  eth0: 5000000    4000    0    0    0     0          0         0  2000000    2000    0    0    0     0       0          0\n@@\n\
  eth0 1000\n\
@@\n\
   8       0 sda 1000 0 2048 100 500 0 4096 200 0 0 0 0 0 0 0 0 0\n\
   8       1 sda1 900 0 2048 100 400 0 4096 200 0 0 0 0 0 0 0 0 0\n@@\n\
    1 root       Ss   0  1  0.5  0.1 /sbin/init splash\n\
  420 brainchild Sl   0 12  2.4  1.2 /usr/bin/kitty --single-instance\n@@\n\
Ubuntu 24.04.4 LTS\n6.8.0-124-generic\nx86_64\n192.168.42.7 172.17.0.1\n";

    /// The same report with every counter moved on, two seconds later.
    fn later_report() -> String {
        REPORT
            .replace("0.42 0.51 0.63 2/345", "1.42 1.51 1.63 3/350")
            .replace("cpu  3357 0 3065 123456", "cpu  5357 0 3065 123956")
            .replace("cpu0 1678", "cpu0 2678")
            .replace("cpu1 1679", "cpu1 2679")
            .replace("eth0: 5000000", "eth0: 9000000")
            .replace(" 2000000    2000", " 6000000    2000")
            .replace("sda 1000 0 2048", "sda 1000 0 4096")
    }

    #[test]
    fn a_full_report_parses_into_a_snapshot() {
        let (snapshot, counters) = parse_report(REPORT, None, 1_000);
        assert_eq!(snapshot.at_ms, 1_000);
        assert_eq!(snapshot.load, [Some(0.42), Some(0.51), Some(0.63)]);
        assert_eq!(snapshot.tasks_running, Some(2));
        assert_eq!(snapshot.tasks_total, Some(345));
        assert_eq!(snapshot.memory.total, Some(16_316_304 * 1024));
        assert_eq!(snapshot.memory.available, Some(8_148_152 * 1024));
        assert_eq!(snapshot.memory.buffers, Some(132_096 * 1024));
        assert_eq!(snapshot.memory.cached, Some(2_580_480 * 1024));
        assert_eq!(snapshot.memory.swap_total, Some(2_097_148 * 1024));
        assert_eq!(snapshot.disk_total, Some(1_024_000 * 1024));
        assert_eq!(snapshot.disk_used, Some(614_400 * 1024));
        assert_eq!(snapshot.disk_free, Some(409_600 * 1024));
        assert_eq!(snapshot.uptime_s, Some(45_678));
        // Per-core counters, and the disk totals skip the `sda1` partition.
        assert_eq!(snapshot.disk_read_total, Some(2048 * 512));
        assert_eq!(snapshot.disk_write_total, Some(4096 * 512));
        // Loopback is not an interface.
        assert_eq!(snapshot.net.len(), 1);
        assert_eq!(snapshot.net[0].name, "eth0");
        assert_eq!(snapshot.net[0].rx_total, 5_000_000);
        // The reported link speed is attached to the interface.
        assert_eq!(snapshot.net[0].speed, Some(1_000));
        assert_eq!(snapshot.net[0].link_speed_mbps(), (1_000, true));
        assert_eq!(snapshot.processes.len(), 2);
        assert_eq!(snapshot.processes[0].pid, 1);
        assert_eq!(snapshot.processes[0].user, "root");
        assert_eq!(snapshot.processes[0].state, "S");
        assert_eq!(snapshot.processes[0].command, "/sbin/init splash");
        assert_eq!(snapshot.processes[1].threads, 12);
        assert_eq!(snapshot.system.os.as_deref(), Some("Ubuntu 24.04.4 LTS"));
        assert_eq!(snapshot.system.kernel.as_deref(), Some("6.8.0-124-generic"));
        assert_eq!(snapshot.system.arch.as_deref(), Some("x86_64"));
        assert_eq!(snapshot.system.ip.as_deref(), Some("192.168.42.7"));
        // The first sample has no previous counters to compare against.
        assert_eq!(snapshot.cpu, None);
        assert!(snapshot.cores.is_empty());
        let counters = counters.expect("counters");
        assert_eq!(counters.cpu.idle, 123_956);
        assert_eq!(counters.cpu.total, 3357 + 3065 + 123_456 + 500 + 12);
        assert_eq!(counters.cores.len(), 2);
        assert_eq!(counters.at_ms, 1_000);
    }

    #[test]
    fn a_second_sample_reports_rates() {
        let (_, counters) = parse_report(REPORT, None, 1_000);
        let (snapshot, counters) = parse_report(&later_report(), counters, 3_000);
        // 2000 new busy jiffies and 500 idle ones: 80% busy.
        assert_eq!(snapshot.cpu, Some(80.0));
        assert_eq!(snapshot.cores.len(), 2);
        // 4 MB more received over two seconds.
        let iface = &snapshot.net[0];
        assert_eq!(iface.rx_rate, Some(2_000_000));
        assert_eq!(iface.tx_rate, Some(2_000_000));
        // 2048 more sectors (1 MB) over two seconds.
        assert_eq!(snapshot.disk_read_rate, Some(524_288));
        assert_eq!(snapshot.disk_write_rate, Some(0));
        assert_eq!(counters.expect("counters").at_ms, 3_000);
    }

    /// One answer of the Windows probe, as PowerShell writes it.
    const WINDOWS_REPORT: &str = r#"{
        "cpu": {"idle": 1000, "total": 4000, "cores": [{"idle": 500, "total": 2000}, {"idle": 500, "total": 2000}]},
        "memory": {"total": 17179869184, "free": 4294967296, "available": 4294967296, "swapTotal": 8589934592, "swapFree": 4294967296},
        "disk": {"total": 107374182400, "free": 53687091200, "used": 53687091200},
        "uptimeS": 3600,
        "net": [{"name": "Ethernet", "rx": 1000, "tx": 2000, "speedMbps": 1000}],
        "diskIo": {"read": 4096, "write": 8192},
        "processes": [
            {"pid": 100, "cpuS": 10, "mem": 858993459, "threads": 4, "command": "alpha"},
            {"pid": 200, "cpuS": 2, "mem": 42949672, "threads": 2, "command": "beta"}
        ],
        "system": {"os": "Microsoft Windows 11 Pro", "kernel": "10.0.22631", "arch": "AMD64", "ip": "192.168.1.5"}
    }"#;

    /// The same probe ten seconds later: half the new CPU time was busy, 2 kB more
    /// arrived, 16 kB more was read and `alpha` burnt four more CPU seconds.
    const WINDOWS_REPORT_LATER: &str = r#"{
        "cpu": {"idle": 1500, "total": 5000, "cores": [{"idle": 750, "total": 2500}, {"idle": 750, "total": 2500}]},
        "memory": {"total": 17179869184, "free": 4294967296, "available": 4294967296, "swapTotal": 8589934592, "swapFree": 4294967296},
        "disk": {"total": 107374182400, "free": 53687091200, "used": 53687091200},
        "uptimeS": 3610,
        "net": [{"name": "Ethernet", "rx": 3000, "tx": 2000, "speedMbps": 1000}],
        "diskIo": {"read": 20480, "write": 8192},
        "processes": [
            {"pid": 100, "cpuS": 14, "mem": 858993459, "threads": 4, "command": "alpha"},
            {"pid": 200, "cpuS": 2, "mem": 42949672, "threads": 2, "command": "beta"}
        ],
        "system": {"os": "Microsoft Windows 11 Pro", "kernel": "10.0.22631", "arch": "AMD64", "ip": "192.168.1.5"}
    }"#;

    #[test]
    fn the_windows_report_becomes_a_snapshot() {
        let (snapshot, counters) = parse_windows_report(WINDOWS_REPORT, None, 1_000_000);
        assert_eq!(snapshot.memory.total, Some(17_179_869_184));
        assert_eq!(snapshot.memory.swap_free, Some(4_294_967_296));
        assert_eq!(snapshot.disk_total, Some(107_374_182_400));
        assert_eq!(snapshot.disk_used, Some(53_687_091_200));
        assert_eq!(snapshot.uptime_s, Some(3600));
        assert_eq!(snapshot.net.len(), 1);
        assert_eq!(snapshot.net[0].name, "Ethernet");
        assert_eq!(snapshot.net[0].rx_total, 1000);
        assert_eq!(snapshot.net[0].speed, Some(1000));
        assert_eq!(snapshot.tasks_total, Some(2));
        assert_eq!(snapshot.tasks_running, Some(2));
        assert_eq!(snapshot.system.os.as_deref(), Some("Microsoft Windows 11 Pro"));
        assert_eq!(snapshot.system.ip.as_deref(), Some("192.168.1.5"));
        // Windows has no load average, and the first sample has no counters to compare.
        assert_eq!(snapshot.load, [None, None, None]);
        assert_eq!(snapshot.cpu, None);
        // Without a previous sample a process shows its average CPU since boot.
        assert_eq!(snapshot.processes.len(), 2);
        assert_eq!(snapshot.processes[0].command, "alpha");
        assert!((snapshot.processes[0].cpu - 10.0 / 36.0).abs() < 0.01);
        assert!((snapshot.processes[0].mem - 5.0).abs() < 0.01);
        let counters = counters.expect("counters");
        assert_eq!(counters.cpu.total, 4000);
        assert_eq!(counters.process_cpu.len(), 2);
    }

    #[test]
    fn the_next_windows_report_turns_counters_into_rates() {
        let (_, counters) = parse_windows_report(WINDOWS_REPORT, None, 1_000_000);
        let (snapshot, _) = parse_windows_report(WINDOWS_REPORT_LATER, counters, 1_010_000);
        // 500 of the 1000 new processor ticks were busy.
        assert_eq!(snapshot.cpu, Some(50.0));
        assert_eq!(snapshot.cores.len(), 2);
        assert_eq!(snapshot.net[0].rx_rate, Some(200));
        assert_eq!(snapshot.net[0].tx_rate, Some(0));
        assert_eq!(snapshot.disk_read_rate, Some(1638));
        assert_eq!(snapshot.disk_write_rate, Some(0));
        // `alpha` burnt four of the ten seconds.
        let alpha = snapshot
            .processes
            .iter()
            .find(|process| process.command == "alpha")
            .expect("alpha");
        assert!((alpha.cpu - 40.0).abs() < 0.01, "alpha at {}", alpha.cpu);
        assert_eq!(alpha.state, "R");
        let beta = snapshot
            .processes
            .iter()
            .find(|process| process.command == "beta")
            .expect("beta");
        assert_eq!(beta.cpu, 0.0);
        assert_eq!(beta.state, "S");
    }

    #[test]
    fn a_broken_windows_report_degrades_instead_of_failing() {
        let (snapshot, counters) = parse_windows_report("no json here", None, 5);
        assert_eq!(snapshot.at_ms, 5);
        assert_eq!(snapshot.memory.total, None);
        assert!(counters.is_none());
    }

    #[test]
    fn percentages_follow_the_counters() {
        let previous = CpuTimes {
            total: 1_000,
            idle: 800,
        };
        let current = CpuTimes {
            total: 2_000,
            idle: 1_400,
        };
        let busy = cpu_percent(previous, current).expect("a percentage");
        assert!((busy - 40.0).abs() < f32::EPSILON);
        assert_eq!(cpu_percent(current, current), None);
    }

    #[test]
    fn memory_and_disk_percentages() {
        let snapshot = Snapshot {
            memory: Memory {
                total: Some(100),
                available: Some(40),
                free: Some(20),
                buffers: Some(10),
                cached: Some(20),
                swap_total: Some(50),
                swap_free: Some(10),
            },
            disk_total: Some(200),
            disk_used: Some(50),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.mem_percent(), Some(60.0));
        assert_eq!(snapshot.swap_percent(), Some(80.0));
        assert_eq!(snapshot.disk_percent(), Some(25.0));
        // Used is 60; buffers + cache are 30, so applications hold 30.
        assert_eq!(snapshot.memory.used(), Some(60));
        assert_eq!(snapshot.memory.cached_total(), Some(30));
        assert_eq!(snapshot.memory.app(), Some(30));
        assert_eq!(Snapshot::default().mem_percent(), None);
        assert_eq!(percent(1, 0), None);
    }

    #[test]
    fn garbage_degrades_instead_of_failing() {
        let (snapshot, counters) = parse_report("", None, 5);
        assert_eq!(snapshot.at_ms, 5);
        assert!(counters.is_none());
        assert_eq!(snapshot.net.len(), 0);
        assert_eq!(snapshot.processes.len(), 0);
        assert_eq!(parse_uptime("nope"), None);
        assert_eq!(parse_load(""), [None, None, None]);
        assert_eq!(parse_memory("MemTotal:\n"), Memory::default());
        assert_eq!(parse_disk("Filesystem 1024-blocks Used\n"), (None, None, None));
        assert_eq!(parse_diskstats("garbage"), (0, 0));
        assert!(parse_processes("not a process list").is_empty());
        assert_eq!(parse_system(""), SystemInfo::default());
    }

    #[test]
    fn link_speeds_come_from_the_host_when_it_knows_them() {
        // `enp8s0 1000` is a gigabit link; the -1 and empty values mean unknown.
        let speeds = parse_speeds("docker0 -1\nenp8s0 1000\nlo \nvirbr0 -1\nwlp9s0\n");
        assert_eq!(
            speeds,
            vec![
                ("docker0".to_owned(), None),
                ("enp8s0".to_owned(), Some(1_000)),
                ("lo".to_owned(), None),
                ("virbr0".to_owned(), None),
                ("wlp9s0".to_owned(), None),
            ]
        );
        // The 0xFFFFFFFF placeholder is not a speed either.
        assert_eq!(parse_speeds("eth0 4294967295\n"), vec![("eth0".to_owned(), None)]);

        let mut iface = NetIface {
            name: "enp8s0".to_owned(),
            ..NetIface::default()
        };
        // Unknown until the host tells us: 100 Mbit/s by default.
        assert_eq!(iface.link_speed_mbps(), (100, false));
        iface = attach_speed(iface, &speeds);
        assert_eq!(iface.link_speed_mbps(), (1_000, true));
        assert_eq!(format_speed(1_000), "1 Gbit");
        assert_eq!(format_speed(2_500), "2.5 Gbit");
        assert_eq!(format_speed(100), "100 Mbit");
    }

    #[test]
    fn an_unreported_link_steps_up_with_the_traffic() {
        // 100 Mbit/s is 12.5 MB/s: anything at or below it keeps the default.
        assert_eq!(assumed_speed_mbps(0), 100);
        assert_eq!(assumed_speed_mbps(10 * 1024 * 1024), 100);
        // 30 MB/s needs more than 100 Mbit/s.
        assert_eq!(assumed_speed_mbps(30 * 1024 * 1024), 1_000);
        // 200 MB/s needs at least 2.5 Gbit/s.
        assert_eq!(assumed_speed_mbps(200 * 1024 * 1024), 2_500);
        // 1 GB/s needs 10 Gbit/s, and that is the top of the ladder.
        assert_eq!(assumed_speed_mbps(1_000 * 1024 * 1024), 10_000);
        assert_eq!(assumed_speed_mbps(u64::MAX), 10_000);

        let iface = NetIface {
            name: "eth0".to_owned(),
            rx_rate: Some(30 * 1024 * 1024),
            ..NetIface::default()
        };
        assert_eq!(iface.peak_rate(), 30 * 1024 * 1024);
        assert_eq!(iface.link_speed_mbps(), (1_000, false));
    }

    #[test]
    fn interface_rates_need_a_previous_sample() {
        let current = vec![("eth0".to_owned(), 100, 50)];
        let first = net_rates(&current, &[], None);
        assert_eq!(first[0].rx_rate, None);
        let previous = vec![("eth0".to_owned(), 40, 20)];
        let second = net_rates(&current, &previous, Some(2.0));
        assert_eq!(second[0].rx_rate, Some(30));
        assert_eq!(second[0].tx_rate, Some(15));
    }

    /// Access that serves one fixed sample and records the commands a pane
    /// asked for.
    #[derive(Default)]
    struct FakeMonitor {
        sample: Option<Snapshot>,
        requested: usize,
        sampling: bool,
        error: Option<String>,
        /// `(key, command)` pairs, in the order they were asked for.
        commands: Vec<(String, String)>,
        /// Answers served by [`CommandAccess::output`].
        answers: std::collections::HashMap<String, crate::command::CommandOutput>,
        screen: String,
        typed: String,
    }

    impl crate::command::CommandAccess for FakeMonitor {
        fn exec(&mut self, key: &str, command: &str) {
            self.commands.push((key.to_owned(), command.to_owned()));
        }

        fn output(&self, key: &str) -> Option<&crate::command::CommandOutput> {
            self.answers.get(key)
        }

        fn failure(&self, _key: &str) -> Option<&str> {
            None
        }

        fn is_running(&self, _key: &str) -> bool {
            false
        }
    }

    impl crate::command::TcpAccess for FakeMonitor {
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

    impl crate::plugin::TerminalAccess for FakeMonitor {
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


    impl MonitorAccess for FakeMonitor {
        fn request_sample(&mut self) {
            self.requested += 1;
        }

        fn sample(&self) -> Option<&Snapshot> {
            self.sample.as_ref()
        }

        fn is_sampling(&self) -> bool {
            self.sampling
        }

        fn last_error(&self) -> Option<&str> {
            self.error.as_deref()
        }
    }

    fn sample() -> Snapshot {
        let (snapshot, _) = parse_report(REPORT, None, 1_700_000_000_000);
        let mut snapshot = snapshot;
        snapshot.cpu = Some(37.0);
        snapshot.cores = vec![12.0, 61.0];
        snapshot
    }

    fn draw_pane(access: &mut FakeMonitor) -> Vec<String> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        // Tall enough that the scroll area does not clip a section away.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(460.0, 2400.0),
            )),
            ..Default::default()
        };
        let mut view = View {
            gauges: true,
            history: true,
            status: true,
            processes: true,
            network: true,
        };
        let mut state = PaneState::default();
        let mut painted = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                draw(ui, &palette, "dev", access, &mut view, &mut state);
            });
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

    /// Per-frame cost with every section full, so a regression is caught here
    /// rather than by the user's fan.
    #[test]
    fn the_pane_redraws_quickly() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            ..FakeMonitor::default()
        };
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(460.0, 900.0),
            )),
            ..Default::default()
        };
        let mut view = View {
            gauges: true,
            history: true,
            status: true,
            processes: true,
            network: true,
        };
        let mut state = PaneState::default();
        // Ten minutes of samples, so every chart holds its full window.
        let mut snapshot = sample();
        for index in 0..300u64 {
            snapshot.at_ms = 1_700_000_000_000 + index * 2_000;
            state.history.push(&snapshot);
            state.record_processes(&snapshot);
            state.last_at = snapshot.at_ms;
        }
        let mut frame = |state: &mut PaneState| {
            let output = ctx.run_ui(input.clone(), |ui| {
                draw(ui, &palette, "dev", &mut access, &mut view, state);
            });
            output.drop_without_applying_deltas();
        };
        for _ in 0..3 {
            frame(&mut state);
        }
        let frames = 20;
        let started = std::time::Instant::now();
        for _ in 0..frames {
            frame(&mut state);
        }
        let per_frame = started.elapsed() / frames;
        println!("monitor frame: {per_frame:?}");
        assert!(
            per_frame < std::time::Duration::from_millis(8),
            "one monitor frame took {per_frame:?}"
        );
    }

    #[test]
    fn the_pane_shows_the_reference_sections() {        let mut access = FakeMonitor {
            sample: Some(sample()),
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        let painted = draw_pane(&mut access);
        assert_eq!(access.requested, 2, "one request per frame");
        let has = |needle: &str| painted.iter().any(|text| text == needle);
        for heading in [
            "Gauges", "History", "Status", "Procs", "Net", "CPU", "Memory", "Swap", "CPU CORES",
            "MEMORY", "NETWORK", "DISK", "PROCESSES",
        ] {
            assert!(has(heading), "{heading:?} missing from {painted:?}");
        }
        assert!(has("37%"));
        assert!(painted.iter().any(|text| text.contains("Ubuntu 24.04.4 LTS")));
        assert!(painted.iter().any(|text| text.contains("/sbin/init splash")));
        assert!(painted.iter().any(|text| text.contains("eth0")));
        assert!(painted.iter().any(|text| text.contains("updated")));
    }

    /// Draw at `width` and return the painted text with its x position.
    fn draw_positions(access: &mut FakeMonitor, width: f32) -> Vec<(f32, f32, String)> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(width, 1_600.0),
            )),
            ..Default::default()
        };
        let mut view = View {
            gauges: true,
            history: true,
            status: true,
            processes: true,
            network: true,
        };
        let mut state = PaneState::default();
        let mut rows = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                draw(ui, &palette, "dev", access, &mut view, &mut state);
            });
            let shapes = std::mem::take(&mut output.shapes);
            rows.clear();
            for clipped in shapes {
                if let egui::Shape::Text(text) = clipped.shape {
                    rows.push((
                        text.pos.x,
                        text.pos.x + text.galley.size().x,
                        text.galley.text().to_owned(),
                    ));
                }
            }
            output.drop_without_applying_deltas();
        }
        rows
    }

    #[test]
    fn nothing_is_painted_past_the_right_edge() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        // A narrow dock pane, then the width the reference screenshot uses.
        for width in [360.0, 430.0, 520.0] {
            for (left, right, text) in draw_positions(&mut access, width) {
                assert!(
                    left >= -1.0 && right <= width + 1.0,
                    "{text:?} spans {left}..{right} in a {width} px pane"
                );
            }
            for (left, right, _, _) in card_rects(&mut access, width) {
                assert!(
                    right <= width + 1.0 && left >= -1.0,
                    "a card spans {left}..{right} in a {width} px pane"
                );
            }
        }
    }

    /// Everything the pane paints at `width`.
    fn shapes_at(access: &mut FakeMonitor, width: f32) -> Vec<egui::Shape> {
        shapes_from(access, width, PaneState::default())
    }

    /// [`shapes_at`] starting from `state` (a sort order, an open dialog, …).
    fn shapes_from(access: &mut FakeMonitor, width: f32, mut state: PaneState) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let palette = Palette::dark();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(width, 1_600.0),
            )),
            ..Default::default()
        };
        let mut view = View {
            gauges: true,
            history: true,
            status: true,
            processes: true,
            network: true,
        };
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let mut output = ctx.run_ui(input.clone(), |ui| {
                draw(ui, &palette, "dev", access, &mut view, &mut state);
            });
            shapes = std::mem::take(&mut output.shapes)
                .into_iter()
                .map(|clipped| clipped.shape)
                .collect();
            output.drop_without_applying_deltas();
        }
        shapes
    }

    /// The frame of every card at `width`, as `(left, right, top, bottom)`.
    fn card_rects(access: &mut FakeMonitor, width: f32) -> Vec<(f32, f32, f32, f32)> {
        let palette = Palette::dark();
        let mut cards = Vec::new();
        for shape in shapes_at(access, width) {
            if let egui::Shape::Rect(rect) = shape {
                // Cards are the rounded rects on the input background.
                if rect.fill == to_color32(palette.bg_input)
                    && rect.corner_radius.nw > 0
                    && rect.rect.width() > 40.0
                {
                    cards.push((
                        rect.rect.left(),
                        rect.rect.right(),
                        rect.rect.top(),
                        rect.rect.bottom(),
                    ));
                }
            }
        }
        // Charts and tiles paint on the same colour inside a card; keep only
        // the outer frames.
        let all = cards.clone();
        cards.retain(|card| {
            !all.iter().any(|outer| {
                outer != card
                    && outer.0 <= card.0 + 0.5
                    && outer.1 >= card.1 - 0.5
                    && outer.2 <= card.2 + 0.5
                    && outer.3 >= card.3 - 0.5
            })
        });
        cards
    }

    #[test]
    fn the_cards_fill_their_row_and_line_up() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        for width in [360.0, 430.0, 520.0] {
            let cards = card_rects(&mut access, width);
            assert!(!cards.is_empty(), "no cards were drawn");
            for (left, right, top, bottom) in &cards {
                assert!(*left >= -1.0, "a card starts at {left} in a {width} px pane");
                assert!(*right <= width + 1.0, "a card ends at {right} in a {width} px pane");
                assert!(bottom > top, "a card has no height");
            }
            // Cards that share vertical space are in the same row, so they must
            // start at the same y — a content-sized card drifts down otherwise.
            for (index, a) in cards.iter().enumerate() {
                for b in cards.iter().skip(index + 1) {
                    let overlaps = a.2 < b.3 && b.2 < a.3;
                    if overlaps && (a.0 - b.0).abs() > 1.0 {
                        assert!(
                            (a.2 - b.2).abs() <= 1.0,
                            "cards {:?} and {:?} overlap but do not line up",
                            (a.0, a.2),
                            (b.0, b.2)
                        );
                    }
                }
            }
            // A row of several cards has equal columns; a lone card fills the
            // pane (minus the scrollbar gutter).
            let mut rows: Vec<(f32, Vec<f32>)> = Vec::new();
            for (left, right, top, _) in &cards {
                match rows.iter_mut().find(|(y, _)| (y - top).abs() <= 1.0) {
                    Some((_, widths)) => widths.push(right - left),
                    None => rows.push((*top, vec![right - left])),
                }
            }
            for (top, widths) in &rows {
                if widths.len() == 1 {
                    assert!(
                        widths[0] >= width - 30.0,
                        "the single card at y {top} is only {} wide in a {width} pane",
                        widths[0]
                    );
                } else {
                    let widest = widths.iter().copied().fold(f32::MIN, f32::max);
                    let narrowest = widths.iter().copied().fold(f32::MAX, f32::min);
                    assert!(
                        widest - narrowest <= 1.0,
                        "the row at y {top} has mismatched cards: {widths:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_missing_sample_says_so() {
        let mut access = FakeMonitor {
            sample: None,
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        let painted = draw_pane(&mut access);
        assert!(
            painted
                .iter()
                .any(|text| text == "Waiting for the first sample."),
            "{painted:?}"
        );
    }

    #[test]
    fn the_section_chips_follow_the_host_settings() {
        let mut settings = serde_json::Map::new();
        settings.insert("showNetwork".to_owned(), serde_json::Value::Bool(false));
        let view = View::from_settings(&settings);
        assert!(!view.network);
        assert!(view.gauges, "sections default to shown");

        let mut context_settings = serde_json::Map::new();
        let mut context = PluginContext {
            palette: Palette::dark(),
            host_label: "dev".to_owned(),
            status: wassh_core::SessionStatus::Connected,
            settings: &mut context_settings,
            scope: "host-1".to_owned(),
            session: None,
            events: &[],
            pending_input: Vec::new(),
            secrets: &EMPTY_SECRETS,
            secret_writes: Vec::new(),
        };
        view.persist(&mut context);
        // Written under the schema keys, so the value mirrors into the dialog.
        assert_eq!(context_settings["showNetwork"], serde_json::json!(false));
        assert_eq!(context_settings["showGauges"], serde_json::json!(true));
    }

    #[test]
    fn the_selected_range_is_remembered() {
        let mut settings = serde_json::Map::new();
        settings.insert(RANGE_KEY.to_owned(), serde_json::Value::from("7d"));
        let state = PaneState::load(&settings);
        assert_eq!(state.range_s, 604_800);
        assert_eq!(state.label(), "7d");

        // An unknown value falls back to the default window.
        settings.insert(RANGE_KEY.to_owned(), serde_json::Value::from("3h"));
        assert_eq!(PaneState::load(&settings).range_s, 600);
        assert_eq!(PaneState::default().label(), "10m");
    }

    #[test]
    fn every_range_has_a_bucket_width_that_fits_a_chart() {
        for (label, seconds) in TIME_RANGES {
            let width = bucket_seconds(seconds);
            let points = seconds / width;
            assert!(
                (20..=1_500).contains(&points),
                "{label} would draw {points} points"
            );
        }
        // Short windows are second-resolution, long ones are minutes.
        assert_eq!(bucket_seconds(60), 1);
        assert_eq!(bucket_seconds(600), 15);
        assert_eq!(bucket_seconds(3_600), 15);
        assert_eq!(bucket_seconds(86_400), 60);
        assert_eq!(bucket_seconds(604_800), 600);
    }

    #[test]
    fn the_ladder_folds_samples_into_buckets() {
        let mut history = History::new();
        let start = 1_700_000_000_000u64;
        for (index, cpu) in [10.0, 20.0, 30.0].iter().enumerate() {
            history.push(&Snapshot {
                at_ms: start + index as u64 * 100,
                cpu: Some(*cpu),
                ..Snapshot::default()
            });
        }
        // All three land in the same second, so they average into one bucket.
        let points = history.series(60, start + 300);
        assert_eq!(points.len(), 1);
        assert!((points[0].cpu.unwrap() - 20.0).abs() < 0.01);

        // A sample in the next second starts a new bucket.
        history.push(&Snapshot {
            at_ms: start + 1_000,
            cpu: Some(40.0),
            ..Snapshot::default()
        });
        assert_eq!(history.series(60, start + 1_000).len(), 2);
    }

    #[test]
    fn each_window_reads_its_own_resolution() {
        let mut history = History::new();
        let start = 1_700_000_000_000u64;
        // Six samples a minute apart: five minutes of history.
        for step in 0..6u64 {
            history.push(&Snapshot {
                at_ms: start + step * 60_000,
                cpu: Some(step as f32 * 10.0),
                ..Snapshot::default()
            });
        }
        let now = start + 300_000;

        // A one-minute window keeps only the samples inside it.
        let minute = history.series(60, now);
        assert_eq!(minute.last().and_then(|point| point.cpu), Some(50.0));
        assert!(minute.len() <= 2, "{minute:?}");
        // An hour of it is read at 15 s, so every sample is its own point.
        assert_eq!(history.series(3_600, now).len(), 6);
        // Seven days are folded into ten-minute buckets, so they merge.
        let week = history.series(604_800, now);
        assert!(week.len() < 6, "the coarse tier should merge samples");
        assert_eq!(week.len(), history.len(604_800));
    }

    #[test]
    fn an_old_window_drops_out_of_the_series() {
        let mut history = History::new();
        history.push(&Snapshot {
            at_ms: 1_000_000,
            cpu: Some(50.0),
            ..Snapshot::default()
        });
        // An hour later the sample is outside a ten-minute window.
        assert!(history.series(600, 1_000_000 + 3_600_000).is_empty());
        assert_eq!(history.series(86_400, 1_000_000 + 3_600_000).len(), 1);
        assert!(History::new().is_empty());
    }

    #[test]
    fn the_core_bars_sit_on_one_baseline() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        let palette = Palette::dark();
        // The bar tracks are the only tall, narrow rects on the border colour.
        let track = faded(palette.border, 0.55);
        let bars: Vec<egui::Rect> = shapes_at(&mut access, 430.0)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == track => Some(rect.rect),
                _ => None,
            })
            .filter(|rect| rect.height() >= CORE_BAR_HEIGHT - 1.0)
            .collect();
        assert_eq!(bars.len(), 2, "the sample has two cores");
        let top = bars[0].top();
        for bar in &bars {
            assert!(
                (bar.top() - top).abs() < 0.5,
                "the bars start at {:?}, expected all at {top}",
                bars.iter().map(|bar| bar.top()).collect::<Vec<_>>()
            );
            assert_eq!(bar.height(), CORE_BAR_HEIGHT);
        }
        // Tight gaps: the reference uses 4 px between 14 px columns.
        let gap = bars[1].left() - bars[0].right();
        assert!((0.0..=CORE_BAR_GAP + 0.5).contains(&gap), "gap is {gap}");
    }

    fn process(pid: u32, user: &str, cpu: f32, mem: f32, command: &str) -> Process {
        Process {
            pid,
            user: user.to_owned(),
            state: "S".to_owned(),
            nice: 0,
            threads: 1,
            cpu,
            mem,
            command: command.to_owned(),
        }
    }

    #[test]
    fn the_process_table_sorts_by_the_clicked_column() {
        let processes = [
            process(3, "root", 1.0, 0.4, "cc1"),
            process(1, "brainchild", 9.0, 2.5, "kitty"),
            process(2, "www", 5.0, 0.1, "nginx"),
        ];
        let order = |column, descending| {
            sort_processes(&processes, column, descending)
                .iter()
                .map(|index| processes[*index].pid)
                .collect::<Vec<_>>()
        };
        // Pressure columns read highest-first on the first click.
        assert!(default_descending(ProcessColumn::Cpu));
        assert_eq!(order(ProcessColumn::Cpu, true), [1, 2, 3]);
        assert_eq!(order(ProcessColumn::Cpu, false), [3, 2, 1]);
        // Text columns start ascending, and pid is numeric but ascending.
        assert!(!default_descending(ProcessColumn::User));
        // brainchild, root, www
        assert_eq!(order(ProcessColumn::User, false), [1, 3, 2]);
        assert_eq!(order(ProcessColumn::Pid, false), [1, 2, 3]);
        assert_eq!(order(ProcessColumn::Command, false), [3, 1, 2]);
    }

    #[test]
    fn clicking_a_heading_sorts_and_a_second_click_flips() {
        let mut state = PaneState::default();
        assert_eq!(state.sort, ProcessColumn::Cpu);
        assert!(state.descending, "CPU starts highest-first");

        state.sort_by(ProcessColumn::Cpu);
        assert!(!state.descending, "same column flips the direction");

        state.sort_by(ProcessColumn::User);
        assert_eq!(state.sort, ProcessColumn::User);
        assert!(!state.descending, "a new text column starts ascending");

        state.sort_by(ProcessColumn::Nice);
        assert!(state.descending, "a new pressure column starts descending");
    }

    #[test]
    fn the_sort_order_is_remembered() {
        let mut settings = serde_json::Map::new();
        settings.insert(PROCESS_SORT_KEY.to_owned(), serde_json::Value::from("mem"));
        settings.insert(
            PROCESS_SORT_DESC_KEY.to_owned(),
            serde_json::Value::Bool(false),
        );
        let state = PaneState::load(&settings);
        assert_eq!(state.sort, ProcessColumn::Mem);
        assert!(!state.descending);

        // A value this build does not know keeps the default.
        settings.insert(PROCESS_SORT_KEY.to_owned(), serde_json::Value::from("nope"));
        assert_eq!(PaneState::load(&settings).sort, ProcessColumn::Cpu);
    }

    /// One probe answer, shaped like the shell command's output.
    const DETAILS_PROBE: &str = "STATUS\u{1f}Name:\tbash\n\
State:\tS (sleeping)\n\
Threads:\t2\n\
Uid:\t1000\t1000\t1000\t1000\n\
Gid:\t1000\t1000\t1000\t1000\n\
VmSize:\t  20480 kB\n\
VmRSS:\t   4096 kB\n\
VmPeak:\t  30000 kB\n\
voluntary_ctxt_switches:\t12\n\
nonvoluntary_ctxt_switches:\t3\n\
CMDLINE\u{1f}/usr/bin/kitty --single-instance \n\
EXE\u{1f}/usr/bin/kitty\n\
CWD\u{1f}/home/brainchild\n\
ROOT\u{1f}/\n\
PARENT\u{1f}    1 brainchild Wed Sep 17 10:11:12 2025 02:13\n\
FDS\u{1f}42\n\
LIMITS\u{1f}Limit                     Soft Limit           Hard Limit           Units\n\
Max open files            1024                 4096                 files\n\
CGROUP\u{1f}0::/user.slice/user-1000.slice\n\
ENVIRON\u{1f}HOME=/home/brainchild\n";

    #[test]
    fn the_details_probe_is_read_into_labelled_rows() {
        let rows = parse_details(DETAILS_PROBE);
        let find = |label: &str| {
            rows.iter()
                .find(|(name, _)| name == label)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(find("Command line"), "/usr/bin/kitty --single-instance");
        assert_eq!(find("Executable"), "/usr/bin/kitty");
        assert_eq!(find("Working dir"), "/home/brainchild");
        assert_eq!(find("Parent PID"), "1");
        assert_eq!(find("Owner"), "brainchild");
        assert_eq!(find("Started"), "Wed Sep 17 10:11:12 2025");
        assert_eq!(find("Elapsed"), "02:13");
        assert_eq!(find("State"), "S (sleeping)");
        assert_eq!(find("Resident memory"), "4096 kB");
        assert_eq!(find("Open file descriptors"), "42");
        assert_eq!(find("Cgroup"), "0::/user.slice/user-1000.slice");
        assert_eq!(find("Max open files"), "1024 files");
        // Nothing was invented for the probes the host did not answer.
        assert!(parse_details("").is_empty());
    }

    /// A details answer from a Windows host, in the same row format.
    const WINDOWS_DETAILS_PROBE: &str = "STATUS\u{1f}State: Running\r\n\
Threads: 12\r\n\
Uid: andreashagelberg\r\n\
VmRSS: 12345678 B\r\n\
VmSize: 87654321 B\r\n\
CMDLINE\u{1f}\"C:\\Windows\\System32\\svchost.exe\" -k netsvcs\r\n\
EXE\u{1f}C:\\Windows\\System32\\svchost.exe\r\n\
FDS\u{1f}420\r\n\
PARENT\u{1f}600 andreashagelberg 2026-10-28 13:02:03 00.01:02:05\r\n";

    #[test]
    fn a_windows_host_answers_the_details_rows() {
        let rows = parse_details(WINDOWS_DETAILS_PROBE);
        let find = |label: &str| {
            rows.iter()
                .find(|(name, _)| name == label)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            find("Command line"),
            "\"C:\\Windows\\System32\\svchost.exe\" -k netsvcs"
        );
        assert_eq!(find("Executable"), "C:\\Windows\\System32\\svchost.exe");
        assert_eq!(find("Parent PID"), "600");
        assert_eq!(find("Owner"), "andreashagelberg");
        assert_eq!(find("Started"), "2026-10-28 13:02:03");
        assert_eq!(find("Elapsed"), "00.01:02:05");
        assert_eq!(find("State"), "Running");
        assert_eq!(find("Threads"), "12");
        assert_eq!(find("Resident memory"), "12345678 B");
        assert_eq!(find("Open file descriptors"), "420");
        // Windows has no working directory, root, cgroup or limits groups.
        assert!(!rows.iter().any(|(label, _)| label == "Working dir"));
    }

    #[test]
    fn a_windows_host_is_probed_with_powershell() {
        let command = windows_details_command(420);
        assert!(command.contains("ProcessId -eq 420"), "{command}");
        assert!(!command.contains("/proc"), "{command}");
        assert_eq!(
            windows_signal_command(420, "KILL").as_deref(),
            Some("Stop-Process -Id 420 -Force")
        );
        assert_eq!(
            windows_signal_command(420, "TERM").as_deref(),
            Some("Stop-Process -Id 420")
        );
        // Windows cannot deliver the rest of the POSIX signals.
        assert_eq!(windows_signal_command(420, "USR1"), None);
    }

    #[test]
    fn a_windows_host_stops_a_process_instead_of_signalling_it() {
        let mut access = FakeMonitor::default();
        let mut state = PaneState {
            windows: true,
            ..PaneState::default()
        };

        open_details(&mut access, &mut state, 420, "svchost".to_owned());
        assert_eq!(access.commands[0].0, DETAILS_COMMAND_KEY);
        assert!(access.commands[0].1.contains("ProcessId -eq 420"));

        send_signal(&mut access, &mut state, 420, "KILL");
        assert_eq!(
            access.commands[1],
            (
                SIGNAL_COMMAND_KEY.to_owned(),
                "Stop-Process -Id 420 -Force".to_owned()
            )
        );
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|(line, failed)| !*failed && line.contains("420"))
        );

        send_signal(&mut access, &mut state, 420, "USR1");
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|(line, failed)| *failed && line.contains("USR1"))
        );
    }

    #[test]
    fn the_details_probe_names_the_pid_it_reads() {
        let command = details_command(420);
        for probe in [
            "/proc/420/status",
            "/proc/420/cmdline",
            "/proc/420/exe",
            "/proc/420/limits",
        ] {
            assert!(command.contains(probe), "{probe} missing from {command}");
        }
    }

    #[test]
    fn signals_are_validated_before_they_reach_the_shell() {
        assert!(valid_signal("TERM"));
        assert!(valid_signal("sigkill"));
        assert!(!valid_signal("TERM; rm -rf /"));
        assert!(!valid_signal("9"));
        assert_eq!(signal_command(42, "sigterm"), Some("kill -s TERM 42".to_owned()));
        // A pid of zero would signal the whole process group.
        assert_eq!(signal_command(0, "TERM"), None);
        assert_eq!(signal_command(42, "nope"), None);
    }

    #[test]
    fn opening_the_details_dialog_asks_the_host_and_shows_the_answer() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            ..FakeMonitor::default()
        };
        let mut state = PaneState::default();
        open_details(&mut access, &mut state, 420, "/usr/bin/kitty".to_owned());

        assert_eq!(access.commands.len(), 1);
        assert_eq!(access.commands[0].0, DETAILS_COMMAND_KEY);
        assert!(access.commands[0].1.contains("/proc/420/status"));
        assert_eq!(state.details.as_ref().map(|details| details.pid), Some(420));

        access.answers.insert(
            DETAILS_COMMAND_KEY.to_owned(),
            crate::command::CommandOutput {
                stdout: DETAILS_PROBE.to_owned(),
                ..Default::default()
            },
        );
        poll_details(&mut access, &mut state);
        let details = state.details.as_ref().expect("still open");
        assert!(!details.loading);
        assert!(details.error.is_none());
        let fields = details.fields.as_ref().expect("parsed");
        assert!(
            fields
                .iter()
                .any(|(label, value)| label == "Executable" && value == "/usr/bin/kitty")
        );
    }

    #[test]
    fn a_failed_probe_says_why_instead_of_staying_blank() {
        let mut access = FakeMonitor::default();
        let mut state = PaneState::default();
        open_details(&mut access, &mut state, 7, "gone".to_owned());
        access.answers.insert(
            DETAILS_COMMAND_KEY.to_owned(),
            crate::command::CommandOutput {
                stderr: "cat: /proc/7/status: No such file or directory".to_owned(),
                code: Some(1),
                ..Default::default()
            },
        );
        poll_details(&mut access, &mut state);
        let details = state.details.as_ref().expect("still open");
        assert!(details.fields.is_none());
        assert!(
            details
                .error
                .as_deref()
                .is_some_and(|error| error.contains("No such file")),
            "{:?}",
            details.error
        );
    }

    #[test]
    fn sending_a_signal_runs_kill_and_reports_the_result() {
        let mut access = FakeMonitor::default();
        let mut state = PaneState {
            details: Some(Details {
                pid: 420,
                command: "kitty".to_owned(),
                fields: Some(Vec::new()),
                error: None,
                signal: SIGNAL_DEFAULT,
                loading: false,
            }),
            ..PaneState::default()
        };

        send_signal(&mut access, &mut state, 420, "KILL");
        assert_eq!(
            access.commands[0],
            (SIGNAL_COMMAND_KEY.to_owned(), "kill -s KILL 420".to_owned())
        );
        assert!(state.status.as_ref().is_some_and(|(line, _)| line.contains("420")));

        access.answers.insert(
            SIGNAL_COMMAND_KEY.to_owned(),
            crate::command::CommandOutput {
                code: Some(0),
                ..Default::default()
            },
        );
        poll_details(&mut access, &mut state);
        let (message, failed) = state.status.clone().expect("a result line");
        assert!(!failed, "{message}");

        // A refused signal is reported as an error, with the host's wording.
        let mut access = FakeMonitor::default();
        let mut state = PaneState {
            details: Some(Details {
                pid: 420,
                command: "kitty".to_owned(),
                fields: Some(Vec::new()),
                error: None,
                signal: SIGNAL_DEFAULT,
                loading: true,
            }),
            ..PaneState::default()
        };
        access.answers.insert(
            SIGNAL_COMMAND_KEY.to_owned(),
            crate::command::CommandOutput {
                stderr: "kill: (420) - Operation not permitted".to_owned(),
                code: Some(1),
                ..Default::default()
            },
        );
        poll_details(&mut access, &mut state);
        let (message, failed) = state.status.clone().expect("a result line");
        assert!(failed);
        assert!(message.contains("not permitted"), "{message}");
    }

    /// Painted text, as `(top, text)`.
    fn texts(shapes: Vec<egui::Shape>) -> Vec<(f32, String)> {
        shapes
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => {
                    Some((text.pos.y, text.galley.text().to_owned()))
                }
                _ => None,
            })
            .collect()
    }

    /// Top of the first painted text starting with `prefix`.
    fn y_of(texts: &[(f32, String)], prefix: &str) -> f32 {
        texts
            .iter()
            .find(|(_, text)| text.starts_with(prefix))
            .map(|(y, _)| *y)
            .unwrap_or_else(|| panic!("{prefix:?} was not painted"))
    }

    #[test]
    fn the_pid_and_user_columns_fit_their_longest_value() {
        let long_user = "systemd-network";
        let mut snapshot = sample();
        snapshot.processes = vec![
            process(4_194_304, long_user, 1.0, 0.1, "/usr/bin/a"),
            process(1, "root", 0.5, 0.1, "/usr/bin/b"),
        ];
        let mut access = FakeMonitor {
            sample: Some(snapshot),
            ..FakeMonitor::default()
        };
        let painted = texts(shapes_at(&mut access, 430.0)).into_iter().map(|(_, text)| text);

        // Both are shown in full: the columns are measured from the data, so a
        // seven-digit pid or a long user name is never ellipsized.
        let painted: Vec<String> = painted.collect();
        assert!(painted.iter().any(|text| text == "4194304"), "{painted:?}");
        assert!(painted.iter().any(|text| text == long_user), "{painted:?}");
        assert!(
            !painted.iter().any(|text| text.starts_with("4194") && text.ends_with('…')),
            "the pid was clipped"
        );
    }

    #[test]
    fn the_process_table_paints_headings_and_reorders_the_rows() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            ..FakeMonitor::default()
        };

        // Default: CPU, highest first, with one direction arrow on the active
        // heading.
        let painted = texts(shapes_at(&mut access, 430.0));
        for heading in ["PID", "User", "S", "NI", "THR", "CPU", "Mem", "Command"] {
            assert!(
                painted.iter().any(|(_, text)| text == heading),
                "heading {heading:?} missing"
            );
        }
        assert_eq!(
            painted.iter().filter(|(_, text)| text == "▼").count(),
            1,
            "the active heading shows one arrow"
        );
        let kitty = y_of(&painted, "/usr/bin/kitty");
        let init = y_of(&painted, "/sbin/init");
        assert!(kitty < init, "2.4% CPU should be listed above 0.5%");

        // Sorted by command, ascending, the rows swap over.
        let state = PaneState {
            sort: ProcessColumn::Command,
            descending: false,
            ..PaneState::default()
        };
        let painted = texts(shapes_from(&mut access, 430.0, state));
        let kitty = y_of(&painted, "/usr/bin/kitty");
        let init = y_of(&painted, "/sbin/init");
        assert!(init < kitty, "/sbin/init sorts before /usr/bin/kitty");
        assert_eq!(painted.iter().filter(|(_, text)| text == "▲").count(), 1);
    }

    #[test]
    fn the_time_range_is_a_slider_over_the_reference_steps() {
        let mut access = FakeMonitor {
            sample: Some(sample()),
            requested: 0,
            sampling: false,
            error: None,
            ..FakeMonitor::default()
        };
        let painted = draw_pane(&mut access);
        assert!(painted.iter().any(|text| text == "TIME RANGE"));
        // The slider shows one window at a time; the rest are steps.
        assert!(painted.iter().any(|text| text == "10m"), "{painted:?}");
        for (label, _) in TIME_RANGES {
            if label != "10m" {
                assert!(
                    !painted.iter().any(|text| text == label),
                    "range {label:?} should only appear when selected"
                );
            }
        }
    }

    #[test]
    fn the_range_steps_are_the_reference_windows() {
        // Every step maps to a window the ladder answers in full, and the
        // slider position is the index into this list.
        for (index, (label, seconds)) in TIME_RANGES.iter().enumerate() {
            assert_eq!(TIME_RANGES[index].0, *label);
            let points = seconds / bucket_seconds(*seconds);
            assert!(points >= 4, "{label} would draw {points} points");
        }
        assert_eq!(TIME_RANGES[TIME_RANGES.len() - 1].1, 7 * 24 * 3_600);
    }

    #[test]
    fn sizes_and_uptimes_read_naturally() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(4 * 1024 * 1024 * 1024), "4.0 GB");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m 05s");
        assert_eq!(format_uptime(7_320), "2h 02m");
        assert_eq!(format_uptime(200_000), "2d 7h");
    }
}
