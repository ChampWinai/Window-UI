#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod sys;
mod tiling;
mod tray;
mod vd;
mod wallpaper;
mod widgets;
mod win;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent, Wry,
};
use windows::Win32::Foundation::RECT;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// %APPDATA%\com.fixui.desktop\config.yaml, written with defaults on first run.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Config {
    launcher_hotkey: String,
    /// re-tile every monitor now
    tile_hotkey: String,
    /// cycle the tiling layout
    layout_hotkey: String,
    gap: i32,
    /// file name in src/themes without .css
    theme: String,
    /// "taskbar": one bar per monitor, placed by the theme. "toolbar-dock": toolbar on top + dock at the bottom.
    bars: String,
    hide_windows_taskbar: bool,
    /// exe paths pinned to the taskbar/dock, in order
    pinned: Vec<String>,
    auto_tile: bool,
    tiling_layout: tiling::Layout,
    /// image file, or folder for a slideshow; empty = leave the wallpaper alone
    wallpaper: String,
    /// minutes between slideshow changes, 0 = off
    wallpaper_interval: u32,
    /// ids (folder names) of the desktop widgets to show
    widgets: Vec<String>,
    /// https URL of the updater's latest.json; empty = updates off
    update_url: String,
    /// check `update_url` at startup and offer to install
    auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            launcher_hotkey: "alt+space".into(),
            tile_hotkey: "alt+shift+t".into(),
            layout_hotkey: "alt+shift+l".into(),
            gap: 8,
            theme: "windows".into(),
            bars: "taskbar".into(),
            hide_windows_taskbar: true,
            pinned: vec![],
            auto_tile: true,
            tiling_layout: tiling::Layout::Dwindle,
            wallpaper: String::new(),
            wallpaper_interval: 30,
            widgets: vec!["clock".into(), "system".into()],
            update_url: String::new(),
            auto_update: true,
        }
    }
}

struct Theme {
    id: &'static str,
    name: &'static str,
    bottom: bool,
    height: f64,
}

// bar placement per theme; the look itself lives in src/themes/<id>.css
const THEMES: &[Theme] = &[
    Theme { id: "windows", name: "Windows 11", bottom: true, height: 48.0 },
    Theme { id: "macos", name: "macOS", bottom: true, height: 72.0 },
    Theme { id: "gnome", name: "GNOME", bottom: false, height: 32.0 },
    Theme { id: "kde", name: "KDE Plasma", bottom: true, height: 44.0 },
];

struct AppState {
    cfg: Mutex<Config>,
    /// original Windows taskbar state, Some = we hid it and must restore on exit
    shell_taskbar: Option<u32>,
}

fn config_path(app: &AppHandle) -> PathBuf {
    app.path().app_config_dir().expect("no config dir").join("config.yaml")
}

fn shell_state_path(app: &AppHandle) -> PathBuf {
    config_path(app).with_file_name("windows-taskbar-state")
}

fn save_config(app: &AppHandle, cfg: &Config) {
    let path = config_path(app);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(&path, serde_yaml::to_string(cfg).unwrap());
}

fn load_config(app: &AppHandle) -> Config {
    let path = config_path(app);
    let cfg = match std::fs::read_to_string(&path).map(|s| serde_yaml::from_str(&s)) {
        // don't overwrite a broken file, the user may be mid-edit
        Ok(Err(e)) => {
            eprintln!("{}: {e}, using defaults", path.display());
            return Config::default();
        }
        Ok(Ok(cfg)) => cfg,
        Err(_) => Config::default(),
    };
    save_config(app, &cfg); // writes defaults for options added since the file was made
    cfg
}

#[derive(Serialize)]
struct WinInfo {
    hwnd: isize,
    title: String,
    /// app identity: exe path, or `shell:AppsFolder\<AUMID>` for Store apps; groups windows and matches pins
    exe: String,
}

#[tauri::command]
fn list_windows() -> Vec<WinInfo> {
    win::windows().into_iter().map(|h| WinInfo { hwnd: h.0 as isize, title: win::title(h), exe: win::app_path(h.0 as isize) }).collect()
}

