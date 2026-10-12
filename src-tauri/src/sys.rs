//! Tray status: audio (Core Audio), now playing (GSMTC), network, battery.
//! Commands are async so COM/WinRT blocking calls stay off the UI thread.
#![allow(non_snake_case)] // IPolicyConfig method names must match the COM interface

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

use base64::Engine;
use serde::Serialize;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus as NotifAccess};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};

use crate::win;
use windows::core::{interface, IUnknown, IUnknown_Vtbl, Result, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Networking::Connectivity::NetworkInformation;
use windows::Storage::Streams::DataReader;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

// ---- audio -------------------------------------------------------------

fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe {
        // already-initialized on this thread is fine
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
    }
}

fn flow(capture: bool) -> EDataFlow {
    if capture { eCapture } else { eRender }
}

fn default_volume(capture: bool) -> Result<IAudioEndpointVolume> {
    unsafe { enumerator()?.GetDefaultAudioEndpoint(flow(capture), eConsole)?.Activate(CLSCTX_ALL, None) }
}

#[derive(Serialize)]
pub struct Volume {
    level: f32,
    muted: bool,
}

fn volume(capture: bool) -> Option<Volume> {
    let v = default_volume(capture).ok()?;
    unsafe { Some(Volume { level: v.GetMasterVolumeLevelScalar().ok()?, muted: v.GetMute().ok()?.as_bool() }) }
}

#[derive(Serialize)]
pub struct Device {
    id: String,
    name: String,
    default: bool,
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const c_void));
    s
}

fn devices(capture: bool) -> Result<Vec<Device>> {
    unsafe {
        let en = enumerator()?;
        let def = en.GetDefaultAudioEndpoint(flow(capture), eConsole).and_then(|d| d.GetId()).map(|p| take_pwstr(p)).unwrap_or_default();
        let all = en.EnumAudioEndpoints(flow(capture), DEVICE_STATE_ACTIVE)?;
        (0..all.GetCount()?)
            .map(|i| {
                let d = all.Item(i)?;
                let id = take_pwstr(d.GetId()?);
                let name = d.OpenPropertyStore(STGM_READ)?.GetValue(&PKEY_Device_FriendlyName)?.to_string();
                Ok(Device { default: id == def, id, name })
            })
            .collect()
    }
}

#[derive(Serialize)]
pub struct Audio {
    output: Option<Volume>,
    input: Option<Volume>,
    outputs: Vec<Device>,
    inputs: Vec<Device>,
}

#[tauri::command]
pub async fn audio_state() -> Audio {
    Audio {
        output: volume(false),
        input: volume(true),
        outputs: devices(false).unwrap_or_default(),
        inputs: devices(true).unwrap_or_default(),
    }
}

#[tauri::command]
pub async fn set_volume(capture: bool, level: f32) {
    if let Ok(v) = default_volume(capture) {
        unsafe {
            let _ = v.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null());
            let _ = v.SetMute(level <= 0.0, std::ptr::null()); // dragging the slider unmutes, like Windows does
        }
    }
}

#[tauri::command]
pub async fn toggle_mute(capture: bool) {
    if let Ok(v) = default_volume(capture) {
        unsafe {
            if let Ok(m) = v.GetMute() {
                let _ = v.SetMute(!m.as_bool(), std::ptr::null());
            }
        }
    }
}

// Undocumented but stable since Vista; it's what every audio switcher (and Windows itself) uses.
// Vtable order matters, so the unused methods stay.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self, id: PCWSTR, fmt: *mut *mut c_void) -> HRESULT;
    fn GetDeviceFormat(&self, id: PCWSTR, default: i32, fmt: *mut *mut c_void) -> HRESULT;
    fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(&self, id: PCWSTR, a: *mut c_void, b: *mut c_void) -> HRESULT;
    fn GetProcessingPeriod(&self, id: PCWSTR, default: i32, a: *mut i64, b: *mut i64) -> HRESULT;
    fn SetProcessingPeriod(&self, id: PCWSTR, a: *mut i64) -> HRESULT;
    fn GetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn SetShareMode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, id: PCWSTR, store: i32, key: *const c_void, pv: *mut c_void) -> HRESULT;
    fn SetPropertyValue(&self, id: PCWSTR, store: i32, key: *const c_void, pv: *mut c_void) -> HRESULT;
    fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
    fn SetEndpointVisibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
}
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[tauri::command]
pub async fn set_default_device(id: String) -> std::result::Result<(), String> {
    let _ = enumerator(); // COM init
    let id = HSTRING::from(id);
    unsafe {
        let pc: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        for role in [eConsole, eMultimedia, eCommunications] {
            pc.SetDefaultEndpoint(PCWSTR(id.as_ptr()), role).ok().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---- now playing -------------------------------------------------------

fn sessions() -> Result<Vec<Session>> {
    Ok(SessionManager::RequestAsync()?.join()?.GetSessions()?.into_iter().collect())
}

fn session(app: &str) -> Option<Session> {
    sessions().ok()?.into_iter().find(|s| s.SourceAppUserModelId().is_ok_and(|a| a.to_string() == app))
}

#[derive(Serialize)]
pub struct Media {
    app: String,
    title: String,
    artist: String,
    playing: bool,
}

#[tauri::command]
pub async fn media_state() -> Vec<Media> {
    sessions()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let p = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
            Some(Media {
                app: s.SourceAppUserModelId().ok()?.to_string(),
                title: p.Title().ok()?.to_string(),
                artist: p.Artist().ok()?.to_string(),
                playing: s.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? == PlaybackStatus::Playing,
            })
        })
        .collect()
}

/// Album art of the app's current track as a data URL.
#[tauri::command]
pub async fn media_thumb(app: String) -> Option<String> {
    let stream = session(&app)?.TryGetMediaPropertiesAsync().ok()?.join().ok()?.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    let mime = stream.ContentType().map(|c| c.to_string()).ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "image/png".into());
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(buf)))
}

