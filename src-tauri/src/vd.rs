//! Virtual desktops. Windows has no public API to list or switch them (the COM interfaces behind
//! Task View change with Windows builds), so: read Explorer's state from the registry, and act
//! through Windows' own shortcuts, which work on every build.
use serde::Serialize;
use windows::core::{GUID, HSTRING};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_SZ};
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_D, VK_F4, VK_LCONTROL, VK_LEFT, VK_LWIN, VK_RIGHT};

use crate::win;

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops";

fn reg(subkey: &str, value: &str, kind: windows::Win32::System::Registry::REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let (subkey, value) = (HSTRING::from(subkey), HSTRING::from(value));
    let mut len = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &subkey, &value, kind, None, None, Some(&mut len)).ok().ok()?;
        let mut buf = vec![0u8; len as usize];
        RegGetValueW(HKEY_CURRENT_USER, &subkey, &value, kind, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)).ok().ok()?;
        buf.truncate(len as usize);
        Some(buf)
    }
}

fn guid(b: &[u8]) -> GUID {
    GUID::from_values(
        u32::from_le_bytes(b[0..4].try_into().unwrap()),
        u16::from_le_bytes(b[4..6].try_into().unwrap()),
        u16::from_le_bytes(b[6..8].try_into().unwrap()),
        b[8..16].try_into().unwrap(),
    )
}

#[derive(Serialize)]
pub struct Desktops {
    current: usize,
    /// user-given names; empty = unnamed ("Desktop 3")
    names: Vec<String>,
}

pub fn desktops() -> Desktops {
    // the list only exists once a second desktop has ever been made
    let ids: Vec<GUID> = reg(KEY, "VirtualDesktopIDs", RRF_RT_REG_BINARY).map(|b| b.chunks_exact(16).map(guid).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Desktops { current: 0, names: vec![String::new()] };
    }
    let current = reg(KEY, "CurrentVirtualDesktop", RRF_RT_REG_BINARY).filter(|b| b.len() == 16).map(|b| guid(&b));
    let names = ids
        .iter()
        .map(|id| {
            reg(&format!(r"{KEY}\Desktops\{{{id:?}}}"), "Name", RRF_RT_REG_SZ)
                .map(|b| String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect::<Vec<_>>()))
                .unwrap_or_default()
        })
        .collect();
    Desktops { current: current.and_then(|c| ids.iter().position(|&i| i == c)).unwrap_or(0), names }
}

const PREV: [VIRTUAL_KEY; 3] = [VK_LWIN, VK_LCONTROL, VK_LEFT];
const NEXT: [VIRTUAL_KEY; 3] = [VK_LWIN, VK_LCONTROL, VK_RIGHT];

#[tauri::command]
pub async fn switch_desktop(index: usize) {
    let current = desktops().current;
    let keys = if index > current { NEXT } else { PREV };
    for _ in 0..index.abs_diff(current) {
        win::send_chord(&keys);
        // let each switch register; Explorer drops chords sent back-to-back
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
}

#[tauri::command]
pub async fn step_desktop(forward: bool) {
    win::send_chord(if forward { &NEXT } else { &PREV });
}

#[tauri::command]
pub fn new_desktop() {
    win::send_chord(&[VK_LWIN, VK_LCONTROL, VK_D]);
}

/// Windows can only close the current desktop, so go there first. Its windows move to the neighbour.
#[tauri::command]
pub async fn remove_desktop(index: usize) {
    switch_desktop(index).await;
    std::thread::sleep(std::time::Duration::from_millis(300));
    win::send_chord(&[VK_LWIN, VK_LCONTROL, VK_F4]);
}
