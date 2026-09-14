use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::Result;

/// Runs Codex's official login flow against an isolated, temporary CODEX_HOME so a new
/// account can be captured without touching the shared auth.json or stopping Desktop.
pub trait LoginRunner: Send + Sync {
    fn run_login(&self, isolated_codex_home: &Path, timeout: Duration) -> Result<()>;
}

/// `codex -c cli_auth_credentials_store="file" login` — forces the file store so the
/// resulting credentials land in `<isolated home>/auth.json` even if the user's global
/// configuration would choose the keyring.
pub fn login_arguments() -> [&'static str; 3] {
    ["-c", "cli_auth_credentials_store=\"file\"", "login"]
}

/// Chooses the most recently modified candidate `codex.exe`.
pub fn pick_newest(mut candidates: Vec<(PathBuf, SystemTime)>) -> Option<PathBuf> {
    candidates.sort_by_key(|(_, t)| *t);
    candidates.pop().map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_candidate_wins() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(10);
        let picked = pick_newest(vec![(PathBuf::from("old"), t0), (PathBuf::from("new"), t1)]);
        assert_eq!(picked, Some(PathBuf::from("new")));
        assert_eq!(pick_newest(vec![]), None);
    }

    #[test]
    fn forces_file_store() {
        assert_eq!(login_arguments()[1], "cli_auth_credentials_store=\"file\"");
    }
}
