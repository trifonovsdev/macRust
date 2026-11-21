//! Control Center in the macOS 26 layout: glass modules on a 60 pt grid that
//! float under the right end of the menu bar, with no panel behind them.
//! Sizes, positions and the material come from measured Tahoe 26 frames
//! (`out/ref/TAHOE-SPEC.md`, section 4).

use std::cell::{Cell, RefCell};
use std::f32::consts::{FRAC_PI_4, PI, TAU};
use std::rc::Rc;

use tiny_skia::{Color, FilterQuality, Path, PathBuilder, Pattern, Pixmap, SpreadMode, Transform};

use crate::anim::{Tween, smoothstep};
use crate::gfx::{self, Image, rgba};
use crate::glass::{self, Backdrop, Rim, Shape};
use crate::panel::{MARGIN, inside};
use crate::text::Weight;
use crate::{Ctx, WinRect, glyphs};

#[derive(Clone)]
pub struct NowPlayingView {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub art: Option<Rc<Image>>,
}

#[derive(Clone, Default)]
pub struct ControlState {
    /// (radio on, connected network name); None without Wi-Fi.
    pub wifi: Option<(bool, Option<String>)>,
    pub bluetooth: Option<bool>,
    pub volume: Option<f32>,
    pub muted: bool,
    pub brightness: Option<f32>,
    pub playing: Option<NowPlayingView>,
    pub dark: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    Wifi(bool),
    Bluetooth(bool),
    Volume(f32),
    Muted(bool),
    Brightness(f32),
    PlayPause,
    Next,
    Previous,
    DarkMode(bool),
    Screenshot,
    Calculator,
    Lock,
    /// Open a settings page: "wifi", "bluetooth", "sound", "display", "" (home).
    Settings(&'static str),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    Wifi,
    WifiLabel,
    Bluetooth,
    BluetoothLabel,
    Previous,
    PlayPause,
    Next,
    Dark,
    Shot,
    Calc,
    Lock,
    Display,
    DisplayButton,
    Sound,
    SoundButton,
    Edit,
}

type R = (f32, f32, f32, f32);

// Points, measured on Tahoe 26 frames at 2 px/pt.
const U: f32 = 60.0;
const G: f32 = 11.0;
const PITCH: f32 = U + G;
const W: f32 = 4.0 * U + 3.0 * G;
/// Every module: a 1x1 is a circle, 2x1 and 4x1 are capsules.
const RADIUS: f32 = U / 2.0;
/// The stack ends 5 pt right of the clock.
const RIGHT: f32 = 13.5;
const TOP: f32 = 9.5;
const PILL_GAP: f32 = 15.0;
const PILL_H: f32 = 22.5;
const DISC: f32 = 34.0;
const DISC_INSET: f32 = 13.0;
const TEXT_X: f32 = 53.0;
const LINE: f32 = 14.5;
/// Inter at 0.95 of a size matches SF Pro's cap height and width at that size.
const K: f32 = 0.95;
/// 12 pt semibold titles and labels, 11 pt status lines (cap 8.5 pt).
const TITLE: f32 = 12.0 * K;
const SUB: f32 = 11.0 * K;
const TITLE_CAP: f32 = 8.5;
/// Slider module: track span and centre line, the round button's centre.
const TRACK_X0: f32 = 31.0;
const TRACK_X1: f32 = 211.0;
/// The speaker with waves is wider than the sun, so the Sound track stops earlier.
const SOUND_X1: f32 = 205.5;
const TRACK_Y: f32 = 40.25;
const TRACK: f32 = 4.5;
const BUTTON_X: f32 = 250.0;
const BUTTON: f32 = 22.0;
/// Opening, measured at 50 fps on four Tahoe 26 captures
/// (TAHOE-CC-MOTION.md section 2): labels, discs and glyphs fade in along an
/// ease-out over 0.20 s while the glass modules (and the dimming around
/// them) follow a smoothstep over 0.18 s, so the white discs lead and the
/// tinted glass catches up. Tahoe closes within one frame; closing here plays
/// the opening backwards (the owner found the instant cut abrupt).
const OPEN_CONTENT: f32 = 0.20;
const OPEN_GLASS: f32 = 0.18;
/// On/off fill of a toggle: not caught on video (NF); it reuses the
/// opening's content curve.
const TOGGLE_SECS: f32 = 0.20;
/// While open the desktop behind the stack dims: x0.83 between modules,
/// about x0.9 just outside, gone a few points further out.
const HALO: f32 = 0.17;
const HALO_SIGMA: f32 = 6.0;
const HALO_GROW: f32 = 2.4;

pub struct ControlCenter {
    scale: f32,
    state: ControlState,
    /// Whether the Display row exists; fixed for the window's lifetime.
    display: bool,
    backdrop: Option<Backdrop>,
    material: RefCell<Option<(bool, Pixmap)>>,
    halo: RefCell<Option<Pixmap>>,
    pill_w: Cell<f32>,
    /// Seconds along the opening; closing runs it back to zero.
    phase: f32,
    closing: bool,
    /// 0 = off, 1 = on: Wi-Fi disc, Bluetooth disc, Dark Mode circle.
    toggles: [Tween; 3],
    hover: Option<Hit>,
    pressed: Option<Hit>,
    dragging: Option<Hit>,
    pub events: Vec<ControlEvent>,
}

impl ControlCenter {
    pub fn new(state: ControlState, scale: f32) -> Self {
        let toggles = toggle_targets(&state).map(Tween::new);
        Self {
            scale,
            display: state.brightness.is_some(),
            state,
            backdrop: None,
            material: RefCell::new(None),
            halo: RefCell::new(None),
            pill_w: Cell::new(90.0 * scale),
            phase: 0.0,
            closing: false,
            toggles,
            hover: None,
            pressed: None,
            dragging: None,
            events: Vec::new(),
        }
    }

    /// What is behind the window (screen capture or wallpaper), window-sized.
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
        *self.material.borrow_mut() = None;
    }

