//! Host side of the menu-bar extras: tray icons, Control Center, Notification
//! Center, notification banners and the microphone/camera indicator. A child
//! module of `app`, so it works on `App` directly.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use macrust_ui::control::{ControlCenter, ControlEvent, ControlState, NowPlayingView};
use macrust_ui::gfx::Image;
use macrust_ui::menubar::{BarItem, TrayItem};
use macrust_ui::notify::{Banner, NoteEvent, NoteView, NotificationCenter, Today};
use macrust_ui::panel::MARGIN;
use macrust_ui::theme::Theme;
use macrust_ui::WinRect;
use macrust_ui::tiny_skia::Pixmap;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer, WS_EX_NOACTIVATE};

use super::super::capture::Capture;
use super::super::media::{ArtId, Command, Media, Snapshot};
use super::super::notifications::{self, Toast, Update, Watcher};
use super::super::surface::Surface;
use super::super::system;
use super::super::util::trace;
use super::super::tray::{Tray, TrayButton, TrayEvent};
use super::{App, ESC_ONLY, MENU_FROM_BAR, MENU_RECT, TIMER_BANNER, WM_APP_MEDIA, WM_APP_NOTES, WM_APP_TRAY, image_from};

/// Notifications kept for the Notification Center.
const KEEP: usize = 100;
const BANNER_MS: u32 = 5000;

pub(super) struct Extras {
    pub(super) tray: Option<Tray>,
    /// Per tray icon: picture hash and its templates.
    tray_images: HashMap<u64, (u64, Templates)>,
    /// Lower-case exe paths of apps with a tray icon: alive even without windows.
    pub(super) tray_exes: HashSet<String>,
    media: Media,
    snap: Snapshot,
    art: HashMap<ArtId, Rc<Image>>,
    watcher: Watcher,
    /// Newest first.
    toasts: Vec<Toast>,
    hidden: HashSet<i64>,
    cc: Option<ControlCenter>,
    cc_s: Surface,
    nc: Option<NotificationCenter>,
    nc_s: Surface,
    banner: Option<Banner>,
    banner_s: Surface,
    queue: VecDeque<Toast>,
    pub(super) privacy: (bool, bool),
    tick: u32,
    /// Live screen under the glass; None when Desktop Duplication is unavailable
    /// (then the glass keeps the picture taken when a panel opened).
    pub(super) capture: Option<Capture>,
    /// Per surface: hash of the last backdrop, to skip unchanged ones.
    backdrop_hashes: HashMap<isize, u64>,
    /// Earliest next live-glass update: heavy updates space themselves out.
    backdrop_next: Instant,
}

impl Extras {
    pub(super) fn new(notify: HWND) -> Self {
        Self {
            tray: None,
            tray_images: HashMap::new(),
            tray_exes: HashSet::new(),
            media: Media::start(notify, WM_APP_MEDIA),
            snap: Snapshot::default(),
            art: HashMap::new(),
            watcher: Watcher::start(notify, WM_APP_NOTES),
            toasts: Vec::new(),
            hidden: HashSet::new(),
            cc: None,
            cc_s: Surface::new(WS_EX_NOACTIVATE, "macRust Control Center"),
            nc: None,
            nc_s: Surface::new(WS_EX_NOACTIVATE, "macRust Notification Center"),
            banner: None,
            banner_s: Surface::new(WS_EX_NOACTIVATE, "macRust notification"),
            queue: VecDeque::new(),
            privacy: (false, false),
            tick: 0,
            capture: Capture::new(),
            backdrop_hashes: HashMap::new(),
            backdrop_next: Instant::now(),
        }
    }

    pub(super) fn panel_open(&self) -> bool {
        self.cc.as_ref().is_some_and(|c| !c.closing()) || self.nc.as_ref().is_some_and(|n| !n.closing())
    }

    pub(super) fn animating(&self) -> bool {
        self.cc.as_ref().is_some_and(|c| c.animating())
            || self.nc.as_ref().is_some_and(|n| n.animating())
            || self.banner.as_ref().is_some_and(|b| b.animating())
    }

    pub(super) fn dirty(&self) -> bool {
        self.cc_s.dirty || self.nc_s.dirty || self.banner_s.dirty
    }

    pub(super) fn owns(&self, h: HWND) -> bool {
        h == self.cc_s.hwnd || h == self.nc_s.hwnd || h == self.banner_s.hwnd
    }
}

/// A cheap fingerprint of a picture: every 61st pixel.
fn sample_hash(pm: &Pixmap) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (pm.width(), pm.height()).hash(&mut h);
    pm.data().chunks_exact(4).step_by(61).for_each(|p| p.hash(&mut h));
    h.finish()
}

