//! The slice of WinFsp's user-mode API this crate binds to.
//!
//! Every structure and constant here mirrors `winfsp/winfsp.h` and `winfsp/fsctl.h` of
//! WinFsp 2.1, which is the version the WinFsp project promises to keep compatible. Field
//! order and padding are load-bearing: WinFsp reads these blocks by offset, so nothing in
//! this file may be reordered or reshaped without checking the header line by line.

use std::ffi::c_void;

/// An NT status code, as every WinFsp callback returns.
pub type NtStatus = i32;

pub const STATUS_SUCCESS: NtStatus = 0;
pub const STATUS_UNSUCCESSFUL: NtStatus = 0xC000_0001u32 as NtStatus;
pub const STATUS_ACCESS_DENIED: NtStatus = 0xC000_0022u32 as NtStatus;
pub const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xC000_0034u32 as NtStatus;
pub const STATUS_OBJECT_NAME_COLLISION: NtStatus = 0xC000_0035u32 as NtStatus;
pub const STATUS_NOT_SUPPORTED: NtStatus = 0xC000_00BBu32 as NtStatus;
pub const STATUS_FILE_IS_A_DIRECTORY: NtStatus = 0xC000_00BAu32 as NtStatus;
pub const STATUS_UNEXPECTED_IO_ERROR: NtStatus = 0xC000_00E9u32 as NtStatus;
pub const STATUS_NOT_A_DIRECTORY: NtStatus = 0xC000_0103u32 as NtStatus;
pub const STATUS_DIRECTORY_NOT_EMPTY: NtStatus = 0xC000_0101u32 as NtStatus;
pub const STATUS_IO_DEVICE_ERROR: NtStatus = 0xC000_0185u32 as NtStatus;
pub const STATUS_CONNECTION_DISCONNECTED: NtStatus = 0xC000_020Cu32 as NtStatus;

pub const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

/// `CreateOptions` bit asking for a directory.
pub const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;

/// `GrantedAccess` bits this filesystem cares about.
pub const FILE_READ_DATA: u32 = 0x0000_0001;
pub const FILE_WRITE_DATA: u32 = 0x0000_0002;
pub const FILE_APPEND_DATA: u32 = 0x0000_0004;

/// `Cleanup` flag saying the handle carried delete-on-close.
pub const FSP_CLEANUP_DELETE: u32 = 0x0000_0001;

/// Seconds between 1601-01-01 and 1970-01-01, in `FILETIME` ticks.
const EPOCH_DIFFERENCE: u64 = 11_644_473_600;
/// `FILETIME` counts 100-nanosecond ticks.
const TICKS_PER_SECOND: u64 = 10_000_000;

/// Largest volume this filesystem reports: 1 PiB, so Explorer's capacity bar is quiet.
pub const VOLUME_SIZE: u64 = 1u64 << 50;

/// Label capacity of [`VolumeInfo`], in UTF-16 units.
pub const VOLUME_LABEL_LEN: usize = 32;

/// Volume flags: case is preserved and Unicode names are kept as sent.
pub const CASE_PRESERVED_NAMES: u32 = 1 << 1;
pub const UNICODE_ON_DISK: u32 = 1 << 2;

/// The name of the disk control device; directory mount points need a disk filesystem.
pub const DISK_DEVICE: &str = "WinFsp.Disk";

/// `FSP_FSCTL_VOLUME_PARAMS` (2.1), whose size WinFsp checks as 504 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VolumeParams {
    pub version: u16,
    pub sector_size: u16,
    pub sectors_per_allocation_unit: u16,
    pub max_component_length: u16,
    pub volume_creation_time: u64,
    pub volume_serial_number: u32,
    pub transact_timeout: u32,
    pub irp_timeout: u32,
    pub irp_capacity: u32,
    pub file_info_timeout: u32,
    pub flags: u32,
    pub prefix: [u16; 192],
    pub file_system_name: [u16; 16],
    pub v1_flags: u32,
    pub volume_info_timeout: u32,
    pub dir_info_timeout: u32,
    pub security_timeout: u32,
    pub stream_info_timeout: u32,
    pub ea_timeout: u32,
    pub fsext_control_code: u32,
    pub reserved32: [u32; 1],
    pub reserved64: [u64; 2],
}

