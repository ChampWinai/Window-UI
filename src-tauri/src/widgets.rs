//! Desktop widgets: folders of HTML in <config dir>\widgets\<id>\ (widget.yaml + index.html).
//!
//! Widgets are third-party code, so they never run in an app page. Each widget window loads our
//! host page (widget.html), which embeds the widget in a sandboxed iframe served from the
//! `widget://` protocol (a separate origin with no IPC). The host answers a fixed set of requests
//! the widget posts to it (see widgets/_api.js).
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

use crate::win;

/// Shipped widgets, written to the widgets folder when missing so users can read and copy them.
const BUILTIN: &[(&str, &str, &str)] = &[
    ("clock", include_str!("../widgets/clock/widget.yaml"), include_str!("../widgets/clock/index.html")),
    ("system", include_str!("../widgets/system/widget.yaml"), include_str!("../widgets/system/index.html")),
    ("media", include_str!("../widgets/media/widget.yaml"), include_str!("../widgets/media/index.html")),
];
const API_JS: &str = include_str!("../widgets/_api.js");

#[derive(Deserialize, Serialize, Clone)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub width: f64,
    pub height: f64,
    /// top-left | top-right | bottom-left | bottom-right
    #[serde(default = "default_corner")]
    pub corner: String,
    /// distance from that corner of the primary monitor's work area, logical px
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

fn default_corner() -> String {
    "top-right".into()
}

#[derive(Serialize)]
pub struct WidgetInfo {
    id: String,
    #[serde(flatten)]
    manifest: Manifest,
}

pub fn dir(app: &AppHandle) -> PathBuf {
    app.path().app_config_dir().expect("no config dir").join("widgets")
}

/// Folder names become ids, URLs and window labels: keep them boring.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn install_builtins(app: &AppHandle) {
    for (id, yaml, html) in BUILTIN {
        let d = dir(app).join(id);
        if !d.exists() && std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(d.join("widget.yaml"), yaml);
            let _ = std::fs::write(d.join("index.html"), html);
        }
    }
}

pub fn list(app: &AppHandle) -> Vec<WidgetInfo> {
    let Ok(entries) = std::fs::read_dir(dir(app)) else { return vec![] };
    let mut out: Vec<WidgetInfo> = entries
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_string();
            if !valid_id(&id) {
                return None;
            }
            let yaml = std::fs::read_to_string(e.path().join("widget.yaml")).ok()?;
            let manifest = serde_yaml::from_str(&yaml).map_err(|err| eprintln!("widget {id}: {err}")).ok()?;
            Some(WidgetInfo { id, manifest })
        })
        .collect();
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    out
}

/// `widget://localhost/<id>/<file>`: files from the widgets folder, never outside it.
pub fn serve(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let respond = |status: StatusCode, mime: &str, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header("Content-Type", mime)
            // inline code is the norm for widgets; no network: a widget can't send anything anywhere
            .header("Content-Security-Policy", "default-src 'self' http://widget.localhost data: 'unsafe-inline'; connect-src 'none'")
            .body(body)
            .unwrap()
    };
    if path == "/_api.js" {
        return respond(StatusCode::OK, "text/javascript", API_JS.as_bytes().to_vec());
    }
    let rel = path.trim_start_matches('/');
    let ok_path = !rel.is_empty() && rel.split('/').all(|seg| !seg.is_empty() && seg != ".." && !seg.contains(['\\', ':']));
    let id_ok = rel.split('/').next().is_some_and(valid_id);
    let base = dir(app);
    let file = base.join(rel);
    // canonicalize: also catches symlinks pointing out of the folder
    let inside = ok_path && id_ok && file.canonicalize().ok().zip(base.canonicalize().ok()).is_some_and(|(f, b)| f.starts_with(b));
    match inside.then(|| std::fs::read(&file).ok()).flatten() {
        Some(bytes) => respond(StatusCode::OK, mime(&file), bytes),
        None => respond(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec()),
    }
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Closes all widget windows and opens the `enabled` ones on the primary monitor.
/// Must not run from a sync command (window creation would deadlock).
pub fn show(app: &AppHandle, enabled: &[String]) -> tauri::Result<()> {
    for (label, w) in app.webview_windows() {
        if label.starts_with("widget-") {
            let _ = w.destroy();
        }
    }
    let Some(monitor) = app.primary_monitor()? else { return Ok(()) };
    let scale = monitor.scale_factor();
    let (mp, ms) = (monitor.position(), monitor.size());
    let mon = win::monitor_at(mp.x + ms.width as i32 / 2, mp.y + ms.height as i32 / 2);
    let (ax, ay, aw, ah) = win::work_area(mon, 0);
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);

    for w in list(app).into_iter().filter(|w| enabled.contains(&w.id)) {
        let m = &w.manifest;
        let (pw, ph) = ((m.width * scale) as i32, (m.height * scale) as i32);
        let (dx, dy) = ((m.x * scale) as i32, (m.y * scale) as i32);
        let x = if m.corner.ends_with("left") { ax + dx } else { ax + aw - pw - dx };
        let y = if m.corner.starts_with("bottom") { ay + ah - ph - dy } else { ay + dy };
        // label carries the id for the host page: widget-<generation>-<id>
        let win = WebviewWindowBuilder::new(app, format!("widget-{generation}-{}", w.id), WebviewUrl::App("widget.html".into()))
            .title(&m.name)
            .inner_size(m.width, m.height)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .skip_taskbar(true)
            .always_on_bottom(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build()?;
        win.set_position(PhysicalPosition::new(x, y))?;
        win.show()?;
        // tool window: out of Alt+Tab. After show(): showing re-applies the window's styles.
        win::make_tool_window(win.hwnd()?.0 as isize);
    }
    Ok(())
}
