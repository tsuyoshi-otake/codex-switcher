//! Codex Desktop lifecycle on Windows.
//!
//! * Identification: MSIX installation plus dedicated Codex runtime descendants. Inherited
//!   package identity and parentage alone never authorize stopping a user application.
//! * Stop: `WM_CLOSE` to Desktop's top-level windows → bounded wait → (optionally)
//!   `TerminateProcess` of processes that are still verified to be the same Desktop-owned
//!   processes (PID + creation time) → bounded wait.
//! * Start: `shell:AppsFolder\<family>!<app>` (the packaged app's AUMID).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, PostMessageW, GW_OWNER, SW_SHOWNORMAL, WM_CLOSE,
};

use super::process;
use super::wide::wide;
use crate::codex::process_model::{classify, owned_processes, ProcessEntry};
use crate::codex::{CodexController, CodexRuntime, ExternalClient, StopPolicy, StopReport};
use crate::error::{Error, Result};

const POLL: Duration = Duration::from_millis(250);

pub struct WindowsCodexController {
    family: String,
    app_id: String,
    runtime_root: Option<String>,
}

impl WindowsCodexController {
    pub fn new(family: String, app_id: String) -> Self {
        let runtime_root = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .map(|root| root.join("OpenAI").join("Codex").to_string_lossy().into_owned());
        WindowsCodexController { family, app_id, runtime_root }
    }

    /// Revalidate executable ownership on every poll, including previously known PIDs.
    fn owned_alive(&self, known: &mut Vec<(u32, u64)>) -> Result<Vec<ProcessEntry>> {
        let entries = process::snapshot()?;
        let alive = owned_processes(&entries, known, &self.family, self.runtime_root.as_deref());
        let mut remembered: HashSet<u32> = known.iter().map(|(pid, _)| *pid).collect();
        for e in &alive {
            if remembered.insert(e.pid) {
                known.push((e.pid, e.created_at));
            }
        }
        Ok(alive)
    }

    fn wait_for_exit(&self, known: &mut Vec<(u32, u64)>, deadline: Instant) -> Result<Vec<ProcessEntry>> {
        loop {
            let alive = self.owned_alive(known)?;
            if alive.is_empty() || Instant::now() >= deadline {
                return Ok(alive);
            }
            std::thread::sleep(POLL);
        }
    }
}

struct EnumCtx<'a> {
    pids: &'a HashSet<u32>,
    windows: Vec<HWND>,
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut EnumCtx passed to EnumWindows below, alive for the call.
    let ctx = unsafe { &mut *(lparam as *mut EnumCtx) };
    let mut pid = 0u32;
    // SAFETY: valid hwnd from the enumeration.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if ctx.pids.contains(&pid) && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() {
        ctx.windows.push(hwnd);
    }
    1
}

fn request_close(pids: &HashSet<u32>) -> usize {
    let mut ctx = EnumCtx { pids, windows: Vec::new() };
    // SAFETY: callback only touches ctx, which outlives the call.
    unsafe { EnumWindows(Some(collect_window), &mut ctx as *mut EnumCtx as LPARAM) };
    for hwnd in &ctx.windows {
        // SAFETY: posting a message to a window handle is safe even if it was destroyed meanwhile.
        unsafe { PostMessageW(*hwnd, WM_CLOSE, 0, 0) };
    }
    ctx.windows.len()
}

impl CodexController for WindowsCodexController {
    fn inspect(&self) -> Result<CodexRuntime> {
        let entries = process::snapshot()?;
        let c = classify(&entries, &self.family, self.runtime_root.as_deref());
        Ok(CodexRuntime {
            desktop_pids: c.desktop.iter().map(|e| e.pid).collect(),
            helper_pids: c.helpers.iter().map(|e| e.pid).collect(),
            external_clients: c
                .external_clients
                .into_iter()
                .map(|e| ExternalClient { pid: e.pid, image_path: e.image_path.unwrap_or_default() })
                .collect(),
        })
    }

    fn stop(&self, policy: &StopPolicy) -> Result<StopReport> {
        let entries = process::snapshot()?;
        let c = classify(&entries, &self.family, self.runtime_root.as_deref());
        if c.desktop.is_empty() && c.helpers.is_empty() {
            return Ok(StopReport { was_running: false, forced: false });
        }
        let was_running = !c.desktop.is_empty();
        let desktop_pids: HashSet<u32> = c.desktop.iter().map(|e| e.pid).collect();
        let mut known: Vec<(u32, u64)> = c.desktop.iter().chain(&c.helpers).map(|e| (e.pid, e.created_at)).collect();

        let windows = request_close(&desktop_pids);
        log_info!(
            "stopping Codex Desktop: {} desktop / {} helper processes, WM_CLOSE sent to {windows} windows",
            c.desktop.len(),
            c.helpers.len()
        );

        let mut remaining = self.wait_for_exit(&mut known, Instant::now() + policy.graceful_timeout)?;
        if remaining.is_empty() {
            return Ok(StopReport { was_running, forced: false });
        }
        if !policy.allow_force {
            return Err(Error::CodexStop(format!(
                "{} Codex processes still running after {}s",
                remaining.len(),
                policy.graceful_timeout.as_secs()
            )));
        }

        // Desktop first so it cannot respawn helpers, then the helpers.
        remaining.sort_by_key(|e| !desktop_pids.contains(&e.pid));
        let mut terminated = 0;
        for e in &remaining {
            if let Some(h) = process::open_for_control(e.pid, e.created_at) {
                if !process::has_exited(&h) && process::terminate(&h) {
                    terminated += 1;
                }
            }
        }
        log_warn!("Codex did not exit gracefully; terminated {terminated} Desktop-owned processes");

        let remaining = self.wait_for_exit(&mut known, Instant::now() + policy.force_timeout)?;
        if !remaining.is_empty() {
            return Err(Error::CodexStop(format!("{} Codex processes could not be terminated", remaining.len())));
        }
        Ok(StopReport { was_running, forced: true })
    }

    fn start(&self) -> Result<()> {
        let target = wide(format!(r"shell:AppsFolder\{}!{}", self.family, self.app_id));
        let verb = wide("open");
        // SAFETY: strings are NUL-terminated and outlive the call.
        let rc = unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
        } as isize;
        if rc <= 32 {
            return Err(Error::CodexStart(format!("ShellExecute failed with code {rc}")));
        }
        Ok(())
    }

    fn wait_until_running(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.inspect()?.desktop_running() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::CodexStart(format!("Codex Desktop did not start within {}s", timeout.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
