//! UTF-16 and handle helpers shared by the Win32 modules.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

use crate::error::Error;

/// NUL-terminated UTF-16.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Copies `s` into a fixed Win32 buffer, truncating on a character boundary.
pub fn copy_to_fixed(dst: &mut [u16], s: &str) {
    let mut i = 0;
    let mut units = [0u16; 2];
    for c in s.chars() {
        let encoded = c.encode_utf16(&mut units);
        if i + encoded.len() >= dst.len() {
            break;
        }
        dst[i..i + encoded.len()].copy_from_slice(encoded);
        i += encoded.len();
    }
    if let Some(slot) = dst.get_mut(i) {
        *slot = 0;
    }
}

pub fn last_error(context: &str) -> Error {
    Error::Platform(format!("{context}: {}", std::io::Error::last_os_error()))
}

pub struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    pub fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(OwnedHandle(handle))
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned, valid, and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_copy_truncates_and_terminates() {
        let mut buf = [0xFFFFu16; 4];
        copy_to_fixed(&mut buf, "abcdef");
        assert_eq!(from_wide(&buf), "abc");
        let mut buf = [0xFFFFu16; 3];
        copy_to_fixed(&mut buf, "a😀");
        assert_eq!(from_wide(&buf), "a", "surrogate pair is not split");
    }
}