impl VolumeParams {
    /// Parameters for a case-preserving Unicode volume named `name`.
    pub fn new(name: &str) -> Self {
        let mut params = Self {
            // The size of the structure this build knows, which is what WinFsp expects.
            version: std::mem::size_of::<Self>() as u16,
            sector_size: 512,
            sectors_per_allocation_unit: 1,
            max_component_length: 255,
            volume_creation_time: 0,
            volume_serial_number: 0x5753_5348,
            transact_timeout: 0,
            irp_timeout: 0,
            irp_capacity: 0,
            file_info_timeout: 1000,
            flags: CASE_PRESERVED_NAMES | UNICODE_ON_DISK,
            prefix: [0; 192],
            file_system_name: [0; 16],
            v1_flags: 0,
            volume_info_timeout: 0,
            dir_info_timeout: 0,
            security_timeout: 0,
            stream_info_timeout: 0,
            ea_timeout: 0,
            fsext_control_code: 0,
            reserved32: [0; 1],
            reserved64: [0; 2],
        };
        write_wide(&mut params.file_system_name, name);
        params
    }
}

const _: () = assert!(std::mem::size_of::<VolumeParams>() == 504);

/// `FSP_FSCTL_VOLUME_INFO`, 88 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VolumeInfo {
    pub total_size: u64,
    pub free_size: u64,
    pub volume_label_length: u16,
    pub volume_label: [u16; VOLUME_LABEL_LEN],
}

/// `FSP_FSCTL_FILE_INFO`, 72 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FileInfo {
    pub file_attributes: u32,
    pub reparse_tag: u32,
    pub allocation_size: u64,
    pub file_size: u64,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub index_number: u64,
    pub hard_links: u32,
    pub ea_size: u32,
}

/// The fixed part of `FSP_FSCTL_DIR_INFO`, 104 bytes; the name follows it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirInfoHeader {
    pub size: u16,
    pub padding: [u8; 6],
    pub file_info: FileInfo,
    pub next_offset: u64,
    pub reserved: [u8; 16],
}

/// Room a directory entry needs for `name`, padded the way WinFsp aligns entries.
pub fn dir_info_len(name: &str) -> usize {
    let mut size = std::mem::size_of::<DirInfoHeader>() + name.encode_utf16().count() * 2;
    if !size.is_multiple_of(8) {
        size += 8 - size % 8;
    }
    size
}

