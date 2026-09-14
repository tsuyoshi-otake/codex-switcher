// GUI subsystem: starting the tray (e.g. from the Run key) must not flash a console.
// CLI commands attach to the parent console instead.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    std::process::exit(codex_switcher::platform::windows::bootstrap::main_entry());

    #[cfg(not(windows))]
    {
        eprintln!("codex-switch supports Windows only");
        std::process::exit(1);
    }
}
