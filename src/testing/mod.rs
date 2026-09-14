//! Test doubles: in-memory file store with fault/crash injection, fake DPAPI, fake Codex.

pub mod fixtures;

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ambient::Ambient;
use crate::app::{PermissionProbe, StartupRegistration};
use crate::codex::LoginRunner;
use crate::auth::{SecretBytes, SecretProtector};
use crate::codex::{CodexController, CodexRuntime, ExternalClient, StopPolicy, StopReport};
use crate::error::{Error, Result};
use crate::fsio::{FileStore, LockGuard};

// ---- MemFs ------------------------------------------------------------------------------

#[derive(Default)]
struct MemState {
    files: BTreeMap<String, Vec<u8>>,
    ops: usize,
    fail_at: Option<usize>,
    crash: bool,
    crashed: bool,
    fail_suffix: Option<(String, usize)>,
}

#[derive(Default)]
pub struct MemFs {
    state: Mutex<MemState>,
    locks: Arc<Mutex<HashSet<String>>>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, path: &Path, data: &[u8]) {
        self.state.lock().unwrap().files.insert(key(path), data.to_vec());
    }

    pub fn get(&self, path: &Path) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(&key(path)).cloned()
    }

    /// Operation number `n` (0-based, all operations counted) fails. With `crash`, every
    /// later operation fails too, modelling the process dying at that point.
    pub fn fail_at_op(&self, n: usize, crash: bool) {
        let mut s = self.state.lock().unwrap();
        s.fail_at = Some(s.ops + n);
        s.crash = crash;
    }

    /// The next `times` writes/removals whose path ends with `suffix` fail.
    pub fn fail_writes_to_suffix(&self, suffix: Option<&str>, times: usize) {
        self.state.lock().unwrap().fail_suffix = suffix.map(|s| (s.to_ascii_lowercase(), times));
    }

    /// Simulated reboot: durable files remain, injected faults are cleared.
    pub fn restart(&self) {
        let mut s = self.state.lock().unwrap();
        s.fail_at = None;
        s.crash = false;
        s.crashed = false;
        s.fail_suffix = None;
        self.locks.lock().unwrap().clear();
    }

    fn check(s: &mut MemState, path: &Path, mutating: bool) -> io::Result<()> {
        if s.crashed {
            return Err(io::Error::other("process crashed"));
        }
        let idx = s.ops;
        s.ops += 1;
        if s.fail_at == Some(idx) {
            if s.crash {
                s.crashed = true;
            }
            return Err(io::Error::other("injected fault"));
        }
        if mutating {
            if let Some((suffix, times)) = &mut s.fail_suffix {
                if *times > 0 && key(path).ends_with(suffix.as_str()) {
                    *times -= 1;
                    return Err(io::Error::other("injected write failure"));
                }
            }
        }
        Ok(())
    }
}

struct MemLock {
    locks: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for MemLock {
    fn drop(&mut self) {
        self.locks.lock().unwrap().remove(&self.key);
    }
}

impl FileStore for MemFs {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, path, false)?;
        Ok(s.files.get(&key(path)).cloned())
    }

    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, path, true)?;
        s.files.insert(key(path), data.to_vec());
        Ok(())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, path, true)?;
        s.files.remove(&key(path));
        Ok(())
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, dir, false)?;
        let d = key(dir);
        Ok(s.files
            .keys()
            .filter_map(|k| k.rsplit_once('/').filter(|(parent, _)| *parent == d).map(|(_, name)| name.to_string()))
            .collect())
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, dir, true)
    }

    fn remove_dir_all(&self, dir: &Path) -> io::Result<()> {
        let mut s = self.state.lock().unwrap();
        Self::check(&mut s, dir, true)?;
        let prefix = format!("{}/", key(dir));
        s.files.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    fn try_lock(&self, path: &Path) -> io::Result<Option<LockGuard>> {
        let k = key(path);
        let mut locks = self.locks.lock().unwrap();
        if !locks.insert(k.clone()) {
            return Ok(None);
        }
        Ok(Some(Box::new(MemLock { locks: self.locks.clone(), key: k })))
    }
}

// ---- FakeProtector ----------------------------------------------------------------------

#[derive(Default)]
pub struct FakeProtector {
    fail_protect: AtomicBool,
    fail_unprotect: AtomicBool,
}

impl FakeProtector {
    pub fn fail_protect(&self, v: bool) {
        self.fail_protect.store(v, Ordering::SeqCst);
    }
    pub fn fail_unprotect(&self, v: bool) {
        self.fail_unprotect.store(v, Ordering::SeqCst);
    }
}

