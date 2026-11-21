//! Volume, Wi-Fi, Bluetooth, brightness and now-playing for the Control Center.
//!
//! Everything slow (WinRT, WLAN, DDC/CI, WIC) runs on one worker thread; the UI only
//! clones the latest `Snapshot` and sends `Command`s.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetPhysicalMonitorsFromHMONITOR,
    PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
    WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected,
    wlan_intf_opcode_current_connection,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

use super::icons::Rgba;
use super::util::{hwnd_from_isize, hwnd_to_isize, log};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub volume: Option<f32>,
    pub muted: bool,
    pub wifi: Option<Wifi>,
    pub bluetooth: Option<bool>,
    pub brightness: Option<f32>,
    pub now_playing: Option<NowPlaying>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub on: bool,
    pub ssid: Option<String>,
    pub signal: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub app: String,
    pub playing: bool,
    pub art: Option<ArtId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArtId(pub u64);

pub enum Command {
    SetVolume(f32),
    SetMuted(bool),
    SetWifi(bool),
    SetBluetooth(bool),
    SetBrightness(f32),
    PlayPause,
    Next,
    Previous,
    Refresh,
}

const MAX_ART_PX: u32 = 256;
const MAX_ART_BYTES: u64 = 16 << 20;
const ART_CACHE: usize = 4;
const TICK: Duration = Duration::from_millis(250);
const VOLUME_EVERY: Duration = Duration::from_secs(1);
const PLAYING_EVERY: Duration = Duration::from_secs(1);
const NET_EVERY: Duration = Duration::from_secs(3);
const BRIGHTNESS_EVERY: Duration = Duration::from_secs(5);
// A display without DDC/CI is probed rarely: each probe can take hundreds of ms.
const BRIGHTNESS_RETRY: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Shared {
    snapshot: Snapshot,
    art: Vec<(ArtId, Rgba)>,
}

pub struct Media {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Command>,
}

