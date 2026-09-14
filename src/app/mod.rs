//! Application use cases shared by the tray and the CLI.
//!
//! [`SwitcherService`] is the only entry point front-ends use; it owns the vault, the
//! journal, settings, and the switch lock. Platform capabilities arrive through the
//! traits in [`ports`] and the codex/auth/fsio seams.

pub mod messages;
pub mod overview;
pub mod ports;
pub mod service;

#[cfg(test)]
mod tests;

pub use overview::{AccountView, AuthState, CorruptedView, Overview};
pub use ports::{PermissionProbe, StartupRegistration};
pub use service::{ServiceDeps, SwitcherService};