impl SecretProtector for FakeProtector {
    fn protect(&self, plaintext: &SecretBytes) -> Result<Vec<u8>> {
        if self.fail_protect.load(Ordering::SeqCst) {
            return Err(Error::Protect("injected".into()));
        }
        let mut out = b"FAKE".to_vec();
        out.extend(plaintext.as_bytes().iter().map(|b| b ^ 0xA5));
        Ok(out)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<SecretBytes> {
        if self.fail_unprotect.load(Ordering::SeqCst) {
            return Err(Error::Protect("injected".into()));
        }
        let body = ciphertext.strip_prefix(b"FAKE").ok_or_else(|| Error::Protect("not a fake blob".into()))?;
        Ok(SecretBytes::new(body.iter().map(|b| b ^ 0xA5).collect()))
    }
}

// ---- FakeCodex ----------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct FakeCodexState {
    pub running: bool,
    pub stop_fails: bool,
    pub start_fails_times: usize,
    pub wait_fails: bool,
    pub lingering_helpers: bool,
    pub external_clients: usize,
    pub stops: usize,
    pub starts: usize,
}

pub struct FakeCodex {
    state: Mutex<FakeCodexState>,
}

impl FakeCodex {
    pub fn new(running: bool) -> Self {
        FakeCodex { state: Mutex::new(FakeCodexState { running, ..Default::default() }) }
    }
    pub fn update(&self, f: impl FnOnce(&mut FakeCodexState)) {
        f(&mut self.state.lock().unwrap());
    }
    pub fn state(&self) -> FakeCodexState {
        self.state.lock().unwrap().clone()
    }
}

impl CodexController for FakeCodex {
    fn inspect(&self) -> Result<CodexRuntime> {
        let s = self.state.lock().unwrap();
        Ok(CodexRuntime {
            desktop_pids: if s.running { vec![1] } else { vec![] },
            helper_pids: if s.lingering_helpers { vec![2] } else { vec![] },
            external_clients: (0..s.external_clients)
                .map(|i| ExternalClient { pid: 100 + i as u32, image_path: "codex.exe".into() })
                .collect(),
        })
    }

    fn stop(&self, _policy: &StopPolicy) -> Result<StopReport> {
        let mut s = self.state.lock().unwrap();
        s.stops += 1;
        if s.stop_fails {
            return Err(Error::CodexStop("injected".into()));
        }
        let was_running = s.running;
        s.running = false;
        Ok(StopReport { was_running, forced: false })
    }

    fn start(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.start_fails_times > 0 {
            s.start_fails_times -= 1;
            return Err(Error::CodexStart("injected".into()));
        }
        s.running = true;
        s.starts += 1;
        Ok(())
    }

    fn wait_until_running(&self, _timeout: Duration) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.wait_fails {
            s.wait_fails = false;
            s.running = false;
            return Err(Error::CodexStart("did not come up".into()));
        }
        if s.running {
            Ok(())
        } else {
            Err(Error::CodexStart("not running".into()))
        }
    }
}

// ---- FakeLogin / FakeStartup / FakePermissions ------------------------------------------

/// Simulates `codex login` writing auth.json into the isolated home (or failing).
pub struct FakeLogin {
    fs: Arc<MemFs>,
    produce: Mutex<Option<Vec<u8>>>,
    calls: Mutex<Vec<std::path::PathBuf>>,
}

impl FakeLogin {
    pub fn new(fs: Arc<MemFs>) -> Self {
        FakeLogin { fs, produce: Mutex::new(None), calls: Mutex::new(Vec::new()) }
    }
    pub fn produce(&self, auth: Option<Vec<u8>>) {
        *self.produce.lock().unwrap() = auth;
    }
    pub fn calls(&self) -> Vec<std::path::PathBuf> {
        self.calls.lock().unwrap().clone()
    }
}

impl LoginRunner for FakeLogin {
    fn run_login(&self, home: &Path, _timeout: Duration) -> Result<()> {
        self.calls.lock().unwrap().push(home.to_path_buf());
        match self.produce.lock().unwrap().clone() {
            Some(bytes) => {
                self.fs.put(&home.join("auth.json"), &bytes);
                Ok(())
            }
            None => {
                self.fs.put(&home.join("partial.tmp"), b"partial");
                Err(Error::Login("cancelled".into()))
            }
        }
    }
}

#[derive(Default)]
pub struct FakeStartup {
    enabled: AtomicBool,
}

impl StartupRegistration for FakeStartup {
    fn is_enabled(&self) -> Result<bool> {
        Ok(self.enabled.load(Ordering::SeqCst))
    }
    fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.enabled.store(enabled, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
pub struct FakePermissions {
    readers: Mutex<Vec<String>>,
}

impl FakePermissions {
    pub fn set(&self, readers: Vec<String>) {
        *self.readers.lock().unwrap() = readers;
    }
}

impl PermissionProbe for FakePermissions {
    fn broad_readers(&self, _path: &Path) -> Result<Vec<String>> {
        Ok(self.readers.lock().unwrap().clone())
    }
}

// ---- FixedAmbient ------------------------------------------------------------------------

#[derive(Default)]
pub struct FixedAmbient {
    counter: AtomicU64,
}

impl Ambient for FixedAmbient {
    fn random_bytes(&self, buf: &mut [u8]) {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (n.rotate_left(i as u32 * 7) as u8) ^ (i as u8).wrapping_mul(31) ^ (n as u8);
        }
        buf[0] = n as u8;
        buf[1] = (n >> 8) as u8;
    }

    fn now_unix_secs(&self) -> u64 {
        1_780_000_000 + self.counter.load(Ordering::SeqCst)
    }
}