impl Media {
    /// Starts the worker thread; it posts `notify_msg` to `notify` whenever the snapshot changes.
    pub fn start(notify: HWND, notify_msg: u32) -> Media {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        let worker_shared = shared.clone();
        let hwnd = hwnd_to_isize(notify);
        let spawned = std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                let mut w = Worker::new(worker_shared, hwnd, notify_msg);
                w.run(rx);
            });
        if let Err(e) = spawned {
            log(format!("media: cannot start worker: {e}"));
        }
        Media { shared, tx }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .lock()
            .map(|s| s.snapshot.clone())
            .unwrap_or_default()
    }

    pub fn artwork(&self, id: ArtId) -> Option<Rgba> {
        let s = self.shared.lock().ok()?;
        s.art.iter().find(|(i, _)| *i == id).map(|(_, a)| a.clone())
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

/// When each source is next due.
struct Due {
    volume: Instant,
    playing: Instant,
    net: Instant,
    brightness: Instant,
}

struct Worker {
    shared: Arc<Mutex<Shared>>,
    hwnd: isize,
    msg: u32,
    current: Snapshot,
    logged: HashSet<String>,
    enumerator: Option<IMMDeviceEnumerator>,
    manager: Option<GlobalSystemMediaTransportControlsSessionManager>,
    radio_access: bool,
    // Last track key and artwork fetch state.
    track: Option<(String, String, String)>,
    art: Option<ArtId>,
    art_tries: u8,
    // None: unknown, Some(false): DDC/CI unavailable (retried after BRIGHTNESS_RETRY).
    ddc: Option<bool>,
}

impl Worker {
    fn new(shared: Arc<Mutex<Shared>>, hwnd: isize, msg: u32) -> Worker {
        Worker {
            shared,
            hwnd,
            msg,
            current: Snapshot::default(),
            logged: HashSet::new(),
            enumerator: None,
            manager: None,
            radio_access: false,
            track: None,
            art: None,
            art_tries: 0,
            ddc: None,
        }
    }

    fn log_once(&mut self, key: &str, msg: impl AsRef<str>) {
        if self.logged.insert(key.to_string()) {
            log(format!("media: {key}: {}", msg.as_ref()));
        }
    }

    fn run(&mut self, rx: Receiver<Command>) {
        // MTA: WinRT `.join()` must not block an STA.
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            self.log_once("init", format!("RoInitialize: {e}"));
        }
        let now = Instant::now();
        let mut due = Due {
            volume: now,
            playing: now,
            net: now,
            brightness: now,
        };
        loop {
            let mut cmds = Vec::new();
            match rx.recv_timeout(TICK) {
                Ok(c) => cmds.push(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(c) = rx.try_recv() {
                cmds.push(c);
            }
            let step = catch_unwind(AssertUnwindSafe(|| self.step(cmds, &mut due)));
            if step.is_err() {
                self.log_once("panic", "worker step panicked");
            }
        }
    }

    fn step(&mut self, cmds: Vec<Command>, due: &mut Due) {
        let mut snap = self.current.clone();
        let mut force = !cmds.is_empty();
        // Sliders fire many commands: only the last value of each is applied.
        let mut volume = None;
        let mut brightness = None;
        for c in cmds {
            match c {
                Command::SetVolume(v) => volume = Some(v),
                Command::SetBrightness(v) => brightness = Some(v),
                Command::SetMuted(m) => self.set_muted(m),
                Command::SetWifi(on) => self.set_radio(RadioKind::WiFi, on),
                Command::SetBluetooth(on) => self.set_radio(RadioKind::Bluetooth, on),
                Command::PlayPause => {
                    self.control(|s| s.TryTogglePlayPauseAsync()?.join().map(|_| ()))
                }
                Command::Next => self.control(|s| s.TrySkipNextAsync()?.join().map(|_| ())),
                Command::Previous => self.control(|s| s.TrySkipPreviousAsync()?.join().map(|_| ())),
                Command::Refresh => force = true,
            }
        }
        if let Some(v) = volume {
            self.set_volume(v);
        }
        if let Some(v) = brightness {
            // A monitor that refused the value keeps its own; the slider follows it.
            snap.brightness = if self.set_brightness(v) { Some(v.clamp(0.0, 1.0)) } else { self.read_brightness() };
            due.brightness = Instant::now() + BRIGHTNESS_EVERY;
        }

        let now = Instant::now();
        if force || now >= due.volume {
            let (v, m) = self.read_volume();
            snap.volume = v;
            snap.muted = m;
            due.volume = now + VOLUME_EVERY;
        }
        if force || now >= due.net {
            snap.wifi = self.read_wifi();
            snap.bluetooth = self.read_bluetooth();
            due.net = now + NET_EVERY;
        }
        if now >= due.brightness && brightness.is_none() {
            snap.brightness = self.read_brightness();
            due.brightness = now
                + if self.ddc == Some(false) {
                    BRIGHTNESS_RETRY
                } else {
                    BRIGHTNESS_EVERY
                };
        } else if force && brightness.is_none() && self.ddc == Some(true) {
            snap.brightness = self.read_brightness();
            due.brightness = now + BRIGHTNESS_EVERY;
        }
        if force || now >= due.playing {
            snap.now_playing = self.read_playing();
            due.playing = now + PLAYING_EVERY;
        }

        if snap != self.current {
            self.current = snap.clone();
            if let Ok(mut s) = self.shared.lock() {
                s.snapshot = snap;
            }
            unsafe {
                let _ = PostMessageW(
                    Some(hwnd_from_isize(self.hwnd)),
                    self.msg,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    // ---- volume ----

    fn endpoint(&mut self) -> Option<IAudioEndpointVolume> {
        let r: windows::core::Result<IAudioEndpointVolume> = (|| unsafe {
            if self.enumerator.is_none() {
                self.enumerator = Some(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?);
            }
            let Some(en) = self.enumerator.as_ref() else {
                return Err(windows::core::Error::empty());
            };
            // The default device can change at any time, so it is looked up on every call.
            let dev = en.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
        })();
        match r {
            Ok(v) => {
                self.logged.remove("volume");
                Some(v)
            }
            Err(e) => {
                self.enumerator = None;
                self.log_once("volume", e.to_string());
                None
            }
        }
    }

    fn read_volume(&mut self) -> (Option<f32>, bool) {
        let Some(ep) = self.endpoint() else {
            return (None, false);
        };
        unsafe {
            let v = ep.GetMasterVolumeLevelScalar().ok();
            let m = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            (v, m)
        }
    }

    fn set_volume(&mut self, v: f32) {
        if let Some(ep) = self.endpoint()
            && let Err(e) =
                unsafe { ep.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null()) }
        {
            self.log_once("set-volume", e.to_string());
        }
    }

    fn set_muted(&mut self, m: bool) {
        if let Some(ep) = self.endpoint()
            && let Err(e) = unsafe { ep.SetMute(m, std::ptr::null()) }
        {
            self.log_once("set-mute", e.to_string());
        }
    }

    // ---- radios (Wi-Fi, Bluetooth) ----

    fn radios(&mut self, kind: RadioKind) -> Vec<Radio> {
        let r = (|| -> windows::core::Result<Vec<Radio>> {
            let all = Radio::GetRadiosAsync()?.join()?;
            Ok(all
                .into_iter()
                .filter(|r| r.Kind().is_ok_and(|k| k == kind))
                .collect())
        })();
        match r {
            Ok(v) => v,
            Err(e) => {
                self.log_once("radios", e.to_string());
                Vec::new()
            }
        }
    }

    fn set_radio(&mut self, kind: RadioKind, on: bool) {
        if !self.radio_access {
            match Radio::RequestAccessAsync().and_then(|o| o.join()) {
                Ok(_) => self.radio_access = true,
                Err(e) => self.log_once("radio-access", e.to_string()),
            }
        }
        let state = if on { RadioState::On } else { RadioState::Off };
        for r in self.radios(kind) {
            if let Err(e) = r.SetStateAsync(state).and_then(|o| o.join()) {
                self.log_once("set-radio", e.to_string());
            }
        }
    }

    fn read_bluetooth(&mut self) -> Option<bool> {
        let radios = self.radios(RadioKind::Bluetooth);
        if radios.is_empty() {
            return None;
        }
        Some(
            radios
                .iter()
                .any(|r| r.State().is_ok_and(|s| s == RadioState::On)),
        )
    }

    fn read_wifi(&mut self) -> Option<Wifi> {
        let radios = self.radios(RadioKind::WiFi);
        let conn = unsafe { wlan_connection(&mut self.logged) };
        let radio_on = if radios.is_empty() {
            None
        } else {
            Some(
                radios
                    .iter()
                    .any(|r| r.State().is_ok_and(|s| s == RadioState::On)),
            )
        };
        let (has_adapter, ssid, signal) = match conn {
            Some((ssid, signal)) => (true, ssid, signal),
            None => (
                !radios.is_empty() || unsafe { wlan_has_interface() },
                None,
                0,
            ),
        };
        if !has_adapter {
            return None;
        }
        let on = radio_on.unwrap_or(true);
        if !on {
            return Some(Wifi {
                on,
                ssid: None,
                signal: 0,
            });
        }
        Some(Wifi { on, ssid, signal })
    }

    // ---- brightness (DDC/CI on external monitors; laptop panels are not supported) ----

    fn with_monitor<T>(&mut self, f: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mons = [PHYSICAL_MONITOR::default()];
            if GetPhysicalMonitorsFromHMONITOR(hmon, &mut mons).is_err() {
                return None;
            }
            let out = f(mons[0].hPhysicalMonitor);
            let _ = DestroyPhysicalMonitors(&mons);
            out
        }
    }

    fn read_brightness(&mut self) -> Option<f32> {
        let r = self.with_monitor(|h| unsafe {
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            if GetMonitorBrightness(h, &mut min, &mut cur, &mut max) == 0 || max <= min {
                return None;
            }
            Some(((cur.clamp(min, max) - min) as f32) / ((max - min) as f32))
        });
        self.ddc = Some(r.is_some());
        r
    }

    fn set_brightness(&mut self, v: f32) -> bool {
        if self.ddc == Some(false) {
            return false;
        }
        let v = v.clamp(0.0, 1.0);
        let ok = self.with_monitor(|h| unsafe {
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            if GetMonitorBrightness(h, &mut min, &mut cur, &mut max) == 0 || max <= min {
                return None;
            }
            let target = min + (v * (max - min) as f32).round() as u32;
            // DDC/CI needs a pause between commands; a busy monitor drops the
            // write right after the read, so it is retried a few times.
            for _ in 0..3 {
                std::thread::sleep(Duration::from_millis(50));
                if SetMonitorBrightness(h, target) != 0 {
                    return Some(());
                }
            }
            None
        });
        if ok.is_none() {
            self.log_once("set-brightness", "DDC/CI set failed");
        }
        ok.is_some()
    }

    // ---- now playing ----

    fn session(&mut self) -> Option<GlobalSystemMediaTransportControlsSession> {
        if self.manager.is_none() {
            match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                .and_then(|o| o.join())
            {
                Ok(m) => self.manager = Some(m),
                Err(e) => {
                    self.log_once("gsmtc", e.to_string());
                    return None;
                }
            }
        }
        // Errors here mean "no session".
        self.manager.as_ref()?.GetCurrentSession().ok()
    }

    fn control(
        &mut self,
        f: impl FnOnce(&GlobalSystemMediaTransportControlsSession) -> windows::core::Result<()>,
    ) {
        if let Some(s) = self.session()
            && let Err(e) = f(&s)
        {
            self.log_once("control", e.to_string());
        }
    }

    fn read_playing(&mut self) -> Option<NowPlaying> {
        let Some(session) = self.session() else {
            self.track = None;
            self.art = None;
            return None;
        };
        let app = session
            .SourceAppUserModelId()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let props = match session.TryGetMediaPropertiesAsync().and_then(|o| o.join()) {
            Ok(p) => p,
            Err(e) => {
                self.log_once("media-props", e.to_string());
                return None;
            }
        };
        let title = props
            .Title()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let artist = props
            .Artist()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let playing = session
            .GetPlaybackInfo()
            .and_then(|i| i.PlaybackStatus())
            .is_ok_and(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing);

        let key = (title.clone(), artist.clone(), app.clone());
        if self.track.as_ref() != Some(&key) {
            self.track = Some(key);
            self.art = None;
            self.art_tries = 0;
        }
        // Players often attach the thumbnail a moment after the title, so retry a few polls.
        if self.art.is_none() && self.art_tries < 4 {
            self.art_tries += 1;
            if let Ok(thumb) = props.Thumbnail() {
                self.art = self.load_art(&thumb);
            }
        }
        if title.is_empty() && artist.is_empty() {
            return None;
        }
        Some(NowPlaying {
            title,
            artist,
            app,
            playing,
            art: self.art,
        })
    }

    fn load_art(&mut self, thumb: &IRandomAccessStreamReference) -> Option<ArtId> {
        let bytes = match read_stream(thumb) {
            Ok(b) => b,
            Err(e) => {
                self.log_once("art-read", e.to_string());
                return None;
            }
        };
        let rgba = match decode_image(&bytes) {
            Ok(r) => r,
            Err(e) => {
                self.log_once("art-decode", e.to_string());
                return None;
            }
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        let id = ArtId(h.finish());
        if let Ok(mut s) = self.shared.lock() {
            s.art.retain(|(i, _)| *i != id);
            s.art.push((id, rgba));
            if s.art.len() > ART_CACHE {
                s.art.remove(0);
            }
        }
        Some(id)
    }
}

fn read_stream(r: &IRandomAccessStreamReference) -> windows::core::Result<Vec<u8>> {
    let stream = r.OpenReadAsync()?.join()?;
    let size = stream.Size()?;
    if size == 0 || size > MAX_ART_BYTES {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(-1),
            "bad thumbnail size",
        ));
    }
    let reader = DataReader::CreateDataReader(&stream)?;
    let loaded = reader.LoadAsync(size as u32)?.join()?;
    let mut buf = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(buf)
}

fn decode_image(bytes: &[u8]) -> windows::core::Result<Rgba> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        // `bytes` outlives the stream and decoder, which are dropped before this function returns.
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            std::ptr::null(),
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        if w == 0 || h == 0 {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(-1),
                "empty image",
            ));
        }
        let mut source: IWICBitmapSource = conv.cast()?;
        if w.max(h) > MAX_ART_PX {
            let scale = MAX_ART_PX as f32 / w.max(h) as f32;
            let (nw, nh) = (
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
            );
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, nw, nh, WICBitmapInterpolationModeFant)?;
            source = scaler.cast()?;
            (w, h) = (nw, nh);
        }
        let mut data = vec![0u8; (w * h * 4) as usize];
        source.CopyPixels(std::ptr::null(), w * 4, &mut data)?;
        for px in data.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Ok(Rgba { w, h, data })
    }
}