/// (dark glyph, light glyph) versions of a tray icon.
type Templates = (Rc<Image>, Rc<Image>);

/// Menu-bar templates of a tray icon, as macOS shows menu extras.
fn tray_images(r: &super::super::icons::Rgba, exe: &str, tip: &str) -> Option<Templates> {
    let pm = system::pixmap_from_rgba(r.w, r.h, r.data.clone())?;
    let (dark, light) = macrust_ui::extras::templates(&pm, exe, tip)?;
    Some((Rc::new(Image::new(dark)), Rc::new(Image::new(light))))
}

fn hash_of(data: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

/// Shell parsing name of a notification sender: an exe path or an AppUserModelID.
fn app_parse_name(app_id: &str) -> String {
    if app_id.contains('\\') || app_id.to_lowercase().ends_with(".exe") {
        app_id.to_string()
    } else {
        format!("shell:AppsFolder\\{app_id}")
    }
}

/// Panels hang from the bar and their shadow margin reaches over it. Any pixel
/// with alpha takes clicks on a layered window, so the rows over the bar are
/// cleared: otherwise the panel swallows the click meant to close it.
pub(super) fn clear_rows(pm: &mut macrust_ui::tiny_skia::Pixmap, rows: i32) {
    let rows = rows.clamp(0, pm.height() as i32) as usize;
    let stride = pm.width() as usize * 4;
    pm.data_mut()[..rows * stride].fill(0);
}

fn minutes_since(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).map(|d| d.as_secs() / 60).unwrap_or(0)
}

impl App {
    pub(super) fn extras_start(&mut self) {
        if self.settings.tray_icons {
            self.x.tray = Tray::start(self.bar_s.hwnd, WM_APP_TRAY);
        }
        trace(format!("tray host started: {}", self.x.tray.is_some()));
        self.x.privacy = system::privacy_in_use();
    }

    /// Once a second, from the status timer.
    pub(super) fn extras_tick(&mut self) {
        self.x.tick += 1;
        if self.x.tick.is_multiple_of(2) {
            let p = system::privacy_in_use();
            if p != self.x.privacy {
                self.x.privacy = p;
                self.refresh_status();
            }
        }
        // A lost wake-up message must not freeze the tray.
        self.on_tray();
        if self.x.tick.is_multiple_of(5)
            && let Some(t) = &self.x.tray
        {
            t.keep_on_top();
            t.prune();
        }
        // Relative times in the open Notification Center age.
        if self.x.tick.is_multiple_of(30) && self.x.nc.is_some() {
            self.refresh_note_views();
        }
    }

    // -------------------------------------------------------------------
    // Live glass

    /// New screen content under a visible glass surface redraws its glass.
    pub(super) fn backdrop_tick(&mut self) {
        let t = Instant::now();
        if t < self.x.backdrop_next {
            return;
        }
        let Some(cap) = &mut self.x.capture else { return };
        if !cap.poll() {
            return;
        }
        let hashes = &mut self.x.backdrop_hashes;
        let mut fresh = |cap: &mut Capture, s: &Surface| -> Option<Pixmap> {
            if !s.shown {
                return None;
            }
            let pm = cap.region(s.rect)?;
            let h = sample_hash(&pm);
            (hashes.insert(s.hwnd.0 as isize, h) != Some(h)).then_some(pm)
        };
        if let Some(pm) = fresh(cap, &self.dock_s) {
            self.dock.set_backdrop(&pm);
            self.dock_s.dirty = true;
        }
        if let Some(m) = &mut self.menu
            && !m.closing()
            && let Some(pm) = fresh(cap, &self.menu_s)
        {
            m.set_backdrop(&pm);
            self.menu_s.dirty = true;
        }
        if let Some(c) = &mut self.x.cc
            && !c.closing()
            && let Some(pm) = fresh(cap, &self.x.cc_s)
        {
            c.set_backdrop(&pm);
            self.x.cc_s.dirty = true;
        }
        if let Some(n) = &mut self.x.nc
            && !n.closing()
            && let Some(pm) = fresh(cap, &self.x.nc_s)
        {
            n.set_backdrop(&pm);
            self.x.nc_s.dirty = true;
        }
        if let Some(b) = &mut self.x.banner
            && let Some(pm) = fresh(cap, &self.x.banner_s)
        {
            b.set_backdrop(&pm);
            self.x.banner_s.dirty = true;
        }
        self.render();
        self.stat("backdrop", t);
        // At most about a quarter of one core while things move behind glass.
        self.x.backdrop_next = Instant::now() + t.elapsed() * 3;
    }

