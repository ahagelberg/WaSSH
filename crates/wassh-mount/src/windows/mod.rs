//! Serving a [`RemoteFs`] as a Windows drive or directory, through WinFsp.
//!
//! WinFsp is loaded by name at run time (`winfsp-x64.dll`, which its installer places in
//! `System32`), so the crate builds without the WinFsp SDK and starts without WinFsp until
//! something is mounted.
//!
//! The callbacks WinFsp dispatches are thin: they look their driver up by the file system
//! pointer WinFsp hands them, convert a Windows path into a remote one, and hand the work to
//! the [`RemoteFs`]. The rules themselves - path translation, case-insensitive lookup,
//! delete-on-close, directory continuation - live in plain functions here so they can be
//! tested without a driver.

pub mod abi;

use std::collections::BTreeMap;
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{Attrs, FsError, FsResult, FileId, OpenMode, RemoteFs, child_path, remote_path};

/// Why a mount could not start, in words a user can read.
pub type MountError = String;

/// Whether this machine can mount at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverState {
    /// WinFsp is installed and answers.
    Ready {
        /// The WinFsp version, as its own numbering reports it (2.1 is `0x201`).
        version: u32,
    },
    /// Nothing can be mounted until WinFsp is installed.
    Missing {
        /// Why the driver could not be used.
        reason: String,
    },
}

impl DriverState {
    /// Whether a mount can be attempted.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// One running mount.
pub struct Mount {
    file_system: *mut c_void,
    key: usize,
    mount_point: String,
    stopped: bool,
}

// SAFETY: the mount is a WinFsp handle the process owns; it is only ever stopped from the
// thread that started it, and its callbacks synchronise through the driver registry.
unsafe impl Send for Mount {}

impl Mount {
    /// Serve `remote` at `mount_point`, which is a drive letter (`X:`) or an empty directory.
    ///
    /// The directory has to exist; WinFsp creates a junction inside it.
    pub fn start(
        remote: Arc<dyn RemoteFs>,
        label: &str,
        mount_point: &str,
    ) -> Result<Self, MountError> {
        let api = api()?;
        let params = abi::VolumeParams::new("WaSSH");
        let interface: &'static abi::FileSystemInterface =
            Box::leak(Box::new(callbacks()));
        let device = abi::wide(abi::DISK_DEVICE);
        let mut file_system: *mut c_void = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call; WinFsp copies what it needs.
        let status = unsafe {
            (api.create)(device.as_ptr(), &params, interface, &mut file_system)
        };
        if status != abi::STATUS_SUCCESS {
            return Err(format!(
                "WinFsp could not create a filesystem (status {status:#010x})"
            ));
        }
        let key = file_system as usize;
        let driver = Arc::new(Driver {
            remote,
            label: label.to_owned(),
        });
        if let Ok(mut table) = drivers().lock() {
            table.insert(key, driver);
        }
        let point = abi::wide(mount_point);
        // The dispatcher must be up before the mount point exists: the moment a drive letter
        // appears, filters and the shell may open it, and a request arriving before the
        // dispatcher runs leaves the volume unusable.
        // SAFETY: `file_system` is the live handle from above; zero asks WinFsp for its own
        // dispatcher thread count.
        let status = unsafe { (api.start_dispatcher)(file_system, 0) };
        if status != abi::STATUS_SUCCESS {
            retire(file_system, key);
            return Err(format!(
                "WinFsp could not start the filesystem (status {status:#010x})"
            ));
        }
        // SAFETY: as above, and `point` outlives the call.
        let status = unsafe { (api.set_mount_point)(file_system, point.as_ptr()) };
        if status != abi::STATUS_SUCCESS {
            retire(file_system, key);
            return Err(format!(
                "WinFsp could not mount at {mount_point} (status {status:#010x}); is it in use?"
            ));
        }
        Ok(Self {
            file_system,
            key,
            mount_point: mount_point.to_owned(),
            stopped: false,
        })
    }

    /// Where the mount is.
    pub fn mount_point(&self) -> &str {
        &self.mount_point
    }

