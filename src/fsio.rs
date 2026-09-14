//! File-system boundary.
//!
//! Every persistent write made by the switcher goes through [`FileStore`], so the
//! switch transaction and recovery can be tested against an in-memory store with
//! fault injection (`testing::MemFs`). The Windows implementation lives in
//! `platform::windows::fs`.

use std::any::Any;
use std::io;
use std::path::Path;

/// Held while an exclusive lock is owned; dropping it releases the lock.
pub type LockGuard = Box<dyn Any + Send>;

/// Hidden, unique sibling name for the temp file of an atomic replace
/// (`.auth.json.<pid>.<hex>.tmp`). Same directory ⇒ same volume ⇒ rename is atomic.
pub fn atomic_temp_name(file_name: &str, random: &[u8]) -> String {
    let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
    format!(".{file_name}.{}.{hex}.tmp", std::process::id())
}

pub trait FileStore: Send + Sync {
    /// Reads a whole file. `Ok(None)` when it does not exist.
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>>;

    /// Replaces `path` so that readers observe either the previous or the new
    /// content, never a partial file. The data is flushed to disk before the swap.
    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()>;

    /// Removes a file, overwriting its content first on a best-effort basis.
    /// Succeeds when the file is already absent.
    fn remove(&self, path: &Path) -> io::Result<()>;

    /// Lists the file names (not paths) directly inside `dir`; empty when missing.
    fn list(&self, dir: &Path) -> io::Result<Vec<String>>;

    fn create_dir_all(&self, dir: &Path) -> io::Result<()>;

    /// Removes a directory tree, scrubbing file contents first (best effort).
    fn remove_dir_all(&self, dir: &Path) -> io::Result<()>;

    /// Takes an exclusive, non-blocking lock. `Ok(None)` if someone else holds it.
    fn try_lock(&self, path: &Path) -> io::Result<Option<LockGuard>>;
}
