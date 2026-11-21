//! The networks of the Wi-Fi menu, from the Native Wifi API (wlanapi) on a
//! worker thread: list, scan, join with a stored profile (or a temporary one
//! for an open network) and disconnect. The radio switch itself goes through
//! `media` (Windows.Devices.Radios). SSIDs and profile names never reach the
//! log or the report.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_SSID, L2_NOTIFICATION_DATA, WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS,
    WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM, WlanCloseHandle, WlanConnect, WlanDisconnect, WlanEnumInterfaces,
    WlanFreeMemory, WlanGetAvailableNetworkList, WlanOpenHandle, WlanRegisterNotification, WlanScan, dot11_BSS_type_any,
    dot11_BSS_type_infrastructure, wlan_connection_mode_profile, wlan_connection_mode_temporary_profile,
    wlan_notification_acm_connection_attempt_fail, wlan_notification_acm_connection_complete, wlan_notification_acm_disconnected,
    wlan_notification_acm_interface_arrival, wlan_notification_acm_interface_removal, wlan_notification_acm_scan_complete,
    wlan_notification_acm_scan_fail,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{GUID, PCWSTR};

use super::util::{hwnd_from_isize, hwnd_to_isize, log, wide};
use crate::wifi_logic::{self, Seen};

/// Windows needs 2-4 s for a scan; past this the list is taken as it is.
const SCAN_LIMIT: Duration = Duration::from_secs(8);

pub enum Cmd {
    /// The menu opened (true) or closed: WLAN events refresh the list only while it is open.
    Active(bool),
    Refresh,
    Scan,
    /// Join the network with this id (`wifi_logic::id_of`).
    Connect(u64),
    Disconnect,
    /// A WLAN notification code (ACM source), from the callback thread.
    Event(u32),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The first answer arrived.
    pub ready: bool,
    /// The WLAN service answered (WlanOpenHandle succeeded).
    pub service: bool,
    pub interfaces: usize,
    pub networks: Vec<Seen>,
    pub scanning: bool,
}

pub struct WifiList {
    shared: Arc<Mutex<Snapshot>>,
    tx: Sender<Cmd>,
}

impl WifiList {
    pub fn start(notify: HWND, msg: u32) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel();
        let (s, t, hwnd) = (shared.clone(), tx.clone(), hwnd_to_isize(notify));
        let spawned = std::thread::Builder::new().name("wifi".into()).spawn(move || Worker::new(s, t, hwnd, msg).run(rx));
        if let Err(e) = spawned {
            log(format!("wifi: cannot start worker: {e}"));
        }
        Self { shared, tx }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Worker {
    shared: Arc<Mutex<Snapshot>>,
    /// Handed to the WLAN callback; lives as long as the process.
    tx: Sender<Cmd>,
    hwnd: isize,
    msg: u32,
    handle: Option<HANDLE>,
    snap: Snapshot,
    active: bool,
    scan_since: Option<Instant>,
    logged: std::collections::HashSet<&'static str>,
}

/// Runs on a WLAN thread-pool thread: hand the event to the worker.
unsafe extern "system" fn on_wlan(data: *mut L2_NOTIFICATION_DATA, ctx: *mut core::ffi::c_void) {
    if data.is_null() || ctx.is_null() {
        return;
    }
    unsafe {
        if (*data).NotificationSource == WLAN_NOTIFICATION_SOURCE_ACM {
            let tx = &*(ctx as *const Sender<Cmd>);
            let _ = tx.send(Cmd::Event((*data).NotificationCode));
        }
    }
}

impl Worker {
    fn new(shared: Arc<Mutex<Snapshot>>, tx: Sender<Cmd>, hwnd: isize, msg: u32) -> Self {
        Self { shared, tx, hwnd, msg, handle: None, snap: Snapshot::default(), active: false, scan_since: None, logged: Default::default() }
    }