    /// Take the mount down, waiting for WinFsp to finish with it.
    pub fn unmount(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Ok(api) = api() {
            // SAFETY: `file_system` is live until `retire`, and the order is the one WinFsp
            // documents: stop new requests, drain the dispatcher, then delete the object.
            unsafe {
                (api.remove_mount_point)(self.file_system);
                (api.stop_dispatcher)(self.file_system);
            }
        }
        retire(self.file_system, self.key);
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Forget the file system object and the driver behind it.
fn retire(file_system: *mut c_void, key: usize) {
    if let Ok(api) = api() {
        // SAFETY: the caller owns the handle and has stopped the dispatcher.
        unsafe { (api.delete)(file_system) };
    }
    if let Ok(mut table) = drivers().lock() {
        table.remove(&key);
    }
}

/// What this machine can do, right now.
pub fn driver_state() -> DriverState {
    match api() {
        Ok(api) => {
            let mut version = 0u32;
            // SAFETY: `version` is a valid out-parameter.
            let status = unsafe { (api.version)(&mut version) };
            if status == abi::STATUS_SUCCESS {
                DriverState::Ready { version }
            } else {
                DriverState::Missing {
                    reason: format!("WinFsp answered with status {status:#010x}"),
                }
            }
        }
        Err(reason) => DriverState::Missing { reason },
    }
}

/// One open file or directory, as WinFsp holds it between callbacks.
struct Context {
    path: String,
    file: Option<FileId>,
    is_dir: bool,
    delete_on_close: AtomicBool,
}

/// The filesystem behind one mount.
struct Driver {
    remote: Arc<dyn RemoteFs>,
    label: String,
}

impl Driver {
    /// The remote path a Windows path names, matching case when only case differs.
    ///
    /// Windows hands out whatever case the user or a program used, and a remote host may be
    /// case-sensitive, so each part that is not found is looked up again without case.
    fn resolve(&self, windows_path: &str) -> FsResult<String> {
        let path = remote_path(windows_path).ok_or(FsError::NotFound)?;
        let mut resolved = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let parent = if resolved.is_empty() {
                "/"
            } else {
                resolved.as_str()
            };
            let candidate = child_path(parent, part);
            resolved = match self.remote.stat(&candidate) {
                Ok(_) => candidate,
                Err(FsError::NotFound) => self
                    .match_child(parent, part)
                    .ok_or(FsError::NotFound)?,
                Err(other) => return Err(other),
            };
        }
        Ok(if resolved.is_empty() {
            "/".to_owned()
        } else {
            resolved
        })
    }

    /// The name in `parent` that differs from `name` only in case.
    fn match_child(&self, parent: &str, name: &str) -> Option<String> {
        let entries = self.remote.read_dir(parent).ok()?;
        entries
            .into_iter()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(name))
            .map(|(entry, _)| child_path(parent, &entry))
    }
}

/// The registry of live filesystems, keyed by the pointer WinFsp gives back.
fn drivers() -> &'static Mutex<BTreeMap<usize, Arc<Driver>>> {
    static DRIVERS: OnceLock<Mutex<BTreeMap<usize, Arc<Driver>>>> = OnceLock::new();
    DRIVERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The driver behind a callback's file system pointer.
///
/// # Safety
/// `file_system` must be the pointer WinFsp passed to a callback of a live filesystem.
unsafe fn driver_of(file_system: *mut abi::FileSystem) -> Option<Arc<Driver>> {
    if file_system.is_null() {
        return None;
    }
    drivers()
        .lock()
        .ok()?
        .get(&(file_system as usize))
        .cloned()
}

/// The context behind a callback's file context pointer.
///
/// # Safety
/// `context` must be one this filesystem handed out and WinFsp has not closed.
unsafe fn context_of<'a>(context: *mut abi::FileContext) -> Option<&'a Context> {
    if context.is_null() {
        return None;
    }
    // SAFETY: the pointer came from `Box::into_raw` and is only freed in `close`.
    Some(unsafe { &*context.cast::<Context>() })
}

/// Run a callback body, turning a panic into a status instead of unwinding into WinFsp.
fn guard(run: impl FnOnce() -> abi::NtStatus) -> abi::NtStatus {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .unwrap_or(abi::STATUS_UNEXPECTED_IO_ERROR)
}

/// Translate a filesystem failure into the status Windows programs understand.
fn status(error: FsError) -> abi::NtStatus {
    match error {
        FsError::NotFound => abi::STATUS_OBJECT_NAME_NOT_FOUND,
        FsError::Denied => abi::STATUS_ACCESS_DENIED,
        FsError::Exists => abi::STATUS_OBJECT_NAME_COLLISION,
        FsError::NotEmpty => abi::STATUS_DIRECTORY_NOT_EMPTY,
        FsError::IsADirectory => abi::STATUS_FILE_IS_A_DIRECTORY,
        FsError::NotADirectory => abi::STATUS_NOT_A_DIRECTORY,
        FsError::Unsupported => abi::STATUS_NOT_SUPPORTED,
        FsError::Disconnected => abi::STATUS_CONNECTION_DISCONNECTED,
        FsError::Io(_) => abi::STATUS_IO_DEVICE_ERROR,
    }
}

/// The Windows view of one remote file.
fn file_info(attrs: &Attrs) -> abi::FileInfo {
    let attributes = if attrs.is_dir {
        abi::FILE_ATTRIBUTE_DIRECTORY
    } else if attrs.mode & 0o200 == 0 && attrs.mode != 0 {
        abi::FILE_ATTRIBUTE_READONLY
    } else {
        abi::FILE_ATTRIBUTE_NORMAL
    };
    let written = abi::filetime(attrs.modified);
    abi::FileInfo {
        file_attributes: attributes,
        reparse_tag: 0,
        allocation_size: attrs.size,
        file_size: attrs.size,
        creation_time: written,
        last_access_time: abi::filetime(attrs.accessed.max(attrs.modified)),
        last_write_time: written,
        change_time: written,
        index_number: 0,
        hard_links: 1,
        ea_size: 0,
    }
}

/// Wrap a context for WinFsp.
fn hand_out(context: Context) -> *mut c_void {
    Box::into_raw(Box::new(context)).cast()
}

/// The callbacks this filesystem offers.
fn callbacks() -> abi::FileSystemInterface {
    let mut interface = abi::FileSystemInterface::empty();
    interface.get_volume_info = Some(volume_info);
    interface.get_security_by_name = Some(security_by_name);
    interface.create = Some(create);
    interface.open = Some(open);
    interface.overwrite = Some(overwrite);
    interface.cleanup = Some(cleanup);
    interface.close = Some(close);
    interface.read = Some(read);
    interface.write = Some(write);
    interface.flush = Some(flush);
    interface.get_file_info = Some(get_file_info);
    interface.set_basic_info = Some(set_basic_info);
    interface.set_file_size = Some(set_file_size);
    interface.can_delete = Some(can_delete);
    interface.rename = Some(rename);
    interface.read_directory = Some(read_directory);
    interface.set_delete = Some(set_delete);
    interface
}

unsafe extern "C" fn volume_info(
    file_system: *mut abi::FileSystem,
    out: *mut abi::VolumeInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if out.is_null() {
            return abi::STATUS_UNSUCCESSFUL;
        }
        // SAFETY: WinFsp passes a buffer it owns for the answer.
        let info = unsafe { &mut *out };
        info.total_size = abi::VOLUME_SIZE;
        info.free_size = abi::VOLUME_SIZE;
        let label: Vec<u16> = driver
            .label
            .encode_utf16()
            .take(abi::VOLUME_LABEL_LEN - 1)
            .collect();
        info.volume_label[..label.len()].copy_from_slice(&label);
        info.volume_label_length = (label.len() * 2) as u16;
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn security_by_name(
    file_system: *mut abi::FileSystem,
    file_name: *const u16,
    attributes: *mut u32,
    _descriptor: *mut c_void,
    _descriptor_size: *mut usize,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let name = unsafe { abi::from_wide(file_name) };
        let path = match driver.resolve(&name) {
            Ok(path) => path,
            Err(error) => return status(error),
        };
        let attrs = match driver.remote.stat(&path) {
            Ok(attrs) => attrs,
            Err(error) => return status(error),
        };
        if !attributes.is_null() {
            // SAFETY: the out-pointer is WinFsp's own.
            unsafe { *attributes = file_info(&attrs).file_attributes };
        }
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn create(
    file_system: *mut abi::FileSystem,
    file_name: *const u16,
    create_options: u32,
    granted_access: u32,
    _attributes: u32,
    _descriptor: *mut c_void,
    _allocation_size: u64,
    context: *mut *mut c_void,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if context.is_null() {
            return abi::STATUS_UNSUCCESSFUL;
        }
        let name = unsafe { abi::from_wide(file_name) };
        let Some(path) = remote_path(&name) else {
            return abi::STATUS_OBJECT_NAME_NOT_FOUND;
        };
        let directory = create_options & abi::FILE_DIRECTORY_FILE != 0;
        if directory {
            if let Err(error) = driver.remote.mkdir(&path) {
                return status(error);
            }
            let attrs = driver.remote.stat(&path).unwrap_or(Attrs {
                is_dir: true,
                ..Attrs::default()
            });
            unsafe { *context = hand_out(Context::directory(path)) };
            if !info.is_null() {
                // SAFETY: the out-pointer is WinFsp's own.
                unsafe { *info = file_info(&attrs) };
            }
            return abi::STATUS_SUCCESS;
        }
        let file = match driver
            .remote
            .open(&path, mode_of(granted_access), true, false)
        {
            Ok(file) => file,
            Err(error) => return status(error),
        };
        let attrs = driver.remote.stat(&path).unwrap_or_default();
        unsafe { *context = hand_out(Context::file(path, file)) };
        if !info.is_null() {
            // SAFETY: as above.
            unsafe { *info = file_info(&attrs) };
        }
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn open(
    file_system: *mut abi::FileSystem,
    file_name: *const u16,
    _create_options: u32,
    granted_access: u32,
    context: *mut *mut c_void,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if context.is_null() {
            return abi::STATUS_UNSUCCESSFUL;
        }
        let name = unsafe { abi::from_wide(file_name) };
        let path = match driver.resolve(&name) {
            Ok(path) => path,
            Err(error) => return status(error),
        };
        let attrs = match driver.remote.stat(&path) {
            Ok(attrs) => attrs,
            Err(error) => return status(error),
        };
        let entry = if attrs.is_dir {
            Context::directory(path)
        } else {
            match driver
                .remote
                .open(&path, mode_of(granted_access), false, false)
            {
                Ok(file) => Context::file(path, file),
                Err(error) => return status(error),
            }
        };
        unsafe { *context = hand_out(entry) };
        if !info.is_null() {
            // SAFETY: the out-pointer is WinFsp's own.
            unsafe { *info = file_info(&attrs) };
        }
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn overwrite(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    _attributes: u32,
    _replace_attributes: u8,
    _allocation_size: u64,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(file) = entry.file else {
            return abi::STATUS_FILE_IS_A_DIRECTORY;
        };
        if let Err(error) = driver.remote.set_len(file, 0) {
            return status(error);
        }
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn cleanup(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    _file_name: *const u16,
    flags: u32,
) {
    let Some(driver) = (unsafe { driver_of(file_system) }) else {
        return;
    };
    let Some(entry) = (unsafe { context_of(context) }) else {
        return;
    };
    if flags & abi::FSP_CLEANUP_DELETE == 0 || !entry.delete_on_close.load(Ordering::SeqCst) {
        return;
    }
    let _ = if entry.is_dir {
        driver.remote.rmdir(&entry.path)
    } else {
        driver.remote.remove(&entry.path)
    };
}

unsafe extern "C" fn close(file_system: *mut abi::FileSystem, context: *mut abi::FileContext) {
    let driver = unsafe { driver_of(file_system) };
    let entry = unsafe { context_of(context) };
    if let (Some(driver), Some(entry)) = (driver, entry)
        && let Some(file) = entry.file
    {
        let _ = driver.remote.close(file);
    }
    if !context.is_null() {
        // SAFETY: this is the box `hand_out` created, and `close` is the only place that
        // frees it.
        drop(unsafe { Box::from_raw(context.cast::<Context>()) });
    }
}

unsafe extern "C" fn read(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    buffer: *mut c_void,
    offset: u64,
    length: u32,
    transferred: *mut u32,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(file) = entry.file else {
            return abi::STATUS_FILE_IS_A_DIRECTORY;
        };
        let data = match driver.remote.read(file, offset, length) {
            Ok(data) => data,
            Err(error) => return status(error),
        };
        let count = data.len().min(length as usize);
        if !buffer.is_null() && count > 0 {
            // SAFETY: WinFsp passes a buffer of `length` bytes for us to fill.
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.cast::<u8>(), count);
            }
        }
        if !transferred.is_null() {
            // SAFETY: the out-pointer is WinFsp's own.
            unsafe { *transferred = count as u32 };
        }
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn write(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    buffer: *const c_void,
    offset: u64,
    length: u32,
    _write_to_end: u8,
    _constrained: u8,
    transferred: *mut u32,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(file) = entry.file else {
            return abi::STATUS_FILE_IS_A_DIRECTORY;
        };
        if buffer.is_null() && length > 0 {
            return abi::STATUS_UNSUCCESSFUL;
        }
        // SAFETY: WinFsp passes a buffer of `length` bytes to write out.
        let data = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), length as usize) };
        let written = match driver.remote.write(file, offset, data) {
            Ok(written) => written,
            Err(error) => return status(error),
        };
        if !transferred.is_null() {
            // SAFETY: the out-pointer is WinFsp's own.
            unsafe { *transferred = written };
        }
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn flush(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if let Some(file) = entry.file
            && let Err(error) = driver.remote.flush(file)
        {
            return status(error);
        }
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn get_file_info(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn set_basic_info(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    _attributes: u32,
    _creation_time: u64,
    last_access_time: u64,
    last_write_time: u64,
    _change_time: u64,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if last_access_time != 0 || last_write_time != 0 {
            let attrs = driver.remote.stat(&entry.path).unwrap_or_default();
            let accessed = if last_access_time == 0 {
                attrs.accessed
            } else {
                abi::unix_seconds(last_access_time)
            };
            let written = if last_write_time == 0 {
                attrs.modified
            } else {
                abi::unix_seconds(last_write_time)
            };
            if let Err(error) = driver.remote.set_times(&entry.path, accessed, written) {
                return status(error);
            }
        }
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn set_file_size(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    new_size: u64,
    _allocation_size: u8,
    info: *mut abi::FileInfo,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(file) = entry.file else {
            return abi::STATUS_FILE_IS_A_DIRECTORY;
        };
        if let Err(error) = driver.remote.set_len(file, new_size) {
            return status(error);
        }
        fill_info(&driver, entry, info)
    })
}

unsafe extern "C" fn can_delete(
    _file_system: *mut abi::FileSystem,
    _context: *mut abi::FileContext,
    _file_name: *const u16,
) -> abi::NtStatus {
    // Deleting is allowed; a directory that still holds entries says so when it is removed.
    abi::STATUS_SUCCESS
}

unsafe extern "C" fn rename(
    file_system: *mut abi::FileSystem,
    _context: *mut abi::FileContext,
    file_name: *const u16,
    new_file_name: *const u16,
    replace_if_exists: u8,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let from = unsafe { abi::from_wide(file_name) };
        let to = unsafe { abi::from_wide(new_file_name) };
        let from = match driver.resolve(&from) {
            Ok(path) => path,
            Err(error) => return status(error),
        };
        let Some(to) = remote_path(&to) else {
            return abi::STATUS_OBJECT_NAME_NOT_FOUND;
        };
        let existing = driver.resolve(&to).ok();
        if let Some(existing) = &existing {
            if replace_if_exists == 0 {
                return abi::STATUS_OBJECT_NAME_COLLISION;
            }
            // SFTP has no replace-on-rename everywhere, so the target goes first.
            if let Err(error) = driver.remote.remove(existing) {
                return status(error);
            }
        }
        match driver.remote.rename(&from, &to) {
            Ok(()) => abi::STATUS_SUCCESS,
            Err(error) => status(error),
        }
    })
}

unsafe extern "C" fn read_directory(
    file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    _pattern: *const u16,
    marker: *const u16,
    buffer: *mut c_void,
    length: u32,
    transferred: *mut u32,
) -> abi::NtStatus {
    guard(|| {
        let Some(driver) = (unsafe { driver_of(file_system) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        let Some(entry) = (unsafe { context_of(context) }) else {
            return abi::STATUS_UNSUCCESSFUL;
        };
        if buffer.is_null() || transferred.is_null() {
            return abi::STATUS_UNSUCCESSFUL;
        }
        let mut entries = match driver.remote.read_dir(&entry.path) {
            Ok(entries) => entries,
            Err(error) => return status(error),
        };
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let marker = unsafe { abi::from_wide(marker) };
        let api = match api() {
            Ok(api) => api,
            Err(_) => return abi::STATUS_UNSUCCESSFUL,
        };
        for (index, (name, attrs)) in entries.iter().enumerate() {
            // A marker names the last entry the caller already has; nothing at or before it
            // is returned again.
            if !marker.is_empty() && name.as_str() <= marker.as_str() {
                continue;
            }
            let mut block = vec![0u8; abi::dir_info_len(name)];
            abi::write_dir_info(&mut block, &file_info(attrs), index as u64 + 1, name);
            // SAFETY: `block` holds one complete entry for the buffer WinFsp provided, and
            // `transferred` tracks how much of that buffer is used.
            let added = unsafe {
                (api.add_dir_info)(
                    block.as_mut_ptr().cast(),
                    buffer,
                    length,
                    transferred,
                )
            };
            if added == 0 {
                break;
            }
        }
        abi::STATUS_SUCCESS
    })
}

unsafe extern "C" fn set_delete(
    _file_system: *mut abi::FileSystem,
    context: *mut abi::FileContext,
    _file_name: *const u16,
    delete: u8,
) -> abi::NtStatus {
    let Some(entry) = (unsafe { context_of(context) }) else {
        return abi::STATUS_UNSUCCESSFUL;
    };
    entry.delete_on_close.store(delete != 0, Ordering::SeqCst);
    abi::STATUS_SUCCESS
}

/// Write the current attributes of a context's path into WinFsp's file info.
fn fill_info(driver: &Driver, entry: &Context, info: *mut abi::FileInfo) -> abi::NtStatus {
    let attrs = match driver.remote.stat(&entry.path) {
        Ok(attrs) => attrs,
        Err(error) => return status(error),
    };
    if !info.is_null() {
        // SAFETY: the out-pointer is WinFsp's own.
        unsafe { *info = file_info(&attrs) };
    }
    abi::STATUS_SUCCESS
}

impl Context {
    /// An open file.
    fn file(path: String, file: FileId) -> Self {
        Self {
            path,
            file: Some(file),
            is_dir: false,
            delete_on_close: AtomicBool::new(false),
        }
    }

    /// An open directory.
    fn directory(path: String) -> Self {
        Self {
            path,
            file: None,
            is_dir: true,
            delete_on_close: AtomicBool::new(false),
        }
    }
}

/// How to open a file for the access Windows granted it.
fn mode_of(granted_access: u32) -> OpenMode {
    let writes = abi::FILE_WRITE_DATA | abi::FILE_APPEND_DATA;
    match (
        granted_access & abi::FILE_READ_DATA != 0,
        granted_access & writes != 0,
    ) {
        (true, true) => OpenMode::ReadWrite,
        (false, true) => OpenMode::Write,
        (true, false) => OpenMode::Read,
        // No data access at all still needs a handle, and read-only is safest.
        (false, false) => OpenMode::Read,
    }
}

/// The WinFsp exports this crate uses, resolved once.
struct Api {
    version: unsafe extern "C" fn(*mut u32) -> abi::NtStatus,
    create: unsafe extern "C" fn(
        *const u16,
        *const abi::VolumeParams,
        *const abi::FileSystemInterface,
        *mut *mut c_void,
    ) -> abi::NtStatus,
    set_mount_point: unsafe extern "C" fn(*mut c_void, *const u16) -> abi::NtStatus,
    remove_mount_point: unsafe extern "C" fn(*mut c_void),
    start_dispatcher: unsafe extern "C" fn(*mut c_void, u32) -> abi::NtStatus,
    stop_dispatcher: unsafe extern "C" fn(*mut c_void),
    delete: unsafe extern "C" fn(*mut c_void),
    add_dir_info: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut u32) -> u8,
}

// SAFETY: `Api` holds only code pointers into a library that is never unloaded.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// The library this process loaded, or why it could not be loaded.
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load)
        .as_ref()
        .map_err(|reason| reason.clone())
}

/// The DLL name for this architecture.
fn library_name() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "winfsp-a64.dll",
        "x86" => "winfsp-x86.dll",
        _ => "winfsp-x64.dll",
    }
}

