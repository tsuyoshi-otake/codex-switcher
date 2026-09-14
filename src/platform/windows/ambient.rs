use std::ptr::null_mut;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};

use crate::ambient::Ambient;

pub struct SystemAmbient;

impl Ambient for SystemAmbient {
    fn random_bytes(&self, buf: &mut [u8]) {
        // SAFETY: buf is a valid writable slice of the given length.
        let status = unsafe { BCryptGenRandom(null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        // The system RNG does not fail in practice; continuing with predictable ids would be worse.
        assert!(status == 0, "BCryptGenRandom failed with NTSTATUS {status:#x}");
    }

    fn now_unix_secs(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
}
