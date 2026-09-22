//! Tray presentation. [`menu`] is a pure model built from the service [`Overview`];
//! the Win32 host (`platform::windows::tray_host`) renders it and dispatches commands
//! back to the service. Nothing here touches files, DPAPI, or processes.
//!
//! [`Overview`]: crate::app::Overview

pub mod menu;

pub use menu::{build_menu, tooltip, Command, MenuEntry};
