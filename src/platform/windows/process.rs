//! Process snapshot with package identity and creation times.

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

use super::wide::{from_wide, last_error, OwnedHandle};
use crate::codex::process_model::ProcessEntry;
use crate::error::Result;

const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;

pub fn snapshot() -> Result<Vec<ProcessEntry>> {
    // SAFETY: plain snapshot call; handle ownership is taken immediately.
    let snap = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        .ok_or_else(|| last_error("CreateToolhelp32Snapshot"))?;
    // SAFETY: PROCESSENTRY32W is plain data; dwSize is set before use.
    let mut pe: PROCESSENTRY32W = unsafe { zeroed() };
    pe.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut out = Vec::new();
    let mut image_buf = vec![0u16; 32_768];
    // SAFETY: valid snapshot handle and initialised entry.
    if unsafe { Process32FirstW(snap.0, &mut pe) } == 0 {
        return Err(last_error("Process32FirstW"));
    }
    loop {
        if pe.th32ProcessID != 0 {
            out.push(describe(pe.th32ProcessID, pe.th32ParentProcessID, &mut image_buf));
        }
        // SAFETY: as above.
        if unsafe { Process32NextW(snap.0, &mut pe) } == 0 {
            break;
        }
    }
    Ok(out)
}

fn describe(pid: u32, parent_pid: u32, image_buf: &mut [u16]) -> ProcessEntry {
    let mut entry = ProcessEntry { pid, parent_pid, image_path: None, package_family: None, created_at: 0 };
    // SAFETY: OpenProcess with query-only rights; failure (protected/system processes) is fine.
    if let Some(h) = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }) {
        entry.image_path = image_path(h.0, image_buf);
        entry.package_family = package_family(h.0);
        entry.created_at = creation_time(h.0).unwrap_or(0);
    }
    entry
}

fn image_path(h: HANDLE, buf: &mut [u16]) -> Option<String> {
    let mut len = buf.len() as u32;
    // SAFETY: buf is writable for len units.
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn package_family(h: HANDLE) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: size query with a null buffer.
    let rc = unsafe { GetPackageFamilyName(h, &mut len, null_mut()) };
    if rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        return None; // APPMODEL_ERROR_NO_PACKAGE: unpackaged process
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: buf has len units.
    let rc = unsafe { GetPackageFamilyName(h, &mut len, buf.as_mut_ptr()) };
    (rc == 0).then(|| from_wide(&buf))
}

fn creation_time(h: HANDLE) -> Option<u64> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: all out-pointers are valid.
    let ok = unsafe { GetProcessTimes(h, &mut created, &mut exit, &mut kernel, &mut user) };
    (ok != 0).then_some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

/// Opens a process for waiting/termination only if it is still the same process
/// (creation time matches), so a recycled PID is never acted upon.
pub fn open_for_control(pid: u32, expected_created_at: u64) -> Option<OwnedHandle> {
    // SAFETY: plain OpenProcess.
    let h = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) })?;
    if expected_created_at != 0 && creation_time(h.0) != Some(expected_created_at) {
        return None;
    }
    Some(h)
}

pub fn has_exited(h: &OwnedHandle) -> bool {
    // SAFETY: handle opened with SYNCHRONIZE.
    unsafe { WaitForSingleObject(h.0, 0) == WAIT_OBJECT_0 }
}

pub fn terminate(h: &OwnedHandle) -> bool {
    // SAFETY: handle opened with PROCESS_TERMINATE.
    unsafe { TerminateProcess(h.0, 1) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_describes_current_process() {
        let me = std::process::id();
        let entries = snapshot().unwrap();
        let entry = entries.iter().find(|e| e.pid == me).expect("current process in snapshot");
        assert!(entry.image_path.as_deref().is_some_and(|p| p.to_ascii_lowercase().ends_with(".exe")));
        assert!(entry.created_at != 0);
        assert_eq!(entry.package_family, None, "test binary is unpackaged");
        let h = open_for_control(me, entry.created_at).expect("open self");
        assert!(!has_exited(&h));
        assert!(open_for_control(me, entry.created_at + 1).is_none(), "creation time mismatch is rejected");
    }
}
