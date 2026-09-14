use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

/// The binary uses the GUI subsystem (no console flash for the tray); CLI commands
/// attach to the console of the launching shell so output is visible.
pub fn attach_parent_console() -> bool {
    // SAFETY: no preconditions.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}