/// Every place the DLL may be, most direct first.
///
/// WinFsp 2.2 keeps its DLLs in a side-by-side folder and names it in the registry; older
/// installers put them under `bin` of the install directory, and some put the DLL on the
/// system path.
fn library_paths() -> Vec<String> {
    let mut paths = vec![library_name().to_owned()];
    for value in ["SxsDir", "InstallDir"] {
        if let Some(dir) = install_dir(value) {
            paths.push(format!("{dir}\\bin\\{}", library_name()));
        }
    }
    paths
}

/// One value of the WinFsp registry key, if the installer wrote it.
fn install_dir(value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let key = abi::wide("SOFTWARE\\WOW6432Node\\WinFsp");
    let mut handle = std::ptr::null_mut();
    // SAFETY: `key` is nul-terminated and `handle` is a valid out-parameter.
    let opened =
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key.as_ptr(), 0, KEY_READ, &mut handle) };
    if opened != 0 {
        return None;
    }
    let mut data = [0u16; 512];
    let mut length = (data.len() * 2) as u32;
    let mut kind = 0u32;
    let name = abi::wide(value);
    // SAFETY: the buffer and its length agree, and the handle is open until below.
    let read = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut length,
        )
    };
    // SAFETY: the handle came from `RegOpenKeyExW` and is not used afterwards.
    unsafe { RegCloseKey(handle) };
    if read != 0 || kind != REG_SZ || length == 0 {
        return None;
    }
    let units = (length as usize / 2).min(data.len());
    let text: Vec<u16> = data[..units].iter().copied().take_while(|unit| *unit != 0).collect();
    let text = String::from_utf16_lossy(&text);
    let text = text.trim_end_matches('\\').to_owned();
    (!text.is_empty()).then_some(text)
}