fn data_url(png: Vec<u8>) -> String {
    use base64::Engine;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

#[tauri::command]
fn file_icon(path: String) -> Option<String> {
    win::file_icon_png(&path).map(data_url)
}

/// Only pinned apps or apps that are already running: the page must not be able to start arbitrary programs.
#[tauri::command]
fn launch_app(path: String, state: State<AppState>) {
    let allowed = state.cfg.lock().unwrap().pinned.contains(&path) || win::windows().iter().any(|&h| win::app_path(h.0 as isize) == path);
    if allowed {
        win::launch(&path);
    }
}

#[tauri::command]
fn close_window(hwnd: isize) {
    win::close(hwnd);
}

#[tauri::command]
fn set_pinned(app: AppHandle, path: String, pinned: bool) {
    let state = app.state::<AppState>();
    let mut cfg = state.cfg.lock().unwrap();
    cfg.pinned.retain(|p| *p != path);
    if pinned {
        cfg.pinned.push(path);
    }
    save_config(&app, &cfg);
}

fn update_tiling(app: &AppHandle, f: impl FnOnce(&mut Config)) {
    let (layout, gap, on) = {
        let state = app.state::<AppState>();
        let mut cfg = state.cfg.lock().unwrap();
        f(&mut cfg);
        save_config(app, &cfg);
        (cfg.tiling_layout, cfg.gap, cfg.auto_tile)
    };
    if on {
        tiling::tick(layout, gap, true);
    }
    refresh_tray_menu(app);
}

#[tauri::command]
fn next_layout(app: AppHandle) {
    update_tiling(&app, |c| c.tiling_layout = c.tiling_layout.next());
}

#[tauri::command]
fn set_layout(app: AppHandle, layout: tiling::Layout) {
    update_tiling(&app, |c| c.tiling_layout = layout);
}

#[tauri::command]
fn toggle_auto_tile(app: AppHandle) {
    update_tiling(&app, |c| c.auto_tile = !c.auto_tile);
}

/// Auto-tiling: re-applies the layout on any monitor whose set of windows changed.
/// ponytail: polls every 400ms; a SetWinEventHook on show/hide/minimize would react instantly
fn run_tiler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        let Some(state) = app.try_state::<AppState>() else { continue };
        let (on, layout, gap) = {
            let c = state.cfg.lock().unwrap();
            (c.auto_tile, c.tiling_layout, c.gap)
        };
        if on {
            tiling::tick(layout, gap, false);
        }
    });
}

/// (Re)registers the global hotkeys; returns one message per key that couldn't be taken.
fn register_hotkeys(app: &AppHandle, keys: [&String; 3]) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    keys.iter()
        // usually means another app (e.g. PowerToys) already owns it
        .filter_map(|k| gs.register(k.as_str()).err().map(|e| format!("{k}: {e}")))
        .collect()
}

/// While Settings records a new shortcut, the current ones must not fire (pressing Alt+Space would
/// open the launcher instead of being recorded). Sync: hotkeys belong to the main thread.
#[tauri::command]
fn pause_hotkeys(app: AppHandle, paused: bool) -> Vec<String> {
    if paused {
        let _ = app.global_shortcut().unregister_all();
        return vec![];
    }
    let c = app.state::<AppState>().cfg.lock().unwrap().clone();
    register_hotkeys(&app, [&c.launcher_hotkey, &c.tile_hotkey, &c.layout_hotkey])
}

// ---- settings window ----------------------------------------------------

fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title(win::SETTINGS_TITLE)
        .inner_size(860.0, 620.0)
        .min_inner_size(640.0, 460.0)
        .center()
        .build();
}

/// Runs `f` on the event-loop (main) thread and waits for its result. All window work (create,
/// destroy, appbars, hotkeys, tray menu) goes through here when it starts from a command:
/// - sync commands already run on that thread, and creating a window there deadlocks;
/// - from a command thread, appbar calls go through our Shell_TrayWnd stand-in (tray.rs), which
///   lives on the main thread, and the two threads end up waiting on each other.
/// Only call from async commands or other non-main threads.
fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce(&AppHandle) -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(f(&handle));
    })
    .ok()?;
    rx.recv().ok()
}

#[tauri::command]
async fn open_settings(app: AppHandle) {
    on_main(&app, show_settings);
}

#[derive(Serialize)]
struct SettingsInfo {
    config: Config,
    config_path: String,
    version: String,
}

#[tauri::command]
fn get_settings(app: AppHandle) -> SettingsInfo {
    SettingsInfo {
        config: app.state::<AppState>().cfg.lock().unwrap().clone(),
        config_path: config_path(&app).display().to_string(),
        version: app.package_info().version.to_string(),
    }
}

