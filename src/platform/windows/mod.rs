//! Win32 implementations. Everything `unsafe` in the crate lives below this module.

pub mod acl;
pub mod ambient;
pub mod bootstrap;
pub mod codex_controller;
pub mod console;
pub mod dpapi;
pub mod fs;
pub mod icon;
pub mod login_cli;
pub mod process;
pub mod settings_window;
pub mod single_instance;
pub mod startup;
pub mod tray_host;
pub mod wide;
