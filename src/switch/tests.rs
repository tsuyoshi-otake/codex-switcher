//! Switch transaction and recovery tests, including a property test over every possible
//! crash / fault point.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;

use super::*;
use crate::auth::{parse_identity, SecretBytes};
use crate::codex::StopPolicy;
use crate::error::Error;
use crate::profile::{ProfileId, ProfileRepository, ProfileVault};
use crate::testing::fixtures::auth_json;
use crate::testing::{FakeCodex, FakeProtector, FixedAmbient, MemFs};

const AUTH: &str = "C:/Users/u/.codex/auth.json";
const STATE: &str = "C:/Users/u/AppData/Local/CodexSwitcher";

struct World {
    fs: Arc<MemFs>,
    protector: Arc<FakeProtector>,
    codex: Arc<FakeCodex>,
    ambient: Arc<FixedAmbient>,
    vault: ProfileVault,
    journal: JournalStore,
    auth: PathBuf,
}

impl World {
    fn new(codex_running: bool) -> Self {
        let fs = Arc::new(MemFs::new());
        let protector = Arc::new(FakeProtector::default());
        let ambient = Arc::new(FixedAmbient::default());
        let vault = ProfileVault::new(
            ProfileRepository::new(fs.clone(), Path::new(STATE).join("profiles")),
            protector.clone(),
            ambient.clone(),
        );
        let journal = JournalStore::new(fs.clone(), Path::new(STATE));
        World { fs, protector, codex: Arc::new(FakeCodex::new(codex_running)), ambient, vault, journal, auth: PathBuf::from(AUTH) }
    }

    fn deps(&self) -> SwitchDeps<'_> {
        SwitchDeps {
            fs: self.fs.as_ref(),
            vault: &self.vault,
            protector: self.protector.as_ref(),
            codex: self.codex.as_ref(),
            journal: &self.journal,
            auth_file: &self.auth,
            ambient: self.ambient.as_ref(),
        }
    }

    fn register(&self, bytes: &[u8]) -> ProfileId {
        self.vault.upsert(&SecretBytes::new(bytes.to_vec())).unwrap().0.id
    }

    fn auth_bytes(&self) -> Option<Vec<u8>> {
        self.fs.get(&self.auth)
    }

    fn switch(&self, target: &ProfileId) -> (std::result::Result<SwitchOutcome, SwitchFailure>, Vec<SwitchPhase>) {
        let mut phases = Vec::new();
        let r = switch_account(&self.deps(), &settings(), target, &mut |p| phases.push(p));
        (r, phases)
    }
}

fn settings() -> SwitchSettings {
    SwitchSettings {
        stop_policy: StopPolicy { graceful_timeout: Duration::from_millis(1), allow_force: true, force_timeout: Duration::from_millis(1) },
        restart_codex: true,
        start_timeout: Duration::from_millis(1),
        external_clients: ExternalClientsPolicy::Warn,
    }
}

fn a_old() -> Vec<u8> {
    auth_json("a@example.com", "acct-a", "user-a", "pro", "rt-a-old")
}
fn a_rotated() -> Vec<u8> {
    auth_json("a@example.com", "acct-a", "user-a", "pro", "rt-a-rotated")
}
fn b() -> Vec<u8> {
    auth_json("t@example.com", "acct-b-business", "user-t", "business", "rt-b")
}

#[test]
fn happy_path_switches_and_saves_rotated_source_credentials() {
    let w = World::new(true);
    w.register(&a_old());
    let target = w.register(&b());
    // Codex rotated A's refresh token since the profile was registered.
    w.fs.put(&w.auth, &a_rotated());

    let (r, phases) = w.switch(&target);
    let outcome = r.unwrap();
    assert!(outcome.codex_restarted);
    assert_eq!(w.auth_bytes().unwrap(), b());
    assert!(w.codex.state().running);
    assert_eq!(w.journal.load().unwrap(), None);
    assert!(w.fs.get(&w.journal.backup_path()).is_none());

    // Profile A now holds the rotated token, not the stale one.
    let a_id = outcome.source.unwrap().id;
    assert!(w.vault.open(&a_id).unwrap().1.ct_eq(&a_rotated()));

    assert_eq!(
        phases,
        vec![
            SwitchPhase::StoppingCodex,
            SwitchPhase::WaitingForExit,
            SwitchPhase::SavingCurrentCredentials,
            SwitchPhase::DecryptingTargetProfile,
            SwitchPhase::ReplacingCredentials,
            SwitchPhase::StartingCodex,
            SwitchPhase::Verifying,
            SwitchPhase::Completed,
        ]
    );
}