/// Replaces the whole config (the page edits a copy) and applies whatever changed.
/// Err = shown to the user; invalid input is rejected before anything is saved.
#[tauri::command]
async fn set_settings(app: AppHandle, config: Config) -> Result<(), String> {
    let keys = [&config.launcher_hotkey, &config.tile_hotkey, &config.layout_hotkey];
    for k in keys {
        k.parse::<Shortcut>().map_err(|e| format!("Shortcut \"{k}\": {e}"))?;
    }
    if keys[0] == keys[1] || keys[0] == keys[2] || keys[1] == keys[2] {
        return Err("Each shortcut must be different".into());
    }
    if !(0..=64).contains(&config.gap) {
        return Err("Gap must be between 0 and 64".into());
    }
    if !THEMES.iter().any(|t| t.id == config.theme) || !BAR_LAYOUTS.iter().any(|(id, _)| *id == config.bars) {
        return Err("Unknown theme or bar layout".into());
    }
    // the updater downloads and runs installers: only over https
    if !config.update_url.is_empty() && !(config.update_url.starts_with("https://") && config.update_url.parse::<tauri::Url>().is_ok()) {
        return Err("Update URL must be an https:// address".into());
    }

    let old = {
        let state = app.state::<AppState>();
        let mut cfg = state.cfg.lock().unwrap();
        let old = std::mem::replace(&mut *cfg, config.clone());
        save_config(&app, &cfg);
        old
    };
    on_main(&app, move |app| apply_settings(app, &old, &config)).unwrap_or(Ok(()))
}

/// The part of set_settings that touches windows; runs on the main thread (see on_main).
fn apply_settings(app: &AppHandle, old: &Config, config: &Config) -> Result<(), String> {
    if old.theme != config.theme || old.bars != config.bars {
        let _ = rebuild_bars(app);
        let _ = app.emit("theme", &config.theme);
    }
    if config.auto_tile && (!old.auto_tile || old.tiling_layout != config.tiling_layout || old.gap != config.gap) {
        tiling::tick(config.tiling_layout, config.gap, true);
    }
    if old.widgets != config.widgets {
        let _ = widgets::show(app, &config.widgets);
    }
    if (old.wallpaper != config.wallpaper) && !config.wallpaper.is_empty() {
        let source = config.wallpaper.clone();
        std::thread::spawn(move || wallpaper::apply(&source));
    }
    refresh_tray_menu(app);
    let keys = [&config.launcher_hotkey, &config.tile_hotkey, &config.layout_hotkey];
    if [&old.launcher_hotkey, &old.tile_hotkey, &old.layout_hotkey] != keys {
        let errors = register_hotkeys(app, keys);
        if !errors.is_empty() {
            return Err(format!("Saved, but these shortcuts are taken by another app: {}", errors.join("; ")));
        }
    }
    Ok(())
}

/// File dialog for the wallpaper page; returns the chosen path (already applied and saved).
#[tauri::command]
async fn pick_wallpaper(app: AppHandle, folder: bool) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().file();
    let picked = if folder { dialog.blocking_pick_folder() } else { dialog.add_filter("Images", &wallpaper::IMAGE_EXTS).blocking_pick_file() };
    let path = picked?.into_path().ok()?.to_string_lossy().to_string();
    set_wallpaper(&app, path.clone());
    on_main(&app, refresh_tray_menu);
    Some(path)
}

#[tauri::command]
fn next_wallpaper(state: State<AppState>) {
    let source = state.cfg.lock().unwrap().wallpaper.clone();
    std::thread::spawn(move || wallpaper::apply(&source));
}

// ---- widgets -------------------------------------------------------------

#[derive(Serialize)]
struct WidgetsInfo {
    installed: Vec<widgets::WidgetInfo>,
    enabled: Vec<String>,
}

#[tauri::command]
fn list_widgets(app: AppHandle) -> WidgetsInfo {
    WidgetsInfo { installed: widgets::list(&app), enabled: app.state::<AppState>().cfg.lock().unwrap().widgets.clone() }
}

/// Re-reads the widgets folder and reopens the enabled ones (after editing a widget's files).
#[tauri::command]
async fn reload_widgets(app: AppHandle) -> Result<(), String> {
    let enabled = app.state::<AppState>().cfg.lock().unwrap().widgets.clone();
    on_main(&app, move |app| widgets::show(app, &enabled).map_err(|e| e.to_string())).unwrap_or(Ok(()))
}

#[tauri::command]
fn open_widgets_folder(app: AppHandle) {
    win::launch(&widgets::dir(&app).display().to_string());
}

// ---- updates -------------------------------------------------------------

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    current: String,
    notes: Option<String>,
}