    pub fn set_state(&mut self, mut st: ControlState) {
        // While a slider is held, the pointer owns its value.
        match self.dragging {
            Some(Hit::Sound) => st.volume = self.state.volume,
            Some(Hit::Display) => st.brightness = self.state.brightness,
            _ => {}
        }
        if st.brightness.is_none() {
            st.brightness = self.state.brightness;
        }
        self.state = st;
        self.sync_toggles();
    }

    fn sync_toggles(&mut self) {
        for (t, on) in self.toggles.iter_mut().zip(toggle_targets(&self.state)) {
            t.to(on, TOGGLE_SECS, ease_out_quad);
        }
    }

    fn rows(&self) -> f32 {
        if self.display { 5.0 } else { 4.0 }
    }

    fn stack_h(&self) -> f32 {
        self.rows() * U + (self.rows() - 1.0) * G
    }

    pub fn window_size(&self) -> (i32, i32) {
        let s = self.scale;
        let h = TOP + self.stack_h() + PILL_GAP + PILL_H + MARGIN;
        (((W + 2.0 * MARGIN) * s).ceil() as i32, (h * s).ceil() as i32)
    }

    /// Where the window goes: the stack's right edge `RIGHT` from the screen
    /// edge, its top `TOP` under the menu bar. The window starts at the bar's
    /// bottom so its faint halo never takes clicks meant for the bar.
    pub fn window_rect(&self, screen_w: i32, bar_bottom: i32) -> WinRect {
        let (w, h) = self.window_size();
        let (ox, _) = self.origin_at_rest();
        let right = (screen_w as f32 - RIGHT * self.scale).round();
        let x = right as i32 - (W * self.scale).round() as i32 - ox as i32;
        WinRect { x, y: bar_bottom, w, h }
    }

    /// The modules and the pill on screen, for a window at `win`.
    pub fn body_rect(&self, win: WinRect) -> WinRect {
        let s = self.scale;
        let (ox, oy) = self.origin_at_rest();
        let h = (self.stack_h() + PILL_GAP + PILL_H) * s;
        WinRect { x: win.x + ox as i32, y: win.y + oy as i32, w: (W * s).round() as i32, h: h.round() as i32 }
    }

    pub fn close(&mut self) {
        self.closing = true;
    }
    pub fn closing(&self) -> bool {
        self.closing
    }
    pub fn finished(&self) -> bool {
        self.closing && self.phase <= 0.0
    }
    pub fn animating(&self) -> bool {
        let moving = if self.closing { self.phase > 0.0 } else { self.phase < OPEN_CONTENT };
        moving || self.toggles.iter().any(|t| !t.settled())
    }
    pub fn update(&mut self, dt: f32) {
        self.phase = if self.closing { self.phase.min(OPEN_CONTENT) - dt } else { (self.phase + dt).min(OPEN_CONTENT) }.max(0.0);
        for t in &mut self.toggles {
            t.step(dt);
        }
    }
    /// Opacity of the glass layer and of the content layer.
    fn reveal(&self) -> (f32, f32) {
        (smoothstep(self.phase / OPEN_GLASS), ease_out_quad(self.phase / OPEN_CONTENT))
    }

    /// Top-left of the stack in the window, whole pixels.
    fn origin_at_rest(&self) -> (f32, f32) {
        ((MARGIN * self.scale).round(), (TOP * self.scale).round())
    }

    /// Module rect: column `c`, row `r`, `cw` columns by `rh` rows, px.
    fn cell(&self, c: f32, r: f32, cw: f32, rh: f32) -> R {
        let s = self.scale;
        let (ox, oy) = self.origin_at_rest();
        (ox + c * PITCH * s, oy + r * PITCH * s, (cw * U + (cw - 1.0) * G) * s, (rh * U + (rh - 1.0) * G) * s)
    }

    fn slider(&self, h: Hit) -> R {
        let row = match h {
            Hit::Display | Hit::DisplayButton => 3.0,
            _ => self.rows() - 1.0,
        };
        self.cell(0.0, row, 4.0, 1.0)
    }

    fn track(&self, h: Hit) -> R {
        let s = self.scale;
        let m = self.slider(h);
        let x1 = if h == Hit::Sound { SOUND_X1 } else { TRACK_X1 };
        (m.0 + TRACK_X0 * s, m.1 + (TRACK_Y - TRACK / 2.0) * s, (x1 - TRACK_X0) * s, TRACK * s)
    }

    fn pill(&self) -> R {
        let s = self.scale;
        let (ox, oy) = self.origin_at_rest();
        let w = self.pill_w.get();
        (ox + (W * s - w) / 2.0, oy + (self.stack_h() + PILL_GAP) * s, w, PILL_H * s)
    }

    fn layout(&self) -> Vec<(Hit, R)> {
        let s = self.scale;
        let mut v = Vec::new();
        for (row, disc, label) in [(0.0, Hit::Wifi, Hit::WifiLabel), (1.0, Hit::Bluetooth, Hit::BluetoothLabel)] {
            let m = self.cell(0.0, row, 2.0, 1.0);
            let d = (m.0 + DISC_INSET * s, m.1 + (U - DISC) / 2.0 * s, DISC * s, DISC * s);
            v.push((disc, d));
            v.push((label, (d.0 + d.2, m.1, m.0 + m.2 - d.0 - d.2, m.3)));
        }
        let np = self.cell(2.0, 0.0, 2.0, 2.0);
        let (cx, cy, b) = (np.0 + np.2 / 2.0, np.1 + 105.5 * s, 32.0 * s);
        for (h, dx) in [(Hit::Previous, -34.0), (Hit::PlayPause, 0.0), (Hit::Next, 34.0)] {
            v.push((h, (cx + dx * s - b / 2.0, cy - b / 2.0, b, b)));
        }
        for (i, h) in [Hit::Dark, Hit::Shot, Hit::Calc, Hit::Lock].into_iter().enumerate() {
            v.push((h, self.cell(i as f32, 2.0, 1.0, 1.0)));
        }
        let sliders: &[(Hit, Hit)] =
            if self.display { &[(Hit::Display, Hit::DisplayButton), (Hit::Sound, Hit::SoundButton)] } else { &[(Hit::Sound, Hit::SoundButton)] };
        for &(h, button) in sliders {
            let m = self.slider(h);
            let b = (BUTTON + 4.0) * s;
            v.push((button, (m.0 + BUTTON_X * s - b / 2.0, m.1 + TRACK_Y * s - b / 2.0, b, b)));
            // The lower part of the module takes the drag, up to the button.
            v.push((h, (m.0 + 8.0 * s, m.1 + 26.0 * s, (BUTTON_X - BUTTON / 2.0 - 12.0) * s, m.3 - 26.0 * s)));
        }
        v.push((Hit::Edit, self.pill()));
        v
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Hit, R)> {
        self.layout().into_iter().find(|(_, r)| inside(*r, x, y))
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if let Some(h) = self.dragging {
            self.drag_to(h, x);
            return;
        }
        self.hover = self.hit(x, y).map(|h| h.0);
    }