    // -------------------------------------------------------------------
    // Tray

    pub(super) fn on_tray(&mut self) {
        let Some(t) = &self.x.tray else { return };
        // Balloons also reach the Windows notification database, which the
        // watcher reads, so they are announced from there only once.
        if t.take_events().iter().any(|e| matches!(e, TrayEvent::Changed)) {
            self.sync_tray();
        }
    }

    pub(super) fn sync_tray(&mut self) {
        let Some(t) = &self.x.tray else { return };
        let icons = t.icons();
        let mut items = Vec::with_capacity(icons.len());
        let mut exes = HashSet::new();
        for ic in &icons {
            if let Some(e) = &ic.owner_exe {
                exes.insert(e.to_lowercase());
            }
            // A picture that could not be read would leave an empty, clickable gap.
            if ic.icon.is_none() {
                continue;
            }
            // The tooltip picks the symbol of Explorer's icons, so it is part of the key.
            let stamp = ic.icon.as_ref().map(|r| hash_of(&r.data)).unwrap_or(0) ^ hash_of(ic.tip.as_bytes()).rotate_left(1);
            let exe = ic.owner_exe.as_deref().unwrap_or("");
            let images = match self.x.tray_images.get(&ic.id) {
                Some((s, imgs)) if *s == stamp => Some(imgs.clone()),
                _ => ic.icon.as_ref().and_then(|r| tray_images(r, exe, &ic.tip)),
            };
            if let Some(imgs) = &images {
                self.x.tray_images.insert(ic.id, (stamp, imgs.clone()));
            }
            items.push(TrayItem {
                id: ic.id,
                icon_dark: images.as_ref().map(|i| i.0.clone()),
                icon_light: images.as_ref().map(|i| i.1.clone()),
                stamp,
            });
        }
        self.x.tray_images.retain(|id, _| icons.iter().any(|i| i.id == *id));
        if self.bar.set_tray(items, &self.ctx) {
            self.bar_s.dirty = true;
        }
        self.tray_rects();
        if exes != self.x.tray_exes {
            self.x.tray_exes = exes;
            self.rebuild_dock();
        }
    }

    /// Where each icon is on screen, for apps that anchor flyouts to it.
    /// Items shift whenever the clock or status items change width.
    pub(super) fn tray_rects(&self) {
        let Some(t) = &self.x.tray else { return };
        let wr = self.bar.window_rect();
        for ic in t.icons() {
            if let Some((a, b)) = self.bar.item_rect(BarItem::Tray(ic.id)) {
                t.set_icon_rect(ic.id, RECT { left: wr.x + a as i32, top: wr.y, right: wr.x + b as i32, bottom: wr.y + self.bar.height_px() });
            }
        }
    }

    pub(super) fn tray_click(&mut self, id: u64, right: bool) {
        let Some(t) = &self.x.tray else { return };
        let (a, b) = self.bar.item_rect(BarItem::Tray(id)).unwrap_or((0.0, 0.0));
        let button = if right { TrayButton::Right } else { TrayButton::Left };
        let wr = self.bar.window_rect();
        t.click(id, button, wr.x + ((a + b) / 2.0) as i32, wr.y + self.bar.height_px());
    }

    pub(super) fn tray_hover(&mut self, id: u64) {
        let Some(t) = &self.x.tray else { return };
        let (a, b) = self.bar.item_rect(BarItem::Tray(id)).unwrap_or((0.0, 0.0));
        let wr = self.bar.window_rect();
        t.hover(id, wr.x + ((a + b) / 2.0) as i32, wr.y + self.bar.height_px() / 2);
    }

    // -------------------------------------------------------------------
    // Control Center

    pub(super) fn on_media(&mut self) {
        self.x.snap = self.x.media.snapshot();
        if self.x.cc.is_some() {
            let st = self.control_state();
            if let Some(cc) = &mut self.x.cc {
                cc.set_state(st);
            }
            self.x.cc_s.dirty = true;
        }
    }

    fn control_state(&mut self) -> ControlState {
        let snap = self.x.snap.clone();
        let art = snap.now_playing.as_ref().and_then(|p| p.art).and_then(|id| {
            if let Some(img) = self.x.art.get(&id) {
                return Some(img.clone());
            }
            let img = self.x.media.artwork(id).and_then(|r| image_from(r, false))?;
            // One cover at a time is all the panel shows.
            self.x.art.clear();
            self.x.art.insert(id, img.clone());
            Some(img)
        });
        ControlState {
            wifi: snap.wifi.map(|w| (w.on, w.ssid)),
            bluetooth: snap.bluetooth,
            volume: snap.volume,
            muted: snap.muted,
            brightness: snap.brightness,
            playing: snap.now_playing.map(|p| NowPlayingView {
                title: if p.title.is_empty() { p.app.clone() } else { p.title },
                artist: p.artist,
                playing: p.playing,
                art,
            }),
            dark: self.ctx.theme.dark,
        }
    }

