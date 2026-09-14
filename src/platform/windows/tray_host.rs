//! Win32 notification-area host: renders [`crate::tray::menu`] and runs service calls on
//! worker threads so the UI never blocks during a switch or login.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, IsDialogMessageW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetTimer, TrackPopupMenuEx, TranslateMessage, HICON, HMENU,
    IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_OK, MB_YESNO, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MSG, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WM_TIMER,
    WNDCLASSW, WS_OVERLAPPED,
};

use super::wide::{copy_to_fixed, last_error, wide};
use super::{icon, settings_window};
use crate::app::messages::{describe_error, describe_failure, describe_outcome, describe_recovery, describe_saved_profile};
use crate::app::SwitcherService;
use crate::error::Result;
use crate::profile::{ProfileId, ProfileMetadata};
use crate::switch::{RecoveryOutcome, SwitchFailure, SwitchOutcome};
use crate::tray::{build_menu, tooltip, Command, MenuEntry};

const WM_TRAY: u32 = WM_APP + 1;
const WM_WORKER_DONE: u32 = WM_APP + 2;
const TRAY_UID: u32 = 1;
const REFRESH_TIMER: usize = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
pub const APP_TITLE: &str = "Codex Account Switcher";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

enum WorkerResult {
    Switched(std::result::Result<SwitchOutcome, SwitchFailure>),
    /// `add` or `import`.
    Saved(Result<(ProfileMetadata, bool)>),
    Recovered(Result<RecoveryOutcome>),
}

struct TrayState {
    service: Arc<SwitcherService>,
    icon_idle: HICON,
    icon_busy: HICON,
    busy: bool,
}

thread_local! {
    static STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// Short, non-reentrant access. Never call anything that pumps messages inside `f`.
fn with_state<R>(f: impl FnOnce(&mut TrayState) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// `hwnd` is only an owner-window handle forwarded to USER32 (null is valid); never dereferenced.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn message_box(hwnd: HWND, text: &str, style: u32) -> i32 {
    let (text, title) = (wide(text), wide(APP_TITLE));
    // SAFETY: NUL-terminated strings.
    unsafe { MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), style) }
}

