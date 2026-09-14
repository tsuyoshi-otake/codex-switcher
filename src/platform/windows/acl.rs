//! Reports broad principals that can read a file (auth.json permission check).

use std::ffi::c_void;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    CreateWellKnownSid, EqualSid, GetAce, WinAnonymousSid, WinAuthenticatedUserSid, WinBuiltinGuestsSid,
    WinBuiltinUsersSid, WinWorldSid, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
    PSECURITY_DESCRIPTOR, PSID, WELL_KNOWN_SID_TYPE,
};

/// From winnt.h (lives in `Win32_System_SystemServices`, not worth a feature for one constant).
const ACCESS_ALLOWED_ACE_TYPE: u32 = 0;

use super::wide::wide;
use crate::app::PermissionProbe;
use crate::error::{Error, Result};

const FILE_READ_DATA: u32 = 0x0001;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;

pub struct AclProbe;

fn well_known_sid(kind: WELL_KNOWN_SID_TYPE) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 68]; // SECURITY_MAX_SID_SIZE
    let mut size = buf.len() as u32;
    // SAFETY: buffer size passed correctly.
    let ok = unsafe { CreateWellKnownSid(kind, null_mut(), buf.as_mut_ptr() as PSID, &mut size) };
    (ok != 0).then_some(buf)
}

impl PermissionProbe for AclProbe {
    fn broad_readers(&self, path: &Path) -> Result<Vec<String>> {
        let name = wide(path);
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: out-pointers are valid; sd is freed with LocalFree below.
        let rc = unsafe {
            GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut dacl, null_mut(), &mut sd)
        };
        if rc != 0 {
            return Err(Error::Platform(format!("GetNamedSecurityInfoW failed ({rc})")));
        }
        let result = (|| {
            if dacl.is_null() {
                return vec!["Everyone (NULL DACL)".to_string()];
            }
            let broad: Vec<(Vec<u8>, &str)> = [
                (WinWorldSid, "Everyone"),
                (WinBuiltinUsersSid, "Users"),
                (WinAuthenticatedUserSid, "Authenticated Users"),
                (WinAnonymousSid, "Anonymous"),
                (WinBuiltinGuestsSid, "Guests"),
            ]
            .into_iter()
            .filter_map(|(kind, label)| well_known_sid(kind).map(|sid| (sid, label)))
            .collect();

            let mut found: Vec<String> = Vec::new();
            // SAFETY: dacl points into the security descriptor returned above.
            let count = unsafe { (*dacl).AceCount };
            for i in 0..u32::from(count) {
                let mut ace: *mut c_void = null_mut();
                // SAFETY: index is within AceCount.
                if unsafe { GetAce(dacl, i, &mut ace) } == 0 || ace.is_null() {
                    continue;
                }
                // SAFETY: every ACE starts with ACE_HEADER; ACCESS_ALLOWED_ACE has Mask at +4
                // and the SID at +8. Read unaligned to avoid alignment assumptions.
                let header = unsafe { std::ptr::read_unaligned(ace as *const ACE_HEADER) };
                if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                    continue;
                }
                let mask = unsafe { std::ptr::read_unaligned((ace as *const u8).add(4) as *const u32) };
                if mask & (FILE_READ_DATA | GENERIC_READ | GENERIC_ALL) == 0 {
                    continue;
                }
                let sid = unsafe { (ace as *mut u8).add(8) } as PSID;
                for (well_known, label) in &broad {
                    // SAFETY: both SIDs are valid.
                    if unsafe { EqualSid(sid, well_known.as_ptr() as PSID) } != 0 && !found.iter().any(|f| f == label) {
                        found.push((*label).to_string());
                    }
                }
            }
            found
        })();
        // SAFETY: allocated by GetNamedSecurityInfoW.
        unsafe { LocalFree(sd as HLOCAL) };
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn detects_everyone_read_grant() {
        let dir = std::env::temp_dir().join(format!("codex-switcher-acl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("auth.json");
        std::fs::write(&file, b"{}").unwrap();
        let before = AclProbe.broad_readers(&file).unwrap();
        assert!(!before.iter().any(|r| r == "Everyone"));
        let out = Command::new("icacls").arg(&file).args(["/grant", "*S-1-1-0:R"]).output().unwrap();
        assert!(out.status.success());
        let after = AclProbe.broad_readers(&file).unwrap();
        assert!(after.iter().any(|r| r == "Everyone"), "{after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