    /// Window rect for Notification Center and banners: Tahoe hangs them in a
    /// column whose right edge lines up with the clock's text (18.5 pt from
    /// the screen edge), 15 pt below the bar.
    fn column_rect(&self, win: (i32, i32)) -> WinRect {
        let s = self.screen.scale;
        let m = (MARGIN * s).round() as i32;
        let x = self.screen.w - (18.5 * s).round() as i32 - win.0 + m;
        let y = self.bar.height_px() + (15.0 * s).round() as i32 - m;
        WinRect { x, y, w: win.0, h: win.1 }
    }

    fn arm_outside_click(&mut self, r: WinRect) {
        let m = (MARGIN * self.screen.scale).round() as i32;
        self.arm_outside_body(WinRect { x: r.x + m, y: r.y + m, w: r.w - 2 * m, h: r.h - 2 * m });
    }

    /// Clicks outside `body` (screen rect) close the panels.
    fn arm_outside_body(&mut self, b: WinRect) {
        for (a, v) in MENU_RECT.iter().zip([b.x, b.y, b.x + b.w, b.y + b.h]) {
            a.store(v, std::sync::atomic::Ordering::Relaxed);
        }
        MENU_FROM_BAR.store(true, std::sync::atomic::Ordering::Relaxed);
        ESC_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
        self.unhook_menu();
        self.hook_menu();
    }

    pub(super) fn toggle_control(&mut self) {
        trace(format!("control center toggle: open {:?}", self.x.cc.as_ref().map(|c| !c.closing())));
        // A click on the item while the panel is up first closes it through
        // the bar's CloseMenus; the panel is then on its way out.
        if self.x.cc.is_some() {
            self.close_panels(true);
            return;
        }
        self.close_menu(false);
        self.x.media.send(Command::Refresh);
        let st = self.control_state();
        let mut cc = ControlCenter::new(st, self.screen.scale);
        let r = cc.window_rect(self.screen.w, self.bar.height_px());
        let body = cc.body_rect(r);
        // The glass blurs what is under the panel; taken once, before it shows.
        if let Some(behind) = system::capture(r) {
            cc.set_backdrop(&behind);
        }
        self.x.cc = Some(cc);
        self.x.cc_s.prepare(r);
        self.x.cc_s.dirty = true;
        self.bar.set_open(Some(BarItem::Control));
        self.bar_s.dirty = true;
        self.arm_outside_body(body);
        self.render();
        self.x.cc_s.raise();
    }

