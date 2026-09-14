use std::path::Path;

use crate::error::Result;

/// "Start with Windows" registration (HKCU Run value on Windows).
pub trait StartupRegistration: Send + Sync {
    fn is_enabled(&self) -> Result<bool>;
    fn set_enabled(&self, enabled: bool) -> Result<()>;
}

/// Inspects file permissions of the credential file.
pub trait PermissionProbe: Send + Sync {
    /// Broad principals (Everyone, Users, Authenticated Users, …) that can read `path`.
    fn broad_readers(&self, path: &Path) -> Result<Vec<String>>;
}
