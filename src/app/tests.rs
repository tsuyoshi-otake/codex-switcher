use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::codex::{CodexHome, CredentialStoreMode};
use crate::config::AppPaths;
use crate::error::Error;
use crate::fsio::FileStore;
use crate::testing::fixtures::auth_json;
use crate::testing::{FakeCodex, FakeLogin, FakePermissions, FakeProtector, FakeStartup, FixedAmbient, MemFs};

const ROOT: &str = "C:/Users/u/AppData/Local/CodexAccountSwitcher";
const HOME: &str = "C:/Users/u/.codex";

struct World {
    fs: Arc<MemFs>,
    codex: Arc<FakeCodex>,
    login: Arc<FakeLogin>,
    permissions: Arc<FakePermissions>,
    service: SwitcherService,
}

fn world_with(setup: impl FnOnce(&MemFs)) -> World {
    let fs = Arc::new(MemFs::new());
    setup(&fs);
    let codex = Arc::new(FakeCodex::new(true));
    let login = Arc::new(FakeLogin::new(fs.clone()));
    let permissions = Arc::new(FakePermissions::default());
    let service = SwitcherService::new(ServiceDeps {
        fs: fs.clone(),
        protector: Arc::new(FakeProtector::default()),
        codex: codex.clone(),
        login: login.clone(),
        startup: Arc::new(FakeStartup::default()),
        permissions: permissions.clone(),
        ambient: Arc::new(FixedAmbient::default()),
        paths: AppPaths::at(PathBuf::from(ROOT)),
        codex_home: CodexHome::at(PathBuf::from(HOME)),
    });
    World { fs, codex, login, permissions, service }
}

fn world() -> World {
    world_with(|_| {})
}

fn auth_path() -> PathBuf {
    CodexHome::at(PathBuf::from(HOME)).auth_file()
}

fn personal() -> Vec<u8> {
    auth_json("t@example.com", "acct-personal", "user-t", "pro", "rt1")
}
fn business() -> Vec<u8> {
    auth_json("t@example.com", "acct-business", "user-t", "business", "rt2")
}

#[test]
fn overview_distinguishes_same_email_accounts_and_marks_active() {
    let w = world();
    w.login.produce(Some(business()));
    w.service.add_account().unwrap();
    w.fs.put(&auth_path(), &personal());
    w.service.import_current().unwrap();

    let ov = w.service.overview().unwrap();
    assert_eq!(ov.accounts.len(), 2);
    let active: Vec<_> = ov.accounts.iter().filter(|a| a.active).collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].plan, "Personal / Pro");
    assert_eq!(ov.active_label().unwrap(), "t@example.com / Personal / Pro");
    assert!(ov.accounts.iter().any(|a| a.plan == "Business" && !a.active));
}

#[test]
fn missing_codex_home_and_auth_json_are_reported_not_errors() {
    let w = world();
    let ov = w.service.overview().unwrap();
    assert_eq!(ov.auth, AuthState::Missing);
    assert!(ov.accounts.is_empty());
    assert_eq!(ov.store_mode, CredentialStoreMode::File);
    assert!(matches!(w.service.import_current(), Err(Error::InvalidCredentials(_))));
}

#[test]
fn add_account_scrubs_login_staging_on_success_and_failure() {
    let w = world();
    w.login.produce(None);
    assert!(matches!(w.service.add_account(), Err(Error::Login(_))));
    let staging = w.service.paths().login_staging_dir();
    assert!(w.fs.list(&staging).unwrap().is_empty(), "plaintext left behind after failed login");

    w.login.produce(Some(personal()));
    let (meta, created) = w.service.add_account().unwrap();
    assert!(created);
    assert_eq!(meta.identity.account_id.as_deref(), Some("acct-personal"));
    assert!(w.fs.list(&staging).unwrap().is_empty(), "plaintext left behind after login");
    assert_eq!(w.login.calls(), vec![staging.clone(), staging]);
    // The live auth.json was never touched and Codex was not stopped.
    assert_eq!(w.fs.get(&auth_path()), None);
    assert_eq!(w.codex.state().stops, 0);
}

