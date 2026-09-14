use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::overview::{build_overview, AuthState, Overview};
use super::ports::{PermissionProbe, StartupRegistration};
use crate::ambient::Ambient;
use crate::auth::{parse_identity, SecretBytes, SecretProtector};
use crate::codex::home::parse_credential_store_mode;
use crate::codex::{CodexController, CodexHome, CredentialStoreMode, LoginRunner};
use crate::config::{AppPaths, Settings};
use crate::error::{Error, Result};
use crate::fsio::{FileStore, LockGuard};
use crate::profile::{ProfileId, ProfileMetadata, ProfileRepository, ProfileVault};
use crate::switch::{
    self, JournalStore, RecoveryOutcome, RollbackStatus, SwitchDeps, SwitchFailure, SwitchOutcome, SwitchPhase,
};

pub struct ServiceDeps {
    pub fs: Arc<dyn FileStore>,
    pub protector: Arc<dyn SecretProtector>,
    pub codex: Arc<dyn CodexController>,
    pub login: Arc<dyn LoginRunner>,
    pub startup: Arc<dyn StartupRegistration>,
    pub permissions: Arc<dyn PermissionProbe>,
    pub ambient: Arc<dyn Ambient>,
    pub paths: AppPaths,
    pub codex_home: CodexHome,
}

pub struct SwitcherService {
    deps: ServiceDeps,
    vault: ProfileVault,
    journal: JournalStore,
    settings: Mutex<Settings>,
    settings_warning: Option<String>,
}

impl SwitcherService {
    pub fn new(deps: ServiceDeps) -> Self {
        let repo = ProfileRepository::new(deps.fs.clone(), deps.paths.profiles_dir());
        let vault = ProfileVault::new(repo, deps.protector.clone(), deps.ambient.clone());
        let journal = JournalStore::new(deps.fs.clone(), deps.paths.root());
        let (settings, settings_warning) = match Settings::load(deps.fs.as_ref(), &deps.paths.config_file()) {
            Ok(s) => (s, None),
            Err(e) => {
                log_warn!("settings could not be loaded, using defaults: {e}");
                (Settings::default(), Some("config.json を読み込めないため既定の設定で動作しています".to_string()))
            }
        };
        SwitcherService { deps, vault, journal, settings: Mutex::new(settings), settings_warning }
    }

    pub fn paths(&self) -> &AppPaths {
        &self.deps.paths
    }