async fn find_update(app: &AppHandle) -> Result<Option<tauri_plugin_updater::Update>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let url = app.state::<AppState>().cfg.lock().unwrap().update_url.clone();
    if url.is_empty() {
        return Err("Set an update URL first (Settings → Updates)".into());
    }
    let url: tauri::Url = url.parse().map_err(|e| format!("Update URL: {e}"))?;
    let updater = app.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    Ok(find_update(&app).await?.map(|u| UpdateInfo { version: u.version.clone(), current: u.current_version.clone(), notes: u.body.clone() }))
}

/// Downloads (signature-checked by the plugin against our pubkey), then hands over to the installer.
/// The installer closes this process, so give the desktop back first.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = find_update(&app).await?.ok_or("Already up to date")?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    on_main(&app, restore_desktop);
    update.install(bytes).map_err(|e| e.to_string())?;
    app.restart();
}

/// Startup: ask before installing anything.
fn offer_update(app: AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
    tauri::async_runtime::spawn(async move {
        let Ok(Some(u)) = find_update(&app).await else { return };
        let handle = app.clone();
        app.dialog()
            .message(format!("Fix UI {} is available (you have {}).\n\nInstall it now? Fix UI restarts afterwards.", u.version, u.current_version))
            .title("Update available")
            .buttons(MessageDialogButtons::OkCancelCustom("Install".into(), "Later".into()))
            .show(move |install| {
                if install {
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = install_update(handle).await {
                            eprintln!("update: {e}");
                        }
                    });
                }
            });
    });
}

/// Our appbars out, the Windows taskbar back. Runs on exit and before an update replaces us.
fn restore_desktop(app: &AppHandle) {
    for h in bars(app).iter().filter_map(|b| b.hwnd().ok()) {
        win::appbar_remove(h);
    }
    if let Some(s) = app.try_state::<AppState>().and_then(|s| s.shell_taskbar) {
        win::show_shell_taskbar(s);
        let _ = std::fs::remove_file(shell_state_path(app));
    }
}

#[tauri::command]
fn open_config_folder(app: AppHandle) {
    if let Some(dir) = config_path(&app).parent() {
        win::launch(&dir.display().to_string());
    }
}

fn set_wallpaper(app: &AppHandle, source: String) {
    {
        let state = app.state::<AppState>();
        let mut cfg = state.cfg.lock().unwrap();
        cfg.wallpaper = source.clone();
        save_config(app, &cfg);
    }
    std::thread::spawn(move || wallpaper::apply(&source));
}

#[tauri::command]
fn focus_window(hwnd: isize) {
    win::focus(hwnd);
}

#[tauri::command]
fn minimize_window(hwnd: isize) {
    win::minimize(hwnd);
}

#[tauri::command]
fn window_icon(hwnd: isize) -> Option<String> {
    win::icon_png(hwnd).map(data_url)
}