    fn control_events(&mut self) {
        let Some(cc) = &mut self.x.cc else { return };
        let events = std::mem::take(&mut cc.events);
        for e in events {
            match e {
                ControlEvent::Wifi(on) => self.x.media.send(Command::SetWifi(on)),
                ControlEvent::Bluetooth(on) => self.x.media.send(Command::SetBluetooth(on)),
                ControlEvent::Volume(v) => self.x.media.send(Command::SetVolume(v)),
                ControlEvent::Muted(m) => self.x.media.send(Command::SetMuted(m)),
                ControlEvent::Brightness(v) => self.x.media.send(Command::SetBrightness(v)),
                ControlEvent::PlayPause => self.x.media.send(Command::PlayPause),
                ControlEvent::Next => self.x.media.send(Command::Next),
                ControlEvent::Previous => self.x.media.send(Command::Previous),
                ControlEvent::DarkMode(dark) => {
                    system::set_system_dark(dark);
                    if self.settings.appearance != "auto" {
                        // A fixed appearance in the settings would ignore the system switch.
                        self.settings.appearance = "auto".into();
                    }
                    self.ctx.theme = Theme { dark };
                    self.configure(false);
                    self.rebuild_dock();
                    self.x.cc_s.dirty = true;
                }
                ControlEvent::Screenshot => {
                    self.close_panels(true);
                    system::open("ms-screenclip:");
                }
                ControlEvent::Calculator => {
                    self.close_panels(true);
                    system::open("calc.exe");
                }
                ControlEvent::Lock => {
                    self.close_panels(false);
                    system::power(system::Power::Lock);
                }
                ControlEvent::Settings(page) => {
                    self.close_panels(true);
                    system::open(match page {
                        "wifi" => "ms-settings:network-wifi",
                        "bluetooth" => "ms-settings:bluetooth",
                        "display" => "ms-settings:display",
                        "" => "ms-settings:",
                        _ => "ms-settings:sound",
                    });
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Notifications

    fn today(&self) -> Today {
        let (year, month, day, weekday) = system::today();
        Today { year, month, day, weekday }
    }

    fn note_view(&mut self, t: &Toast) -> NoteView {
        let icon_key = format!("note:{}", t.app_id.to_lowercase());
        self.request_icon(icon_key.clone(), app_parse_name(&t.app_id));
        let icon = self.small_icons.get(&icon_key).cloned().flatten();
        let image = t.image.as_ref().and_then(|p| {
            let path = p.to_string_lossy().into_owned();
            let key = format!("img:{}", path.to_lowercase());
            self.request_image(key.clone(), path, true);
            self.small_icons.get(&key).cloned().flatten()
        });
        NoteView {
            id: t.id,
            app: notifications::app_name(&t.app_id),
            title: t.title.clone(),
            body: t.body.clone(),
            time: self.ctx.strings.ago(minutes_since(t.arrival)),
            icon,
            image,
        }
    }

    fn visible_toasts(&self) -> Vec<Toast> {
        self.x.toasts.iter().filter(|t| !self.x.hidden.contains(&t.id)).cloned().collect()
    }

    pub(super) fn on_notes(&mut self) {
        let mut changed = false;
        while let Ok(u) = self.x.watcher.rx.try_recv() {
            changed = true;
            match u {
                Update::Initial(v) => self.x.toasts = v,
                Update::New(v) => {
                    for t in v {
                        if self.settings.banners && !self.fullscreen && self.x.nc.is_none() && self.x.queue.len() < 3 {
                            self.x.queue.push_back(t.clone());
                        }
                        self.x.toasts.insert(0, t);
                    }
                    self.x.toasts.truncate(KEEP);
                }
                Update::Removed(ids) => {
                    self.x.toasts.retain(|t| !ids.contains(&t.id));
                    self.x.queue.retain(|t| !ids.contains(&t.id));
                }
            }
        }
        if changed {
            self.refresh_note_views();
            self.next_banner();
        }
    }

    /// Icons and avatars arrive later than the text; views pick them up here.
    pub(super) fn refresh_note_views(&mut self) {
        if self.x.nc.is_some() {
            let notes: Vec<NoteView> = self.visible_toasts().iter().map(|t| self.note_view(t)).collect();
            let today = self.today();
            if let Some(nc) = &mut self.x.nc {
                nc.set_notes(notes, today);
            }
            self.x.nc_s.dirty = true;
        }
        if let Some(id) = self.x.banner.as_ref().map(|b| b.note.id)
            && let Some(t) = self.x.toasts.iter().find(|t| t.id == id).cloned()
        {
            let v = self.note_view(&t);
            if let Some(b) = &mut self.x.banner {
                b.note = v;
            }
            self.x.banner_s.dirty = true;
        }
    }

    fn next_banner(&mut self) {
        if self.x.banner.is_some() {
            return;
        }
        let Some(t) = self.x.queue.pop_front() else { return };
        let v = self.note_view(&t);
        let mut b = Banner::new(v, self.screen.scale);
        let r = self.column_rect(b.window_size(&self.ctx));
        if let Some(behind) = system::capture(r) {
            b.set_backdrop(&behind);
        }
        self.x.banner = Some(b);
        self.x.banner_s.prepare(r);
        self.x.banner_s.dirty = true;
        self.render();
        self.x.banner_s.raise();
        unsafe {
            SetTimer(Some(self.bar_s.hwnd), TIMER_BANNER, BANNER_MS, None);
        }
    }

    /// Banner time is up, unless the pointer rests on it (then it asks again).
    pub(super) fn banner_timer(&mut self) {
        match &mut self.x.banner {
            Some(b) if b.hovered() => {}
            Some(b) => {
                b.close();
                self.stop_banner_timer();
            }
            None => self.stop_banner_timer(),
        }
    }

    fn stop_banner_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.bar_s.hwnd), TIMER_BANNER);
        }
    }