    // ---- settings ------------------------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.settings_guard().clone()
    }

    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut guard = self.settings_guard();
        let mut next = guard.clone();
        change(&mut next);
        let next = next.normalized();
        next.save(self.deps.fs.as_ref(), &self.deps.paths.config_file())?;
        *guard = next.clone();
        Ok(next)
    }

    pub fn autostart_enabled(&self) -> Result<bool> {
        self.deps.startup.is_enabled()
    }

    pub fn set_autostart(&self, enabled: bool) -> Result<()> {
        self.deps.startup.set_enabled(enabled)
    }

    /// Autostart is on by default: the first tray run registers it once. Later runs leave
    /// the registry alone, so turning it off in the settings window sticks.
    /// Returns whether it was registered by this call.
    pub fn apply_default_autostart(&self) -> Result<bool> {
        if self.settings_guard().autostart_default_applied {
            return Ok(false);
        }
        self.deps.startup.set_enabled(true)?;
        self.update_settings(|s| s.autostart_default_applied = true)?;
        Ok(true)
    }

    fn settings_guard(&self) -> MutexGuard<'_, Settings> {
        self.settings.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- queries --------------------------------------------------------------------------

    pub fn overview(&self) -> Result<Overview> {
        let entries = self.vault.list()?;
        let auth_file = self.deps.codex_home.auth_file();
        let auth = match self.deps.fs.read(&auth_file).map_err(|e| Error::io("reading auth.json", e))? {
            None => AuthState::Missing,
            Some(bytes) => {
                let secret = SecretBytes::new(bytes);
                match parse_identity(secret.as_bytes()) {
                    Ok(identity) => AuthState::SignedIn(identity),
                    Err(_) => AuthState::Unrecognized,
                }
            }
        };
        let store_mode = self.store_mode()?;
        let recovery_pending = !matches!(self.journal.load(), Ok(None));

        let mut warnings: Vec<String> = self.settings_warning.iter().cloned().collect();
        if !store_mode.is_file() {
            warnings.push(format!("Codexの資格情報ストア「{}」には未対応のため切り替えできません", store_mode.name()));
        }
        if auth != AuthState::Missing {
            match self.deps.permissions.broad_readers(&auth_file) {
                Ok(readers) if !readers.is_empty() => {
                    warnings.push(format!("auth.json が広いグループから読み取り可能です: {}", readers.join(", ")))
                }
                Ok(_) => {}
                Err(e) => log_warn!("auth.json permission check failed: {e}"),
            }
        }
        Ok(build_overview(entries, auth, store_mode, recovery_pending, warnings))
    }

    fn store_mode(&self) -> Result<CredentialStoreMode> {
        let config = self
            .deps
            .fs
            .read(&self.deps.codex_home.config_file())
            .map_err(|e| Error::io("reading Codex config.toml", e))?;
        Ok(config.map_or(CredentialStoreMode::File, |b| parse_credential_store_mode(&String::from_utf8_lossy(&b))))
    }

    fn require_file_store(&self) -> Result<()> {
        let mode = self.store_mode()?;
        if mode.is_file() {
            Ok(())
        } else {
            Err(Error::UnsupportedCredentialStore(mode.name().to_string()))
        }
    }

    // ---- commands -------------------------------------------------------------------------

    /// Switches Codex Desktop to `target`. See [`switch::switch_account`] for guarantees.
    pub fn switch_to(
        &self,
        target: &ProfileId,
        progress: &mut dyn FnMut(SwitchPhase),
    ) -> std::result::Result<SwitchOutcome, SwitchFailure> {
        let fail = |error| SwitchFailure { error, phase: SwitchPhase::Idle, rollback: RollbackStatus::NotNeeded };
        let _lock = self.lock().map_err(fail)?;
        self.require_file_store().map_err(fail)?;
        let settings = self.settings().switch_settings();
        log_info!("switch started: target profile {target}");
        let result = self.with_switch_deps(|deps| switch::switch_account(deps, &settings, target, progress));
        match &result {
            Ok(o) => log_info!(
                "switch completed: target {} already_active={} restarted={} forced_stop={} external_clients={}",
                o.target.id,
                o.already_active,
                o.codex_restarted,
                o.forced_stop,
                o.external_clients
            ),
            Err(f) => log_warn!("switch failed at {}: {} (rollback: {:?})", f.phase, f.error, f.rollback),
        }
        result
    }

    /// Registers (or refreshes) the account currently in auth.json.
    pub fn import_current(&self) -> Result<(ProfileMetadata, bool)> {
        let _lock = self.lock()?;
        self.require_no_pending_recovery()?;
        let bytes = self
            .deps
            .fs
            .read(&self.deps.codex_home.auth_file())
            .map_err(|e| Error::io("reading auth.json", e))?
            .ok_or_else(|| Error::InvalidCredentials("Codex is not signed in (auth.json not found)".into()))?;
        let result = self.vault.upsert(&SecretBytes::new(bytes))?;
        log_info!("imported current account into profile {} (created={})", result.0.id, result.1);
        Ok(result)
    }

    /// Runs the official `codex login` against an isolated, temporary CODEX_HOME and stores
    /// the resulting credentials as a profile. The live Codex installation is not touched.
    pub fn add_account(&self) -> Result<(ProfileMetadata, bool)> {
        let _lock = self.lock()?;
        let staging = self.deps.paths.login_staging_dir();
        let fs = self.deps.fs.as_ref();
        fs.remove_dir_all(&staging).map_err(|e| Error::io("cleaning login staging directory", e))?;
        fs.create_dir_all(&staging).map_err(|e| Error::io("creating login staging directory", e))?;

        let timeout = Duration::from_secs(self.settings().login_timeout_secs);
        let result = self.deps.login.run_login(&staging, timeout).and_then(|()| {
            let bytes = fs
                .read(&staging.join("auth.json"))
                .map_err(|e| Error::io("reading login result", e))?
                .ok_or_else(|| Error::Login("sign-in finished without producing credentials".into()))?;
            self.vault.upsert(&SecretBytes::new(bytes))
        });
        let cleanup = fs.remove_dir_all(&staging);

        let (meta, created) = result?;
        cleanup.map_err(|e| Error::io("scrubbing login staging directory", e))?;
        log_info!("added account profile {} (created={})", meta.id, created);
        Ok((meta, created))
    }

    pub fn remove_profile(&self, id: &ProfileId) -> Result<()> {
        let _lock = self.lock()?;
        if self.overview()?.active_profile.as_ref() == Some(id) {
            return Err(Error::CurrentProfileRemoval);
        }
        self.vault.delete(id)?;
        log_info!("removed profile {id}");
        Ok(())
    }

    /// Completes or reverts an interrupted switch and scrubs leftovers. Call at startup.
    pub fn recover(&self) -> Result<RecoveryOutcome> {
        let _lock = self.lock()?;
        self.deps
            .fs
            .remove_dir_all(&self.deps.paths.login_staging_dir())
            .map_err(|e| Error::io("cleaning login staging directory", e))?;
        let outcome = self.with_switch_deps(switch::recover)?;
        if outcome != RecoveryOutcome::Clean {
            log_info!("recovery: {outcome:?}");
        }
        Ok(outcome)
    }

    pub fn open_codex(&self) -> Result<()> {
        self.deps.codex.start()
    }

    // ---- internals ------------------------------------------------------------------------

    fn require_no_pending_recovery(&self) -> Result<()> {
        match self.journal.load()? {
            None => Ok(()),
            Some(_) => Err(Error::RecoveryPending),
        }
    }

    fn lock(&self) -> Result<LockGuard> {
        let fs = self.deps.fs.as_ref();
        fs.create_dir_all(self.deps.paths.root()).map_err(|e| Error::io("creating state directory", e))?;
        fs.try_lock(&self.deps.paths.switch_lock())
            .map_err(|e| Error::io("acquiring switch lock", e))?
            .ok_or(Error::SwitchInProgress)
    }

    fn with_switch_deps<T>(&self, f: impl FnOnce(&SwitchDeps) -> T) -> T {
        let auth_file: PathBuf = self.deps.codex_home.auth_file();
        let deps = SwitchDeps {
            fs: self.deps.fs.as_ref(),
            vault: &self.vault,
            protector: self.deps.protector.as_ref(),
            codex: self.deps.codex.as_ref(),
            journal: &self.journal,
            auth_file: &auth_file,
            ambient: self.deps.ambient.as_ref(),
        };
        f(&deps)
    }
}
