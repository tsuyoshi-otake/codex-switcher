use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The shared Codex home (`CODEX_HOME`, default `%USERPROFILE%\.codex`). Only `auth.json`
/// inside it is switched; everything else stays common to all profiles.
#[derive(Clone, Debug)]
pub struct CodexHome {
    root: PathBuf,
}

impl CodexHome {
    pub fn resolve(codex_home_env: Option<OsString>, user_profile: Option<PathBuf>) -> Result<Self> {
        if let Some(v) = codex_home_env.filter(|v| !v.is_empty()) {
            return Ok(CodexHome { root: PathBuf::from(v) });
        }
        let profile = user_profile.ok_or_else(|| Error::Config("USERPROFILE is not set; cannot locate Codex home".into()))?;
        Ok(CodexHome { root: profile.join(".codex") })
    }

    pub fn at(root: PathBuf) -> Self {
        CodexHome { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn auth_file(&self) -> PathBuf {
        self.root.join("auth.json")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }
}

/// `cli_auth_credentials_store` as understood by Codex (`codex-rs/config/src/types.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialStoreMode {
    File,
    Keyring,
    Auto,
    Ephemeral,
    Unknown(String),
}

impl CredentialStoreMode {
    pub fn is_file(&self) -> bool {
        matches!(self, CredentialStoreMode::File)
    }

    pub fn name(&self) -> &str {
        match self {
            CredentialStoreMode::File => "file",
            CredentialStoreMode::Keyring => "keyring",
            CredentialStoreMode::Auto => "auto",
            CredentialStoreMode::Ephemeral => "ephemeral",
            CredentialStoreMode::Unknown(s) => s,
        }
    }
}

/// Reads the top-level `cli_auth_credentials_store` key. Absent → `File` (Codex's default).
/// Deliberately tiny: only top-level `key = "value"` lines before the first table header.
pub fn parse_credential_store_mode(config_toml: &str) -> CredentialStoreMode {
    for raw in config_toml.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            break;
        }
        let Some(rest) = line.strip_prefix("cli_auth_credentials_store") else { continue };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        let value = value.trim();
        let quote = match value.chars().next() {
            Some(q @ ('"' | '\'')) => q,
            _ => return CredentialStoreMode::Unknown(value.split('#').next().unwrap_or("").trim().to_string()),
        };
        let inner = &value[1..];
        let Some(end) = inner.find(quote) else {
            return CredentialStoreMode::Unknown(inner.to_string());
        };
        return match &inner[..end] {
            "file" => CredentialStoreMode::File,
            "keyring" => CredentialStoreMode::Keyring,
            "auto" => CredentialStoreMode::Auto,
            "ephemeral" => CredentialStoreMode::Ephemeral,
            other => CredentialStoreMode::Unknown(other.to_string()),
        };
    }
    CredentialStoreMode::File
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_env_before_profile() {
        let h = CodexHome::resolve(Some("D:/ch".into()), Some("C:/Users/x".into())).unwrap();
        assert_eq!(h.auth_file(), PathBuf::from("D:/ch").join("auth.json"));
        let h = CodexHome::resolve(Some("".into()), Some("C:/Users/x".into())).unwrap();
        assert_eq!(h.root(), Path::new("C:/Users/x").join(".codex"));
        assert!(CodexHome::resolve(None, None).is_err());
    }

    #[test]
    fn parses_store_mode() {
        assert_eq!(parse_credential_store_mode(""), CredentialStoreMode::File);
        assert_eq!(parse_credential_store_mode("model = \"x\"\ncli_auth_credentials_store = \"keyring\" # c"), CredentialStoreMode::Keyring);
        assert_eq!(parse_credential_store_mode("cli_auth_credentials_store='auto'"), CredentialStoreMode::Auto);
        assert_eq!(parse_credential_store_mode("cli_auth_credentials_store = \"file\""), CredentialStoreMode::File);
        assert_eq!(parse_credential_store_mode("# cli_auth_credentials_store = \"keyring\""), CredentialStoreMode::File);
        assert_eq!(parse_credential_store_mode("[profiles.x]\ncli_auth_credentials_store = \"keyring\""), CredentialStoreMode::File);
        assert!(matches!(parse_credential_store_mode("cli_auth_credentials_store = \"weird\""), CredentialStoreMode::Unknown(_)));
    }
}