    fn log_once(&mut self, key: &'static str, msg: String) {
        if self.logged.insert(key) {
            log(format!("wifi: {key}: {msg}"));
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            let wait = if self.scan_since.is_some() { Duration::from_millis(500) } else { Duration::from_secs(3600) };
            let mut cmds = Vec::new();
            match rx.recv_timeout(wait) {
                Ok(c) => cmds.push(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            cmds.extend(rx.try_iter());
            let mut refresh = false;
            for c in cmds {
                match c {
                    Cmd::Active(on) => {
                        self.active = on;
                        refresh |= on;
                    }
                    Cmd::Refresh => refresh = true,
                    Cmd::Scan => self.scan(),
                    Cmd::Connect(id) => self.connect(id),
                    Cmd::Disconnect => self.disconnect(),
                    Cmd::Event(code) => refresh |= self.event(code),
                }
            }
            if self.scan_since.is_some_and(|t| t.elapsed() > SCAN_LIMIT) {
                self.scan_since = None;
                refresh = true;
            }
            if refresh {
                self.refresh();
            }
        }
    }

    /// Whether a WLAN event should refresh the list.
    fn event(&mut self, code: u32) -> bool {
        let code = code as i32;
        if code == wlan_notification_acm_scan_complete.0 || code == wlan_notification_acm_scan_fail.0 {
            self.scan_since = None;
            return self.active;
        }
        let changes = [
            wlan_notification_acm_connection_complete.0,
            wlan_notification_acm_connection_attempt_fail.0,
            wlan_notification_acm_disconnected.0,
            wlan_notification_acm_interface_arrival.0,
            wlan_notification_acm_interface_removal.0,
        ];
        self.active && changes.contains(&code)
    }

    fn handle(&mut self) -> Option<HANDLE> {
        if self.handle.is_some() {
            return self.handle;
        }
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        let rc = unsafe { WlanOpenHandle(2, None, &mut ver, &mut h) };
        if rc != 0 {
            self.log_once("open", format!("WlanOpenHandle failed ({rc}): WLAN AutoConfig is not running"));
            return None;
        }
        // The callback keeps a pointer to this sender for the life of the handle,
        // which is the life of the process.
        let ctx = Box::into_raw(Box::new(self.tx.clone())) as *const core::ffi::c_void;
        let rc = unsafe { WlanRegisterNotification(h, WLAN_NOTIFICATION_SOURCE_ACM, true, Some(on_wlan), Some(ctx), None, None) };
        if rc != 0 {
            self.log_once("notify", format!("WlanRegisterNotification failed ({rc}); scans end by timeout"));
        }
        self.handle = Some(h);
        Some(h)
    }

    /// The first Wi-Fi interface and how many there are.
    fn interface(&mut self, h: HANDLE) -> (Option<GUID>, usize) {
        unsafe {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let rc = WlanEnumInterfaces(h, None, &mut list);
            if rc != 0 || list.is_null() {
                self.log_once("interfaces", format!("WlanEnumInterfaces failed ({rc})"));
                return (None, 0);
            }
            let n = (*list).dwNumberOfItems as usize;
            let first = (n > 0).then(|| (*list).InterfaceInfo[0].InterfaceGuid);
            WlanFreeMemory(list as *const _);
            (first, n)
        }
    }

    fn refresh(&mut self) {
        let mut snap = Snapshot { ready: true, scanning: self.scan_since.is_some(), ..Snapshot::default() };
        if let Some(h) = self.handle() {
            snap.service = true;
            let (guid, n) = self.interface(h);
            snap.interfaces = n;
            if let Some(guid) = guid {
                snap.networks = self.networks(h, &guid);
            }
        }
        if snap != self.snap {
            self.publish(snap);
        }
    }

    fn networks(&mut self, h: HANDLE, guid: &GUID) -> Vec<Seen> {
        unsafe {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
            let rc = WlanGetAvailableNetworkList(h, guid, 0, None, &mut list);
            if rc != 0 || list.is_null() {
                // Also the answer while the radio is off.
                self.log_once("list", format!("WlanGetAvailableNetworkList failed ({rc})"));
                return Vec::new();
            }
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), n);
            let seen = items.iter().filter(|it| it.dot11BssType == dot11_BSS_type_infrastructure).map(|it| {
                let len = (it.dot11Ssid.uSSIDLength as usize).min(32);
                let end = it.strProfileName.iter().position(|&c| c == 0).unwrap_or(it.strProfileName.len());
                Seen {
                    ssid: it.dot11Ssid.ucSSID[..len].to_vec(),
                    secure: it.bSecurityEnabled.as_bool(),
                    signal: it.wlanSignalQuality.min(100) as u8,
                    profile: (end > 0).then(|| String::from_utf16_lossy(&it.strProfileName[..end])),
                    connected: it.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                }
            });
            let out = wifi_logic::merge(seen.collect::<Vec<_>>());
            WlanFreeMemory(list as *const _);
            out
        }
    }

    fn publish(&mut self, snap: Snapshot) {
        self.snap = snap.clone();
        if let Ok(mut s) = self.shared.lock() {
            *s = snap;
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd_from_isize(self.hwnd)), self.msg, WPARAM(0), LPARAM(0));
        }
    }

    fn scan(&mut self) {
        let Some(h) = self.handle() else { return self.refresh() };
        let (Some(guid), _) = self.interface(h) else { return self.refresh() };
        let rc = unsafe { WlanScan(h, &guid, None, None, None) };
        if rc == 0 {
            self.scan_since = Some(Instant::now());
        } else {
            self.log_once("scan", format!("WlanScan failed ({rc})"));
        }
        // The cached list right away; the fresh one after the scan.
        self.refresh();
    }

    fn connect(&mut self, id: u64) {
        let Some(net) = self.snap.networks.iter().find(|n| wifi_logic::id_of(&n.ssid) == id).cloned() else { return };
        let Some(h) = self.handle() else { return };
        let (Some(guid), _) = self.interface(h) else { return };
        // Profile names and XML must outlive the call.
        let (mode, text, bss) = match &net.profile {
            Some(p) => (wlan_connection_mode_profile, wide(p), dot11_BSS_type_infrastructure),
            None if !net.secure => (wlan_connection_mode_temporary_profile, wide(&wifi_logic::open_profile(&net.ssid)), dot11_BSS_type_any),
            // A password is needed: the host opens Windows' own sheet instead.
            None => return,
        };
        let mut ssid = DOT11_SSID { uSSIDLength: net.ssid.len().min(32) as u32, ..Default::default() };
        ssid.ucSSID[..net.ssid.len().min(32)].copy_from_slice(&net.ssid[..net.ssid.len().min(32)]);
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: mode,
            strProfile: PCWSTR(text.as_ptr()),
            pDot11Ssid: &mut ssid,
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: bss,
            dwFlags: 0,
        };
        let rc = unsafe { WlanConnect(h, &guid, &params, None) };
        if rc != 0 {
            log(format!("wifi: WlanConnect failed ({rc}, {} profile)", if net.profile.is_some() { "stored" } else { "temporary" }));
        }
    }

    fn disconnect(&mut self) {
        let Some(h) = self.handle() else { return };
        let (Some(guid), _) = self.interface(h) else { return };
        let rc = unsafe { WlanDisconnect(h, &guid, None) };
        if rc != 0 {
            log(format!("wifi: WlanDisconnect failed ({rc})"));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            unsafe {
                WlanCloseHandle(h, None);
            }
        }
    }
}
