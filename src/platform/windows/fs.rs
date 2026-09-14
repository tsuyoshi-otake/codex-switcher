//! [`FileStore`] on NTFS: atomic replace via same-directory temp file + `MoveFileExW`,
//! reparse-point refusal, best-effort scrubbing of deleted secrets, exclusive lock files.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

use super::wide::wide;
use crate::ambient::Ambient;
use crate::fsio::{atomic_temp_name, FileStore, LockGuard};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

pub struct WindowsFileStore {
    ambient: Arc<dyn Ambient>,
}

impl WindowsFileStore {
    pub fn new(ambient: Arc<dyn Ambient>) -> Self {
        WindowsFileStore { ambient }
    }
}

fn reparse_error(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("refusing to use reparse point (symlink/junction): {}", path.display()))
}

/// `Ok(false)` if absent, `Ok(true)` if present and not a reparse point.
fn check_not_reparse(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => Err(reparse_error(path)),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn check_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => check_not_reparse(p).map(|_| ()),
        _ => Ok(()),
    }
}

/// Overwrites file content with zeros before deletion. Best effort: NTFS/SSD may retain
/// old clusters, which is why secrets at rest are always DPAPI-encrypted anyway.
fn scrub_file(path: &Path) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).share_mode(0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    if f.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(reparse_error(path));
    }
    let mut left = f.metadata()?.len();
    let zeros = [0u8; 8192];
    while left > 0 {
        let n = left.min(zeros.len() as u64) as usize;
        f.write_all(&zeros[..n])?;
        left -= n as u64;
    }
    f.sync_all()
}

fn delete_file(path: &Path) -> io::Result<()> {
    if let Ok(m) = fs::symlink_metadata(path) {
        let mut perms = m.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    let (src, dst) = (wide(from), wide(to));
    let mut attempt = 0u64;
    loop {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let ok = unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) };
        if ok != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        let transient = matches!(err.raw_os_error(), Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION));
        attempt += 1;
        if !transient || attempt >= 5 {
            return Err(err);
        }
        // Antivirus / indexer briefly holding the file.
        std::thread::sleep(Duration::from_millis(50 * attempt));
    }
}

impl FileStore for WindowsFileStore {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        check_parent(path)?;
        let mut file = match OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // Checked on the opened handle: no window between check and use.
        let meta = file.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(reparse_error(path));
        }
        if !meta.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a regular file: {}", path.display())));
        }
        let mut buf = Vec::with_capacity(meta.len() as usize);
        file.read_to_end(&mut buf)?;
        Ok(Some(buf))
    }

    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        check_parent(path)?;
        check_not_reparse(path)?;
        let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid file name"))?;
        let mut rnd = [0u8; 8];
        self.ambient.random_bytes(&mut rnd);
        let tmp = parent.join(atomic_temp_name(name, &rnd));

        let result = (|| {
            // create_new + no sharing: nobody else can open the temp file while it holds data.
            let mut f: File = OpenOptions::new().write(true).create_new(true).share_mode(0).open(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            drop(f);
            replace_file(&tmp, path)
        })();
        if result.is_err() {
            let _ = scrub_file(&tmp);
            let _ = delete_file(&tmp);
        }
        result
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        check_parent(path)?;
        if !check_not_reparse(path)? {
            return Ok(());
        }
        let _ = scrub_file(path);
        delete_file(path)
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        check_parent(dir)?;
        if !check_not_reparse(dir)? {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_file() && meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                if let Ok(name) = entry.file_name().into_string() {
                    names.push(name);
                }
            }
        }
        Ok(names)
    }

    fn create_dir_all(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        check_parent(dir)?;
        check_not_reparse(dir).map(|_| ())
    }

    fn remove_dir_all(&self, dir: &Path) -> io::Result<()> {
        match fs::symlink_metadata(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => return Err(reparse_error(dir)),
            Ok(m) if !m.is_dir() => return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a directory")),
            Ok(_) => {}
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(reparse_error(&path));
            }
            if meta.is_dir() {
                self.remove_dir_all(&path)?;
            } else {
                let _ = scrub_file(&path);
                delete_file(&path)?;
            }
        }
        fs::remove_dir(dir)
    }

    fn try_lock(&self, path: &Path) -> io::Result<Option<LockGuard>> {
        check_parent(path)?;
        match OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(path) {
            Ok(f) => Ok(Some(Box::new(f))),
            Err(e) if matches!(e.raw_os_error(), Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;
    use crate::platform::windows::ambient::SystemAmbient;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut rnd = [0u8; 6];
            SystemAmbient.random_bytes(&mut rnd);
            let p = std::env::temp_dir().join(format!("codex-switcher-test-{tag}-{}", rnd.iter().map(|b| format!("{b:02x}")).collect::<String>()));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> WindowsFileStore {
        WindowsFileStore::new(Arc::new(SystemAmbient))
    }

    #[test]
    fn atomic_replace_leaves_no_temp_files() {
        let dir = TempDir::new("atomic");
        let target = dir.0.join("auth.json");
        let s = store();
        assert_eq!(s.read(&target).unwrap(), None);
        s.write_atomic(&target, b"first").unwrap();
        s.write_atomic(&target, b"second").unwrap();
        assert_eq!(s.read(&target).unwrap().unwrap(), b"second");
        assert_eq!(s.list(&dir.0).unwrap(), vec!["auth.json".to_string()]);
        s.remove(&target).unwrap();
        s.remove(&target).unwrap();
        assert_eq!(s.read(&target).unwrap(), None);
    }

    #[test]
    fn junction_parent_is_refused() {
        let dir = TempDir::new("junction");
        let real = dir.0.join("real");
        let link = dir.0.join("link");
        fs::create_dir(&real).unwrap();
        let status = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&real).output().unwrap();
        assert!(status.status.success(), "mklink /J failed");
        let s = store();
        let err = s.write_atomic(&link.join("auth.json"), b"x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(s.read(&link.join("auth.json")).is_err());
        assert!(s.remove_dir_all(&link).is_err());
        assert!(!real.join("auth.json").exists());
        fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = TempDir::new("lock");
        let s = store();
        let lock = dir.0.join("switch.lock");
        let guard = s.try_lock(&lock).unwrap().expect("first lock");
        assert!(s.try_lock(&lock).unwrap().is_none());
        drop(guard);
        assert!(s.try_lock(&lock).unwrap().is_some());
    }

    #[test]
    fn remove_dir_all_scrubs_nested_content() {
        let dir = TempDir::new("rmdir");
        let staging = dir.0.join("login-staging");
        let s = store();
        s.create_dir_all(&staging.join("sessions")).unwrap();
        s.write_atomic(&staging.join("auth.json"), b"secret").unwrap();
        s.write_atomic(&staging.join("sessions").join("x.jsonl"), b"data").unwrap();
        s.remove_dir_all(&staging).unwrap();
        assert!(!staging.exists());
        s.remove_dir_all(&staging).unwrap();
    }
}
