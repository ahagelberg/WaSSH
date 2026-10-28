//! The Win32 implementation: process, CPU, memory, disk and network facts read through
//! the kernel APIs, so a sample costs a handful of syscalls instead of a shell.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, SYSTEMTIME};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetAdaptersAddresses, GetIfTable2, GAA_FLAG_SKIP_ANYCAST,
    GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK,
    IP_ADAPTER_ADDRESSES_LH, MIB_IF_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows_sys::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{
    GetNativeSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, OSVERSIONINFOW,
    PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM64, SYSTEM_INFO,
};
use windows_sys::Win32::System::Threading::{
    GetProcessHandleCount, GetProcessIoCounters, GetProcessTimes, GetSystemTimes, OpenProcess,
    OpenProcessToken, QueryFullProcessImageNameW, TerminateProcess, IO_COUNTERS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
};
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime,
};
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessorPerformanceInformation,
};
use windows_sys::Wdk::System::SystemServices::RtlGetVersion;

use crate::{Cpu, CpuTimes, Disk, Interface, Memory, Process, ProcessDetails, System};

/// Ticks per second of every `FILETIME` and process-time counter.
const TICKS_PER_SECOND: f64 = 10_000_000.0;
/// 100 ns intervals between 1601-01-01 and 1970-01-01.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
/// Longest process image path `QueryFullProcessImageNameW` may return.
const MAX_PATH_CHARS: usize = 1024;
/// First build of Windows 11.
const WINDOWS_11_BUILD: u32 = 22_000;
/// First build of Windows 10.
const WINDOWS_10_BUILD: u32 = 10_240;
/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` bit: the interface is backed by hardware.
const INTERFACE_HARDWARE: u8 = 0x01;
/// The same flags' bit for the NDIS filter interfaces that sit on top of a NIC.
const INTERFACE_FILTER: u8 = 0x02;

/// Counter value of a `FILETIME`.
fn ticks(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// A handle to `pid`, preferring the rights the memory counters need.
fn open_for_details(pid: u32) -> Option<(HANDLE, bool)> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if !handle.is_null() {
        return Some((handle, true));
    }
    // A protected process refuses the memory read; the rest of the rows still work.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    (!handle.is_null()).then_some((handle, false))
}

pub fn cpu() -> Option<Cpu> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // Kernel time includes the idle time.
    let total = CpuTimes {
        total: ticks(kernel) + ticks(user),
        idle: ticks(idle),
    };
    Some(Cpu {
        total,
        cores: cores(),
    })
}

/// Per-core counters, from the one kernel call that reports all of them.
fn cores() -> Vec<CpuTimes> {
    let mut info = SYSTEM_INFO::default();
    unsafe { GetNativeSystemInfo(&mut info) };
    let count = info.dwNumberOfProcessors.max(1) as usize;
    let mut buffer = vec![SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default(); count];
    let mut returned = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SystemProcessorPerformanceInformation,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()) as u32,
            &mut returned,
        )
    };
    if status < 0 {
        return Vec::new();
    }
    let filled = (returned as usize / size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()).min(count);
    buffer.truncate(filled);
    buffer
        .iter()
        .map(|core| CpuTimes {
            total: (core.KernelTime + core.UserTime) as u64,
            idle: core.IdleTime as u64,
        })
        .collect()
}

pub fn memory() -> Option<Memory> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    Some(Memory {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,
        swap_total: status.ullTotalPageFile.saturating_sub(status.ullTotalPhys),
        swap_available: status
            .ullAvailPageFile
            .saturating_sub(status.ullAvailPhys),
    })
}

pub fn disk() -> Option<Disk> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    let path = wide(&format!("{drive}\\"));
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return None;
    }
    Some(Disk { total, free })
}

pub fn network() -> Vec<Interface> {
    let mut table: *mut MIB_IF_TABLE2 = null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return Vec::new();
    }
    let mut interfaces = Vec::new();
    unsafe {
        let rows = (*table).Table.as_ptr();
        for index in 0..(*table).NumEntries as usize {
            let row = &*rows.add(index);
            // The table also holds virtual adapters and the NDIS filter layers stacked on
            // each NIC (WFP, QoS, packet capture), which all repeat the NIC's counters.
            // Only the hardware interface is a device, the way `/proc/net/dev` lists them.
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            if flags & INTERFACE_HARDWARE == 0 || flags & INTERFACE_FILTER != 0 {
                continue;
            }
            if row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.OperStatus != IfOperStatusUp {
                continue;
            }
            let name = from_wide(&row.Alias);
            if name.is_empty() {
                continue;
            }
            let speed = row.ReceiveLinkSpeed.max(row.TransmitLinkSpeed) / 1_000_000;
            interfaces.push(Interface {
                name,
                rx: row.InOctets,
                tx: row.OutOctets,
                speed_mbps: (speed > 0).then_some(speed),
            });
        }
        FreeMibTable(table.cast());
    }
    interfaces
}

pub fn processes() -> Vec<Process> {
    let mut processes = Vec::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return processes;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let pid = entry.th32ProcessID;
        let (cpu_s, working_set, private_bytes, read_bytes, write_bytes) = process_counters(pid);
        processes.push(Process {
            pid,
            parent: entry.th32ParentProcessID,
            name: from_wide(&entry.szExeFile),
            threads: entry.cntThreads,
            cpu_s,
            working_set,
            private_bytes,
            read_bytes,
            write_bytes,
        });
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    processes
}

/// CPU seconds, memory and IO a process used, or zeros for one that cannot be opened.
fn process_counters(pid: u32) -> (f64, u64, u64, u64, u64) {
    // Reading the memory counters needs the VM-read right, which protected processes
    // refuse; the rest of the row is still worth having.
    let mut readable = true;
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) } {
        handle if !handle.is_null() => handle,
        _ => {
            readable = false;
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }
        }
    };
    if handle.is_null() {
        return (0.0, 0, 0, 0, 0);
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let cpu_s = if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } != 0 {
        (ticks(kernel) + ticks(user)) as f64 / TICKS_PER_SECOND
    } else {
        0.0
    };
    let mut io = IO_COUNTERS::default();
    let (read_bytes, write_bytes) = if unsafe { GetProcessIoCounters(handle, &mut io) } != 0 {
        (io.ReadTransferCount, io.WriteTransferCount)
    } else {
        (0, 0)
    };
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let (working_set, private_bytes) =
        if readable && unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) } != 0 {
            (
                counters.WorkingSetSize as u64,
                counters.PagefileUsage as u64,
            )
        } else {
            (0, 0)
        };
    unsafe { CloseHandle(handle) };
    (cpu_s, working_set, private_bytes, read_bytes, write_bytes)
}

pub fn system() -> Option<System> {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    if unsafe { RtlGetVersion(&mut version) } < 0 {
        return None;
    }
    let build = version.dwBuildNumber;
    let os = if build >= WINDOWS_11_BUILD {
        "Windows 11".to_owned()
    } else if build >= WINDOWS_10_BUILD {
        "Windows 10".to_owned()
    } else {
        format!("Windows {}.{}", version.dwMajorVersion, version.dwMinorVersion)
    };
    Some(System {
        os,
        kernel: format!(
            "{}.{}.{}",
            version.dwMajorVersion, version.dwMinorVersion, build
        ),
        arch: architecture(),
        ip: ipv4_address(),
        uptime_s: unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() } / 1000,
    })
}

fn architecture() -> String {
    let mut info = SYSTEM_INFO::default();
    unsafe { GetNativeSystemInfo(&mut info) };
    let architecture = unsafe { info.Anonymous.Anonymous.wProcessorArchitecture };
    match architecture {
        PROCESSOR_ARCHITECTURE_AMD64 => "x86_64",
        PROCESSOR_ARCHITECTURE_ARM64 => "arm64",
        _ => "x86",
    }
    .to_owned()
}

/// The first non-loopback IPv4 address any adapter holds.
fn ipv4_address() -> Option<String> {
    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 0u32;
    unsafe { GetAdaptersAddresses(AF_INET as u32, FLAGS, null(), null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    let result = unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            FLAGS,
            null(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return None;
    }
    let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !adapter.is_null() {
        let mut unicast = unsafe { (*adapter).FirstUnicastAddress };
        while !unicast.is_null() {
            let address = unsafe { (*unicast).Address };
            if !address.lpSockaddr.is_null()
                && address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
            {
                let socket = unsafe { &*(address.lpSockaddr as *const SOCKADDR_IN) };
                let octets = unsafe { socket.sin_addr.S_un.S_addr }.to_ne_bytes();
                if octets[0] != 127 {
                    return Some(std::net::Ipv4Addr::from(octets).to_string());
                }
            }
            unicast = unsafe { (*unicast).Next };
        }
        adapter = unsafe { (*adapter).Next };
    }
    None
}

pub fn process_details(pid: u32) -> Option<ProcessDetails> {
    let process = details_entry(pid)?;
    let mut details = ProcessDetails {
        parent_pid: process.parent,
        threads: process.threads,
        ..Default::default()
    };
    let (handle, readable) = match open_for_details(pid) {
        Some(opened) => opened,
        None => return Some(details),
    };
    unsafe {
        let mut path = [0u16; MAX_PATH_CHARS];
        let mut length = path.len() as u32;
        if QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) != 0 {
            details.executable = Some(String::from_utf16_lossy(&path[..length as usize]));
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            details.started = local_time(created);
            let started = ticks(created).saturating_sub(UNIX_EPOCH_TICKS) / 10_000_000;
            details.elapsed_s = unix_now().saturating_sub(started);
        }
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        if readable && GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0 {
            details.working_set = counters.WorkingSetSize as u64;
            details.peak_working_set = counters.PeakWorkingSetSize as u64;
            details.private_bytes = counters.PagefileUsage as u64;
        }
        let mut handles = 0u32;
        if GetProcessHandleCount(handle, &mut handles) != 0 {
            details.handles = handles;
        }
    }
    details.owner = owner(handle);
    unsafe { CloseHandle(handle) };
    Some(details)
}

/// The parent and thread count from the tool help snapshot.
fn details_entry(pid: u32) -> Option<Process> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = None;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32ProcessID == pid {
            found = Some(Process {
                pid,
                parent: entry.th32ParentProcessID,
                name: from_wide(&entry.szExeFile),
                threads: entry.cntThreads,
                ..Default::default()
            });
            break;
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    found
}

/// `DOMAIN\user` of the account the process runs as.
fn owner(handle: HANDLE) -> Option<String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let mut size = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size) };
    if size == 0 {
        unsafe { CloseHandle(token) };
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    let result = unsafe {
        GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size)
    };
    unsafe { CloseHandle(token) };
    if result == 0 {
        return None;
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let mut name = [0u16; 256];
    let mut name_len = name.len() as u32;
    let mut domain = [0u16; 256];
    let mut domain_len = domain.len() as u32;
    let mut kind = 0;
    if unsafe {
        LookupAccountSidW(
            null(),
            user.User.Sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    } == 0
    {
        return None;
    }
    let account = from_wide(&name[..name_len as usize]);
    let domain = from_wide(&domain[..domain_len as usize]);
    if account.is_empty() {
        return None;
    }
    Some(if domain.is_empty() {
        account
    } else {
        format!("{domain}\\{account}")
    })
}

/// A creation time formatted in local time, ready to print.
fn local_time(time: FILETIME) -> Option<String> {
    let mut utc = SYSTEMTIME::default();
    if unsafe { FileTimeToSystemTime(&time, &mut utc) } == 0 {
        return None;
    }
    let mut local = SYSTEMTIME::default();
    if unsafe { SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) } == 0 {
        return None;
    }
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    ))
}

pub fn stop_process(pid: u32) -> Result<(), String> {
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(format!("cannot open process {pid}"));
    }
    let stopped = unsafe { TerminateProcess(handle, 1) };
    unsafe { CloseHandle(handle) };
    if stopped == 0 {
        return Err(format!("cannot stop process {pid}"));
    }
    Ok(())
}

/// Silence the unused-import warning for `c_void` on older SDKs.
const _: Option<*const c_void> = None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_reports_its_own_numbers() {
        let memory = memory().expect("memory");
        assert!(memory.total > 0);
        assert!(disk().is_some(), "the system drive is readable");
        let cpu = cpu().expect("cpu");
        assert!(cpu.total.total > 0);
        assert!(!cpu.cores.is_empty(), "one counter per core");
        let system = system().expect("system");
        assert!(system.os.starts_with("Windows"), "{}", system.os);
        assert!(system.kernel.contains('.'), "{}", system.kernel);
        assert!(system.uptime_s > 0);
        assert!(!processes().is_empty());
    }

    #[test]
    fn this_process_has_details() {
        let details = process_details(std::process::id()).expect("this process");
        assert!(details.executable.is_some());
        assert_eq!(details.threads, details.threads.max(1));
    }

    #[test]
    fn only_the_hardware_interfaces_are_listed() {
        for interface in network() {
            // Filter layers repeat a NIC's counters under names such as
            // `Wi-Fi-Npcap Packet Driver (NPCAP)-0000`.
            assert!(
                !interface.name.ends_with("-0000"),
                "filter interface {}",
                interface.name
            );
        }
    }
}