#[test]
fn switching_back_and_forth_between_three_profiles() {
    let w = World::new(true);
    let p1 = w.register(&auth_json("carol@example.com", "acct-n", "user-n", "pro", "rn"));
    let p2 = w.register(&auth_json("t@example.com", "acct-tp", "user-t", "pro", "rtp"));
    let p3 = w.register(&auth_json("t@example.com", "acct-tb", "user-t", "business", "rtb"));
    w.fs.put(&w.auth, &auth_json("carol@example.com", "acct-n", "user-n", "pro", "rn"));
    for target in [&p2, &p3, &p1, &p3] {
        w.switch(target).0.unwrap();
        let id = parse_identity(&w.auth_bytes().unwrap()).unwrap();
        assert!(id.same_account(&w.vault.metadata(target).unwrap().identity));
    }
    assert_eq!(w.vault.list().unwrap().len(), 3);
}

#[test]
fn unregistered_current_account_is_registered_before_switching() {
    let w = World::new(false);
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    let outcome = w.switch(&target).0.unwrap();
    assert!(outcome.source_registered);
    assert!(!outcome.codex_restarted, "Codex was not running, so it is not started");
    assert_eq!(w.vault.list().unwrap().len(), 2);
}

#[test]
fn already_active_target_saves_latest_and_restarts() {
    let w = World::new(true);
    let target = w.register(&a_old());
    w.fs.put(&w.auth, &a_rotated());
    let outcome = w.switch(&target).0.unwrap();
    assert!(outcome.already_active);
    assert_eq!(w.auth_bytes().unwrap(), a_rotated());
    assert!(w.vault.open(&target).unwrap().1.ct_eq(&a_rotated()));
    assert!(w.codex.state().running);
}

#[test]
fn missing_auth_json_is_supported_and_rolls_back_to_missing() {
    let w = World::new(true);
    let target = w.register(&b());
    w.codex.update(|s| s.wait_fails = true);
    let (r, _) = w.switch(&target);
    let f = r.unwrap_err();
    assert_eq!(f.rollback, RollbackStatus::Restored);
    assert_eq!(w.auth_bytes(), None);
}

#[test]
fn codex_stop_failure_changes_nothing() {
    let w = World::new(true);
    w.register(&a_old());
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.codex.update(|s| s.stop_fails = true);
    let f = w.switch(&target).0.unwrap_err();
    assert!(matches!(f.error, Error::CodexStop(_)));
    assert_eq!(f.rollback, RollbackStatus::NotNeeded);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
    assert_eq!(w.journal.load().unwrap(), None);
}

#[test]
fn lingering_helper_processes_abort_the_switch() {
    let w = World::new(true);
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.codex.update(|s| s.lingering_helpers = true);
    let f = w.switch(&target).0.unwrap_err();
    assert_eq!(f.phase, SwitchPhase::WaitingForExit);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
}

#[test]
fn target_decrypt_failure_is_detected_before_stopping_codex() {
    let w = World::new(true);
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.protector.fail_unprotect(true);
    let f = w.switch(&target).0.unwrap_err();
    assert!(matches!(f.error, Error::Protect(_)));
    assert_eq!(w.codex.state().stops, 0);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
}

#[test]
fn corrupted_target_profile_is_rejected() {
    let w = World::new(true);
    let target = w.register(&b());
    let path = Path::new(STATE).join("profiles").join(format!("{target}.profile"));
    w.fs.put(&path, b"{\"format\":1,");
    w.fs.put(&w.auth, &a_old());
    let f = w.switch(&target).0.unwrap_err();
    assert!(matches!(f.error, Error::ProfileCorrupted { .. }));
    assert_eq!(w.auth_bytes().unwrap(), a_old());
}