#[test]
fn current_profile_cannot_be_removed_but_others_can() {
    let w = world();
    w.fs.put(&auth_path(), &personal());
    let (current, _) = w.service.import_current().unwrap();
    w.login.produce(Some(business()));
    let (other, _) = w.service.add_account().unwrap();

    assert!(matches!(w.service.remove_profile(&current.id), Err(Error::CurrentProfileRemoval)));
    w.service.remove_profile(&other.id).unwrap();
    assert_eq!(w.service.overview().unwrap().accounts.len(), 1);
}

#[test]
fn switch_through_service_updates_active_account() {
    let w = world();
    w.fs.put(&auth_path(), &personal());
    w.service.import_current().unwrap();
    w.login.produce(Some(business()));
    let (target, _) = w.service.add_account().unwrap();

    w.service.switch_to(&target.id, &mut |_| {}).unwrap();
    let ov = w.service.overview().unwrap();
    assert_eq!(ov.active_profile, Some(target.id));
    assert!(w.codex.state().running);
}

#[test]
fn keyring_store_is_refused_before_stopping_codex() {
    let w = world_with(|fs| fs.put(&Path::new(HOME).join("config.toml"), b"cli_auth_credentials_store = \"keyring\"\n"));
    w.login.produce(Some(business()));
    let (target, _) = w.service.add_account().unwrap();
    let f = w.service.switch_to(&target.id, &mut |_| {}).unwrap_err();
    assert!(matches!(f.error, Error::UnsupportedCredentialStore(ref m) if m == "keyring"));
    assert_eq!(w.codex.state().stops, 0);
    assert!(!w.service.overview().unwrap().can_switch());
}

#[test]
fn concurrent_switch_is_rejected() {
    let w = world();
    let _held = w.fs.try_lock(&w.service.paths().switch_lock()).unwrap().unwrap();
    let id = crate::profile::ProfileId::from_random_bytes([7; 16]);
    let f = w.service.switch_to(&id, &mut |_| {}).unwrap_err();
    assert!(matches!(f.error, Error::SwitchInProgress));
}

#[test]
fn broken_settings_fall_back_to_defaults_with_warning() {
    let w = world_with(|fs| fs.put(&Path::new(ROOT).join("config.json"), b"{broken"));
    assert!(w.service.settings().restart_codex_after_switch);
    assert_eq!(w.service.overview().unwrap().warnings.len(), 1);
    let s = w.service.update_settings(|s| s.notifications = false).unwrap();
    assert!(!s.notifications);
}

#[test]
fn broad_auth_json_permissions_are_warned() {
    let w = world();
    w.fs.put(&auth_path(), &personal());
    w.permissions.set(vec!["Everyone".into()]);
    let warnings = w.service.overview().unwrap().warnings;
    assert!(warnings.iter().any(|x| x.contains("Everyone")));
}

#[test]
fn autostart_is_on_by_default_once_and_a_later_opt_out_sticks() {
    let w = world();
    assert!(!w.service.autostart_enabled().unwrap());
    assert!(w.service.apply_default_autostart().unwrap());
    assert!(w.service.autostart_enabled().unwrap());

    w.service.set_autostart(false).unwrap();
    assert!(!w.service.apply_default_autostart().unwrap());
    assert!(!w.service.autostart_enabled().unwrap(), "user opt-out must not be overridden");
    let saved = crate::config::Settings::load(w.fs.as_ref(), &w.service.paths().config_file()).unwrap();
    assert!(saved.autostart_default_applied);
}

#[test]
fn startup_recovery_scrubs_stale_login_staging() {
    let w = world();
    let stale = w.service.paths().login_staging_dir().join("auth.json");
    w.fs.put(&stale, &personal());
    w.service.recover().unwrap();
    assert_eq!(w.fs.get(&stale), None);
}
