//! Crate-wide error type.
//!
//! Messages are user-facing and must never contain credential material.
//! Only non-secret identifiers (profile ids, file names, OS error codes) may be embedded.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io { context: String, source: io::Error },
    /// DPAPI (or test protector) failure. Carries only an OS error code / short reason.
    Protect(String),
    /// Credential bytes could not be interpreted as a ChatGPT login.
    InvalidCredentials(String),
    /// Codex is configured to keep credentials somewhere other than auth.json.
    UnsupportedCredentialStore(String),
    ProfileNotFound(String),
    ProfileCorrupted { id: String, reason: String },
    CurrentProfileRemoval,
    CodexStop(String),
    CodexStart(String),
    ExternalClientsRunning(usize),
    RecoveryPending,
    RecoveryBlocked(String),
    SwitchInProgress,
    UnsafePath(String),
    Login(String),
    Config(String),
    Platform(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { context, source } => write!(f, "{context}: {source}"),
            Error::Protect(reason) => write!(f, "credential encryption failed: {reason}"),
            Error::InvalidCredentials(reason) => write!(f, "invalid Codex credentials: {reason}"),
            Error::UnsupportedCredentialStore(mode) => write!(
                f,
                "Codex is configured with cli_auth_credentials_store = \"{mode}\"; only the file store (auth.json) is supported"
            ),
            Error::ProfileNotFound(id) => write!(f, "profile {id} was not found"),
            Error::ProfileCorrupted { id, reason } => write!(f, "profile {id} is corrupted: {reason}"),
            Error::CurrentProfileRemoval => write!(f, "the profile currently used by Codex cannot be removed"),
            Error::CodexStop(reason) => write!(f, "could not stop Codex: {reason}"),
            Error::CodexStart(reason) => write!(f, "could not start Codex: {reason}"),
            Error::ExternalClientsRunning(n) => write!(
                f,
                "{n} other Codex client process(es) are using the shared credentials; close them first"
            ),
            Error::RecoveryPending => write!(f, "a previous account switch has not been recovered yet"),
            Error::RecoveryBlocked(reason) => write!(f, "recovery of the previous switch is blocked: {reason}"),
            Error::SwitchInProgress => write!(f, "another account switch is in progress"),
            Error::UnsafePath(reason) => write!(f, "refusing to use an unsafe path: {reason}"),
            Error::Login(reason) => write!(f, "Codex login failed: {reason}"),
            Error::Config(reason) => write!(f, "configuration error: {reason}"),
            Error::Platform(reason) => write!(f, "Windows API error: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