    pub fn mouse_leave(&mut self) {
        if self.dragging.is_none() {
            self.hover = None;
        }
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) {
        let Some((h, _)) = self.hit(x, y) else { return };
        self.hover = Some(h);
        if matches!(h, Hit::Sound | Hit::Display) {
            self.dragging = Some(h);
            self.drag_to(h, x);
        } else {
            self.pressed = Some(h);
        }
    }

    fn drag_to(&mut self, h: Hit, x: f32) {
        let t = self.track(h);
        let v = ((x - t.0) / t.2.max(1.0)).clamp(0.0, 1.0);
        match h {
            Hit::Sound => {
                self.state.volume = Some(v);
                if self.state.muted {
                    self.state.muted = false;
                    self.events.push(ControlEvent::Muted(false));
                }
                self.events.push(ControlEvent::Volume(v));
            }
            Hit::Display => {
                self.state.brightness = Some(v);
                self.events.push(ControlEvent::Brightness(v));
            }
            _ => {}
        }
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) {
        if self.dragging.take().is_some() {
            return;
        }
        let Some(p) = self.pressed.take() else { return };
        if self.hit(x, y).map(|h| h.0) != Some(p) {
            return;
        }
        let st = &mut self.state;
        let ev = match p {
            Hit::Wifi => st.wifi.as_mut().map(|w| {
                w.0 = !w.0;
                ControlEvent::Wifi(w.0)
            }),
            Hit::Bluetooth => st.bluetooth.as_mut().map(|b| {
                *b = !*b;
                ControlEvent::Bluetooth(*b)
            }),
            Hit::WifiLabel => Some(ControlEvent::Settings("wifi")),
            Hit::BluetoothLabel => Some(ControlEvent::Settings("bluetooth")),
            Hit::Previous => st.playing.is_some().then_some(ControlEvent::Previous),
            Hit::Next => st.playing.is_some().then_some(ControlEvent::Next),
            Hit::PlayPause => st.playing.as_mut().map(|p| {
                p.playing = !p.playing;
                ControlEvent::PlayPause
            }),
            Hit::Dark => {
                st.dark = !st.dark;
                Some(ControlEvent::DarkMode(st.dark))
            }
            Hit::Shot => Some(ControlEvent::Screenshot),
            Hit::Calc => Some(ControlEvent::Calculator),
            Hit::Lock => Some(ControlEvent::Lock),
            Hit::DisplayButton => Some(ControlEvent::Settings("display")),
            Hit::SoundButton => Some(ControlEvent::Settings("sound")),
            Hit::Edit => Some(ControlEvent::Settings("")),
            Hit::Display | Hit::Sound => None,
        };
        self.events.extend(ev);
        self.sync_toggles();
    }

    /// The module material over the whole stack: the blurred backdrop with its
    /// luminance pulled to a fixed level and its colour kept, so white text
    /// reads over any wallpaper and the modules still carry its hue.
    fn build_material(&self, w: u32, h: u32, dark: bool) -> Option<Pixmap> {
        let b = self.backdrop.as_ref()?;
        let mut pm = Pixmap::new(w, h)?;
        let s = self.scale;
        let (ox, oy) = self.origin_at_rest();
        let x0 = (ox - 2.0).max(0.0) as usize;
        let y0 = (oy - 2.0).max(0.0) as usize;
        let x1 = ((ox + W * s + 2.0).ceil() as usize).min(w as usize);
        let y1 = ((oy + (self.stack_h() + PILL_GAP + PILL_H) * s + 2.0).ceil() as usize).min(h as usize);
        let data = pm.data_mut();
        for y in y0..y1 {
            for x in x0..x1 {
                let c = lift(b.vib.sample_frost(x as f32 + 0.5, y as f32 + 0.5), dark);
                data[(y * w as usize + x) * 4..][..4].copy_from_slice(&c);
            }
        }
        Some(pm)
    }

    fn draw_halo(&self, pm: &mut Pixmap) {
        let (w, h) = (pm.width(), pm.height());
        let mut cache = self.halo.borrow_mut();
        if cache.as_ref().is_none_or(|p| p.width() != w || p.height() != h) {
            *cache = halo(w, h, self.cell(0.0, 0.0, 4.0, self.rows()), RADIUS * self.scale, self.scale);
        }
        if let Some(p) = cache.as_ref() {
            pm.data_mut().copy_from_slice(p.data());
        }
    }

    /// A module: the material inside `path` and the lit 1 pt rim.
    fn module(&self, pm: &mut Pixmap, ctx: &Ctx, r: R, radius: f32, pressed: bool) {
        let dark = ctx.theme.dark;
        let Some(path) = rrect(r.0, r.1, r.2, r.3, radius) else { return };
        match self.material.borrow().as_ref() {
            Some((_, m)) => gfx::fill_from(pm, &path, m),
            None => gfx::fill(pm, &path, if dark { rgba(52, 52, 56, 1.0) } else { rgba(120, 122, 130, 1.0) }),
        }
        if pressed {
            gfx::fill(pm, &path, rgba(0, 0, 0, 0.14));
        }
        glass::lit_rim(pm, Shape { x: r.0, y: r.1, w: r.2, h: r.3, r: radius.min(r.2 / 2.0).min(r.3 / 2.0) }, Rim::module(dark), self.scale);
    }

