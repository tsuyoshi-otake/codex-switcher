//! Composition root: wires Win32 implementations into the service and picks tray or CLI.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK};

use super::acl::AclProbe;
use super::ambient::SystemAmbient;
use super::codex_controller::WindowsCodexController;
use super::dpapi::DpapiProtector;
use super::fs::WindowsFileStore;
use super::login_cli::CodexCliLogin;
use super::startup::RunKeyStartup;
use super::tray_host::message_box;
use super::{console, single_instance, tray_host};
use crate::ambient::Ambient;
use crate::app::messages::describe_error;
use crate::app::{ServiceDeps, SwitcherService};
use crate::cli::{self, CliCommand};
use crate::codex::CodexHome;
use crate::config::{AppPaths, Settings};
use crate::error::Result;
use crate::fsio::FileStore;

pub fn main_entry() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&args) {
        Ok(cmd) => cmd,
        Err(msg) => {
            console::attach_parent_console();
            eprintln!("error: {msg}\n\n{}", cli::USAGE);
            return 2;
        }
    };
    let tray = cmd == CliCommand::Tray;
    if !tray {
        console::attach_parent_console();
    }

    let service = match build_service(!tray) {
        Ok(s) => s,
        Err(e) => {
            if tray {
                message_box(std::ptr::null_mut(), &describe_error(&e), MB_OK | MB_ICONERROR);
            } else {
                eprintln!("error: {e}");
            }
            return 1;
        }
    };
    log_info!("codex-switch {} started ({})", env!("CARGO_PKG_VERSION"), if tray { "tray" } else { "cli" });

    if !tray {
        return cli::run(&cmd, &service, &mut io::stdout(), &mut io::stderr());
    }
    let _guard = match single_instance::acquire(single_instance::TRAY_MUTEX) {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            message_box(std::ptr::null_mut(), "Codex Account Switcher は既に起動しています。", MB_OK | MB_ICONINFORMATION);
            return 0;
        }
        Err(e) => {
            message_box(std::ptr::null_mut(), &describe_error(&e), MB_OK | MB_ICONERROR);
            return 1;
        }
    };
    if let Err(e) = service.apply_default_autostart() {
        log_warn!("default autostart registration failed: {e}");
    }
    match tray_host::run(Arc::new(service)) {
        Ok(()) => 0,
        Err(e) => {
            log_error!("tray failed: {e}");
            message_box(std::ptr::null_mut(), &describe_error(&e), MB_OK | MB_ICONERROR);
            1
        }
    }
}

fn build_service(cli_mode: bool) -> Result<SwitcherService> {
    let paths = AppPaths::resolve(std::env::var_os("LOCALAPPDATA"))?;
    let codex_home = CodexHome::resolve(std::env::var_os("CODEX_HOME"), std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    let ambient: Arc<dyn Ambient> = Arc::new(SystemAmbient);
    let fs: Arc<dyn FileStore> = Arc::new(WindowsFileStore::new(ambient.clone()));

    // Controller and login runner are configured from settings before the service exists.
    let initial = Settings::load(fs.as_ref(), &paths.config_file()).unwrap_or_default();
    crate::logging::init(paths.log_file(), initial.log_level);

    Ok(SwitcherService::new(ServiceDeps {
        protector: Arc::new(DpapiProtector),
        codex: Arc::new(WindowsCodexController::new(initial.codex_package_family.clone(), initial.codex_app_id.clone())),
        login: Arc::new(CodexCliLogin::new(initial.codex_cli_path.clone(), cli_mode)),
        startup: Arc::new(RunKeyStartup::for_current_exe()?),
        permissions: Arc::new(AclProbe),
        fs,
        ambient,
        paths,
        codex_home,
    }))
}
