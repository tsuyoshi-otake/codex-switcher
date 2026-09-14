use std::time::Duration;

use super::journal::{JournalPhase, SwitchJournal};
use super::phase::SwitchPhase;
use super::{ExternalClientsPolicy, SwitchDeps};
use crate::auth::{parse_identity, SecretBytes};
use crate::codex::StopPolicy;
use crate::error::{Error, Result};
use crate::profile::{ProfileId, ProfileMetadata};

#[derive(Clone, Debug)]
pub struct SwitchSettings {
    pub stop_policy: StopPolicy,
    /// Start Codex again after the switch if it was running before.
    pub restart_codex: bool,
    pub start_timeout: Duration,
    pub external_clients: ExternalClientsPolicy,
}

#[derive(Debug)]
pub struct SwitchOutcome {
    pub target: ProfileMetadata,
    pub source: Option<ProfileMetadata>,
    /// The previously active account was not registered and has been added as a profile.
    pub source_registered: bool,
    /// The target was already the active account; its latest credentials were saved.
    pub already_active: bool,
    pub codex_restarted: bool,
    pub forced_stop: bool,
    pub external_clients: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RollbackStatus {
    /// Failure happened before auth.json was touched.
    NotNeeded,
    /// auth.json was restored to the previous account.
    Restored,
    /// Restoration could not complete; the journal is kept for startup recovery.
    Failed(String),
}

#[derive(Debug)]
pub struct SwitchFailure {
    pub error: Error,
    pub phase: SwitchPhase,
    pub rollback: RollbackStatus,
}

/// Runs one account switch. Progress is reported through `progress`.
pub fn switch_account(
    deps: &SwitchDeps,
    settings: &SwitchSettings,
    target_id: &ProfileId,
    progress: &mut dyn FnMut(SwitchPhase),
) -> std::result::Result<SwitchOutcome, SwitchFailure> {
    let early = |phase, error| SwitchFailure { error, phase, rollback: RollbackStatus::NotNeeded };

    // ---- Preconditions: nothing is modified until Codex is confirmed stopped.
    match deps.journal.load() {
        Ok(None) => {}
        Ok(Some(_)) => return Err(early(SwitchPhase::Idle, Error::RecoveryPending)),
        Err(e) => return Err(early(SwitchPhase::Idle, e)),
    }
    // Pre-flight decrypt: a corrupted target is reported without restarting Codex.
    let target = match deps.vault.open(target_id) {
        Ok((meta, secret)) => {
            drop(secret);
            meta
        }
        Err(e) => return Err(early(SwitchPhase::Idle, e)),
    };

    progress(SwitchPhase::StoppingCodex);
    let runtime = deps.codex.inspect().map_err(|e| early(SwitchPhase::StoppingCodex, e))?;
    let external_clients = runtime.external_clients.len();
    if external_clients > 0 && settings.external_clients == ExternalClientsPolicy::Block {
        return Err(early(SwitchPhase::StoppingCodex, Error::ExternalClientsRunning(external_clients)));
    }
    let was_running = runtime.desktop_running();
    let stop = match deps.codex.stop(&settings.stop_policy) {
        Ok(r) => r,
        Err(e) => {
            restart_best_effort(deps, was_running);
            return Err(early(SwitchPhase::StoppingCodex, e));
        }
    };

    progress(SwitchPhase::WaitingForExit);
    match deps.codex.inspect() {
        Ok(rt) if !rt.any_owned_running() => {}
        Ok(_) => {
            restart_best_effort(deps, was_running);
            return Err(early(SwitchPhase::WaitingForExit, Error::CodexStop("Codex processes are still running".into())));
        }
        Err(e) => {
            restart_best_effort(deps, was_running);
            return Err(early(SwitchPhase::WaitingForExit, e));
        }
    }

    let mut tx = Tx {
        deps,
        settings,
        journal: SwitchJournal::new(target_id.clone(), target.identity.clone(), was_running, deps.ambient.now_unix_secs()),
        progress,
    };

    // ---- Journal + backup + save current credentials into their profile.
    tx.progress(SwitchPhase::SavingCurrentCredentials);
    let saved = tx.save_current(&target);
    let (source, source_registered) = match saved {
        Ok(SaveResult::AlreadyActive) => {
            let _ = deps.journal.clear();
            let restarted = restart_best_effort(deps, was_running && settings.restart_codex);
            tx.progress(SwitchPhase::Completed);
            return Ok(SwitchOutcome {
                target,
                source: None,
                source_registered: false,
                already_active: true,
                codex_restarted: restarted,
                forced_stop: stop.forced,
                external_clients,
            });
        }
        Ok(SaveResult::Saved { source, registered }) => (source, registered),
        Err(e) => return Err(tx.abort_untouched(SwitchPhase::SavingCurrentCredentials, e)),
    };

    // ---- Replace auth.json with the target credentials.
    tx.progress(SwitchPhase::DecryptingTargetProfile);
    let target_secret = match deps.vault.open(target_id) {
        Ok((_, s)) => s,
        Err(e) => return Err(tx.rollback(SwitchPhase::DecryptingTargetProfile, e)),
    };

    tx.progress(SwitchPhase::ReplacingCredentials);
    if let Err(e) = tx.replace_credentials(&target_secret) {
        return Err(tx.rollback(SwitchPhase::ReplacingCredentials, e));
    }
    drop(target_secret);

    // ---- Start Codex and confirm it came up.
    let mut restarted = false;
    if was_running && settings.restart_codex {
        tx.progress(SwitchPhase::StartingCodex);
        if let Err(e) = deps.codex.start() {
            return Err(tx.rollback(SwitchPhase::StartingCodex, e));
        }
        tx.progress(SwitchPhase::Verifying);
        if let Err(e) = deps.codex.wait_until_running(settings.start_timeout) {
            return Err(tx.rollback(SwitchPhase::Verifying, e));
        }
        restarted = true;
    } else {
        tx.progress(SwitchPhase::Verifying);
    }

    // A leftover journal in phase CredentialsReplaced is rolled forward by recovery,
    // so a failure to delete it does not change the outcome.
    let _ = deps.journal.clear();
    tx.progress(SwitchPhase::Completed);
    Ok(SwitchOutcome {
        target,
        source,
        source_registered,
        already_active: false,
        codex_restarted: restarted,
        forced_stop: stop.forced,
        external_clients,
    })
}

enum SaveResult {
    AlreadyActive,
    Saved { source: Option<ProfileMetadata>, registered: bool },
}

struct Tx<'a, 'p> {
    deps: &'a SwitchDeps<'a>,
    settings: &'a SwitchSettings,
    journal: SwitchJournal,
    progress: &'p mut dyn FnMut(SwitchPhase),
}

impl Tx<'_, '_> {
    fn progress(&mut self, phase: SwitchPhase) {
        (self.progress)(phase);
    }

