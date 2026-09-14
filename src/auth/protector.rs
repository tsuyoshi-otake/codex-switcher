use super::SecretBytes;
use crate::error::Result;

/// Encrypts credentials at rest. Production: Windows DPAPI, current-user scope
/// (`platform::windows::dpapi::DpapiProtector`).
pub trait SecretProtector: Send + Sync {
    fn protect(&self, plaintext: &SecretBytes) -> Result<Vec<u8>>;
    fn unprotect(&self, ciphertext: &[u8]) -> Result<SecretBytes>;
}