/// Pages load before setup() runs, so wait for the config instead of failing (it's only asked once).
#[tauri::command]
async fn theme(app: AppHandle) -> String {
    loop {
        if let Some(state) = app.try_state::<AppState>() {
            return state.cfg.lock().unwrap().theme.clone();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Serialize)]
struct TaskbarState {
    windows: Vec<WinInfo>,
    /// None when the foreground window is ours (taskbar/launcher) or not an app window
    active: Option<isize>,
    /// title of the last app window in front, for the toolbar
    active_title: String,
    pinned: Vec<String>,
    desktops: vd::Desktops,
    layout: tiling::Layout,
    auto_tile: bool,
}

// ponytail: frontend polls this every 500ms; switch to SetWinEventHook if it ever shows up in a profile
/// Called by each monitor's bar; `window` is that bar.
#[tauri::command]
fn taskbar_state(window: WebviewWindow, state: State<AppState>) -> TaskbarState {
    if state.shell_taskbar.is_some() {
        win::keep_shell_taskbar_hidden();
    }
    tray::keep_on_top();
    keep_in_place(&window);
    let mut windows = list_windows();
    let fg = win::foreground();
    let active = windows.iter().any(|w| w.hwnd == fg).then_some(fg);
    if let Some(a) = active {
        win::LAST_APP.store(a, Ordering::Relaxed);
    }
    let last = win::LAST_APP.load(Ordering::Relaxed);
    let active_title = windows.iter().find(|w| w.hwnd == last).map(|w| w.title.clone()).unwrap_or_default();
    // each bar lists the windows on its own monitor (Windows' "taskbar where window is open")
    if let Ok(h) = window.hwnd() {
        let mon = win::monitor_of(h.0 as isize);
        windows.retain(|w| win::monitor_of(w.hwnd) == mon);
    }
    let cfg = state.cfg.lock().unwrap();
    TaskbarState {
        active,
        windows,
        active_title,
        pinned: cfg.pinned.clone(),
        desktops: vd::desktops(),
        layout: cfg.tiling_layout,
        auto_tile: cfg.auto_tile,
    }
}

const BAR_PREFIX: &str = "taskbar-";
static BAR_GEN: AtomicUsize = AtomicUsize::new(0);

fn bars(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows().into_iter().filter(|(label, _)| label.starts_with(BAR_PREFIX)).map(|(_, w)| w).collect()
}

/// Where each bar belongs (label -> physical rect). Windows shoves freshly reserved appbar windows
/// into the remaining work area right after we place them, so bars put themselves back on each poll.
static BAR_RECTS: LazyLock<Mutex<HashMap<String, RECT>>> = LazyLock::new(Default::default);

fn move_to(bar: &WebviewWindow, rc: RECT) -> tauri::Result<()> {
    // move first: crossing to a monitor with another DPI rescales the window, then fix the size
    bar.set_position(PhysicalPosition::new(rc.left, rc.top))?;
    bar.set_size(PhysicalSize::new((rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32))
}

fn keep_in_place(bar: &WebviewWindow) {
    let Some(rc) = BAR_RECTS.lock().unwrap().get(bar.label()).copied() else { return };
    if let (Ok(p), Ok(s)) = (bar.outer_position(), bar.outer_size()) {
        if (p.x, p.y, s.width as i32, s.height as i32) != (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top) {
            let _ = move_to(bar, rc);
        }
    }
}

/// Shows `bar` as an appbar `height` (logical px) tall on the top or bottom edge of `monitor`.
fn place_bar(bar: &WebviewWindow, monitor: &Monitor, bottom: bool, height: f64) -> tauri::Result<()> {
    let h = (height * monitor.scale_factor()) as i32;
    let (p, s) = (monitor.position(), monitor.size());
    let rc = win::appbar_set(bar.hwnd()?, RECT { left: p.x, top: p.y, right: p.x + s.width as i32, bottom: p.y + s.height as i32 }, h, bottom);
    BAR_RECTS.lock().unwrap().insert(bar.label().into(), rc);
    move_to(bar, rc)?;
    bar.show()
}

const TOOLBAR_HEIGHT: f64 = 32.0;
const DOCK_HEIGHT: f64 = 68.0;

/// Bars per monitor, rebuilt from scratch when monitors, theme or bar layout change (simpler than diffing).
/// The page reads its role ("bar", "toolbar", "dock") from the end of its window label.
fn rebuild_bars(app: &AppHandle) -> tauri::Result<()> {
    for bar in bars(app) {
        if let Ok(h) = bar.hwnd() {
            win::appbar_remove(h);
        }
        let _ = bar.destroy();
    }
    BAR_RECTS.lock().unwrap().clear();
    let (theme, split) = {
        let state = app.state::<AppState>();
        let cfg = state.cfg.lock().unwrap();
        (cfg.theme.clone(), cfg.bars == "toolbar-dock")
    };
    let t = THEMES.iter().find(|t| t.id == theme).unwrap_or(&THEMES[0]);
    let roles: &[(&str, bool, f64)] = if split { &[("toolbar", false, TOOLBAR_HEIGHT), ("dock", true, DOCK_HEIGHT)] } else { &[("bar", t.bottom, t.height)] };
    // fresh labels: a destroyed window's label isn't free until its close finishes
    let generation = BAR_GEN.fetch_add(1, Ordering::Relaxed);
    for (i, monitor) in app.available_monitors()?.iter().enumerate() {
        for &(role, bottom, height) in roles {
            let bar = WebviewWindowBuilder::new(app, format!("{BAR_PREFIX}{generation}-{i}-{role}"), WebviewUrl::App("taskbar.html".into()))
                .title("Taskbar")
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .shadow(false)
                .focused(false)
                .visible(false)
                .build()?;
            place_bar(&bar, monitor, bottom, height)?;
        }
    }
    Ok(())
}

/// Monitors plugged/unplugged, resolution or scaling changed -> rebuild the bars.
/// ponytail: polls every 2s; WM_DISPLAYCHANGE on a hidden window would be instant
fn watch_monitors(app: AppHandle) {
    let layout = |app: &AppHandle| -> Vec<(i32, i32, u32, u32, u64)> {
        let monitors = app.available_monitors().unwrap_or_default();
        monitors.iter().map(|m| (m.position().x, m.position().y, m.size().width, m.size().height, m.scale_factor().to_bits())).collect()
    };
    std::thread::spawn(move || {
        let mut last = layout(&app);
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = layout(&app);
            if now != last {
                last = now;
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    let _ = rebuild_bars(&handle);
                });
            }
        }
    });
}

