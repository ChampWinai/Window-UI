//! Other apps' notification-area icons (Steam, Discord, Core Temp, ...).
//!
//! Shell_NotifyIcon sends WM_COPYDATA to the first "Shell_TrayWnd" FindWindow returns, which is the
//! highest in z-order. We register a hidden topmost window with that class, so icon updates reach us.
//! Every message is forwarded to explorer's real one: appbar calls keep working, and when we quit
//! explorer already has every icon.
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use base64::Engine;
use serde::Serialize;
use windows::core::{w, GUID, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIS_HIDDEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::win;

#[derive(Serialize, Clone)]
pub struct TrayIcon {
    key: String,
    tip: String,
    icon: Option<String>,
    #[serde(skip)]
    hwnd: isize,
    #[serde(skip)]
    uid: u32,
    #[serde(skip)]
    callback: u32,
    #[serde(skip)]
    version: u32,
    #[serde(skip)]
    hidden: bool,
}

static ICONS: Mutex<Vec<TrayIcon>> = Mutex::new(Vec::new());
static HOST: AtomicIsize = AtomicIsize::new(0);

// What shell32 actually sends: NOTIFYICONDATA with 32-bit handles (same for 32/64-bit callers).
#[repr(C)]
struct TrayData {
    signature: u32, // 0x34753423
    message: u32,   // NIM_*
    cb_size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32, // union with uTimeout
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: u32,
}

fn utf16(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// 32-bit handles are sign-extended to 64-bit, like LongToHandle.
fn handle(h: u32) -> *mut c_void {
    h as i32 as isize as *mut c_void
}

unsafe fn on_tray_message(cds: &COPYDATASTRUCT) {
    let mut d: TrayData = std::mem::zeroed();
    let len = (cds.cbData as usize).min(size_of::<TrayData>());
    std::ptr::copy_nonoverlapping(cds.lpData as *const u8, &mut d as *mut TrayData as *mut u8, len);
    if d.signature != 0x34753423 {
        return;
    }
    let key = if d.flags & NIF_GUID.0 != 0 { format!("{:?}", d.guid) } else { format!("{}:{}", d.hwnd, d.uid) };
    let mut icons = ICONS.lock().unwrap();
    let pos = icons.iter().position(|i| i.key == key);

    if d.message == NIM_DELETE.0 {
        if let Some(p) = pos {
            icons.remove(p);
        }
        return;
    }
    if d.message == NIM_SETVERSION.0 {
        if let Some(p) = pos {
            icons[p].version = d.version;
        }
        return;
    }
    let i = match (pos, d.message) {
        (Some(p), _) => &mut icons[p],
        (None, m) if m == NIM_ADD.0 => {
            icons.push(TrayIcon { key, tip: String::new(), icon: None, hwnd: 0, uid: 0, callback: 0, version: 0, hidden: false });
            icons.last_mut().unwrap()
        }
        _ => return, // MODIFY of an icon we never saw
    };
    i.hwnd = handle(d.hwnd) as isize;
    i.uid = d.uid;
    if d.flags & NIF_MESSAGE.0 != 0 {
        i.callback = d.callback;
    }
    if d.flags & NIF_ICON.0 != 0 {
        i.icon = win::hicon_png(HICON(handle(d.icon)))
            .map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)));
    }
    if d.flags & NIF_TIP.0 != 0 {
        i.tip = utf16(&d.tip);
    }    if d.flags & NIF_STATE.0 != 0 && d.state_mask & NIS_HIDDEN.0 != 0 {
        i.hidden = d.state & NIS_HIDDEN.0 != 0;
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_COPYDATA {
        // explorer talks to "its" tray window through FindWindow too (e.g. 0x4EF to recompute the
        // work area after an appbar moves); those land here and must reach the real one
        if (WM_USER..0xC000).contains(&msg) {
            if let Some(real) = win::shell_tray() {
                let mut r = 0usize;
                SendMessageTimeoutW(real, msg, wp, lp, SMTO_ABORTIFHUNG, 2000, Some(&mut r));
                return LRESULT(r as isize);
            }
        }
        return DefWindowProcW(h, msg, wp, lp);
    }
    let cds = &*(lp.0 as *const COPYDATASTRUCT);
    let is_tray = cds.dwData == 1;
    if is_tray {
        on_tray_message(cds);
    }
    // forward everything (tray icons, appbars, icon-rect queries) so explorer stays in sync
    let mut r = 0usize;
    if let Some(real) = win::shell_tray() {
        SendMessageTimeoutW(real, WM_COPYDATA, wp, lp, SMTO_ABORTIFHUNG, 2000, Some(&mut r));
    }
    // re-adds after our TaskbarCreated broadcast fail in explorer (it has them already); tell apps it worked
    LRESULT(if is_tray { 1 } else { r as isize })
}