#[test]
fn auth_json_write_failure_rolls_back() {
    let w = World::new(true);
    w.register(&a_old());
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.fs.fail_writes_to_suffix(Some("auth.json"), 1);
    let f = w.switch(&target).0.unwrap_err();
    assert_eq!(f.phase, SwitchPhase::ReplacingCredentials);
    assert_eq!(f.rollback, RollbackStatus::Restored);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
    assert!(w.codex.state().running);
    assert_eq!(w.journal.load().unwrap(), None);
}

#[test]
fn codex_restart_failure_rolls_back_and_keeps_rotated_target_token() {
    let w = World::new(true);
    w.register(&a_old());
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.codex.update(|s| s.start_fails_times = 1);
    let f = w.switch(&target).0.unwrap_err();
    assert_eq!(f.phase, SwitchPhase::StartingCodex);
    assert_eq!(f.rollback, RollbackStatus::Restored);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
    assert!(w.codex.state().running, "previous account is started again");
}

#[test]
fn pending_journal_blocks_new_switch_until_recovered() {
    let w = World::new(false);
    let target = w.register(&b());
    w.journal.save(&SwitchJournal::new(target.clone(), Default::default(), false, 0)).unwrap();
    assert!(matches!(w.switch(&target).0.unwrap_err().error, Error::RecoveryPending));
    assert_eq!(recover(&w.deps()).unwrap(), RecoveryOutcome::Discarded);
    assert!(w.switch(&target).0.is_ok());
}

#[test]
fn external_clients_block_policy() {
    let w = World::new(true);
    let target = w.register(&b());
    w.fs.put(&w.auth, &a_old());
    w.codex.update(|s| s.external_clients = 2);
    let mut s = settings();
    s.external_clients = ExternalClientsPolicy::Block;
    let f = switch_account(&w.deps(), &s, &target, &mut |_| {}).unwrap_err();
    assert!(matches!(f.error, Error::ExternalClientsRunning(2)));
    assert_eq!(w.codex.state().stops, 0);
    // Warn policy proceeds and reports the count.
    let o = w.switch(&target).0.unwrap();
    assert_eq!(o.external_clients, 2);
}

