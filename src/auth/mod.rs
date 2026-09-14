//! Credential handling: opaque secret bytes, read-only identity extraction, encryption seam.
//!
//! The switcher never rewrites Codex's auth.json schema. Credentials travel as raw
//! bytes ([`SecretBytes`]); only [`identity::parse_identity`] looks inside, read-only,
//! to obtain display metadata.

pub mod identity;
pub mod jwt;
pub mod protector;
pub mod secret;

pub use identity::{parse_identity, AccountIdentity};
pub use protector::SecretProtector;
pub use secret::SecretBytes;
