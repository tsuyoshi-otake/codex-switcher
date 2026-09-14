//! Minimal settings window: autostart, restart after switch, notifications, log level.

use std::cell::RefCell;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT, HBRUSH};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetDlgItem, GetSystemMetrics, LoadCursorW, RegisterClassW,
    SendMessageW, SetForegroundWindow, ShowWindow, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, CBS_DROPDOWNLIST, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, HMENU, IDC_ARROW, MB_ICONERROR, MB_OK,
    SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD,
    WS_EX_DLGMODALFRAME, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use super::tray_host::{message_box, APP_TITLE};
use super::wide::{last_error, wide};
use crate::app::messages::describe_error;
use crate::app::SwitcherService;
use crate::config::LogLevel;
use crate::error::Result;

const CLASS: &str = "CodexAccountSwitcherSettings";
const ID_OK: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_AUTOSTART: i32 = 101;
const ID_RESTART: i32 = 102;
const ID_NOTIFY: i32 = 103;
const ID_LOG_LEVEL: i32 = 104;
const COLOR_BTNFACE: usize = 15;

struct Dialog {
    hwnd: HWND,
    service: Arc<SwitcherService>,
    autostart_initial: bool,
}

thread_local! {
    static DIALOG: RefCell<Option<Dialog>> = const { RefCell::new(None) };
}

pub fn register_class(hinstance: HINSTANCE) -> Result<()> {
    let class = wide(CLASS);
    // SAFETY: standard class registration.
    unsafe {
        let mut wc: WNDCLASSW = zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hbrBackground = (COLOR_BTNFACE + 1) as HBRUSH;
        if RegisterClassW(&wc) == 0 {
            return Err(last_error("RegisterClassW(settings)"));
        }
    }
    Ok(())
}

/// The open settings window, for `IsDialogMessageW` keyboard navigation.
pub fn current() -> HWND {
    DIALOG.with(|d| d.try_borrow().ok().and_then(|d| d.as_ref().map(|d| d.hwnd)).unwrap_or(null_mut()))
}

#[allow(clippy::too_many_arguments)] // mirrors CreateWindowExW's control geometry
fn child(parent: HWND, class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    let (class, text) = (wide(class), wide(text));
    // SAFETY: parent is a valid window; control ids are passed through the HMENU slot.
    unsafe {
        let hwnd = CreateWindowExW(0, class.as_ptr(), text.as_ptr(), WS_CHILD | WS_VISIBLE | style, x, y, w, h, parent, id as usize as HMENU, null_mut(), null());
        SendMessageW(hwnd, WM_SETFONT, GetStockObject(DEFAULT_GUI_FONT) as usize, 1);
        hwnd
    }
}

pub fn open(service: Arc<SwitcherService>) {
    let existing = current();
    if !existing.is_null() {
        // SAFETY: valid window.
        unsafe { SetForegroundWindow(existing) };
        return;
    }
    let settings = service.settings();
    let autostart = service.autostart_enabled().unwrap_or(false);
    let (w, h) = (380, 230);
    let (class, title) = (wide(CLASS), wide(format!("設定 - {APP_TITLE}")));
    // SAFETY: window creation on the UI thread.
    let hwnd = unsafe {
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;
        CreateWindowExW(WS_EX_DLGMODALFRAME, class.as_ptr(), title.as_ptr(), WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU, x, y, w, h, null_mut(), null_mut(), null_mut(), null())
    };
    if hwnd.is_null() {
        log_warn!("settings window could not be created");
        return;
    }
    let check = |id: i32, text: &str, y: i32, on: bool| {
        let c = child(hwnd, "BUTTON", text, WS_TABSTOP | BS_AUTOCHECKBOX as u32, 20, y, 330, 22, id);
        // SAFETY: valid control.
        unsafe { SendMessageW(c, BM_SETCHECK, usize::from(on), 0) };
    };
    check(ID_AUTOSTART, "Windows 起動時に自動で起動する", 16, autostart);
    check(ID_RESTART, "切り替え後に Codex を再起動する", 42, settings.restart_codex_after_switch);
    check(ID_NOTIFY, "成功時に通知を表示する（失敗は常に表示）", 68, settings.notifications);
    child(hwnd, "STATIC", "ログレベル:", 0, 20, 104, 90, 20, 0);
    let combo = child(hwnd, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32, 115, 100, 120, 120, ID_LOG_LEVEL);
    for level in LogLevel::ALL {
        let name = wide(level.name());
        // SAFETY: valid control and string.
        unsafe { SendMessageW(combo, CB_ADDSTRING, 0, name.as_ptr() as LPARAM) };
    }
    // SAFETY: valid control.
    unsafe { SendMessageW(combo, CB_SETCURSEL, settings.log_level as usize, 0) };
    child(hwnd, "BUTTON", "OK", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, 170, 150, 90, 28, ID_OK);
    child(hwnd, "BUTTON", "キャンセル", WS_TABSTOP | BS_PUSHBUTTON as u32, 268, 150, 90, 28, ID_CANCEL);

    DIALOG.with(|d| *d.borrow_mut() = Some(Dialog { hwnd, service, autostart_initial: autostart }));
    // SAFETY: valid window.
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

fn is_checked(hwnd: HWND, id: i32) -> bool {
    // SAFETY: control belongs to hwnd.
    unsafe { SendMessageW(GetDlgItem(hwnd, id), BM_GETCHECK, 0, 0) == 1 }
}

fn apply(hwnd: HWND) -> bool {
    let Some((service, autostart_initial)) =
        DIALOG.with(|d| d.try_borrow().ok().and_then(|d| d.as_ref().map(|d| (d.service.clone(), d.autostart_initial))))
    else {
        return true;
    };
    let autostart = is_checked(hwnd, ID_AUTOSTART);
    let restart = is_checked(hwnd, ID_RESTART);
    let notifications = is_checked(hwnd, ID_NOTIFY);
    // SAFETY: control belongs to hwnd.
    let level_index = unsafe { SendMessageW(GetDlgItem(hwnd, ID_LOG_LEVEL), CB_GETCURSEL, 0, 0) };
    let level = LogLevel::from_u8(level_index.clamp(0, 3) as u8);

    let result = (|| {
        if autostart != autostart_initial {
            service.set_autostart(autostart)?;
        }
        service.update_settings(|s| {
            s.restart_codex_after_switch = restart;
            s.notifications = notifications;
            s.log_level = level;
        })
    })();
    match result {
        Ok(saved) => {
            crate::logging::set_level(saved.log_level);
            true
        }
        Err(e) => {
            message_box(hwnd, &describe_error(&e), MB_OK | MB_ICONERROR);
            false
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                ID_OK => {
                    if apply(hwnd) {
                        // SAFETY: own window.
                        unsafe { DestroyWindow(hwnd) };
                    }
                }
                // SAFETY: own window.
                ID_CANCEL => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            // SAFETY: own window.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            DIALOG.with(|d| {
                if let Ok(mut d) = d.try_borrow_mut() {
                    *d = None;
                }
            });
            0
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