#[test]
fn api_key_login_is_not_switched_away() {
    let w = World::new(false);
    let target = w.register(&b());
    w.fs.put(&w.auth, br#"{"OPENAI_API_KEY":"sk-x","tokens":null}"#);
    let f = w.switch(&target).0.unwrap_err();
    assert_eq!(f.rollback, RollbackStatus::NotNeeded);
    assert_eq!(w.auth_bytes().unwrap(), br#"{"OPENAI_API_KEY":"sk-x","tokens":null}"#);
}

#[test]
fn recovery_rolls_forward_when_target_is_on_disk() {
    let w = World::new(false);
    let target = w.register(&b());
    let mut j = SwitchJournal::new(target.clone(), w.vault.metadata(&target).unwrap().identity, true, 0);
    j.phase = JournalPhase::ReplacingCredentials;
    j.auth_existed = true;
    w.journal.save(&j).unwrap();
    w.fs.put(&w.auth, &b());
    assert_eq!(recover(&w.deps()).unwrap(), RecoveryOutcome::RolledForward { target });
    assert_eq!(recover(&w.deps()).unwrap(), RecoveryOutcome::Clean);
}

#[test]
fn recovery_restores_backup_when_auth_json_is_foreign_and_codex_stopped() {
    let w = World::new(true);
    let target = w.register(&b());
    let mut j = SwitchJournal::new(target.clone(), w.vault.metadata(&target).unwrap().identity, true, 0);
    j.phase = JournalPhase::CredentialsReplaced;
    j.auth_existed = true;
    w.journal.save(&j).unwrap();
    w.journal.write_backup(&w.protector.protect(&SecretBytes::new(a_old())).unwrap()).unwrap();
    w.fs.put(&w.auth, b"garbage");
    assert!(matches!(recover(&w.deps()), Err(Error::RecoveryBlocked(_))), "never hot-swaps under running Codex");
    w.codex.update(|s| s.running = false);
    assert_eq!(recover(&w.deps()).unwrap(), RecoveryOutcome::RolledBack);
    assert_eq!(w.auth_bytes().unwrap(), a_old());
}

#[test]
fn unreadable_journal_blocks_instead_of_guessing() {
    let w = World::new(false);
    w.fs.put(&w.journal.journal_path(), b"{nope");
    assert!(matches!(recover(&w.deps()), Err(Error::RecoveryBlocked(_))));
}

// ---------------------------------------------------------------------------------------
// Property: any single fault or crash at any file-system operation leaves auth.json as the
// complete source or the complete target, recovery always succeeds, and the source's latest
// credentials are never lost.

#[derive(Debug, Clone)]
struct Scenario {
    op_index: usize,
    crash: bool,
    source_present: bool,
    source_registered: bool,
    codex_running: bool,
    protect_fails: bool,
    start_fails: bool,
}

fn scenario() -> impl Strategy<Value = Scenario> {
    (0usize..60, any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>(), prop::bool::weighted(0.1), prop::bool::weighted(0.15)).prop_map(
        |(op_index, crash, source_present, source_registered, codex_running, protect_fails, start_fails)| Scenario {
            op_index,
            crash,
            source_present,
            source_registered,
            codex_running,
            protect_fails,
            start_fails,
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 768, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn auth_json_is_always_complete_source_or_target(s in scenario()) {
        check_invariant(&s)?;
    }
}

/// Deterministic sweep of every fault position for every flag combination, so a
/// counterexample cannot hide behind the random seed (the journal-clear ordering bug was
/// first found only on some proptest runs).
#[test]
fn every_single_fault_position_preserves_the_invariant() {
    for bits in 0u32..64 {
        for op_index in 0..60 {
            let s = Scenario {
                op_index,
                crash: bits & 1 != 0,
                source_present: bits & 2 != 0,
                source_registered: bits & 4 != 0,
                codex_running: bits & 8 != 0,
                protect_fails: bits & 16 != 0,
                start_fails: bits & 32 != 0,
            };
            if let Err(e) = check_invariant(&s) {
                panic!("{s:?}: {e}");
            }
        }
    }
}

fn check_invariant(s: &Scenario) -> Result<(), TestCaseError> {
    {
        let w = World::new(s.codex_running);
        let target = w.register(&b());
        if s.source_registered {
            w.register(&a_old());
        }
        if s.source_present {
            w.fs.put(&w.auth, &a_rotated());
        }
        let source_bytes = s.source_present.then(a_rotated);
        w.protector.fail_protect(s.protect_fails);
        w.codex.update(|c| c.start_fails_times = if s.start_fails { 1 } else { 0 });
        w.fs.fail_at_op(s.op_index, s.crash);

        let result = switch_account(&w.deps(), &settings(), &target, &mut |_| {});

        // "Reboot": durable state survives, injected faults are gone.
        w.fs.restart();
        w.protector.fail_protect(false);
        let recovered = recover(&w.deps());
        prop_assert!(recovered.is_ok(), "recovery failed: {:?}", recovered);
        prop_assert_eq!(w.journal.load().unwrap(), None);

        let on_disk = w.auth_bytes();
        let is_target = on_disk.as_deref() == Some(&b()[..]);
        let is_source = on_disk == source_bytes;
        prop_assert!(is_target || is_source, "auth.json is neither source nor target");

        if let Ok(outcome) = &result {
            if !s.crash {
                prop_assert!(outcome.already_active || is_target);
            }
        }
        if let Err(f) = &result {
            if !s.crash && !matches!(f.rollback, RollbackStatus::Failed(_)) {
                prop_assert!(is_source, "non-crash failure must leave the source in place: {:?}", f);
            }
        }

        // The source account's newest credentials are recoverable from auth.json or its profile.
        if let Some(src) = &source_bytes {
            let src_identity = parse_identity(src).unwrap();
            let profile = w.vault.find_by_identity(&src_identity).unwrap();
            let in_profile = profile.is_some_and(|m| w.vault.open(&m.id).unwrap().1.ct_eq(src));
            prop_assert!(is_source || in_profile, "latest source credentials lost");
        }
    }
    Ok(())
}