/// Theme or bar layout change: both need the bars rebuilt.
fn set_bars(app: &AppHandle, f: impl FnOnce(&mut Config)) {
    let theme = {
        let state = app.state::<AppState>();
        let mut cfg = state.cfg.lock().unwrap();
        f(&mut cfg);
        save_config(app, &cfg);
        cfg.theme.clone()
    };
    let _ = rebuild_bars(app);
    let _ = app.emit("theme", theme);
    refresh_tray_menu(app);
}

// Popups hide on blur. Clicking the button that opens one blurs it first, so without this
// the same click would hide it and immediately show it again.
static BLUR_HIDDEN: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

fn just_blur_hidden(label: &str) -> bool {
    BLUR_HIDDEN.lock().unwrap().get(label).is_some_and(|t| t.elapsed() < Duration::from_millis(300))
}

#[tauri::command]
fn toggle_launcher(app: AppHandle) {
    toggle(&app)
}

static FLYOUT_PAGE: Mutex<String> = Mutex::new(String::new());

/// page: "quick" (sound, radios, media) or "notifications" (notifications + calendar).
/// `window` is the bar that was clicked; the flyout opens next to it, on its monitor.
#[tauri::command]
fn toggle_flyout(app: AppHandle, window: WebviewWindow, page: String) {
    let Some(f) = app.get_webview_window("flyout") else { return };
    let bar = window;
    let same_page = std::mem::replace(&mut *FLYOUT_PAGE.lock().unwrap(), page.clone()) == page;
    if f.is_visible().unwrap_or(false) {
        if same_page {
            let _ = f.hide();
        } else {
            let _ = f.emit("flyout-open", &page);
        }
        return;
    }
    if same_page && just_blur_hidden("flyout") {
        return;
    }
    // right edge, on the screen side of the bar
    let (Ok(bp), Ok(bs), Ok(Some(mon))) = (bar.outer_position(), bar.outer_size(), bar.current_monitor()) else { return };
    // hop onto the bar's monitor first so the flyout takes that monitor's DPI before we measure it
    let _ = f.set_position(bp);
    let Ok(fs) = f.outer_size() else { return };
    let margin = (8.0 * mon.scale_factor()) as i32;
    let x = bp.x + bs.width as i32 - fs.width as i32 - margin;
    let bar_at_bottom = bp.y > mon.position().y + mon.size().height as i32 / 2;
    let y = if bar_at_bottom { bp.y - fs.height as i32 - margin } else { bp.y + bs.height as i32 + margin };
    let _ = f.set_position(PhysicalPosition::new(x, y));
    let _ = f.show();
    let _ = f.set_focus();
    let _ = f.emit("flyout-open", &page);
}

fn toggle(app: &AppHandle) {
    let Some(w) = app.get_webview_window("launcher") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    } else if !just_blur_hidden("launcher") {
        // open on the monitor the mouse is on
        let monitor = app.cursor_position().ok().and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten());
        if let Some(m) = monitor {
            let _ = w.set_position(*m.position()); // take that monitor's DPI before measuring
            if let Ok(s) = w.outer_size() {
                let (p, ms) = (m.position(), m.size());
                let _ = w.set_position(PhysicalPosition::new(p.x + (ms.width as i32 - s.width as i32) / 2, p.y + (ms.height as i32 - s.height as i32) / 2));
            }
        } else {
            let _ = w.center();
        }
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("launcher-open", ());
    }
}

const BAR_LAYOUTS: [(&str, &str); 2] = [("taskbar", "Taskbar"), ("toolbar-dock", "Toolbar + Dock")];