pub fn run(service: Arc<SwitcherService>) -> Result<()> {
    // SAFETY: standard window-class registration and window creation on this thread.
    unsafe {
        let hinstance = GetModuleHandleW(null());
        let class = wide("CodexAccountSwitcherTray");
        let mut wc: WNDCLASSW = zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        if RegisterClassW(&wc) == 0 {
            return Err(last_error("RegisterClassW"));
        }
        settings_window::register_class(hinstance)?;

        let title = wide(APP_TITLE);
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, null_mut(), null_mut(), hinstance, null());
        if hwnd.is_null() {
            return Err(last_error("CreateWindowExW"));
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);

        let state = TrayState { service, icon_idle: icon::create(false), icon_busy: icon::create(true), busy: false };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        add_icon(hwnd);
        SetTimer(hwnd, REFRESH_TIMER, 5_000, None);

        // Finish or roll back a switch interrupted by a crash / power loss.
        spawn_worker(hwnd, |svc| WorkerResult::Recovered(svc.recover()));

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let dialog = settings_window::current();
            if !dialog.is_null() && IsDialogMessageW(dialog, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: plain data structure.
    let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid
}

fn current_tip_and_icon() -> Option<(String, HICON)> {
    let (service, busy, icon) = with_state(|s| (s.service.clone(), s.busy, if s.busy { s.icon_busy } else { s.icon_idle }))?;
    let overview = service.overview();
    if let Err(e) = &overview {
        log_debug!("overview failed: {e}");
    }
    Some((tooltip(overview.as_ref().ok(), busy), icon))
}

fn add_icon(hwnd: HWND) {
    let Some((tip, icon)) = current_tip_and_icon() else { return };
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    copy_to_fixed(&mut nid.szTip, &tip);
    for attempt in 0..10 {
        // SAFETY: nid is fully initialised.
        if unsafe { Shell_NotifyIconW(NIM_ADD, &nid) } != 0 {
            break;
        }
        // Explorer may not be ready yet right after sign-in.
        log_debug!("Shell_NotifyIconW(NIM_ADD) attempt {attempt} failed");
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: as above.
    unsafe { Shell_NotifyIconW(NIM_SETVERSION, &nid) };
}

fn refresh_icon(hwnd: HWND) {
    let Some((tip, icon)) = current_tip_and_icon() else { return };
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.hIcon = icon;
    copy_to_fixed(&mut nid.szTip, &tip);
    // SAFETY: nid is fully initialised.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn balloon(hwnd: HWND, title: &str, text: &str, error: bool) {
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_INFO;
    copy_to_fixed(&mut nid.szInfoTitle, title);
    copy_to_fixed(&mut nid.szInfo, text);
    nid.dwInfoFlags = if error { NIIF_ERROR } else { NIIF_INFO };
    // SAFETY: nid is fully initialised.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

/// Success notifications honour the setting; failures are always shown.
fn notify(hwnd: HWND, service: &SwitcherService, title: &str, text: &str, error: bool) {
    if error || service.settings().notifications {
        balloon(hwnd, title, text, error);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = (lparam as u32) & 0xFFFF;
            if matches!(event, WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT) {
                show_menu(hwnd);
            }
            0
        }
        WM_WORKER_DONE => {
            // SAFETY: pointer produced by Box::into_raw in spawn_worker, delivered exactly once.
            let result = unsafe { Box::from_raw(lparam as *mut WorkerResult) };
            on_worker_done(hwnd, *result);
            0
        }
        WM_TIMER => {
            refresh_icon(hwnd);
            0
        }
        WM_DESTROY => {
            let nid = icon_data(hwnd);
            // SAFETY: removing our own icon, then ending the message loop.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
            }
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_icon(hwnd);
            0
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn append_entries(menu: HMENU, entries: &[MenuEntry], commands: &mut Vec<Command>) {
    for entry in entries {
        match entry {
            MenuEntry::Separator => {
                // SAFETY: valid menu handle.
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, null()) };
            }
            MenuEntry::Item(item) => {
                let id = match &item.command {
                    Some(c) => {
                        commands.push(c.clone());
                        commands.len()
                    }
                    None => 0,
                };
                let mut flags = MF_STRING;
                if item.checked {
                    flags |= MF_CHECKED;
                }
                if !item.enabled || item.command.is_none() {
                    flags |= MF_GRAYED;
                }
                let label = wide(&item.label);
                // SAFETY: valid menu handle and label.
                unsafe { AppendMenuW(menu, flags, id, label.as_ptr()) };
            }
            MenuEntry::Submenu { label, entries } => {
                // SAFETY: the submenu is owned by (and destroyed with) the parent menu.
                unsafe {
                    let sub = CreatePopupMenu();
                    append_entries(sub, entries, commands);
                    let label = wide(label);
                    AppendMenuW(menu, MF_POPUP | MF_STRING, sub as usize, label.as_ptr());
                }
            }
        }
    }
}

fn show_menu(hwnd: HWND) {
    let Some((service, busy)) = with_state(|s| (s.service.clone(), s.busy)) else { return };
    let overview = service.overview();
    if let Err(e) = &overview {
        log_warn!("overview failed: {e}");
    }
    let entries = build_menu(overview.as_ref().ok(), busy);
    let mut commands = Vec::new();
    // SAFETY: menu is created, tracked modally, and destroyed here.
    let selected = unsafe {
        let menu = CreatePopupMenu();
        append_entries(menu, &entries, &mut commands);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let id = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, hwnd, null());
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        id
    };
    if selected > 0 {
        if let Some(cmd) = commands.get(selected as usize - 1).cloned() {
            dispatch(hwnd, &service, cmd);
        }
    }
}

fn dispatch(hwnd: HWND, service: &Arc<SwitcherService>, cmd: Command) {
    match cmd {
        Command::SwitchTo(id) => spawn_worker(hwnd, move |svc| WorkerResult::Switched(svc.switch_to(&id, &mut |_| {}))),
        Command::AddAccount => {
            notify(hwnd, service, APP_TITLE, "ブラウザで追加するアカウントにサインインしてください。", false);
            spawn_worker(hwnd, |svc| WorkerResult::Saved(svc.add_account()));
        }
        Command::ImportCurrent => spawn_worker(hwnd, |svc| WorkerResult::Saved(svc.import_current())),
        Command::Recover => spawn_worker(hwnd, |svc| WorkerResult::Recovered(svc.recover())),
        Command::OpenCodex => {
            if let Err(e) = service.open_codex() {
                notify(hwnd, service, APP_TITLE, &describe_error(&e), true);
            }
        }
        Command::Settings => settings_window::open(service.clone()),
        Command::RemoveProfile(id) => confirm_remove(hwnd, service, id),
        Command::RemoveCorrupted(raw) => match ProfileId::parse(&raw) {
            Ok(id) => confirm_remove(hwnd, service, id),
            Err(e) => notify(hwnd, service, APP_TITLE, &describe_error(&e), true),
        },
        Command::Quit => {
            if with_state(|s| s.busy).unwrap_or(false) {
                message_box(hwnd, "切り替えやログインの処理中は終了できません。", MB_OK | MB_ICONWARNING);
            } else {
                // SAFETY: destroying our own window.
                unsafe { DestroyWindow(hwnd) };
            }
        }
    }
}

fn confirm_remove(hwnd: HWND, service: &SwitcherService, id: ProfileId) {
    let label = service
        .overview()
        .ok()
        .and_then(|ov| ov.accounts.into_iter().find(|a| a.id == id).map(|a| format!("{} / {}", a.email, a.plan)))
        .unwrap_or_else(|| id.to_string());
    let text = format!("このプロファイルを削除しますか？\n\n{label}\n\n保存されている暗号化済みの認証情報も削除されます。");
    if message_box(hwnd, &text, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES {
        return;
    }
    match service.remove_profile(&id) {
        Ok(()) => notify(hwnd, service, APP_TITLE, &format!("削除しました: {label}"), false),
        Err(e) => notify(hwnd, service, APP_TITLE, &describe_error(&e), true),
    }
    refresh_icon(hwnd);
}

fn spawn_worker(hwnd: HWND, job: impl FnOnce(&SwitcherService) -> WorkerResult + Send + 'static) {
    let started = with_state(|s| {
        if s.busy {
            None
        } else {
            s.busy = true;
            Some(s.service.clone())
        }
    });
    let Some(Some(service)) = started else { return };
    refresh_icon(hwnd);
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let result = Box::into_raw(Box::new(job(&service)));
        // SAFETY: ownership of `result` passes to the UI thread if the post succeeds.
        unsafe {
            if PostMessageW(target as HWND, WM_WORKER_DONE, 0, result as LPARAM) == 0 {
                drop(Box::from_raw(result));
            }
        }
    });
}

fn on_worker_done(hwnd: HWND, result: WorkerResult) {
    let Some(service) = with_state(|s| {
        s.busy = false;
        s.service.clone()
    }) else {
        return;
    };
    match result {
        WorkerResult::Switched(Ok(o)) => notify(hwnd, &service, "Codex アカウント", &describe_outcome(&o), false),
        WorkerResult::Switched(Err(f)) => notify(hwnd, &service, "切り替えに失敗しました", &describe_failure(&f), true),
        WorkerResult::Saved(Ok((meta, created))) => {
            notify(hwnd, &service, APP_TITLE, &describe_saved_profile(&meta, created), false)
        }
        WorkerResult::Saved(Err(e)) | WorkerResult::Recovered(Err(e)) => {
            notify(hwnd, &service, APP_TITLE, &describe_error(&e), true)
        }
        WorkerResult::Recovered(Ok(outcome)) => {
            if let Some(text) = describe_recovery(&outcome) {
                notify(hwnd, &service, APP_TITLE, text, false);
            }
        }
    }
    refresh_icon(hwnd);
}