/// Fill one directory entry into `out`, which must be [`dir_info_len`] bytes long.
pub fn write_dir_info(out: &mut [u8], info: &FileInfo, next_offset: u64, name: &str) {
    let header = out.as_mut_ptr().cast::<DirInfoHeader>();
    // SAFETY: `out` is at least `size_of::<DirInfoHeader>()` bytes, which `dir_info_len`
    // guarantees, and every field written here is plain data.
    unsafe {
        header.write(DirInfoHeader {
            size: out.len() as u16,
            padding: [0; 6],
            file_info: *info,
            next_offset,
            reserved: [0; 16],
        });
    }
    let name_at = std::mem::size_of::<DirInfoHeader>();
    for (index, unit) in name.encode_utf16().enumerate() {
        let at = name_at + index * 2;
        out[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
}

/// The `FILETIME` for a Unix second count.
pub fn filetime(seconds: u64) -> u64 {
    seconds
        .saturating_add(EPOCH_DIFFERENCE)
        .saturating_mul(TICKS_PER_SECOND)
}

/// The Unix second count for a `FILETIME`.
pub fn unix_seconds(filetime: u64) -> u64 {
    (filetime / TICKS_PER_SECOND).saturating_sub(EPOCH_DIFFERENCE)
}

/// `text` as a nul-terminated wide string.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Copy `text` into a fixed wide field, leaving room for the nul.
fn write_wide(field: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(field.len() - 1).collect();
    for (slot, unit) in field.iter_mut().zip(units) {
        *slot = unit;
    }
}

/// Read a nul-terminated wide string that the caller owns.
///
/// # Safety
/// `text` must be null or point to a nul-terminated UTF-16 string the caller keeps alive
/// for the duration of the call.
pub unsafe fn from_wide(text: *const u16) -> String {
    if text.is_null() {
        return String::new();
    }
    let mut units = Vec::new();
    let mut index = 0;
    // SAFETY: the caller promises a nul-terminated string; the loop stops at the nul and
    // never reads past it.
    unsafe {
        loop {
            let unit = *text.add(index);
            if unit == 0 {
                break;
            }
            units.push(unit);
            index += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

/// A raw handle WinFsp owns; this crate never dereferences it.
pub type FileSystem = c_void;

/// An opaque per-open context, which is always one of ours.
pub type FileContext = c_void;

/// One callback WinFsp may leave null when the filesystem does not offer it.
pub type Unused = Option<unsafe extern "C" fn()>;

/// `FSP_FILE_SYSTEM_INTERFACE`: the callbacks WinFsp dispatches to.
///
/// The field order is the header's. The ones this filesystem leaves null are typed as
/// [`Unused`]; WinFsp never calls a null entry.
#[repr(C)]
pub struct FileSystemInterface {
    pub get_volume_info:
        Option<unsafe extern "C" fn(*mut FileSystem, *mut VolumeInfo) -> NtStatus>,
    pub set_volume_label: Unused,
    pub get_security_by_name: Option<
        unsafe extern "C" fn(*mut FileSystem, *const u16, *mut u32, *mut c_void, *mut usize) -> NtStatus,
    >,
    pub create: Option<
        unsafe extern "C" fn(
            *mut FileSystem,
            *const u16,
            u32,
            u32,
            u32,
            *mut c_void,
            u64,
            *mut *mut c_void,
            *mut FileInfo,
        ) -> NtStatus,
    >,
    pub open: Option<
        unsafe extern "C" fn(
            *mut FileSystem,
            *const u16,
            u32,
            u32,
            *mut *mut c_void,
            *mut FileInfo,
        ) -> NtStatus,
    >,
    pub overwrite: Option<
        unsafe extern "C" fn(*mut FileSystem, *mut FileContext, u32, u8, u64, *mut FileInfo) -> NtStatus,
    >,
    pub cleanup:
        Option<unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *const u16, u32)>,
    pub close: Option<unsafe extern "C" fn(*mut FileSystem, *mut FileContext)>,
    pub read: Option<
        unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *mut c_void, u64, u32, *mut u32) -> NtStatus,
    >,
    pub write: Option<
        unsafe extern "C" fn(
            *mut FileSystem,
            *mut FileContext,
            *const c_void,
            u64,
            u32,
            u8,
            u8,
            *mut u32,
            *mut FileInfo,
        ) -> NtStatus,
    >,
    pub flush:
        Option<unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *mut FileInfo) -> NtStatus>,
    pub get_file_info:
        Option<unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *mut FileInfo) -> NtStatus>,
    pub set_basic_info: Option<
        unsafe extern "C" fn(
            *mut FileSystem,
            *mut FileContext,
            u32,
            u64,
            u64,
            u64,
            u64,
            *mut FileInfo,
        ) -> NtStatus,
    >,
    pub set_file_size: Option<
        unsafe extern "C" fn(*mut FileSystem, *mut FileContext, u64, u8, *mut FileInfo) -> NtStatus,
    >,
    pub can_delete:
        Option<unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *const u16) -> NtStatus>,
    pub rename: Option<
        unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *const u16, *const u16, u8) -> NtStatus,
    >,
    pub get_security: Unused,
    pub set_security: Unused,
    pub read_directory: Option<
        unsafe extern "C" fn(
            *mut FileSystem,
            *mut FileContext,
            *const u16,
            *const u16,
            *mut c_void,
            u32,
            *mut u32,
        ) -> NtStatus,
    >,
    pub resolve_reparse_points: Unused,
    pub get_reparse_point: Unused,
    pub set_reparse_point: Unused,
    pub delete_reparse_point: Unused,
    pub get_stream_info: Unused,
    pub get_dir_info_by_name: Unused,
    pub control: Unused,
    pub set_delete: Option<
        unsafe extern "C" fn(*mut FileSystem, *mut FileContext, *const u16, u8) -> NtStatus,
    >,
    pub create_ex: Unused,
    pub overwrite_ex: Unused,
    pub get_ea: Unused,
    pub set_ea: Unused,
    pub obsolete0: Unused,
    pub dispatcher_stopped: Unused,
    pub reserved: [Unused; 31],
}

