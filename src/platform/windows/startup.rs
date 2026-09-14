//! "Start with Windows" through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Chosen over the Startup folder (shortcut creation needs COM IShellLink) and Task
//! Scheduler (heavier, needs XML/COM, overkill for a per-user tray app). MSIX
//! StartupTask is unavailable because codex-switch is not packaged.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

use super::wide::{from_wide, wide};
use crate::app::StartupRegistration;
use crate::error::{Error, Result};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CodexAccountSwitcher";

pub struct RunKeyStartup {
    command: String,
}

impl RunKeyStartup {
    pub fn for_current_exe() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|e| Error::io("resolving executable path", e))?;
        Ok(RunKeyStartup { command: format!("\"{}\"", exe.display()) })
    }
}

impl StartupRegistration for RunKeyStartup {
    fn is_enabled(&self) -> Result<bool> {
        let mut buf = vec![0u16; 4096];
        let mut size = (buf.len() * 2) as u32;
        let (key, value) = (wide(RUN_KEY), wide(VALUE_NAME));
        // SAFETY: buffer size is passed in bytes.
        let rc = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut size)
        };
        match rc {
            0 => Ok(from_wide(&buf).eq_ignore_ascii_case(&self.command)),
            ERROR_FILE_NOT_FOUND | ERROR_MORE_DATA => Ok(false),
            e => Err(Error::Platform(format!("RegGetValueW failed ({e})"))),
        }
    }

    fn set_enabled(&self, enabled: bool) -> Result<()> {
        let key = wide(RUN_KEY);
        let value = wide(VALUE_NAME);
        if !enabled {
            // SAFETY: NUL-terminated strings.
            let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
            return match rc {
                0 | ERROR_FILE_NOT_FOUND => Ok(()),
                e => Err(Error::Platform(format!("RegDeleteKeyValueW failed ({e})"))),
            };
        }
        let mut hkey: HKEY = null_mut();
        // SAFETY: out-pointer is valid.
        let rc = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, null(), &mut hkey, null_mut())
        };
        if rc != 0 {
            return Err(Error::Platform(format!("RegCreateKeyExW failed ({rc})")));
        }
        let data = wide(&self.command);
        // SAFETY: data includes the terminating NUL; size is in bytes.
        let rc = unsafe { RegSetValueExW(hkey, value.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        // SAFETY: key opened above.
        unsafe { RegCloseKey(hkey) };
        if rc != 0 {
            return Err(Error::Platform(format!("RegSetValueExW failed ({rc})")));
        }
        Ok(())
    }
}
