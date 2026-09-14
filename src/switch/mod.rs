//! The account switch as a crash-safe transaction.
//!
//! * [`phase`] – observable progress states.
//! * [`journal`] – the durable write-ahead record + encrypted backup of the previous auth.json.
//! * [`transaction`] – STOP → SAVE → REPLACE → START with rollback.
//! * [`recovery`] – startup repair of an interrupted switch.
//!
//! Invariant (property-tested in `tests`): whatever step fails or crashes, auth.json is left
//! holding either the complete source credentials or the complete target credentials, and
//! the source account's latest credentials are never lost.

pub mod journal;
pub mod phase;
pub mod recovery;
pub mod transaction;

#[cfg(test)]
mod tests;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ambient::Ambient;
use crate::auth::SecretProtector;
use crate::codex::CodexController;
use crate::fsio::FileStore;
use crate::profile::ProfileVault;

pub use journal::{JournalPhase, JournalStore, SwitchJournal};
pub use phase::SwitchPhase;
pub use recovery::{recover, RecoveryOutcome};
pub use transaction::{switch_account, RollbackStatus, SwitchFailure, SwitchOutcome, SwitchSettings};

/// What to do when non-Desktop Codex clients (CLI, IDE extensions) share CODEX_HOME.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalClientsPolicy {
    /// Switch anyway and report the count. Codex re-reads auth.json before refreshing and
    /// refuses to refresh when the account id changed, so stale clients cannot write back.
    #[default]
    Warn,
    Block,
}

/// Collaborators of a switch, borrowed for its duration.
pub struct SwitchDeps<'a> {
    pub fs: &'a dyn FileStore,
    pub vault: &'a ProfileVault,
    pub protector: &'a dyn SecretProtector,
    pub codex: &'a dyn CodexController,
    pub journal: &'a JournalStore,
    pub auth_file: &'a Path,
    pub ambient: &'a dyn Ambient,
}
