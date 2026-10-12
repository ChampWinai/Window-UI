//! Win32 helpers: list app windows, focus one, master-stack tiling.
use std::ffi::c_void;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABE_BOTTOM, ABE_TOP, ABM_GETSTATE, ABM_NEW, ABM_REMOVE, ABM_SETPOS, ABM_SETSTATE, ABS_AUTOHIDE,
    APPBARDATA,
};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{SHParseDisplayName, SHGFI_PIDL};
use windows::Win32::Globalization::{GetLocaleInfoW, LOCALE_SISO639LANGNAME2, LOCALE_SNATIVELANGNAME};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{SHGetFileInfoW, ShellExecuteW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
    (*(lp.0 as *mut Vec<HWND>)).push(h);
    true.into()
}

/// Title of our Settings window: the one window of ours that behaves like a normal app window.
pub const SETTINGS_TITLE: &str = "Fix UI Settings";

/// Windows a user would see in Alt+Tab, in z-order (front first).
pub fn windows() -> Vec<HWND> {
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        let (me, shell) = (std::process::id(), GetShellWindow());
        all.retain(|&h| {
            let (mut pid, mut cloaked) = (0u32, 0u32);
            GetWindowThreadProcessId(h, Some(&mut pid as *mut _));
            // UWP apps keep hidden "cloaked" windows around
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
            IsWindowVisible(h).as_bool()
                && h != shell
                // our own windows are shell surfaces (bars, launcher, flyout, widgets), except Settings
                && (pid != me || title(h) == SETTINGS_TITLE)
                && cloaked == 0
                && GetWindow(h, GW_OWNER).is_err()
                && (GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0) == 0
                && GetWindowTextLengthW(h) > 0
        });
    }
    all
}

pub fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}

pub fn focus(hwnd: isize) {
    let h = HWND(hwnd as *mut c_void);
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
    }
}

pub fn minimize(hwnd: isize) {
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut c_void), SW_MINIMIZE);
    }
}

/// Last app window that was in front (not ours). Clicking the bar focuses the bar, so tray actions
/// like switching keyboard language need to know who was active before.
pub static LAST_APP: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Window icon rendered to a 32px RGBA PNG.
pub fn icon_png(hwnd: isize) -> Option<Vec<u8>> {
    let h = HWND(hwnd as *mut c_void);
    unsafe {
        // timeout: a hung app must not freeze the taskbar
        let mut r = 0usize;
        SendMessageTimeoutW(h, WM_GETICON, WPARAM(ICON_BIG as usize), LPARAM(0), SMTO_ABORTIFHUNG, 100, Some(&mut r));
        if r == 0 {
            r = GetClassLongPtrW(h, GCLP_HICON);
        }
        if r == 0 {
            return None;
        }
        hicon_png(HICON(r as *mut c_void))
    }
}

const ICON_PX: i32 = 32;

/// Any HICON rendered to a 32px RGBA PNG.
pub fn hicon_png(icon: HICON) -> Option<Vec<u8>> {
    unsafe {
        let mut px = draw_icon(icon, DI_NORMAL)?;
        if px.chunks(4).all(|p| p[3] == 0) {
            // old-style icon without alpha: transparency lives in its AND mask (white = see-through)
            let mask = draw_icon(icon, DI_MASK)?;
            for (p, m) in px.chunks_mut(4).zip(mask.chunks(4)) {
                p[3] = 255 - m[0];
            }
        }
        for p in px.chunks_mut(4) {
            p.swap(0, 2); // BGRA -> RGBA
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, ICON_PX as u32, ICON_PX as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&px).ok()?;
        }
        Some(out)
    }
}

/// Draws into a fresh transparent 32-bit DIB and returns its BGRA pixels.
unsafe fn draw_icon(icon: HICON, flags: DI_FLAGS) -> Option<Vec<u8>> {
    const S: i32 = ICON_PX;
    {
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: S,
                biHeight: -S, // top-down
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old = SelectObject(dc, bmp.into());
        let drawn = DrawIconEx(dc, 0, 0, icon, S, S, 0, None, flags).is_ok();
        let px = std::slice::from_raw_parts(bits as *const u8, (S * S * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        drawn.then_some(px)
    }
}

/// Reserve a strip at the top or bottom of `monitor` (physical virtual-screen rect) so maximized
/// windows avoid it. Safe to call again to move/resize. Returns the granted rect.
pub fn appbar_set(hwnd: HWND, monitor: RECT, height: i32, bottom: bool) -> RECT {
    unsafe {
        let mut rc = monitor;
        if bottom {
            rc.top = rc.bottom - height;
        } else {
            rc.bottom = rc.top + height;
        }
        let mut abd = APPBARDATA {
            cbSize: size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            uEdge: if bottom { ABE_BOTTOM } else { ABE_TOP },
            rc,
            ..Default::default()
        };
        SHAppBarMessage(ABM_NEW, &mut abd); // no-op if already registered
        // Always the screen edge, no ABM_QUERYPOS: explorer keeps registrations of appbars whose process
        // was killed, and QUERYPOS would stack us below those ghosts. ponytail: overlaps a real
        // third-party appbar on the same edge; rare enough.
        SHAppBarMessage(ABM_SETPOS, &mut abd);
        rc
    }
}

/// Explorer's taskbar window. Skips our own stand-in from tray.rs, which shares the class name.
pub fn shell_tray() -> Option<HWND> {
    let me = std::process::id();
    let mut after = None;
    loop {
        let h = unsafe { FindWindowExW(None, after, w!("Shell_TrayWnd"), PCWSTR::null()) }.ok()?;
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid as *mut _)) };
        if pid != me {
            return Some(h);
        }
        after = Some(h);
    }
}