    pub(super) fn toggle_notes(&mut self) {
        trace(format!("notification center toggle: open {:?}", self.x.nc.as_ref().map(|n| !n.closing())));
        if self.x.nc.is_some() {
            self.close_panels(true);
            return;
        }
        self.close_menu(false);
        if let Some(b) = &mut self.x.banner {
            b.close();
        }
        self.x.queue.clear();
        let notes: Vec<NoteView> = self.visible_toasts().iter().map(|t| self.note_view(t)).collect();
        let max_h = (self.screen.h - self.bar.height_px()) as f32 - self.dock.reserved_height() as f32;
        let mut nc = NotificationCenter::new(notes, self.today(), self.screen.scale, max_h);
        let r = self.column_rect(nc.window_size(&self.ctx));
        if let Some(behind) = system::capture(r) {
            nc.set_backdrop(&behind);
        }
        self.x.nc = Some(nc);
        self.x.nc_s.prepare(r);
        self.x.nc_s.dirty = true;
        self.bar.set_open(Some(BarItem::Clock));
        self.bar_s.dirty = true;
        self.arm_outside_click(r);
        self.render();
        self.x.nc_s.raise();
    }

    /// Brings the sender forward; the notification is done with.
    fn open_note(&mut self, id: i64) {
        self.x.hidden.insert(id);
        let Some(t) = self.x.toasts.iter().find(|t| t.id == id).cloned() else { return };
        let key = super::tracker::app_key(&t.app_id, None);
        let aumid_key = format!("aumid:{}", t.app_id.to_lowercase());
        let app = self.running.iter().find(|a| a.key == key || a.key == aumid_key).cloned();
        match app {
            Some(a) => {
                let w = a.windows.iter().copied().find(|&w| !super::winops::is_minimized(w)).unwrap_or(a.windows[0]);
                super::winops::activate(w);
            }
            None => system::open(&app_parse_name(&t.app_id)),
        }
    }