    /// Vibrant ink over module `r`: luminance `lum` (0..255) carrying
    /// `chroma` times the module's own colour, as Tahoe tints its labels and
    /// glyphs with what is behind them.
    fn vibrant(&self, r: R, lum: f32, chroma: f32) -> Color {
        let m = self.material.borrow();
        let Some((_, mat)) = m.as_ref() else { return rgba(lum as u8, lum as u8, lum as u8, 1.0) };
        let (w, h) = (mat.width() as i32, mat.height() as i32);
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for i in 1..6 {
            for j in 1..4 {
                let x = ((r.0 + r.2 * i as f32 / 6.0) as i32).clamp(0, w - 1);
                let y = ((r.1 + r.3 * j as f32 / 4.0) as i32).clamp(0, h - 1);
                let px = &mat.data()[((y * w + x) * 4) as usize..][..3];
                for k in 0..3 {
                    sum[k] += px[k] as f32;
                }
                n += 1.0;
            }
        }
        let c = sum.map(|v| v / n);
        let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let ch = |v: f32| (lum + (v - y) * chroma).clamp(0.0, 255.0) as u8;
        Color::from_rgba8(ch(c[0]), ch(c[1]), ch(c[2]), 255)
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let dark = ctx.theme.dark;
        {
            let mut m = self.material.borrow_mut();
            if m.as_ref().is_none_or(|(d, p)| *d != dark || p.width() != pm.width() || p.height() != pm.height()) {
                *m = self.build_material(pm.width(), pm.height(), dark).map(|p| (dark, p));
            }
        }
        let fs = TITLE * self.scale;
        let tw = ctx.text.measure(ctx.strings.control_settings, fs, Weight::Medium);
        self.pill_w.set((tw + 21.0 * self.scale).round());
        let (glass_a, content_a) = self.reveal();
        self.draw_halo(pm);
        self.draw_glass(pm, ctx);
        crate::panel::fade_all(pm, glass_a);
        if content_a >= 0.999 {
            self.draw_content(pm, ctx);
        } else if content_a > 0.0
            && let Some(mut layer) = Pixmap::new(pm.width(), pm.height())
        {
            self.draw_content(&mut layer, ctx);
            crate::panel::fade_all(&mut layer, content_a);
            pm.draw_pixmap(0, 0, layer.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
        }
    }

    /// Whether `h` is held down with the pointer still on it.
    fn lit(&self, h: Hit) -> bool {
        self.pressed == Some(h) && self.hover == Some(h)
    }

    fn rect(&self, h: Hit) -> R {
        self.layout().into_iter().find(|(k, _)| *k == h).map(|(_, r)| r).unwrap_or_default()
    }

    /// Every glass module: the layer that fades in behind the content.
    fn draw_glass(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let s = self.scale;
        for (row, label) in [(0.0, Hit::WifiLabel), (1.0, Hit::BluetoothLabel)] {
            self.module(pm, ctx, self.cell(0.0, row, 2.0, 1.0), RADIUS * s, self.lit(label));
        }
        self.module(pm, ctx, self.cell(2.0, 0.0, 2.0, 2.0), RADIUS * s, false);
        for h in [Hit::Dark, Hit::Shot, Hit::Calc, Hit::Lock] {
            let r = self.rect(h);
            self.module(pm, ctx, r, r.2 / 2.0, self.lit(h) && h != Hit::Dark);
        }
        let mut sliders = vec![Hit::Sound];
        if self.display {
            sliders.insert(0, Hit::Display);
        }
        for h in sliders {
            self.module(pm, ctx, self.slider(h), RADIUS * s, false);
        }
        let p = self.pill();
        self.module(pm, ctx, p, p.3 / 2.0, self.lit(Hit::Edit));
    }

    /// Labels, discs, glyphs and slider fills over the glass.
    fn draw_content(&self, pm: &mut Pixmap, ctx: &Ctx) {
        let s = self.scale;
        let st = &self.state;
        let dark = ctx.theme.dark;
        // Measured BB label colours over a (113,39,72) module: title (255,230,250),
        // status (240,186,212), transport (250,183,212): white text tinted by the module.
        // Light modules sit at a higher luminance, so their labels go whiter.
        let title_ink = |r: R| self.vibrant(r, if dark { 237.0 } else { 248.0 }, 0.3);
        let sub_ink = |r: R| self.vibrant(r, if dark { 200.0 } else { 228.0 }, if dark { 0.75 } else { 0.5 });
        let glyph_ink = |r: R| self.vibrant(r, 240.0, 0.4);
        let accent = ctx.theme.accent();
        let l = self.layout();
        let rect = |h: Hit| l.iter().find(|(k, _)| *k == h).map(|(_, r)| *r).unwrap_or_default();
        let lit = |h: Hit| self.lit(h);
        let off_disc = rgba(255, 255, 255, if dark { 0.14 } else { 0.22 });
        let on_disc = rgba(255, 253, 255, 1.0);

        // Wi-Fi and Bluetooth: a disc, a title and a status that may wrap.
        let wifi_sub = match &st.wifi {
            None | Some((true, None)) => ctx.strings.not_connected.to_string(),
            Some((false, _)) => ctx.strings.off.to_string(),
            Some((true, Some(n))) => n.clone(),
        };
        let bt_on = st.bluetooth.unwrap_or(false);
        let bt_sub = if bt_on { ctx.strings.on } else { ctx.strings.off };
        let wifi_idle = st.wifi.as_ref().is_some_and(|w| w.1.is_none());
        for (row, disc, title, sub, on) in [
            (0.0, Hit::Wifi, ctx.strings.wifi, wifi_sub.as_str(), self.toggles[0].value),
            (1.0, Hit::Bluetooth, ctx.strings.bluetooth, bt_sub, self.toggles[1].value),
        ] {
            let m = self.cell(0.0, row, 2.0, 1.0);
            let d = rect(disc);
            let (cx, cy) = (d.0 + d.2 / 2.0, d.1 + d.3 / 2.0);
            if let Some(p) = gfx::circle(cx, cy, d.2 / 2.0) {
                gfx::fill(pm, &p, mix(off_disc, on_disc, on));
                if lit(disc) {
                    gfx::fill(pm, &p, rgba(0, 0, 0, 0.12));
                }
            }
            let on_glyph = if disc == Hit::Wifi && wifi_idle { rgba(190, 190, 194, 1.0) } else { accent };
            let gc = mix(glyph_ink(m), on_glyph, on);
            if disc == Hit::Wifi {
                wifi(pm, cx, cy, 16.0 * s, gc);
            } else {
                let g = 17.0 * s;
                glyphs::bluetooth(pm, cx - g / 2.0, cy - g / 2.0, g, gc);
            }
            let tx = m.0 + TEXT_X * s;
            let maxw = m.0 + m.2 - tx - 10.0 * s;
            let lines = ctx.text.wrap(sub, SUB * s, Weight::Medium, maxw, 2);
            // BB: 14.5 pt from title to a single status line, 13-13.5 pt when it wraps.
            let pitch = if lines.len() > 1 { 13.25 } else { LINE };
            let block = pitch * lines.len() as f32;
            let mut base = m.1 + (U / 2.0 + TITLE_CAP / 2.0 - block / 2.0) * s;
            let t = ctx.text.ellipsize(title, TITLE * s, Weight::Bold, maxw);
            ctx.text.draw(pm, &t, tx, base, TITLE * s, Weight::Bold, title_ink(m), None);
            let ink = sub_ink(m);
            for line in &lines {
                base += pitch * s;
                ctx.text.draw(pm, line, tx, base, SUB * s, Weight::Medium, ink, None);
            }
        }

        self.now_playing(pm, ctx, &rect, &lit);

        // Appearance, screen capture, calculator, lock. Dark Mode on is a
        // white circle over its module.
        for h in [Hit::Dark, Hit::Shot, Hit::Calc, Hit::Lock] {
            let r = rect(h);
            let on = if h == Hit::Dark { self.toggles[2].value } else { 0.0 };
            let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
            if on > 0.0
                && let Some(p) = gfx::circle(cx, cy, r.2 / 2.0)
            {
                gfx::fill(pm, &p, rgba(255, 253, 255, on));
                if lit(h) {
                    gfx::fill(pm, &p, rgba(0, 0, 0, 0.12 * on));
                }
            }
            if h == Hit::Dark && lit(h) && on < 1.0 {
                // The module's own press shade, under a half-faded disc.
                if let Some(p) = rrect(r.0, r.1, r.2, r.3, r.2 / 2.0) {
                    gfx::fill(pm, &p, rgba(0, 0, 0, 0.14 * (1.0 - on)));
                }
            }
            let c = mix(glyph_ink(r), accent, on);
            match h {
                Hit::Dark => appearance(pm, cx, cy, 20.0 * s, c),
                Hit::Shot => viewfinder(pm, cx, cy, 23.0 * s, c, sub_ink(r)),
                Hit::Calc => {
                    let g = 24.0 * s;
                    glyphs::calculator_glyph(pm, cx - g / 2.0, cy - g / 2.0, g, c);
                }
                _ => {
                    let g = 23.0 * s;
                    glyphs::lock(pm, cx - g / 2.0, cy - g * 0.53, g, c);
                }
            }
        }

        // Display and Sound.
        let mut sliders = Vec::new();
        if self.display {
            sliders.push((Hit::Display, Hit::DisplayButton, ctx.strings.display, st.brightness.unwrap_or(0.0)));
        }
        let vol = if st.muted { 0.0 } else { st.volume.unwrap_or(0.0) };
        sliders.push((Hit::Sound, Hit::SoundButton, ctx.strings.sound, vol));
        for (h, button, label, v) in sliders {
            let m = self.slider(h);
            ctx.text.draw(pm, label, m.0 + 16.0 * s, m.1 + 23.5 * s, TITLE * s, Weight::Bold, title_ink(m), None);
            let t = self.track(h);
            if let Some(p) = rrect(t.0, t.1, t.2, t.3, t.3 / 2.0) {
                gfx::fill(pm, &p, rgba(0, 0, 0, 0.08));
            }
            let fw = (t.2 * v.clamp(0.0, 1.0)).max(t.3);
            if let Some(p) = rrect(t.0, t.1, fw, t.3, t.3 / 2.0) {
                gfx::fill(pm, &p, self.vibrant(m, 250.0, 0.1));
            }
            let cy = m.1 + TRACK_Y * s;
            let ink = glyph_ink(m);
            if h == Hit::Display {
                sun(pm, m.0 + 19.5 * s, cy, 11.0 * s, ink, false);
                sun(pm, m.0 + 221.0 * s, cy, 13.0 * s, ink, true);
            } else {
                speaker(pm, m.0 + 16.5 * s, cy, 11.0 * s, ink, 0);
                speaker(pm, m.0 + 210.5 * s, cy, 14.0 * s, ink, 3);
            }
            let b = rect(button);
            let (bx, by) = (b.0 + b.2 / 2.0, b.1 + b.3 / 2.0);
            if let Some(p) = gfx::circle(bx, by, BUTTON / 2.0 * s) {
                gfx::fill(pm, &p, rgba(255, 255, 255, if dark { 0.22 } else { 0.3 }));
                if lit(button) {
                    gfx::fill(pm, &p, rgba(0, 0, 0, 0.14));
                }
            }
            if h == Hit::Display {
                monitor(pm, bx, by, 13.0 * s, ink);
            } else {
                airplay_audio(pm, bx, by, 13.0 * s, ink);
            }
        }

        // The text pill under the stack (Tahoe's "Edit Controls").
        let fs = TITLE * s;
        let tw = ctx.text.measure(ctx.strings.control_settings, fs, Weight::Medium);
        let p = self.pill();
        let base = p.1 + p.3 / 2.0 + ctx.text.cap_height(fs) / 2.0;
        let ink = self.vibrant(p, if dark { 185.0 } else { 215.0 }, 1.4);
        ctx.text.draw(pm, ctx.strings.control_settings, p.0 + (p.2 - tw) / 2.0, base, fs, Weight::Medium, ink, None);
    }

    fn now_playing(&self, pm: &mut Pixmap, ctx: &Ctx, rect: &dyn Fn(Hit) -> R, lit: &dyn Fn(Hit) -> bool) {
        let s = self.scale;
        let st = &self.state;
        let np = self.cell(2.0, 0.0, 2.0, 2.0);
        let art = 37.5 * s;
        let (ax, ay) = (np.0 + 13.0 * s, np.1 + 12.5 * s);
        let text_w = np.2 - 26.0 * s;
        let blank = rgba(255, 255, 255, if ctx.theme.dark { 0.16 } else { 0.24 });
        let dark = ctx.theme.dark;
        let title_ink = self.vibrant(np, if dark { 237.0 } else { 248.0 }, 0.3);
        let (big, small) = (13.0 * K * s, 12.0 * K * s);
        match st.playing.as_ref() {
            Some(p) => {
                match &p.art {
                    Some(img) => artwork(pm, img, ax, ay, art, 8.0 * s),
                    None => {
                        if let Some(path) = rrect(ax, ay, art, art, 8.0 * s) {
                            gfx::fill(pm, &path, blank);
                        }
                        let g = art * 0.5;
                        glyphs::music_note(pm, ax + (art - g) / 2.0, ay + (art - g) / 2.0, g, self.vibrant(np, 200.0, 0.75));
                    }
                }
                let title = ctx.text.ellipsize(&p.title, big, Weight::Bold, text_w);
                ctx.text.draw(pm, &title, ax, np.1 + 69.5 * s, big, Weight::Bold, title_ink, None);
                let artist = ctx.text.ellipsize(&p.artist, small, Weight::Medium, text_w);
                ctx.text.draw(pm, &artist, ax, np.1 + 83.5 * s, small, Weight::Medium, self.vibrant(np, if dark { 185.0 } else { 220.0 }, 0.75), None);
            }
            None => {
                if let Some(path) = rrect(ax, ay, art, art, 8.0 * s) {
                    gfx::fill(pm, &path, blank);
                }
                // «Не воспроизводится» is longer than "Not Playing": wrap, and
                // shrink only if a single word still does not fit.
                let text = ctx.strings.not_playing;
                let mut fs = big;
                while fs > 11.0 * s && text.split_whitespace().any(|w| ctx.text.measure(w, fs, Weight::Bold) > text_w) {
                    fs -= 0.5 * s;
                }
                let lines = ctx.text.wrap(text, fs, Weight::Bold, text_w, 2);
                let mut base = np.1 + 80.0 * s - (lines.len() as f32 - 1.0) * 15.0 * s;
                for line in &lines {
                    ctx.text.draw(pm, line, ax, base, fs, Weight::Bold, title_ink, None);
                    base += 15.0 * s;
                }
            }
        }
        // BB, playing: all three (250,183,212); Not Playing (Apple, MM): a
        // near-white play button between tinted skips.
        let ctl = self.vibrant(np, if dark { 200.0 } else { 225.0 }, 0.9);
        let play = if st.playing.is_some() { ctl } else { self.vibrant(np, 240.0, 0.4) };
        for h in [Hit::Previous, Hit::PlayPause, Hit::Next] {
            let r = rect(h);
            let (cx, cy) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
            if lit(h)
                && let Some(p) = gfx::circle(cx, cy, r.2 / 2.0)
            {
                gfx::fill(pm, &p, rgba(255, 255, 255, 0.16));
            }
            let g = 22.0 * s;
            let (gx, gy) = (cx - g / 2.0, cy - g / 2.0);
            match h {
                Hit::Previous => glyphs::skip(pm, gx, gy, g, ctl, true),
                Hit::Next => glyphs::skip(pm, gx, gy, g, ctl, false),
                _ if st.playing.as_ref().is_some_and(|p| p.playing) => glyphs::pause(pm, gx, gy, g, play),
                _ => glyphs::play(pm, gx, gy, g, play),
            }
        }
    }
}

/// Dark: luminance pulled to about 58/255 (measured: (60,12,37) -> (96,42,68),
/// (156,7,72) -> (194,11,90)). Light (four 50 fps Tahoe captures over a blue
/// wallpaper, before vs after opening): `L' = 0.5 L + 45` (L 174 -> 132,
/// L 60 -> 74) with the chroma x1.25. The frosted level is already saturated
/// x1.6, so `sat` brings the net chroma to x1.2 (dark) and x1.25 (light).
fn lift(c: [f32; 3], dark: bool) -> [u8; 4] {
    let (target, k, sat) = if dark { (58.0, 0.9, 0.75) } else { (90.0, 0.5, 0.78) };
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let ny = y + (target - y) * k;
    let ch = |v: f32| (ny + (v - y) * sat).clamp(0.0, 255.0).round() as u8;
    [ch(c[0]), ch(c[1]), ch(c[2]), 255]
}

fn ease_out_quad(u: f32) -> f32 {
    let v = 1.0 - u.clamp(0.0, 1.0);
    1.0 - v * v
}

/// Targets of the three toggles for `st`.
fn toggle_targets(st: &ControlState) -> [f32; 3] {
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    [on(st.wifi.as_ref().is_some_and(|w| w.0)), on(st.bluetooth.unwrap_or(false)), on(st.dark)]
}

/// Straight-alpha mix of two colours.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let f = |x: f32, y: f32| x + (y - x) * t;
    Color::from_rgba(f(a.red(), b.red()), f(a.green(), b.green()), f(a.blue(), b.blue()), f(a.alpha(), b.alpha())).unwrap_or(b)
}