/// Rebuilt whenever a setting it shows changes, so the checkmarks stay true.
fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let cfg = state.cfg.lock().unwrap();
    let check = |id: String, text: &str, on: bool| CheckMenuItem::with_id(app, id, text, true, on, None::<&str>);
    let submenu = |text: &str, items: Vec<CheckMenuItem<Wry>>, extra: Vec<Box<dyn IsMenuItem<Wry>>>| -> tauri::Result<Submenu<Wry>> {
        let refs: Vec<&dyn IsMenuItem<Wry>> = extra.iter().map(|b| b.as_ref()).chain(items.iter().map(|i| i as &dyn IsMenuItem<Wry>)).collect();
        Submenu::with_items(app, text, true, &refs)
    };

    let themes = THEMES.iter().map(|t| check(format!("theme:{}", t.id), t.name, cfg.theme == t.id)).collect::<tauri::Result<_>>()?;
    let bar_layouts = BAR_LAYOUTS.iter().map(|(id, name)| check(format!("bars:{id}"), name, cfg.bars == *id)).collect::<tauri::Result<_>>()?;
    let layouts = tiling::LAYOUTS
        .iter()
        .map(|&l| check(format!("layout:{}", serde_yaml::to_string(&l).unwrap().trim()), &format!("{l:?}"), cfg.tiling_layout == l))
        .collect::<tauri::Result<_>>()?;
    let tiling_extra: Vec<Box<dyn IsMenuItem<Wry>>> = vec![
        Box::new(check("auto-tile".into(), "Auto-tile windows", cfg.auto_tile)?),
        Box::new(MenuItem::with_id(app, "tile-now", "Re-tile now", true, None::<&str>)?),
        Box::new(PredefinedMenuItem::separator(app)?),
    ];
    let wallpaper = Submenu::with_items(
        app,
        "Wallpaper",
        true,
        &[
            &MenuItem::with_id(app, "wp:image", "Choose image…", true, None::<&str>)?,
            &MenuItem::with_id(app, "wp:folder", "Choose folder (slideshow)…", true, None::<&str>)?,
            &MenuItem::with_id(app, "wp:next", "Next wallpaper", Path::new(&cfg.wallpaper).is_dir(), None::<&str>)?,
        ],
    )?;
    Menu::with_items(
        app,
        &[
            &submenu("Theme", themes, vec![])?,
            &submenu("Bars", bar_layouts, vec![])?,
            &submenu("Tiling", layouts, tiling_extra)?,
            &wallpaper,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )
}

fn refresh_tray_menu(app: &AppHandle) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn on_tray_menu(app: &AppHandle, id: &str) {
    use tauri_plugin_dialog::DialogExt;
    if let Some(t) = id.strip_prefix("theme:") {
        set_bars(app, |c| c.theme = t.into());
    } else if let Some(b) = id.strip_prefix("bars:") {
        set_bars(app, |c| c.bars = b.into());
    } else if let Some(l) = id.strip_prefix("layout:") {
        if let Ok(l) = serde_yaml::from_str(l) {
            update_tiling(app, |c| c.tiling_layout = l);
        }
    } else {
        let handle = app.clone();
        match id {
            "auto-tile" => update_tiling(app, |c| c.auto_tile = !c.auto_tile),
            "tile-now" => update_tiling(app, |_| {}),
            "wp:image" => app.dialog().file().add_filter("Images", &wallpaper::IMAGE_EXTS).pick_file(move |p| {
                if let Some(p) = p.and_then(|p| p.into_path().ok()) {
                    set_wallpaper(&handle, p.to_string_lossy().into());
                }
            }),
            "wp:folder" => app.dialog().file().pick_folder(move |p| {
                if let Some(p) = p.and_then(|p| p.into_path().ok()) {
                    set_wallpaper(&handle, p.to_string_lossy().into());
                    refresh_tray_menu(&handle);
                }
            }),
            "wp:next" => {
                let source = app.state::<AppState>().cfg.lock().unwrap().wallpaper.clone();
                std::thread::spawn(move || wallpaper::apply(&source));
            }
            "settings" => show_settings(app),
            "quit" => app.exit(0),
            _ => {}
        }
    }
}