/// Short name of the keyboard language of `hwnd`'s thread: "ไทย", "ENG", "日本語".
pub fn input_language(hwnd: isize) -> String {
    unsafe {
        let tid = GetWindowThreadProcessId(HWND(hwnd as *mut c_void), None);
        let langid = (GetKeyboardLayout(tid).0 as usize & 0xffff) as u32;
        let info = |kind: u32| {
            let mut buf = [0u16; 85];
            let n = GetLocaleInfoW(langid, kind, Some(&mut buf));
            String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
        };
        let native = info(LOCALE_SNATIVELANGNAME);
        // Windows shows short native names as-is ("ไทย") and an ISO code otherwise ("ENG")
        if (1..=4).contains(&native.chars().count()) { native } else { info(LOCALE_SISO639LANGNAME2).to_uppercase() }
    }
}

/// Cycle `hwnd`'s keyboard layout, like Win+Space, then give it focus back.
pub fn next_input_language(hwnd: isize) {
    const HKL_NEXT: isize = 1;
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(HKL_NEXT));
    }
    focus(hwnd);
}

/// Windows taskbar auto-hide/always-on-top flags (ABS_*), to restore later.
pub fn shell_taskbar_state() -> Option<u32> {
    let tray = shell_tray()?;
    let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: tray, ..Default::default() };
    Some(unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) } as u32)
}

/// Explorer's taskbars on every monitor: the main one plus one Shell_SecondaryTrayWnd per extra monitor.
fn shell_taskbars() -> Vec<HWND> {
    let mut all: Vec<HWND> = shell_tray().into_iter().collect();
    let mut after = None;
    while let Ok(h) = unsafe { FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) } {
        all.push(h);
        after = Some(h);
    }
    all
}

pub fn hide_shell_taskbar() {
    let Some(tray) = shell_tray() else { return };
    unsafe {
        // auto-hide (a global setting, covers every monitor) releases the space the taskbars reserve;
        // hiding alone would leave a gap
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: tray, lParam: LPARAM(ABS_AUTOHIDE as isize), ..Default::default() };
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
    }
    keep_shell_taskbar_hidden();
}

/// Explorer re-shows its taskbars on its own (after the auto-hide switch, Start menu, explorer restart,
/// a monitor being plugged in).
pub fn keep_shell_taskbar_hidden() {
    for h in shell_taskbars() {
        unsafe {
            if IsWindowVisible(h).as_bool() {
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
    }
}

pub fn show_shell_taskbar(state: u32) {
    let Some(tray) = shell_tray() else { return };
    unsafe {
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: tray, lParam: LPARAM(state as isize), ..Default::default() };
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
        for h in shell_taskbars() {
            let _ = ShowWindow(h, SW_SHOW);
        }
    }
}

/// What identifies (and relaunches) the app behind a window: `shell:AppsFolder\<AUMID>` for Store /
/// packaged apps, which can't be started from their exe, otherwise the exe path.
/// UWP windows are hosted by ApplicationFrameHost.exe; the real app owns the inner CoreWindow.
pub fn app_path(hwnd: isize) -> String {
    let mut h = HWND(hwnd as *mut c_void);
    if process_path(h).to_lowercase().ends_with("\\applicationframehost.exe") {
        if let Ok(core) = unsafe { FindWindowExW(Some(h), None, w!("Windows.UI.Core.CoreWindow"), PCWSTR::null()) } {
            h = core;
        }
    }
    match package_app_id(h) {
        Some(aumid) => format!(r"shell:AppsFolder\{aumid}"),
        None => process_path(h),
    }
}

fn with_process<T>(h: HWND, f: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid as *mut _));
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let out = f(proc);
        let _ = CloseHandle(proc);
        out
    }
}

fn process_path(h: HWND) -> String {
    with_process(h, |proc| unsafe {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    })
    .unwrap_or_default()
}