/// Premultiplied black that is `HALO` over the stack's box and fades out
/// around it like a Gaussian edge.
fn halo(w: u32, h: u32, b: R, r: f32, s: f32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w, h)?;
    let (hw, hh) = (b.2 / 2.0, b.3 / 2.0);
    let (cx, cy) = (b.0 + hw, b.1 + hh);
    let (sigma, grow) = (HALO_SIGMA * s, HALO_GROW * s);
    let data = pm.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let qx = (x as f32 + 0.5 - cx).abs() - (hw - r);
            let qy = (y as f32 + 0.5 - cy).abs() - (hh - r);
            let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r;
            // Logistic stand-in for the normal CDF.
            let a = HALO / (1.0 + (1.702 * (d - grow) / sigma).exp());
            data[(y * w as usize + x) * 4 + 3] = (a * 255.0).round() as u8;
        }
    }
    Some(pm)
}

/// Rounded rectangle with circular corners: Tahoe's modules are true
/// capsules and circles, which the continuous-corner squircle is not.
fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn artwork(pm: &mut Pixmap, img: &Image, x: f32, y: f32, size: f32, r: f32) {
    let n = size.ceil() as u32 + 1;
    let Some(mut tmp) = Pixmap::new(n, n) else { return };
    img.draw(&mut tmp, 0.0, 0.0, size, 1.0, false);
    let Some(path) = rrect(x, y, size, size, r) else { return };
    let shader = Pattern::new(tmp.as_ref(), SpreadMode::Pad, FilterQuality::Nearest, 1.0, Transform::from_translate(x, y));
    gfx::fill_shader(pm, &path, shader);
}