/// Load WinFsp and resolve the exports.
fn load() -> Result<Api, String> {
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

    let mut module = std::ptr::null_mut();
    for path in library_paths() {
        let name = abi::wide(&path);
        // SAFETY: the name is a valid nul-terminated wide string.
        module = unsafe { LoadLibraryW(name.as_ptr()) };
        if !module.is_null() {
            break;
        }
    }
    if module.is_null() {
        return Err(format!(
            "WinFsp is not installed ({} was not found)",
            library_name()
        ));
    }
    // SAFETY: each name below is a nul-terminated string and each symbol is a WinFsp export
    // with the signature declared here; the library stays loaded for the process's life.
    unsafe {
        Ok(Api {
            version: symbol(module, "FspVersion")?,
            create: symbol(module, "FspFileSystemCreate")?,
            set_mount_point: symbol(module, "FspFileSystemSetMountPoint")?,
            remove_mount_point: symbol(module, "FspFileSystemRemoveMountPoint")?,
            start_dispatcher: symbol(module, "FspFileSystemStartDispatcher")?,
            stop_dispatcher: symbol(module, "FspFileSystemStopDispatcher")?,
            delete: symbol(module, "FspFileSystemDelete")?,
            add_dir_info: symbol(module, "FspFileSystemAddDirInfo")?,
        })
    }
}