impl FileSystemInterface {
    /// Every callback null, ready for the ones this filesystem offers.
    pub const fn empty() -> Self {
        Self {
            get_volume_info: None,
            set_volume_label: None,
            get_security_by_name: None,
            create: None,
            open: None,
            overwrite: None,
            cleanup: None,
            close: None,
            read: None,
            write: None,
            flush: None,
            get_file_info: None,
            set_basic_info: None,
            set_file_size: None,
            can_delete: None,
            rename: None,
            get_security: None,
            set_security: None,
            read_directory: None,
            resolve_reparse_points: None,
            get_reparse_point: None,
            set_reparse_point: None,
            delete_reparse_point: None,
            get_stream_info: None,
            get_dir_info_by_name: None,
            control: None,
            set_delete: None,
            create_ex: None,
            overwrite_ex: None,
            get_ea: None,
            set_ea: None,
            obsolete0: None,
            dispatcher_stopped: None,
            reserved: [None; 31],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structures_match_the_sizes_winfsp_asserts() {
        assert_eq!(std::mem::size_of::<VolumeInfo>(), 88);
        assert_eq!(std::mem::size_of::<FileInfo>(), 72);
        assert_eq!(std::mem::size_of::<DirInfoHeader>(), 104);
        assert_eq!(std::mem::offset_of!(VolumeParams, prefix), 40);
        assert_eq!(std::mem::offset_of!(VolumeParams, file_system_name), 424);
        assert_eq!(std::mem::offset_of!(VolumeParams, v1_flags), 456);
    }

    #[test]
    fn times_round_trip_through_the_windows_epoch() {
        assert_eq!(filetime(0), 116_444_736_000_000_000);
        assert_eq!(unix_seconds(filetime(1_700_000_000)), 1_700_000_000);
    }

    #[test]
    fn a_directory_entry_is_aligned_and_carries_its_name() {
        assert_eq!(dir_info_len("a"), 112);
        let mut entry = vec![0u8; dir_info_len("hosts")];
        let info = FileInfo {
            file_size: 12,
            ..FileInfo::default()
        };
        write_dir_info(&mut entry, &info, 3, "hosts");
        let header = unsafe { &*entry.as_ptr().cast::<DirInfoHeader>() };
        assert_eq!(header.size as usize, entry.len());
        assert_eq!(header.next_offset, 3);
        assert_eq!(header.file_info.file_size, 12);
        let name: Vec<u16> = (104..114)
            .step_by(2)
            .map(|at| u16::from_le_bytes([entry[at], entry[at + 1]]))
            .collect();
        assert_eq!(String::from_utf16(&name).unwrap(), "hosts");
    }

    #[test]
    fn wide_strings_round_trip_by_hand() {
        let text = wide("Log");
        assert_eq!(text, vec![b'L' as u16, b'o' as u16, b'g' as u16, 0]);
        let read = unsafe { from_wide(text.as_ptr()) };
        assert_eq!(read, "Log");
        assert_eq!(unsafe { from_wide(std::ptr::null()) }, "");
    }
}
