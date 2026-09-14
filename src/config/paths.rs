use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const APP_DIR_NAME: &str = "CodexAccountSwitcher";

/// All locations under `%LOCALAPPDATA%\CodexAccountSwitcher\`.
#[derive(Clone, Debug)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn resolve(local_app_data: Option<OsString>) -> Result<Self> {
        let base = local_app_data
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| Error::Config("LOCALAPPDATA is not set".into()))?;
        if !base.is_absolute() {
            return Err(Error::UnsafePath("LOCALAPPDATA is not an absolute path".into()));
        }
        Ok(AppPaths { root: base.join(APP_DIR_NAME) })
    }

    pub fn at(root: PathBuf) -> Self {
        AppPaths { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.root.join("logs").join("codex-switch.log")
    }

    pub fn switch_lock(&self) -> PathBuf {
        self.root.join("switch.lock")
    }

    /// Temporary CODEX_HOME for the official login flow; scrubbed after every attempt and
    /// at startup.
    pub fn login_staging_dir(&self) -> PathBuf {
        self.root.join("login-staging")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_absolute_local_app_data() {
        assert!(AppPaths::resolve(None).is_err());
        assert!(AppPaths::resolve(Some("relative".into())).is_err());
        let p = AppPaths::resolve(Some(r"C:\Users\u\AppData\Local".into())).unwrap();
        assert!(p.profiles_dir().ends_with(r"CodexAccountSwitcher\profiles"));
    }
}