fn main() {
    tauri::Builder::default()
        .register_uri_scheme_protocol("widget", |ctx, request| widgets::serve(ctx.app_handle(), request.uri().path()))
        .setup(|app| {
            let handle = app.handle().clone();
            let cfg = load_config(&handle);
            let (launcher_key, tile_key, layout_key) = (cfg.launcher_hotkey.clone(), cfg.tile_hotkey.clone(), cfg.layout_hotkey.clone()); // registered after the plugin loads
            let wallpaper_source = cfg.wallpaper.clone();
            let (enabled_widgets, check_updates) = (cfg.widgets.clone(), cfg.auto_update && !cfg.update_url.is_empty());

            // hide the Windows taskbar before placing ours, or ours gets stacked above it
            let shell_taskbar = if cfg.hide_windows_taskbar {
                // a saved state means last run never restored it (killed/crashed): that one is the user's real setting
                let saved = shell_state_path(&handle);
                let orig = std::fs::read_to_string(&saved).ok().and_then(|s| s.trim().parse().ok()).or_else(win::shell_taskbar_state);
                if let Some(s) = orig {
                    let _ = std::fs::write(&saved, s.to_string());
                    win::hide_shell_taskbar();
                }
                orig
            } else {
                None
            };
            // manage state first: the pages are already loading and calling commands like `theme`
            app.manage(AppState { cfg: Mutex::new(cfg), shell_taskbar });

            handle.plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|app, sc, ev| {
                        if ev.state() != ShortcutState::Pressed {
                            return;
                        }
                        // read the live config: Settings can change the keys at any time
                        let keys = {
                            let state = app.state::<AppState>();
                            let c = state.cfg.lock().unwrap();
                            [c.launcher_hotkey.clone(), c.tile_hotkey.clone(), c.layout_hotkey.clone()]
                        };
                        let is = |k: &str| k.parse::<Shortcut>().is_ok_and(|k| k == *sc);
                        if is(&keys[0]) {
                            toggle(app)
                        } else if is(&keys[1]) {
                            update_tiling(app, |_| {})
                        } else if is(&keys[2]) {
                            update_tiling(app, |c| c.tiling_layout = c.tiling_layout.next())
                        }
                    })
                    .build(),
            )?;
            handle.plugin(tauri_plugin_dialog::init())?;
            handle.plugin(tauri_plugin_updater::Builder::new().build())?;
            for e in register_hotkeys(&handle, [&launcher_key, &tile_key, &layout_key]) {
                eprintln!("{e}");
            }

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Fix UI")
                .menu(&tray_menu(&handle)?)
                .on_menu_event(|app, e| on_tray_menu(app, e.id.as_ref()))
                .build(app)?;

            rebuild_bars(&handle)?;
            watch_monitors(handle.clone());
            run_tiler(handle.clone());
            widgets::install_builtins(&handle);
            if let Err(e) = widgets::show(&handle, &enabled_widgets) {
                eprintln!("widgets: {e}");
            }
            if check_updates {
                offer_update(handle.clone());
            }
            if !wallpaper_source.is_empty() {
                std::thread::spawn(move || wallpaper::apply(&wallpaper_source));
            }
            let slideshow = handle.clone();
            wallpaper::run_slideshow(move || {
                let state = slideshow.state::<AppState>();
                let cfg = state.cfg.lock().unwrap();
                (cfg.wallpaper.clone(), cfg.wallpaper_interval)
            });
            // only take over app tray icons when explorer's tray is out of sight
            if shell_taskbar.is_some() {
                tray::start();
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            if let WindowEvent::Focused(false) = e {
                if matches!(w.label(), "launcher" | "flyout") && w.is_visible().unwrap_or(false) {
                    let _ = w.hide();
                    BLUR_HIDDEN.lock().unwrap().insert(w.label().into(), Instant::now());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_windows,
            focus_window,
            minimize_window,
            window_icon,
            taskbar_state,
            toggle_launcher,
            toggle_flyout,
            theme,
            file_icon,
            launch_app,
            close_window,
            set_pinned,
            next_layout,
            set_layout,
            toggle_auto_tile,
            open_settings,
            pause_hotkeys,
            get_settings,
            set_settings,
            pick_wallpaper,
            next_wallpaper,
            open_config_folder,
            list_widgets,
            reload_widgets,
            open_widgets_folder,
            check_update,
            install_update,
            sys::system_stats,
            vd::switch_desktop,
            vd::step_desktop,
            vd::new_desktop,
            vd::remove_desktop,
            sys::audio_state,
            sys::set_volume,
            sys::toggle_mute,
            sys::set_default_device,
            sys::media_state,
            sys::media_thumb,
            sys::media_control,
            sys::system_status,
            sys::open_uri,
            sys::radios,
            sys::set_radio,
            sys::notifications,
            sys::dismiss_notification,
            sys::next_language,
            tray::tray_icons,
            tray::tray_click
        ])
        .build(tauri::generate_context!())
        .expect("error while building app")
        .run(|app, e| {
            // give the screen back: our reserved strip, and the Windows taskbar if we hid it
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = e {
                restore_desktop(app);
            }
        });
}