/// AppUserModelID of a packaged process; None for ordinary Win32 apps.
fn package_app_id(h: HWND) -> Option<String> {
    with_process(h, |proc| unsafe {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        (GetApplicationUserModelId(proc, &mut len, Some(PWSTR(buf.as_mut_ptr()))) == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]))
    })
}

/// The shell's icon for an exe or a `shell:AppsFolder\…` app, as PNG.
pub fn file_icon_png(path: &str) -> Option<Vec<u8>> {
    let mut info = SHFILEINFOW::default();
    let size = size_of::<SHFILEINFOW>() as u32;
    unsafe {
        let ok = if path.starts_with("shell:") {
            // not a file: resolve the shell namespace item and ask for its icon by PIDL
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None).ok()?;
            let r = SHGetFileInfoW(PCWSTR(pidl as *const u16), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), size, SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON);
            CoTaskMemFree(Some(pidl as *const c_void));
            r
        } else {
            SHGetFileInfoW(&HSTRING::from(path), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), size, SHGFI_ICON | SHGFI_LARGEICON)
        };
        if ok == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let png = hicon_png(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        png
    }
}

/// Starts an exe, or a `shell:AppsFolder\…` app through Explorer (the only way to activate packaged apps).
pub fn launch(path: &str) {
    unsafe {
        if path.starts_with("shell:") {
            ShellExecuteW(None, w!("open"), w!("explorer.exe"), &HSTRING::from(path), PCWSTR::null(), SW_SHOWNORMAL);
        } else {
            ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

/// Asks the window to close, like clicking its X (the app may still prompt to save).
pub fn close(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Press a key chord system-wide, e.g. Win+Ctrl+Right.
pub fn send_chord(keys: &[VIRTUAL_KEY]) {
    let input = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } },
    };
    let inputs: Vec<INPUT> = keys.iter().map(|&k| input(k, false)).chain(keys.iter().rev().map(|&k| input(k, true))).collect();
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

pub fn monitor_at(x: i32, y: i32) -> isize {
    unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTOPRIMARY).0 as isize }
}

/// Adds WS_EX_TOOLWINDOW: hidden from Alt+Tab and from our own window lists.
pub fn make_tool_window(hwnd: isize) {
    let h = HWND(hwnd as *mut c_void);
    unsafe {
        let ex = GetWindowLongW(h, GWL_EXSTYLE);
        SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as i32);
    }
}

/// Monitor a window is (mostly) on, as a comparable id.
pub fn monitor_of(hwnd: isize) -> isize {
    unsafe { MonitorFromWindow(HWND(hwnd as *mut c_void), MONITOR_DEFAULTTONEAREST).0 as isize }
}

pub fn appbar_remove(hwnd: HWND) {
    unsafe {
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() };
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
}

/// Windows the tiler manages: app windows that are resizable and neither minimized nor maximized.
/// Maximize a window to float it out of the layout; restore it to put it back.
/// Our own Settings window floats, like settings dialogs do in tiling WMs.
pub fn tileable_windows() -> Vec<isize> {
    let me = std::process::id();
    windows()
        .into_iter()
        .filter(|&h| unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid as *mut _));
            let style = GetWindowLongW(h, GWL_STYLE) as u32;
            // no resize frame: dialogs, splash screens, borderless fullscreen games
            pid != me && style & WS_THICKFRAME.0 != 0 && !IsIconic(h).as_bool() && !IsZoomed(h).as_bool()
        })
        .map(|h| h.0 as isize)
        .collect()
}

/// Work area (screen minus appbars) of a monitor, inset by `gap`.
pub fn work_area(monitor: isize, gap: i32) -> (i32, i32, i32, i32) {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(HMONITOR(monitor as *mut c_void), &mut mi);
    }
    let w = mi.rcWork;
    (w.left + gap, w.top + gap, w.right - w.left - 2 * gap, w.bottom - w.top - 2 * gap)
}

pub fn place_window(hwnd: isize, r: (i32, i32, i32, i32)) {
    unsafe { place(HWND(hwnd as *mut c_void), r) }
}

unsafe fn place(h: HWND, (x, y, w, ht): (i32, i32, i32, i32)) {
    // Win10/11 windows have invisible resize borders; grow the rect so the visible frame lands on target.
    let (mut wr, mut fr) = (RECT::default(), RECT::default());
    let ok = GetWindowRect(h, &mut wr).is_ok()
        && DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut RECT as *mut c_void, size_of::<RECT>() as u32).is_ok();
    let (l, t, r, b) = if ok {
        (fr.left - wr.left, fr.top - wr.top, wr.right - fr.right, wr.bottom - fr.bottom)
    } else {
        (0, 0, 0, 0)
    };
    // ASYNC so a hung app can't freeze us
    let _ = SetWindowPos(h, None, x - l, y - t, w + l + r, ht + t + b, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
}
