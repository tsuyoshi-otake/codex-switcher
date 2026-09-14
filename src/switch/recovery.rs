use super::journal::JournalPhase;
use super::SwitchDeps;
use crate::auth::{parse_identity, SecretBytes};
use crate::error::{Error, Result};
use crate::profile::ProfileId;

#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// No interrupted switch.
    Clean,
    /// Interrupted before auth.json was touched; the journal was discarded.
    Discarded,
    /// auth.json already holds the complete target credentials; the switch is finished.
    RolledForward { target: ProfileId },
    /// auth.json holds (or was restored to) the previous credentials.
    RolledBack,
}

/// Repairs an interrupted switch. Safe to call repeatedly (idempotent).
///
/// auth.json is always replaced atomically, so after a crash it is either the source or
/// the target. Only if it is neither (e.g. modified externally) are the backed-up source
/// credentials written back, and only while Codex Desktop is not running.
pub fn recover(deps: &SwitchDeps) -> Result<RecoveryOutcome> {
    let Some(journal) = deps.journal.load()? else {
        // A backup without a journal is left when clearing was interrupted; not needed.
        deps.journal.remove_orphaned_backup()?;
        return Ok(RecoveryOutcome::Clean);
    };

    if journal.phase < JournalPhase::ReplacingCredentials {
        deps.journal.clear()?;
        return Ok(RecoveryOutcome::Discarded);
    }

    let current = deps.fs.read(deps.auth_file).map_err(|e| Error::io("reading auth.json", e))?.map(SecretBytes::new);
    let current_identity = current.as_ref().and_then(|c| parse_identity(c.as_bytes()).ok());

    if current_identity.as_ref().is_some_and(|id| id.same_account(&journal.target_identity)) {
        deps.journal.clear()?;
        return Ok(RecoveryOutcome::RolledForward { target: journal.target_profile_id });
    }

    // Source identity is recognisable without the backup, so a missing backup alone
    // never blocks an already rolled-back state.
    if journal.auth_existed
        && matches!((&current_identity, &journal.source_identity), (Some(cur), Some(src)) if cur.same_account(src))
    {
        deps.journal.clear()?;
        return Ok(RecoveryOutcome::RolledBack);
    }

    let backup = if journal.auth_existed {
        let encrypted = deps
            .journal
            .read_backup()?
            .ok_or_else(|| Error::RecoveryBlocked("credential backup is missing".into()))?;
        Some(deps.protector.unprotect(&encrypted)?)
    } else {
        None
    };

    let is_source = match (&current, &backup) {
        (None, None) => true,
        (Some(c), Some(b)) => {
            b.ct_eq(c.as_bytes())
                || match (&current_identity, &journal.source_identity) {
                    (Some(cur), Some(src)) => cur.same_account(src),
                    _ => false,
                }
        }
        _ => false,
    };
    if is_source {
        deps.journal.clear()?;
        return Ok(RecoveryOutcome::RolledBack);
    }

    if deps.codex.inspect()?.any_owned_running() {
        return Err(Error::RecoveryBlocked("close Codex so the previous account can be restored".into()));
    }
    match backup {
        Some(b) => deps.fs.write_atomic(deps.auth_file, b.as_bytes()).map_err(|e| Error::io("restoring auth.json", e))?,
        None => deps.fs.remove(deps.auth_file).map_err(|e| Error::io("removing auth.json", e))?,
    }
    deps.journal.clear()?;
    Ok(RecoveryOutcome::RolledBack)
}