fn arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let n = ((a1 - a0).abs() / (PI / 8.0)).ceil().max(1.0) as usize;
    for i in 0..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        let (px, py) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_arc(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, c: Color, lw: f32) {
    let mut pb = PathBuilder::new();
    arc(&mut pb, cx, cy, r, a0, a1);
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, lw);
    }
}

/// SF "wifi": three bands fanning up from a dot, `size` wide.
fn wifi(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color) {
    let s = size / 1.05;
    let apex = cy + s * 0.36;
    let lw = s * 0.13;
    for r in [0.3f32, 0.52, 0.74] {
        stroke_arc(pm, cx, apex, s * r, -PI + FRAC_PI_4, -FRAC_PI_4, c, lw);
    }
    if let Some(p) = gfx::circle(cx, apex - s * 0.02, s * 0.1) {
        gfx::fill(pm, &p, c);
    }
}

/// Tahoe's Dark Mode glyph: a ring whose inside is split into a filled
/// left half and an empty right half, with a small disc in the centre
/// whose halves are swapped. Drawn on a layer so the empty parts are holes.
fn appearance(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color) {
    let n = size.ceil() as u32 + 4;
    let Some(mut tmp) = Pixmap::new(n, n) else { return };
    let (ox, oy) = ((cx - n as f32 / 2.0).round(), (cy - n as f32 / 2.0).round());
    let (lx, ly) = (cx - ox, cy - oy);
    let r = size * 0.5;
    let clear = tiny_skia::Paint { blend_mode: tiny_skia::BlendMode::Clear, anti_alias: true, ..Default::default() };
    let half = |r: f32, right: bool| {
        let mut pb = PathBuilder::new();
        let a0 = if right { -PI / 2.0 } else { PI / 2.0 };
        arc(&mut pb, lx, ly, r, a0, a0 + PI);
        pb.close();
        pb.finish()
    };
    if let Some(p) = gfx::circle(lx, ly, r) {
        gfx::fill(&mut tmp, &p, c);
    }
    if let Some(p) = half(r * 0.76, true) {
        tmp.fill_path(&p, &clear, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
    if let Some(p) = half(r * 0.36, false) {
        tmp.fill_path(&p, &clear, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
    if let Some(p) = half(r * 0.36, true) {
        gfx::fill(&mut tmp, &p, c);
    }
    pm.draw_pixmap(ox as i32, oy as i32, tmp.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
}

/// SF "camera.viewfinder": corner brackets around a small camera whose lens
/// is a hole, drawn on a layer so the hole shows the module.
fn viewfinder(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color, frame: Color) {
    let n = size.ceil() as u32 + 4;
    let Some(mut tmp) = Pixmap::new(n, n) else { return };
    let (ox, oy) = ((cx - n as f32 / 2.0).round(), (cy - n as f32 / 2.0).round());
    let (lx, ly) = (cx - ox, cy - oy);
    let (hw, hh) = (size * 0.5, size * 0.42);
    let (l, lw, rr) = (size * 0.26, size * 0.085, size * 0.12);
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (x, y) = (lx + sx * (hw - lw / 2.0), ly + sy * (hh - lw / 2.0));
        let mut pb = PathBuilder::new();
        pb.move_to(x, y - sy * l);
        pb.line_to(x, y - sy * rr);
        pb.quad_to(x, y, x - sx * rr, y);
        pb.line_to(x - sx * l, y);
        if let Some(p) = pb.finish() {
            gfx::stroke_round(&mut tmp, &p, frame, lw);
        }
    }
    let (bw, bh) = (size * 0.5, size * 0.34);
    let by = ly - bh / 2.0 + size * 0.03;
    if let Some(p) = gfx::squircle(lx - bw / 2.0, by, bw, bh, size * 0.06) {
        gfx::fill(&mut tmp, &p, c);
    }
    if let Some(p) = gfx::squircle(lx - size * 0.1, by - size * 0.06, size * 0.2, size * 0.1, size * 0.03) {
        gfx::fill(&mut tmp, &p, c);
    }
    if let Some(p) = gfx::circle(lx, by + bh / 2.0, size * 0.085) {
        let paint = tiny_skia::Paint { blend_mode: tiny_skia::BlendMode::Clear, anti_alias: true, ..Default::default() };
        tmp.fill_path(&p, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
    }
    pm.draw_pixmap(ox as i32, oy as i32, tmp.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
}

/// SF "sun.min" (dots) or "sun.max" (rays), `size` across.
fn sun(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color, rays: bool) {
    if let Some(p) = gfx::circle(cx, cy, size * if rays { 0.2 } else { 0.22 }) {
        gfx::fill(pm, &p, c);
    }
    for i in 0..8 {
        let a = i as f32 * TAU / 8.0;
        let (dx, dy) = (a.cos(), a.sin());
        if rays {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + dx * size * 0.34, cy + dy * size * 0.34);
            pb.line_to(cx + dx * size * 0.47, cy + dy * size * 0.47);
            if let Some(p) = pb.finish() {
                gfx::stroke_round(pm, &p, c, size * 0.09);
            }
        } else if let Some(p) = gfx::circle(cx + dx * size * 0.42, cy + dy * size * 0.42, size * 0.065) {
            gfx::fill(pm, &p, c);
        }
    }
}

/// SF "speaker.fill" with 0-3 waves, its body's left edge at `x`.
fn speaker(pm: &mut Pixmap, x: f32, cy: f32, size: f32, c: Color, waves: u8) {
    let p = |a: f32, b: f32| (x + size * a, cy + size * b);
    let mut pb = PathBuilder::new();
    for (i, (px, py)) in [p(0.0, -0.17), p(0.18, -0.17), p(0.5, -0.42), p(0.5, 0.42), p(0.18, 0.17), p(0.0, 0.17)].into_iter().enumerate() {
        if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        gfx::fill(pm, &path, c);
    }
    for i in 0..waves.min(3) {
        let r = size * (0.2 + 0.18 * i as f32);
        stroke_arc(pm, x + size * 0.5, cy, r, -0.3 * PI, 0.3 * PI, c, size * 0.08);
    }
}

/// SF "display": a screen on a short stand.
fn monitor(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color) {
    let (w, h) = (size, size * 0.7);
    let lw = size * 0.1;
    if let Some(p) = gfx::squircle(cx - w / 2.0 + lw / 2.0, cy - size * 0.45 + lw / 2.0, w - lw, h - lw, size * 0.1) {
        gfx::stroke(pm, &p, c, lw);
    }
    gfx::fill_rect(pm, cx - size * 0.22, cy + size * 0.36, size * 0.44, size * 0.09, c);
}

/// SF "airplay.audio": open rings over a triangle; here it stands for the
/// sound output choice.
fn airplay_audio(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: Color) {
    let lw = size * 0.08;
    let ccy = cy - size * 0.06;
    for r in [0.16f32, 0.3, 0.44] {
        stroke_arc(pm, cx, ccy, size * r, PI * 0.78, PI * 2.22, c, lw);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy + size * 0.12);
    pb.line_to(cx + size * 0.24, cy + size * 0.48);
    pb.line_to(cx - size * 0.24, cy + size * 0.48);
    pb.close();
    if let Some(p) = pb.finish() {
        gfx::fill(pm, &p, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(brightness: Option<f32>) -> ControlCenter {
        ControlCenter::new(ControlState { wifi: Some((true, None)), bluetooth: Some(false), volume: Some(0.5), brightness, ..Default::default() }, 2.0)
    }

    fn click(c: &mut ControlCenter, r: R) -> Vec<ControlEvent> {
        let (x, y) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0);
        c.mouse_down(x, y);
        c.mouse_up(x, y);
        std::mem::take(&mut c.events)
    }

    #[test]
    fn stack_sits_under_the_bar_end_on_the_tahoe_grid() {
        let c = cc(Some(0.5));
        let win = c.window_rect(3840, 48);
        let body = c.body_rect(win);
        assert_eq!(body.x + body.w, 3840 - 27);
        assert_eq!(body.y, 48 + 19);
        assert_eq!(body.w, 546);
        // Five rows (60 pt, 11 pt gaps), then the 15 pt gap and the 22.5 pt pill.
        assert_eq!(body.h, ((5.0 * 60.0 + 4.0 * 11.0 + 15.0 + 22.5) * 2.0f32).round() as i32);
        assert_eq!(cc(None).body_rect(win).h, body.h - 142);
    }

    #[test]
    fn buttons_open_their_settings_pages() {
        let mut c = cc(Some(0.5));
        let l = c.layout();
        let at = |h: Hit| l.iter().find(|(k, _)| *k == h).map(|(_, r)| *r).unwrap();
        assert_eq!(click(&mut c, at(Hit::DisplayButton)), [ControlEvent::Settings("display")]);
        assert_eq!(click(&mut c, at(Hit::SoundButton)), [ControlEvent::Settings("sound")]);
        assert_eq!(click(&mut c, at(Hit::Edit)), [ControlEvent::Settings("")]);
        assert_eq!(click(&mut c, at(Hit::Bluetooth)), [ControlEvent::Bluetooth(true)]);
        assert_eq!(click(&mut c, at(Hit::Lock)), [ControlEvent::Lock]);
    }

    #[test]
    fn opening_fades_in_as_measured_and_closing_reverses_it() {
        let mut c = cc(None);
        assert!(c.animating() && c.reveal() == (0.0, 0.0));
        c.update(0.1);
        let (glass, content) = c.reveal();
        // Content leads the glass: 0.75 vs 0.62 half way through.
        assert!((content - 0.75).abs() < 0.01 && glass > 0.55 && glass < content);
        c.update(0.2);
        assert!(!c.animating() && c.reveal() == (1.0, 1.0));
        c.close();
        c.update(0.1);
        assert!(c.animating() && !c.finished());
        c.update(0.11);
        assert!(c.finished());
    }

    #[test]
    fn toggles_cross_fade() {
        let mut c = cc(None);
        c.update(0.3);
        let l = c.layout();
        let bt = l.iter().find(|(k, _)| *k == Hit::Bluetooth).map(|(_, r)| *r).unwrap();
        assert_eq!(click(&mut c, bt), [ControlEvent::Bluetooth(true)]);
        assert!(c.animating() && c.toggles[1].value == 0.0);
        c.update(0.1);
        assert!((c.toggles[1].value - 0.75).abs() < 0.01);
        c.update(0.1);
        assert!(!c.animating() && c.toggles[1].value == 1.0);
    }
}
