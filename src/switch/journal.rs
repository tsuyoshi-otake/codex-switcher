//! Durable switch journal (`switch-journal.json`) and encrypted backup
//! (`switch-backup.bin`, DPAPI blob of the pre-switch auth.json).
//!
//! The journal contains only non-secret data (profile ids, account identity labels).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::auth::AccountIdentity;
use crate::error::{Error, Result};
use crate::fsio::FileStore;
use crate::profile::ProfileId;

const JOURNAL_VERSION: u32 = 1;

/// Ordered: a later phase implies all earlier durable effects happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    /// Codex stopped; nothing written yet.
    Prepared,
    /// Encrypted backup of auth.json written (or auth.json was absent).
    CredentialsBackedUp,
    /// About to swap auth.json; on disk it may be source or target.
    ReplacingCredentials,
    /// auth.json holds the target credentials.
    CredentialsReplaced,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwitchJournal {
    pub version: u32,
    pub phase: JournalPhase,
    pub target_profile_id: ProfileId,
    pub target_identity: AccountIdentity,
    #[serde(default)]
    pub source_profile_id: Option<ProfileId>,
    #[serde(default)]
    pub source_identity: Option<AccountIdentity>,
    /// Whether auth.json existed before the switch (decides restore vs delete).
    pub auth_existed: bool,
    pub codex_was_running: bool,
    pub started_at: u64,
}

impl SwitchJournal {
    pub fn new(target_profile_id: ProfileId, target_identity: AccountIdentity, codex_was_running: bool, started_at: u64) -> Self {
        SwitchJournal {
            version: JOURNAL_VERSION,
            phase: JournalPhase::Prepared,
            target_profile_id,
            target_identity,
            source_profile_id: None,
            source_identity: None,
            auth_existed: false,
            codex_was_running,
            started_at,
        }
    }
}

pub struct JournalStore {
    fs: Arc<dyn FileStore>,
    dir: PathBuf,
}

impl JournalStore {
    pub fn new(fs: Arc<dyn FileStore>, state_dir: &Path) -> Self {
        JournalStore { fs, dir: state_dir.to_path_buf() }
    }

    pub fn journal_path(&self) -> PathBuf {
        self.dir.join("switch-journal.json")
    }

    pub fn backup_path(&self) -> PathBuf {
        self.dir.join("switch-backup.bin")
    }

    pub fn load(&self) -> Result<Option<SwitchJournal>> {
        let Some(bytes) = self.fs.read(&self.journal_path()).map_err(|e| Error::io("reading switch journal", e))? else {
            return Ok(None);
        };
        let journal: SwitchJournal = serde_json::from_slice(&bytes)
            .map_err(|_| Error::RecoveryBlocked("switch journal is unreadable; inspect it manually".into()))?;
        if journal.version != JOURNAL_VERSION {
            return Err(Error::RecoveryBlocked(format!("unsupported switch journal version {}", journal.version)));
        }
        Ok(Some(journal))
    }

    pub fn save(&self, journal: &SwitchJournal) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(journal).map_err(|e| Error::Config(e.to_string()))?;
        self.fs.create_dir_all(&self.dir).map_err(|e| Error::io("creating state directory", e))?;
        self.fs.write_atomic(&self.journal_path(), &bytes).map_err(|e| Error::io("writing switch journal", e))
    }

    pub fn write_backup(&self, encrypted: &[u8]) -> Result<()> {
        self.fs.create_dir_all(&self.dir).map_err(|e| Error::io("creating state directory", e))?;
        self.fs.write_atomic(&self.backup_path(), encrypted).map_err(|e| Error::io("writing credential backup", e))
    }

    pub fn read_backup(&self) -> Result<Option<Vec<u8>>> {
        self.fs.read(&self.backup_path()).map_err(|e| Error::io("reading credential backup", e))
    }

    /// Removes the journal first, then the backup. If the second step fails, what is left
    /// is an orphaned (DPAPI-encrypted) backup that nothing refers to and that the next
    /// switch or recovery overwrites/removes. The reverse order could leave a journal
    /// whose backup is gone, which blocks recovery (found by the fault-injection proptest).
    pub fn clear(&self) -> Result<()> {
        self.fs.remove(&self.journal_path()).map_err(|e| Error::io("removing switch journal", e))?;
        self.remove_orphaned_backup()
    }

    pub fn remove_orphaned_backup(&self) -> Result<()> {
        self.fs.remove(&self.backup_path()).map_err(|e| Error::io("removing credential backup", e))
    }
}
