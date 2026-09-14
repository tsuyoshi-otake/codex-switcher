use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;

use super::wide::{last_error, wide, OwnedHandle};
use crate::error::Result;

pub const TRAY_MUTEX: &str = r"Local\CodexAccountSwitcher-Tray";

/// Held for the lifetime of the tray process.
pub struct InstanceGuard(#[allow(dead_code)] OwnedHandle);

/// `Ok(None)` when another tray instance already runs in this session.
pub fn acquire(name: &str) -> Result<Option<InstanceGuard>> {
    let name = wide(name);
    // SAFETY: NUL-terminated name, default security.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: read immediately after the call.
    let err = unsafe { GetLastError() };
    let handle = OwnedHandle::new(handle).ok_or_else(|| last_error("CreateMutexW"))?;
    Ok((err != ERROR_ALREADY_EXISTS).then(|| InstanceGuard(handle)))
}
