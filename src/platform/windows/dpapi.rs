//! DPAPI (CryptProtectData, current user, no UI) implementation of [`SecretProtector`].

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

use crate::auth::secret::scrub;
use crate::auth::{SecretBytes, SecretProtector};
use crate::error::{Error, Result};

/// Secondary entropy binds blobs to this application; other DPAPI consumers running as
/// the same user cannot decrypt them by accident.
const ENTROPY: &[u8] = b"codex-switcher/v1";

pub struct DpapiProtector;

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

impl SecretProtector for DpapiProtector {
    fn protect(&self, plaintext: &SecretBytes) -> Result<Vec<u8>> {
        let input = blob(plaintext.as_bytes());
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: input/entropy point to live slices; out receives a LocalAlloc buffer.
        let ok = unsafe { CryptProtectData(&input, null(), &entropy, null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 {
            return Err(Error::Protect(format!("CryptProtectData failed: {}", std::io::Error::last_os_error())));
        }
        // SAFETY: DPAPI returned cbData valid bytes at pbData.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: buffer was allocated by DPAPI with LocalAlloc.
        unsafe { LocalFree(out.pbData as HLOCAL) };
        Ok(bytes)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<SecretBytes> {
        let input = blob(ciphertext);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // SAFETY: as above.
        let ok = unsafe { CryptUnprotectData(&input, null_mut(), &entropy, null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
        if ok == 0 {
            return Err(Error::Protect(format!("CryptUnprotectData failed: {}", std::io::Error::last_os_error())));
        }
        // SAFETY: DPAPI returned cbData valid, exclusively owned bytes at pbData.
        let plain = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
        let secret = SecretBytes::new(plain.to_vec());
        scrub(plain);
        // SAFETY: buffer was allocated by DPAPI with LocalAlloc.
        unsafe { LocalFree(out.pbData as HLOCAL) };
        Ok(secret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper_detection() {
        let plain = SecretBytes::new(b"{\"tokens\":{\"refresh_token\":\"x\"}}".to_vec());
        let blob = DpapiProtector.protect(&plain).unwrap();
        assert!(!blob.windows(plain.as_bytes().len()).any(|w| w == plain.as_bytes()), "ciphertext contains plaintext");
        assert!(DpapiProtector.unprotect(&blob).unwrap().ct_eq(plain.as_bytes()));

        let mut tampered = blob.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x55;
        assert!(matches!(DpapiProtector.unprotect(&tampered), Err(Error::Protect(_))));
        assert!(DpapiProtector.unprotect(b"not a dpapi blob").is_err());
    }
}