/// Creates the stand-in tray window and asks every app to re-send its icons.
pub fn start() {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = w!("Shell_TrayWnd");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: module.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class, PCWSTR::null(), WS_POPUP, 0, 0, 0, 0, None, None, Some(module.into()), None)
        else {
            return;
        };
        HOST.store(hwnd.0 as isize, Ordering::Relaxed);
        // same message explorer sends after it restarts; apps answer with NIM_ADD for each icon.
        // Not a broadcast: explorer itself would take it as a restart and drop every appbar.
        let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
        let explorer = win::shell_tray().map(|h| {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid as *mut _));
            pid
        });
        let mut all: Vec<HWND> = Vec::new();
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        for h in all {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid as *mut _));
            if Some(pid) != explorer {
                let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> windows::core::BOOL {
    (*(lp.0 as *mut Vec<HWND>)).push(h);
    true.into()
}

/// Explorer can raise its own window above ours; stay first in FindWindow's z-order walk.
pub fn keep_on_top() {
    let h = HOST.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = SetWindowPos(HWND(h as *mut c_void), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

#[tauri::command]
pub fn tray_icons() -> Vec<TrayIcon> {
    let mut icons = ICONS.lock().unwrap();
    // apps that crash never send NIM_DELETE
    icons.retain(|i| unsafe { IsWindow(Some(HWND(i.hwnd as *mut c_void))).as_bool() });
    icons.iter().filter(|i| !i.hidden).cloned().collect()
}

/// Replays a click the way explorer delivers it to the icon's owner.
#[tauri::command]
pub fn tray_click(key: String, button: String) {
    let Some(i) = ICONS.lock().unwrap().iter().find(|i| i.key == key).cloned() else { return };
    if i.callback == 0 {
        return;
    }
    let owner = HWND(i.hwnd as *mut c_void);
    unsafe {
        // the app's menu/window must be allowed to come to the front
        let mut pid = 0u32;
        GetWindowThreadProcessId(owner, Some(&mut pid as *mut _));
        let _ = AllowSetForegroundWindow(pid);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        const NIN_SELECT: u32 = WM_USER;
        let v4 = i.version >= 4;
        let msgs: &[u32] = match (button.as_str(), v4) {
            ("right", true) => &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU],
            ("right", false) => &[WM_RBUTTONDOWN, WM_RBUTTONUP],
            ("double", _) => &[WM_LBUTTONDBLCLK],
            (_, true) => &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT],
            (_, false) => &[WM_LBUTTONDOWN, WM_LBUTTONUP],
        };
        for &m in msgs {
            // v4: wParam = anchor point, lParam = LOWORD event | HIWORD id. Older: wParam = id, lParam = event.
            let (wp, lp) = if v4 {
                (((pt.y as u16 as usize) << 16) | pt.x as u16 as usize, ((i.uid as isize) << 16) | m as isize)
            } else {
                (i.uid as usize, m as isize)
            };
            let _ = PostMessageW(Some(owner), i.callback, WPARAM(wp), LPARAM(lp));
        }
    }
}