/// Look one export up and read it as the function pointer `T` this crate declared.
///
/// # Safety
/// `T` must be the exact signature WinFsp gives `name`.
unsafe fn symbol<T: Copy>(
    module: windows_sys::Win32::Foundation::HMODULE,
    name: &str,
) -> Result<T, String> {
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    let c_name = CString::new(name).map_err(|error| error.to_string())?;
    let Some(address) = (unsafe { GetProcAddress(module, c_name.as_ptr().cast()) }) else {
        return Err(format!("WinFsp does not export {name}"));
    };
    // SAFETY: the caller states that T is the export's own signature, and the address is a
    // code pointer the loader resolved.
    Ok(unsafe { std::mem::transmute_copy(&address) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryFs;

    /// Pointers no WinFsp ever hands out, so the callbacks can be called from a test. Each
    /// test takes its own so they can run side by side.
    static NEXT_KEY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0x1000);

    /// Register `remote` under a pointer of its own and answer it with its key.
    fn install(remote: Arc<MemoryFs>) -> (*mut abi::FileSystem, usize) {
        let key = NEXT_KEY.fetch_add(8, Ordering::Relaxed);
        let driver = Arc::new(Driver {
            remote,
            label: "test".to_owned(),
        });
        drivers()
            .lock()
            .expect("the registry")
            .insert(key, driver);
        (key as *mut abi::FileSystem, key)
    }

    /// Forget the driver registered under `key`.
    fn uninstall(key: usize) {
        drivers().lock().expect("the registry").remove(&key);
    }

    #[test]
    fn a_file_can_be_created_written_read_and_deleted() {
        let remote = Arc::new(MemoryFs::new());
        let (fs, key) = install(remote.clone());
        let mut context: *mut c_void = std::ptr::null_mut();
        let mut info = abi::FileInfo::default();
        let path = abi::wide("\\new.txt");
        // SAFETY: every pointer is a live local or a buffer WinFsp would own.
        let status = unsafe {
            create(
                fs,
                path.as_ptr(),
                0,
                abi::FILE_READ_DATA | abi::FILE_WRITE_DATA,
                0,
                std::ptr::null_mut(),
                0,
                &mut context,
                &mut info,
            )
        };
        assert_eq!(status, abi::STATUS_SUCCESS);
        assert!(!context.is_null());

        let mut written = 0u32;
        // SAFETY: as above.
        let status = unsafe {
            write(
                fs,
                context,
                b"hello".as_ptr().cast(),
                0,
                5,
                0,
                0,
                &mut written,
                &mut info,
            )
        };
        assert_eq!((status, written), (abi::STATUS_SUCCESS, 5));

        let mut buffer = [0u8; 5];
        let mut got = 0u32;
        // SAFETY: the buffer holds exactly the five bytes asked for.
        let status = unsafe { read(fs, context, buffer.as_mut_ptr().cast(), 0, 5, &mut got) };
        assert_eq!((status, got), (abi::STATUS_SUCCESS, 5));
        assert_eq!(&buffer, b"hello");

        // SAFETY: the context is the one created above.
        unsafe {
            assert_eq!(set_delete(fs, context, path.as_ptr(), 1), abi::STATUS_SUCCESS);
            cleanup(fs, context, path.as_ptr(), abi::FSP_CLEANUP_DELETE);
            close(fs, context);
        }
        assert_eq!(remote.read("/new.txt"), None);
        assert!(remote.paths() == vec!["/".to_owned()]);
        uninstall(key);
    }

    #[test]
    fn a_name_is_found_whatever_case_it_is_typed_in() {
        let remote = Arc::new(MemoryFs::new());
        remote.write("/etc/hosts", b"127.0.0.1");
        let (fs, key) = install(remote);
        let mut context: *mut c_void = std::ptr::null_mut();
        let mut info = abi::FileInfo::default();
        let path = abi::wide("\\ETC\\HOSTS");
        // SAFETY: every pointer is live for the call.
        let status = unsafe {
            open(
                fs,
                path.as_ptr(),
                0,
                abi::FILE_READ_DATA,
                &mut context,
                &mut info,
            )
        };
        assert_eq!(status, abi::STATUS_SUCCESS);
        assert_eq!(info.file_size, 9);
        assert_eq!(info.file_attributes, abi::FILE_ATTRIBUTE_NORMAL);
        // SAFETY: the context is the one just opened.
        unsafe { close(fs, context) };
        uninstall(key);
    }

    #[test]
    fn a_missing_name_reports_the_status_windows_expects() {
        let remote = Arc::new(MemoryFs::new());
        let (fs, key) = install(remote);
        let mut context: *mut c_void = std::ptr::null_mut();
        let mut info = abi::FileInfo::default();
        let path = abi::wide("\\nope.txt");
        // SAFETY: every pointer is live for the call.
        let status = unsafe {
            open(
                fs,
                path.as_ptr(),
                0,
                abi::FILE_READ_DATA,
                &mut context,
                &mut info,
            )
        };
        assert_eq!(status, abi::STATUS_OBJECT_NAME_NOT_FOUND);
        uninstall(key);
    }

    #[test]
    fn a_directory_is_created_and_then_listed() {
        let remote = Arc::new(MemoryFs::new());
        let (fs, key) = install(remote.clone());
        let mut context: *mut c_void = std::ptr::null_mut();
        let mut info = abi::FileInfo::default();
        let path = abi::wide("\\logs");
        // SAFETY: every pointer is live for the call.
        let status = unsafe {
            create(
                fs,
                path.as_ptr(),
                abi::FILE_DIRECTORY_FILE,
                0,
                0,
                std::ptr::null_mut(),
                0,
                &mut context,
                &mut info,
            )
        };
        assert_eq!(status, abi::STATUS_SUCCESS);
        assert_eq!(info.file_attributes, abi::FILE_ATTRIBUTE_DIRECTORY);
        assert!(remote.stat("/logs").expect("directory").is_dir);
        // SAFETY: the context is the one just created.
        unsafe { close(fs, context) };

        // Without WinFsp the listing cannot be filled into a WinFsp buffer.
        if api().is_err() {
            uninstall(key);
            return;
        }
        remote.write("/logs/a.log", b"x");
        let mut directory: *mut c_void = std::ptr::null_mut();
        // SAFETY: as above.
        let status = unsafe {
            open(
                fs,
                path.as_ptr(),
                0,
                0,
                &mut directory,
                &mut info,
            )
        };
        assert_eq!(status, abi::STATUS_SUCCESS);
        let mut buffer = vec![0u8; 4096];
        let mut used = 0u32;
        // SAFETY: the buffer is as long as the length passed.
        let status = unsafe {
            read_directory(
                fs,
                directory,
                std::ptr::null(),
                std::ptr::null(),
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut used,
            )
        };
        assert_eq!(status, abi::STATUS_SUCCESS);
        assert!(used >= abi::dir_info_len("a.log") as u32);
        let entry = unsafe { &*buffer.as_ptr().cast::<abi::DirInfoHeader>() };
        assert_eq!(entry.file_info.file_size, 1);
        // SAFETY: the context is the one just opened.
        unsafe { close(fs, directory) };
        uninstall(key);
    }

    #[test]
    fn a_rename_replaces_only_when_asked_to() {
        let remote = Arc::new(MemoryFs::new());
        remote.write("/one.txt", b"1");
        remote.write("/two.txt", b"2");
        let (fs, key) = install(remote.clone());
        let from = abi::wide("\\one.txt");
        let to = abi::wide("\\two.txt");
        // SAFETY: the names are live for the call.
        let refused = unsafe { rename(fs, std::ptr::null_mut(), from.as_ptr(), to.as_ptr(), 0) };
        assert_eq!(refused, abi::STATUS_OBJECT_NAME_COLLISION);
        // SAFETY: as above.
        let replaced = unsafe { rename(fs, std::ptr::null_mut(), from.as_ptr(), to.as_ptr(), 1) };
        assert_eq!(replaced, abi::STATUS_SUCCESS);
        assert_eq!(remote.read("/two.txt").expect("contents"), b"1");
        assert!(remote.stat("/one.txt").is_err());
        uninstall(key);
    }

    #[test]
    fn the_windows_view_of_a_file_carries_its_size_and_read_only_bit() {
        let attrs = Attrs {
            is_dir: false,
            size: 42,
            modified: 1_700_000_000,
            accessed: 1_700_000_100,
            mode: 0o444,
            ..Attrs::default()
        };
        let info = file_info(&attrs);
        assert_eq!(info.file_size, 42);
        assert_eq!(info.file_attributes, abi::FILE_ATTRIBUTE_READONLY);
        assert_eq!(info.last_write_time, abi::filetime(1_700_000_000));
        assert_eq!(info.last_access_time, abi::filetime(1_700_000_100));
        let dir = file_info(&Attrs {
            is_dir: true,
            ..Attrs::default()
        });
        assert_eq!(dir.file_attributes, abi::FILE_ATTRIBUTE_DIRECTORY);
    }

    #[test]
    fn failures_map_to_the_codes_windows_programs_read() {
        assert_eq!(status(FsError::NotFound), abi::STATUS_OBJECT_NAME_NOT_FOUND);
        assert_eq!(status(FsError::Denied), abi::STATUS_ACCESS_DENIED);
        assert_eq!(status(FsError::Exists), abi::STATUS_OBJECT_NAME_COLLISION);
        assert_eq!(status(FsError::NotEmpty), abi::STATUS_DIRECTORY_NOT_EMPTY);
        assert_eq!(status(FsError::IsADirectory), abi::STATUS_FILE_IS_A_DIRECTORY);
        assert_eq!(status(FsError::NotADirectory), abi::STATUS_NOT_A_DIRECTORY);
        assert_eq!(status(FsError::Unsupported), abi::STATUS_NOT_SUPPORTED);
        assert_eq!(
            status(FsError::Disconnected),
            abi::STATUS_CONNECTION_DISCONNECTED
        );
    }

    #[test]
    fn the_access_windows_granted_decides_how_a_file_opens() {
        assert_eq!(mode_of(abi::FILE_READ_DATA), OpenMode::Read);
        assert_eq!(mode_of(abi::FILE_WRITE_DATA), OpenMode::Write);
        assert_eq!(
            mode_of(abi::FILE_READ_DATA | abi::FILE_APPEND_DATA),
            OpenMode::ReadWrite
        );
        assert_eq!(mode_of(0), OpenMode::Read);
    }

    #[test]
    fn a_path_splits_into_its_parent_and_name() {
        assert_eq!(child_path("/", "etc"), "/etc");
        assert_eq!(child_path("/etc", "hosts"), "/etc/hosts");
    }

    /// A drive letter nothing is using.
    fn free_drive() -> String {
        for letter in b'D'..=b'Z' {
            let candidate = format!("{}:", letter as char);
            if !std::path::Path::new(&candidate).exists() {
                return candidate;
            }
        }
        panic!("every drive letter is in use");
    }

    #[test]
    #[ignore = "needs WinFsp installed on this machine"]
    fn a_memory_filesystem_is_reachable_at_a_directory() {
        let remote = Arc::new(MemoryFs::new());
        remote.write("/hello.txt", b"from the remote folder");
        let point = std::env::temp_dir().join("wassh-mount-dir-test");
        let _ = std::fs::remove_dir_all(&point);
        let mount = Mount::start(
            remote.clone(),
            "wassh test",
            &point.to_string_lossy(),
        )
        .expect("mounted");

        let mut seen = None;
        for _ in 0..30 {
            match std::fs::read(point.join("hello.txt")) {
                Ok(data) => {
                    seen = Some(data);
                    break;
                }
                Err(error) => {
                    eprintln!("waiting for the directory mount: {error}");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        let seen = seen.expect("read through the mount point");
        assert_eq!(seen, b"from the remote folder");

        std::fs::write(point.join("written.txt"), b"written").expect("write");
        assert_eq!(remote.read("/written.txt").expect("contents"), b"written");

        mount.unmount();
        let _ = std::fs::remove_dir_all(&point);
    }

    #[test]
    #[ignore = "needs WinFsp installed on this machine"]
    fn a_memory_filesystem_is_reachable_as_a_drive() {
        let remote = Arc::new(MemoryFs::new());
        remote.write("/hello.txt", b"from the remote folder");
        let drive = free_drive();
        let mount = Mount::start(remote.clone(), "wassh test", &drive).expect("mounted");

        // The volume arrives asynchronously: give the OS a moment, then read it.
        let mut seen = None;
        for _ in 0..50 {
            match std::fs::read(format!("{drive}\\hello.txt")) {
                Ok(data) => {
                    seen = Some(data);
                    break;
                }
                Err(error) => {
                    eprintln!("waiting for {drive}: {error}");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        let seen = seen.expect("read through the drive");
        assert_eq!(seen, b"from the remote folder");

        std::fs::write(format!("{drive}\\written.txt"), b"written through windows")
            .expect("write through the drive");
        assert_eq!(
            remote.read("/written.txt").expect("contents"),
            b"written through windows"
        );

        let names: Vec<String> = std::fs::read_dir(&drive)
            .expect("listing the drive")
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"hello.txt".to_owned()), "listing: {names:?}");

        std::fs::create_dir(format!("{drive}\\sub")).expect("mkdir through the drive");
        assert!(remote.stat("/sub").expect("the new directory").is_dir);
        mount.unmount();
        assert!(std::fs::read_dir(&drive).is_err(), "the drive is gone");
    }
}