#[tauri::command]
pub async fn media_control(app: String, action: String) {
    let Some(s) = session(&app) else { return };
    let op = match action.as_str() {
        "prev" => s.TrySkipPreviousAsync(),
        "next" => s.TrySkipNextAsync(),
        _ => s.TryTogglePlayPauseAsync(),
    };
    if let Ok(op) = op {
        let _ = op.join();
    }
}

// ---- network + battery -------------------------------------------------

#[derive(Serialize)]
pub struct Network {
    kind: &'static str, // wifi | ethernet | none
    name: String,
    bars: u8, // 0-5
}

fn network() -> Network {
    let Ok(p) = NetworkInformation::GetInternetConnectionProfile() else {
        return Network { kind: "none", name: String::new(), bars: 0 };
    };
    let name = p.ProfileName().map(|s| s.to_string()).unwrap_or_default();
    if p.IsWlanConnectionProfile().unwrap_or(false) {
        let bars = p.GetSignalBars().and_then(|b| b.Value()).unwrap_or(0);
        let ssid = p.WlanConnectionProfileDetails().and_then(|d| d.GetConnectedSsid()).map(|s| s.to_string()).unwrap_or(name);
        Network { kind: "wifi", name: ssid, bars }
    } else {
        Network { kind: "ethernet", name, bars: 5 }
    }
}

#[derive(Serialize)]
pub struct Battery {
    percent: u8,
    charging: bool,
}

fn battery() -> Option<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    // 128 = no system battery (desktop), 255 = unknown
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery { percent: s.BatteryLifePercent, charging: s.ACLineStatus == 1 })
}

// ---- radios + bluetooth ------------------------------------------------

fn radio(kind: RadioKind) -> Option<Radio> {
    Radio::GetRadiosAsync().ok()?.join().ok()?.into_iter().find(|r| r.Kind().is_ok_and(|k| k == kind))
}

fn radio_on(kind: RadioKind) -> Option<bool> {
    radio(kind)?.State().ok().map(|s| s == RadioState::On)
}

#[tauri::command]
pub async fn set_radio(kind: String, on: bool) {
    let kind = if kind == "wifi" { RadioKind::WiFi } else { RadioKind::Bluetooth };
    let _ = Radio::RequestAccessAsync().and_then(|a| a.join());
    if let Some(r) = radio(kind) {
        if let Ok(op) = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }) {
            let _ = op.join();
        }
    }
}

#[derive(Serialize)]
pub struct Radios {
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    /// names of connected Bluetooth devices (classic + LE)
    devices: Vec<String>,
}

fn connected_bluetooth() -> Vec<String> {
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
    ];
    let mut names: Vec<String> = selectors
        .into_iter()
        .flatten()
        .filter_map(|sel| DeviceInformation::FindAllAsyncAqsFilter(&sel).ok()?.join().ok())
        .flat_map(|list| list.into_iter().filter_map(|d| d.Name().ok().map(|n| n.to_string())).collect::<Vec<_>>())
        .collect();
    names.sort();
    names.dedup(); // dual-mode devices (e.g. earbuds) show up in both lists
    names
}

#[tauri::command]
pub async fn radios() -> Radios {
    Radios { wifi: radio_on(RadioKind::WiFi), bluetooth: radio_on(RadioKind::Bluetooth), devices: connected_bluetooth() }
}

// ---- notifications -----------------------------------------------------

#[derive(Serialize)]
pub struct Notif {
    id: u32,
    app: String,
    title: String,
    body: String,
    /// unix ms
    time: i64,
}