    fn set_phase(&mut self, phase: JournalPhase) -> Result<()> {
        self.journal.phase = phase;
        self.deps.journal.save(&self.journal)
    }

    fn save_current(&mut self, target: &ProfileMetadata) -> Result<SaveResult> {
        let d = self.deps;
        d.journal.save(&self.journal)?;
        let current = d.fs.read(d.auth_file).map_err(|e| Error::io("reading auth.json", e))?;
        let Some(bytes) = current else {
            self.journal.auth_existed = false;
            self.set_phase(JournalPhase::CredentialsBackedUp)?;
            return Ok(SaveResult::Saved { source: None, registered: false });
        };
        let secret = SecretBytes::new(bytes);
        // Refuse to switch away from credentials we could not re-register: they would only
        // survive in the temporary backup.
        let identity = parse_identity(secret.as_bytes())?;

        let encrypted = d.protector.protect(&secret)?;
        d.journal.write_backup(&encrypted)?;
        self.journal.auth_existed = true;
        self.journal.source_identity = Some(identity.clone());
        self.set_phase(JournalPhase::CredentialsBackedUp)?;

        let (source, created) = d.vault.upsert(&secret)?;
        if identity.same_account(&target.identity) {
            return Ok(SaveResult::AlreadyActive);
        }
        self.journal.source_profile_id = Some(source.id.clone());
        d.journal.save(&self.journal)?;
        Ok(SaveResult::Saved { source: Some(source), registered: created })
    }

    fn replace_credentials(&mut self, target_secret: &SecretBytes) -> Result<()> {
        let d = self.deps;
        self.set_phase(JournalPhase::ReplacingCredentials)?;
        d.fs.write_atomic(d.auth_file, target_secret.as_bytes()).map_err(|e| Error::io("replacing auth.json", e))?;
        let written = d.fs.read(d.auth_file).map_err(|e| Error::io("verifying auth.json", e))?;
        let matches = written.map(SecretBytes::new).is_some_and(|w| w.ct_eq(target_secret.as_bytes()));
        if !matches {
            return Err(Error::InvalidCredentials("auth.json content differs from the target profile after replacement".into()));
        }
        self.set_phase(JournalPhase::CredentialsReplaced)
    }

    /// Failure before auth.json was replaced: discard the journal and bring Codex back.
    fn abort_untouched(&mut self, phase: SwitchPhase, error: Error) -> SwitchFailure {
        let _ = self.deps.journal.clear();
        restart_best_effort(self.deps, self.journal.codex_was_running);
        SwitchFailure { error, phase, rollback: RollbackStatus::NotNeeded }
    }

    /// Failure at or after replacement: put the previous credentials back.
    fn rollback(&mut self, phase: SwitchPhase, error: Error) -> SwitchFailure {
        self.progress(SwitchPhase::RollingBack);
        let rollback = match self.restore_previous() {
            Ok(()) => RollbackStatus::Restored,
            Err(e) => RollbackStatus::Failed(e.to_string()),
        };
        // auth.json is complete in every case (atomic replace), so starting Codex is safe.
        restart_best_effort(self.deps, self.journal.codex_was_running);
        SwitchFailure { error, phase, rollback }
    }

    fn restore_previous(&mut self) -> Result<()> {
        let d = self.deps;
        if d.codex.inspect()?.any_owned_running() {
            d.codex.stop(&self.settings.stop_policy)?;
        }
        if let Some(bytes) = d.fs.read(d.auth_file).map_err(|e| Error::io("reading auth.json", e))? {
            let current = SecretBytes::new(bytes);
            // Codex may have rotated the target's refresh token before failing; keep it.
            if parse_identity(current.as_bytes()).is_ok_and(|id| id.same_account(&self.journal.target_identity)) {
                d.vault.upsert(&current)?;
            }
        }
        if self.journal.auth_existed {
            let encrypted = d
                .journal
                .read_backup()?
                .ok_or_else(|| Error::RecoveryBlocked("credential backup is missing".into()))?;
            let previous = d.protector.unprotect(&encrypted)?;
            d.fs.write_atomic(d.auth_file, previous.as_bytes()).map_err(|e| Error::io("restoring auth.json", e))?;
        } else {
            d.fs.remove(d.auth_file).map_err(|e| Error::io("removing auth.json", e))?;
        }
        let _ = d.journal.clear();
        Ok(())
    }
}

fn restart_best_effort(deps: &SwitchDeps, should_start: bool) -> bool {
    if !should_start {
        return false;
    }
    match deps.codex.inspect() {
        Ok(rt) if rt.desktop_running() => true,
        _ => deps.codex.start().is_ok(),
    }
}
