//! Runs the official `codex login` with `CODEX_HOME` pointing at an isolated directory.

use std::mem::{size_of, zeroed};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

use super::wide::{last_error, OwnedHandle};
use crate::codex::login::{login_arguments, pick_newest};
use crate::codex::LoginRunner;
use crate::error::{Error, Result};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct CodexCliLogin {
    explicit: Option<PathBuf>,
    /// CLI mode shows codex's own output (login URL) in the console.
    inherit_stdio: bool,
}

impl CodexCliLogin {
    pub fn new(explicit: Option<PathBuf>, inherit_stdio: bool) -> Self {
        CodexCliLogin { explicit, inherit_stdio }
    }

    /// Explicit setting → newest CLI bundled with Codex Desktop → `codex.exe` on PATH.
    pub fn locate(&self) -> Result<PathBuf> {
        if let Some(p) = &self.explicit {
            return if p.is_file() {
                Ok(p.clone())
            } else {
                Err(Error::Login(format!("configured codex_cli_path does not exist: {}", p.display())))
            };
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let bin = PathBuf::from(local).join("OpenAI").join("Codex").join("bin");
            let candidates = std::fs::read_dir(bin)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let exe = e.path().join("codex.exe");
                    let meta = std::fs::metadata(&exe).ok().filter(|m| m.is_file())?;
                    Some((exe, meta.modified().unwrap_or(UNIX_EPOCH)))
                })
                .collect();
            if let Some(p) = pick_newest(candidates) {
                return Ok(p);
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            if let Some(exe) = std::env::split_paths(&path).map(|d| d.join("codex.exe")).find(|p| p.is_file()) {
                return Ok(exe);
            }
        }
        Err(Error::Login("codex.exe not found; install Codex Desktop or set codex_cli_path in config.json".into()))
    }
}

struct KillOnCloseJob(OwnedHandle);

impl KillOnCloseJob {
    fn new() -> Result<Self> {
        // SAFETY: anonymous job object.
        let job = OwnedHandle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
            .ok_or_else(|| last_error("CreateJobObjectW"))?;
        // SAFETY: plain data structure.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: info is valid for its size.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(last_error("SetInformationJobObject"));
        }
        Ok(KillOnCloseJob(job))
    }
}

impl LoginRunner for CodexCliLogin {
    fn run_login(&self, isolated_codex_home: &Path, timeout: Duration) -> Result<()> {
        let exe = self.locate()?;
        log_info!("starting official codex login ({}) with isolated CODEX_HOME", exe.display());
        let mut cmd = Command::new(&exe);
        cmd.args(login_arguments())
            .env("CODEX_HOME", isolated_codex_home)
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_API_KEY")
            .current_dir(isolated_codex_home)
            .stdin(Stdio::null());
        if !self.inherit_stdio {
            cmd.stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
        }

        let job = KillOnCloseJob::new()?;
        let mut child = cmd.spawn().map_err(|e| Error::Login(format!("failed to start codex login: {e}")))?;
        // SAFETY: the child's process handle is valid while `child` lives.
        unsafe { AssignProcessToJobObject(job.0 .0, child.as_raw_handle() as _) };

        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(Error::Login(format!("codex login exited with {status}"))),
                Ok(None) => {}
                Err(e) => return Err(Error::Login(format!("waiting for codex login failed: {e}"))),
            }
            if Instant::now() >= deadline {
                // SAFETY: valid job handle.
                unsafe { TerminateJobObject(job.0 .0, 1) };
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Login(format!("sign-in did not complete within {} minutes", timeout.as_secs() / 60)));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}
