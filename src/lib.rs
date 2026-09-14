//! codex-switcher: switch the OpenAI Codex Desktop account from the Windows tray.
//!
//! Dependency direction (arrows = "uses"):
//!
//! ```text
//! tray / cli ──► app (SwitcherService) ──► switch ──► profile ──► auth
//!                        │                   │
//!                        └──► codex (traits) ◄┘
//! platform::windows implements fsio::FileStore, auth::SecretProtector,
//! codex::CodexController, codex::LoginRunner and the tray host; main wires it up.
//! ```

#[macro_use]
pub mod logging;

pub mod ambient;
pub mod app;
pub mod auth;
pub mod cli;
pub mod codex;
pub mod config;
pub mod error;
pub mod fsio;
pub mod profile;
pub mod switch;
pub mod tray;

#[cfg(windows)]
pub mod platform;

#[cfg(test)]
pub mod testing;