    fn note_events(&mut self) {
        let mut events = Vec::new();
        if let Some(nc) = &mut self.x.nc {
            events.append(&mut nc.events);
        }
        if let Some(b) = &mut self.x.banner {
            events.append(&mut b.events);
        }
        for e in events {
            match e {
                NoteEvent::Open(id) => {
                    self.close_panels(true);
                    self.open_note(id);
                }
                NoteEvent::Dismiss(id) => {
                    self.x.hidden.insert(id);
                    self.x.nc_s.dirty = true;
                }
                NoteEvent::ClearAll => {
                    let ids: Vec<i64> = self.x.toasts.iter().map(|t| t.id).collect();
                    self.x.hidden.extend(ids);
                    self.x.nc_s.dirty = true;
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Diagnostic report (no titles, texts or network names)

    pub(super) fn report_extras(&self, info: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(info, "\n[tray] hosted {}", self.x.tray.is_some());
        if let Some(t) = &self.x.tray {
            for (n, ic) in t.icons().iter().enumerate() {
                let size = ic.icon.as_ref().map(|r| format!("{}x{}", r.w, r.h)).unwrap_or_else(|| "no picture".into());
                let owner = ic.owner_exe.as_deref().map(super::report::file_name).unwrap_or_else(|| "?".into());
                // The source picture, to check the menu-bar conversion against
                // real icons; the tooltip stays out (it can carry chat names).
                let pm = ic.icon.as_ref().and_then(|r| system::pixmap_from_rgba(r.w, r.h, r.data.clone()));
                let symbol = pm.as_ref().and_then(|pm| macrust_ui::extras::identify(&owner, &ic.tip, pm));
                if let Some(pm) = &pm {
                    let _ = pm.save_png(super::report::dir().join(format!("tray-{n}.png")));
                }
                let _ = writeln!(info, "{owner} | {size} | tray-{n}.png | symbol {symbol:?}");
            }
        }
        let m = &self.x.snap;
        let _ = writeln!(
            info,
            "\n[media] volume {:?} muted {} wifi {:?} bluetooth {:?} brightness {} now playing {:?}",
            m.volume,
            m.muted,
            m.wifi.as_ref().map(|w| (w.on, w.ssid.is_some(), w.signal)),
            m.bluetooth,
            m.brightness.is_some(),
            m.now_playing.as_ref().map(|p| (p.app.clone(), p.playing, p.art.is_some()))
        );
        let _ = writeln!(info, "\n[notifications] {} kept, {} dismissed here", self.x.toasts.len(), self.x.hidden.len());
        let mut apps: Vec<&str> = self.x.toasts.iter().map(|t| t.app_id.as_str()).collect();
        apps.sort_unstable();
        apps.dedup();
        for a in apps {
            let _ = writeln!(info, "{}", super::report::file_name(a));
        }
        let _ = writeln!(info, "\n[privacy] microphone {} camera {}", self.x.privacy.0, self.x.privacy.1);
        let _ = writeln!(info, "[panels] control open {} notifications open {} banner {}", self.x.cc.is_some(), self.x.nc.is_some(), self.x.banner.is_some());
    }

    pub(super) fn report_images(&mut self, wall: &macrust_ui::tiny_skia::Pixmap, dir: &std::path::Path, notes: &mut String) {
        use macrust_ui::tiny_skia::Pixmap;
        let s = self.screen.scale;
        let mut st = self.control_state();
        if let Some((_, ssid)) = &mut st.wifi
            && ssid.is_some()
        {
            *ssid = Some("Network".into());
        }
        if let Some(p) = &mut st.playing {
            p.title = "Title".into();
            p.artist = "Artist".into();
        }
        let mut cc = ControlCenter::new(st, s);
        for _ in 0..240 {
            cc.update(1.0 / 60.0);
        }
        let crop = |r: WinRect| {
            let mut behind = Pixmap::new(r.w.max(1) as u32, r.h.max(1) as u32)?;
            behind.draw_pixmap(-r.x, -r.y, wall.as_ref(), &macrust_ui::tiny_skia::PixmapPaint::default(), macrust_ui::tiny_skia::Transform::identity(), None);
            Some(behind)
        };
        let r = cc.window_rect(self.screen.w, self.bar.height_px());
        if let Some(behind) = crop(r) {
            cc.set_backdrop(&behind);
        }
        if let Some(mut pm) = Pixmap::new(r.w as u32, r.h as u32) {
            cc.draw(&mut pm, &self.ctx);
            if let Some(out) = super::report::over_wallpaper(wall, &pm, r) {
                let _ = out.save_png(dir.join("control-center.png")).map_err(|e| notes.push_str(&format!("control: {e}\n")));
            }
        }
        let mut views: Vec<NoteView> = self.visible_toasts().iter().take(6).map(|t| self.note_view(t)).collect();
        for v in &mut views {
            v.title = "Title".into();
            v.body = "Message text".into();
            v.image = None;
        }
        let max_h = (self.screen.h - self.bar.height_px()) as f32 - self.dock.reserved_height() as f32;
        let mut nc = NotificationCenter::new(views, self.today(), s, max_h);
        for _ in 0..240 {
            nc.update(1.0 / 60.0);
        }
        let r = self.column_rect(nc.window_size(&self.ctx));
        if let Some(behind) = crop(r) {
            nc.set_backdrop(&behind);
        }
        if let Some(mut pm) = Pixmap::new(r.w as u32, r.h as u32) {
            nc.draw(&mut pm, &self.ctx);
            if let Some(out) = super::report::over_wallpaper(wall, &pm, r) {
                let _ = out.save_png(dir.join("notification-center.png")).map_err(|e| notes.push_str(&format!("notifications: {e}\n")));
            }
        }
        // The status area of the bar, enlarged, to judge tray icons pixel by pixel.
        let br = self.bar.window_rect();
        if let Some(mut pm) = Pixmap::new(br.w.max(1) as u32, br.h.max(1) as u32) {
            self.bar.draw(&mut pm, &self.ctx);
            if let Some(full) = super::report::over_wallpaper(wall, &pm, br) {
                let cw = (700.0 * s).min(full.width() as f32) as u32;
                let k = 3u32;
                if let Some(mut zoom) = Pixmap::new(cw * k, full.height() * k) {
                    let src = full.data();
                    let fw = full.width();
                    let x0 = fw - cw;
                    let zw = cw * k;
                    let d = zoom.data_mut();
                    for y in 0..full.height() * k {
                        for x in 0..zw {
                            let si = (((y / k) * fw + x0 + x / k) * 4) as usize;
                            let di = ((y * zw + x) * 4) as usize;
                            d[di..di + 4].copy_from_slice(&src[si..si + 4]);
                        }
                    }
                    let _ = zoom.save_png(dir.join("menu-bar-right-x3.png")).map_err(|e| notes.push_str(&format!("zoom: {e}\n")));
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Panels: lifetime, input, frames

    pub(super) fn close_panels(&mut self, animated: bool) {
        let was_open = self.x.cc.is_some() || self.x.nc.is_some();
        if was_open {
            trace(format!("panels close (animated {animated})"));
        }
        if animated {
            if let Some(c) = &mut self.x.cc {
                c.close();
            }
            if let Some(n) = &mut self.x.nc {
                n.close();
            }
        } else {
            if self.x.cc.take().is_some() {
                self.x.cc_s.hide();
            }
            if self.x.nc.take().is_some() {
                self.x.nc_s.hide();
            }
        }
        if was_open && matches!(self.bar.open_item(), Some(BarItem::Control | BarItem::Clock)) {
            self.bar.set_open(None);
            self.bar_s.dirty = true;
        }
    }

    /// Mouse input of the panel windows. None: not ours.
    pub(super) fn panel_mouse(&mut self, hwnd: HWND, msg: u32, x: f32, y: f32) -> Option<()> {
        use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
        const WM_MOUSELEAVE: u32 = 0x02A3;
        if msg != WM_MOUSEMOVE {
            trace(format!("panel input {msg:#x} at {x:.0},{y:.0}"));
        }
        if hwnd == self.x.cc_s.hwnd {
            let cc = self.x.cc.as_mut()?;
            match msg {
                WM_MOUSEMOVE => {
                    self.x.cc_s.track_leave();
                    cc.mouse_move(x, y);
                }
                WM_MOUSELEAVE => {
                    self.x.cc_s.left();
                    cc.mouse_leave();
                }
                WM_LBUTTONDOWN => {
                    cc.mouse_down(x, y);
                    // Slider drags keep working when the pointer leaves the panel.
                    unsafe {
                        windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.x.cc_s.hwnd);
                    }
                }
                WM_LBUTTONUP => {
                    unsafe {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                    }
                    cc.mouse_up(x, y);
                }
                _ => return None,
            }
            self.x.cc_s.dirty = true;
            self.control_events();
            return Some(());
        }
        if hwnd == self.x.nc_s.hwnd {
            let nc = self.x.nc.as_mut()?;
            match msg {
                WM_MOUSEMOVE => {
                    self.x.nc_s.track_leave();
                    nc.mouse_move(&self.ctx, x, y);
                }
                WM_MOUSELEAVE => {
                    self.x.nc_s.left();
                    nc.mouse_leave();
                }
                WM_LBUTTONDOWN => nc.mouse_down(&self.ctx, x, y),
                WM_LBUTTONUP => nc.mouse_up(&self.ctx, x, y),
                _ => return None,
            }
            self.x.nc_s.dirty = true;
            self.note_events();
            return Some(());
        }
        if hwnd == self.x.banner_s.hwnd {
            let b = self.x.banner.as_mut()?;
            match msg {
                WM_MOUSEMOVE => {
                    self.x.banner_s.track_leave();
                    b.mouse_move(&self.ctx, x, y);
                }
                WM_MOUSELEAVE => {
                    self.x.banner_s.left();
                    b.mouse_leave();
                }
                WM_LBUTTONDOWN => b.mouse_down(),
                WM_LBUTTONUP => b.mouse_up(),
                _ => return None,
            }
            self.x.banner_s.dirty = true;
            self.note_events();
            return Some(());
        }
        None
    }

    pub(super) fn extras_update(&mut self, dt: f32) {
        if let Some(c) = &mut self.x.cc
            && c.animating()
        {
            c.update(dt);
            self.x.cc_s.dirty = true;
        }
        if let Some(n) = &mut self.x.nc
            && n.animating()
        {
            n.update(dt);
            self.x.nc_s.dirty = true;
        }
        if let Some(b) = &mut self.x.banner
            && b.animating()
        {
            b.update(dt);
            self.x.banner_s.dirty = true;
        }
        if self.x.cc.as_ref().is_some_and(|c| c.finished()) {
            self.x.cc = None;
            self.x.cc_s.hide();
        }
        if self.x.nc.as_ref().is_some_and(|n| n.finished()) {
            self.x.nc = None;
            self.x.nc_s.hide();
        }
        if self.x.banner.as_ref().is_some_and(|b| b.finished()) {
            self.x.banner = None;
            self.x.banner_s.hide();
            self.next_banner();
        }
    }

    pub(super) fn extras_render(&mut self) {
        let bar_h = self.bar.height_px();
        if self.x.cc_s.dirty
            && let Some(cc) = &self.x.cc
        {
            let r = cc.window_rect(self.screen.w, self.bar.height_px());
            self.x.cc_s.prepare(r);
            cc.draw(&mut self.x.cc_s.pm, &self.ctx);
            clear_rows(&mut self.x.cc_s.pm, bar_h - r.y);
            self.x.cc_s.present();
        }
        if self.x.nc_s.dirty
            && let Some(nc) = &self.x.nc
        {
            let r = self.column_rect(nc.window_size(&self.ctx));
            self.x.nc_s.prepare(r);
            nc.draw(&mut self.x.nc_s.pm, &self.ctx);
            clear_rows(&mut self.x.nc_s.pm, bar_h - r.y);
            self.x.nc_s.present();
        }
        if self.x.banner_s.dirty
            && let Some(b) = &self.x.banner
        {
            let r = self.column_rect(b.window_size(&self.ctx));
            self.x.banner_s.prepare(r);
            b.draw(&mut self.x.banner_s.pm, &self.ctx);
            clear_rows(&mut self.x.banner_s.pm, bar_h - r.y);
            self.x.banner_s.present();
        }
    }
}