static NOTIF_ACCESS: OnceLock<bool> = OnceLock::new();

/// Windows asks the user once; unpackaged apps may simply be refused.
fn listener() -> Option<UserNotificationListener> {
    let l = UserNotificationListener::Current().ok()?;
    let ok = *NOTIF_ACCESS.get_or_init(|| {
        l.GetAccessStatus().is_ok_and(|s| s == NotifAccess::Allowed)
            || l.RequestAccessAsync().and_then(|a| a.join()).is_ok_and(|s| s == NotifAccess::Allowed)
    });
    ok.then_some(l)
}

/// None = no access; the UI then hands off to Windows' own notification center.
#[tauri::command]
pub async fn notifications() -> Option<Vec<Notif>> {
    let l = listener()?;
    let generic = KnownNotificationBindings::ToastGeneric().ok()?;
    let list = l.GetNotificationsAsync(NotificationKinds::Toast).ok()?.join().ok()?;
    let mut out: Vec<Notif> = list
        .into_iter()
        .filter_map(|n| {
            let texts: Vec<String> = n
                .Notification().ok()?.Visual().ok()?.GetBinding(&generic).ok()?.GetTextElements().ok()?
                .into_iter()
                .filter_map(|t| t.Text().ok().map(|s| s.to_string()))
                .collect();
            Some(Notif {
                id: n.Id().ok()?,
                app: n.AppInfo().and_then(|a| a.DisplayInfo()).and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_default(),
                title: texts.first().cloned().unwrap_or_default(),
                body: texts.get(1..).map(|t| t.join("\n")).unwrap_or_default(),
                // 100ns ticks since 1601 -> unix ms
                time: (n.CreationTime().ok()?.UniversalTime - 116_444_736_000_000_000) / 10_000,
            })
        })
        .collect();
    out.sort_by_key(|n| -n.time);
    Some(out)
}

#[tauri::command]
pub async fn dismiss_notification(id: Option<u32>) {
    if let Some(l) = listener() {
        let _ = match id {
            Some(id) => l.RemoveNotification(id),
            None => l.ClearNotifications(),
        };
    }
}

// ---- cpu + memory (for widgets) -----------------------------------------

#[derive(Serialize)]
pub struct Stats {
    /// 0-1, since the previous call
    cpu: f32,
    /// 0-1
    memory: f32,
    memory_used: u64,
    memory_total: u64,
}

static CPU_LAST: Mutex<(u64, u64)> = Mutex::new((0, 0));

#[tauri::command]
pub fn system_stats() -> Stats {
    let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    let mut mem = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user));
        let _ = GlobalMemoryStatusEx(&mut mem);
    }
    // kernel time includes idle time
    let (idle, total) = (ticks(idle), ticks(kernel) + ticks(user));
    // ponytail: one shared baseline; several widgets polling make each sample shorter, still correct on average
    let (last_idle, last_total) = std::mem::replace(&mut *CPU_LAST.lock().unwrap(), (idle, total));
    let busy = if last_total == 0 || total <= last_total { 0.0 } else { 1.0 - (idle - last_idle) as f32 / (total - last_total) as f32 };
    let used = mem.ullTotalPhys - mem.ullAvailPhys;
    Stats { cpu: busy.clamp(0.0, 1.0), memory: used as f32 / mem.ullTotalPhys.max(1) as f32, memory_used: used, memory_total: mem.ullTotalPhys }
}

// ---- bar status --------------------------------------------------------

#[derive(Serialize)]
pub struct Status {
    volume: Option<Volume>,
    network: Network,
    battery: Option<Battery>,
    bluetooth: Option<bool>,
    language: String,
    /// None = no access to notifications
    notifications: Option<usize>,
}

#[tauri::command]
pub async fn system_status() -> Status {
    let app = win::LAST_APP.load(Ordering::Relaxed);
    Status {
        volume: volume(false),
        network: network(),
        battery: battery(),
        bluetooth: radio_on(RadioKind::Bluetooth),
        language: win::input_language(if app != 0 { app } else { win::foreground() }),
        notifications: notifications().await.map(|n| n.len()),
    }
}

#[tauri::command]
pub fn next_language() {
    let app = win::LAST_APP.load(Ordering::Relaxed);
    if app != 0 {
        win::next_input_language(app);
    }
}

/// Opens a Windows settings/shell page. Only ms-* URIs: the webview must not be able to launch programs.
#[tauri::command]
pub fn open_uri(uri: String) {
    if uri.starts_with("ms-") && uri.bytes().all(|b| b.is_ascii_alphanumeric() || b"-:/".contains(&b)) {
        let _ = std::process::Command::new("explorer").arg(uri).spawn();
    }
}