// ---- WLAN ----

unsafe fn wlan_open() -> Option<HANDLE> {
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        (WlanOpenHandle(2, None, &mut ver, &mut h) == 0).then_some(h)
    }
}

unsafe fn wlan_interfaces(h: HANDLE) -> Option<*mut WLAN_INTERFACE_INFO_LIST> {
    unsafe {
        let mut list = std::ptr::null_mut();
        (WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null()).then_some(list)
    }
}

unsafe fn wlan_has_interface() -> bool {
    unsafe {
        let Some(h) = wlan_open() else { return false };
        let n = match wlan_interfaces(h) {
            Some(list) => {
                let n = (*list).dwNumberOfItems;
                WlanFreeMemory(list as *const _);
                n
            }
            None => 0,
        };
        WlanCloseHandle(h, None);
        n > 0
    }
}

/// SSID and signal quality of the first connected Wi-Fi interface.
unsafe fn wlan_connection(logged: &mut HashSet<String>) -> Option<(Option<String>, u8)> {
    unsafe {
        let Some(h) = wlan_open() else {
            if logged.insert("wlan-open".into()) {
                log("media: wlan-open: WlanOpenHandle failed (no WLAN service?)");
            }
            return None;
        };
        let mut result = None;
        if let Some(list) = wlan_interfaces(h) {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            for it in items {
                if it.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                let rc = WlanQueryInterface(
                    h,
                    &it.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                );
                if rc != 0 || data.is_null() {
                    continue;
                }
                let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                let assoc = &attrs.wlanAssociationAttributes;
                let len = (assoc.dot11Ssid.uSSIDLength as usize).min(32);
                let ssid = String::from_utf8_lossy(&assoc.dot11Ssid.ucSSID[..len]).into_owned();
                let signal = assoc.wlanSignalQuality.min(100) as u8;
                WlanFreeMemory(data as *const _);
                result = Some(((!ssid.is_empty()).then_some(ssid), signal));
                break;
            }
            WlanFreeMemory(list as *const _);
        }
        WlanCloseHandle(h, None);
        result
    }
}
