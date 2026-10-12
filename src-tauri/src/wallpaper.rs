//! Wallpaper: one image on every monitor, or a folder slideshow where each monitor gets its own pick.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::{Result, HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DWPOS_FILL};

pub const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "bmp", "webp", "gif"];

fn images(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.contains(&e.to_lowercase().as_str())))
        .collect();
    out.sort();
    out
}

// ponytail: xorshift seeded from the clock; std has no RNG and a slideshow doesn't need a good one
fn random(n: usize) -> usize {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x % n as u64) as usize
}

/// Sets `source` (an image, or a folder to pick from per monitor) as the wallpaper.
pub fn apply(source: &str) -> Result<()> {
    let path = Path::new(source);
    let pool = if path.is_dir() { images(path) } else { vec![path.to_path_buf()] };
    if pool.is_empty() {
        return Ok(());
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let dw: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL)?;
        dw.SetPosition(DWPOS_FILL)?;
        for i in 0..dw.GetMonitorDevicePathCount()? {
            let monitor = dw.GetMonitorDevicePathAt(i)?;
            let pick = &pool[if pool.len() > 1 { random(pool.len()) } else { 0 }];
            let set = dw.SetWallpaper(PCWSTR(monitor.0), &HSTRING::from(pick.as_os_str()));
            CoTaskMemFree(Some(monitor.0 as *const _));
            set?;
        }
    }
    Ok(())
}

/// Background slideshow: re-applies `get()` every `interval()` minutes (0 = off, checked each minute).
pub fn run_slideshow(get: impl Fn() -> (String, u32) + Send + 'static) {
    std::thread::spawn(move || {
        let mut waited = 0;
        loop {
            std::thread::sleep(Duration::from_secs(60));
            waited += 1;
            let (source, minutes) = get();
            if minutes > 0 && waited >= minutes && Path::new(&source).is_dir() {
                waited = 0;
                let _ = apply(&source);
            }
        }
    });
}
