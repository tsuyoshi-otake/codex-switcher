//! User settings (`config.json`) and application paths.

pub mod paths;

pub use paths::AppPaths;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::codex::{StopPolicy, DEFAULT_APP_ID, DEFAULT_PACKAGE_FAMILY};
use crate::error::{Error, Result};
use crate::fsio::FileStore;
use crate::switch::{ExternalClientsPolicy, SwitchSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl LogLevel {
    pub const ALL: [LogLevel; 4] = [LogLevel::Error, LogLevel::Warn, LogLevel::Info, LogLevel::Debug];

    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    pub fn from_u8(v: u8) -> LogLevel {
        match v {
            0 => LogLevel::Error,
            1 => LogLevel::Warn,
            2 => LogLevel::Info,
            _ => LogLevel::Debug,
        }
    }
}

/// Persisted settings. Whether autostart is on is not stored here: the registry Run value
/// is the source of truth so the two can never disagree. Only the one-time "default on"
/// marker is kept, so a user who turns autostart off is never re-enabled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub restart_codex_after_switch: bool,
    pub notifications: bool,
    pub log_level: LogLevel,
    pub graceful_stop_timeout_secs: u64,
    pub force_stop: bool,
    pub force_stop_timeout_secs: u64,
    pub start_timeout_secs: u64,
    pub login_timeout_secs: u64,
    pub external_clients_policy: ExternalClientsPolicy,
    pub codex_package_family: String,
    pub codex_app_id: String,
    /// Explicit codex.exe used for "Add account"; default: newest bundled CLI, then PATH.
    pub codex_cli_path: Option<PathBuf>,
    /// Set once the tray has registered autostart on its first run (default on).
    pub autostart_default_applied: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            restart_codex_after_switch: true,
            notifications: true,
            log_level: LogLevel::Info,
            graceful_stop_timeout_secs: 10,
            force_stop: true,
            force_stop_timeout_secs: 5,
            start_timeout_secs: 30,
            login_timeout_secs: 600,
            external_clients_policy: ExternalClientsPolicy::Warn,
            codex_package_family: DEFAULT_PACKAGE_FAMILY.to_string(),
            codex_app_id: DEFAULT_APP_ID.to_string(),
            codex_cli_path: None,
            autostart_default_applied: false,
        }
    }
}

impl Settings {
    /// Missing file → defaults. Unparseable file → `Error::Config` (caller decides).
    pub fn load(fs: &dyn FileStore, path: &Path) -> Result<Settings> {
        let Some(bytes) = fs.read(path).map_err(|e| Error::io("reading settings", e))? else {
            return Ok(Settings::default());
        };
        let parsed: Settings = serde_json::from_slice(&bytes).map_err(|e| Error::Config(format!("config.json: {e}")))?;
        Ok(parsed.normalized())
    }

    pub fn save(&self, fs: &dyn FileStore, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        if let Some(dir) = path.parent() {
            fs.create_dir_all(dir).map_err(|e| Error::io("creating settings directory", e))?;
        }
        fs.write_atomic(path, &bytes).map_err(|e| Error::io("writing settings", e))
    }

    pub fn normalized(mut self) -> Settings {
        self.graceful_stop_timeout_secs = self.graceful_stop_timeout_secs.clamp(1, 300);
        self.force_stop_timeout_secs = self.force_stop_timeout_secs.clamp(1, 300);
        self.start_timeout_secs = self.start_timeout_secs.clamp(5, 600);
        self.login_timeout_secs = self.login_timeout_secs.clamp(60, 3600);
        let valid_family = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
        if !valid_family(&self.codex_package_family) {
            self.codex_package_family = DEFAULT_PACKAGE_FAMILY.to_string();
        }
        if !valid_family(&self.codex_app_id) {
            self.codex_app_id = DEFAULT_APP_ID.to_string();
        }
        self
    }

    pub fn switch_settings(&self) -> SwitchSettings {
        SwitchSettings {
            stop_policy: StopPolicy {
                graceful_timeout: Duration::from_secs(self.graceful_stop_timeout_secs),
                allow_force: self.force_stop,
                force_timeout: Duration::from_secs(self.force_stop_timeout_secs),
            },
            restart_codex: self.restart_codex_after_switch,
            start_timeout: Duration::from_secs(self.start_timeout_secs),
            external_clients: self.external_clients_policy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MemFs;

    #[test]
    fn missing_file_gives_defaults_and_roundtrips() {
        let fs = MemFs::new();
        let path = Path::new("C:/x/config.json");
        assert_eq!(Settings::load(&fs, path).unwrap(), Settings::default());
        let s = Settings { notifications: false, log_level: LogLevel::Debug, ..Settings::default() };
        s.save(&fs, path).unwrap();
        assert_eq!(Settings::load(&fs, path).unwrap(), s);
    }

    #[test]
    fn partial_file_uses_defaults_for_missing_keys_and_clamps() {
        let fs = MemFs::new();
        let path = Path::new("C:/x/config.json");
        fs.put(path, br#"{"graceful_stop_timeout_secs":0,"codex_package_family":"..\\evil path"}"#);
        let s = Settings::load(&fs, path).unwrap();
        assert_eq!(s.graceful_stop_timeout_secs, 1);
        assert!(s.restart_codex_after_switch);
        assert_eq!(s.codex_package_family, DEFAULT_PACKAGE_FAMILY);
    }

    #[test]
    fn broken_file_is_a_config_error() {
        let fs = MemFs::new();
        let path = Path::new("C:/x/config.json");
        fs.put(path, b"{");
        assert!(matches!(Settings::load(&fs, path), Err(Error::Config(_))));
    }
}
