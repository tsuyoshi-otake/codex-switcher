//! Account profiles: identity metadata plus DPAPI-encrypted credential blobs.
//!
//! * [`model`] – ids, metadata, display labels (pure).
//! * [`repository`] – on-disk format, corruption detection, path-safe file naming.
//! * [`vault`] – encrypt/decrypt + identity-based upsert; the only place that pairs
//!   plaintext credentials with profiles.

pub mod model;
pub mod repository;
pub mod vault;

pub use model::{plan_label, ProfileId, ProfileMetadata, StoredProfile};
pub use repository::{ProfileEntry, ProfileRepository};
pub use vault::ProfileVault;
