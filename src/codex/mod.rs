//! Knowledge about the installed Codex: where its home is, which processes belong to
//! Codex Desktop, and the lifecycle / login seams implemented per platform.

pub mod controller;
pub mod home;
pub mod login;
pub mod process_model;

pub use controller::{CodexController, CodexRuntime, ExternalClient, StopPolicy, StopReport};
pub use home::{CodexHome, CredentialStoreMode};
pub use login::LoginRunner;

/// MSIX package family of Codex Desktop (publisher id of OpenAI's Store packages).
pub const DEFAULT_PACKAGE_FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
/// Application id from the package's AppxManifest.xml.
pub const DEFAULT_APP_ID: &str = "App";
