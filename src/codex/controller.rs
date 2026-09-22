use std::time::Duration;

use crate::error::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalClient {
    pub pid: u32,
    pub image_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexRuntime {
    pub desktop_pids: Vec<u32>,
    pub helper_pids: Vec<u32>,
    pub external_clients: Vec<ExternalClient>,
}

impl CodexRuntime {
    pub fn desktop_running(&self) -> bool {
        !self.desktop_pids.is_empty()
    }

    /// Anything owned by Desktop still alive (main process or helpers).
    pub fn any_owned_running(&self) -> bool {
        !self.desktop_pids.is_empty() || !self.helper_pids.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct StopPolicy {
    /// Time allowed after asking Desktop windows to close.
    pub graceful_timeout: Duration,
    /// Terminate remaining Desktop-owned processes after the graceful timeout.
    pub allow_force: bool,
    pub force_timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopReport {
    pub was_running: bool,
    pub forced: bool,
}

/// Lifecycle of Codex Desktop. Implementations must only ever act on processes that
/// run from the Codex Desktop installation or its dedicated runtime cache with verified
/// Desktop ancestry. User applications/shells must survive, including those launched by
/// Desktop or carrying inherited package identity. Unknown images/timestamps are not targets.
pub trait CodexController: Send + Sync {
    fn inspect(&self) -> Result<CodexRuntime>;

    /// Stops Desktop and its helpers. `Ok` means no Desktop-owned process remains.
    fn stop(&self, policy: &StopPolicy) -> Result<StopReport>;

    fn start(&self) -> Result<()>;

    fn wait_until_running(&self, timeout: Duration) -> Result<()>;
}
