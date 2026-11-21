//! Control Center in the macOS 26 layout: glass modules floating on the
//! desktop (no panel behind them), a Wi-Fi capsule, Now Playing, round
//! toggles, Display and Sound modules with thin knob-less sliders and a row
//! of round shortcuts.

use std::rc::Rc;

use tiny_skia::{Color, Pixmap};

use crate::gfx::{self, Image, rgba};
use crate::glass::{Backdrop, GlassStyle, Shape};
use crate::panel::{MARGIN, Reveal, inside};
use crate::text::Weight;
use crate::{Ctx, glyphs};

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
    Dark,
    Previous,
    PlayPause,
    Next,
    Display,
    Sound,
    SoundIcon,
    Shot,
    Calc,
    Settings,
    Lock,
}

type R = (f32, f32, f32, f32);

// Points. One module unit, the gap between modules, as in macOS 26.
const U: f32 = 66.0;
const G: f32 = 12.0;
const W: f32 = 4.0 * U + 3.0 * G;
const TRACK: f32 = 8.0;

pub struct ControlCenter {
    scale: f32,
    state: ControlState,
    reveal: Reveal,
    backdrop: Option<Backdrop>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    dragging: Option<Hit>,
    pub events: Vec<ControlEvent>,
}

impl ControlCenter {
    pub fn new(state: ControlState, scale: f32) -> Self {
        Self { scale, state, reveal: Reveal::new(), backdrop: None, hover: None, pressed: None, dragging: None, events: Vec::new() }
    }

    /// What is behind the window (screen capture or wallpaper), window-sized.
    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
    }

    pub fn set_state(&mut self, mut st: ControlState) {
        // While a slider is held, the pointer owns its value.
        match self.dragging {
            Some(Hit::Sound) => st.volume = self.state.volume,
            Some(Hit::Display) => st.brightness = self.state.brightness,
            _ => {}
        }
        self.state = st;
    }

    fn rows(&self) -> f32 {
        if self.state.brightness.is_some() { 5.0 } else { 4.0 }
    }

    fn content_h(&self) -> f32 {
        self.rows() * U + (self.rows() - 1.0) * G
    }

    pub fn body_size(&self) -> (f32, f32) {
        (W * self.scale, self.content_h() * self.scale)
    }

    pub fn window_size(&self) -> (i32, i32) {
        let s = self.scale;
        (((W + 2.0 * MARGIN) * s).ceil() as i32, ((self.content_h() + 2.0 * MARGIN) * s).ceil() as i32)
    }

    pub fn close(&mut self) {
        self.reveal.close();
    }
    pub fn closing(&self) -> bool {
        self.reveal.closing()
    }
    pub fn finished(&self) -> bool {
        self.reveal.finished()
    }
    pub fn animating(&self) -> bool {
        self.reveal.animating()
    }
    pub fn update(&mut self, dt: f32) {
        self.reveal.update(dt);
    }

    fn origin(&self) -> (f32, f32) {
        let m = MARGIN * self.scale;
        (m, m - (1.0 - self.reveal.alpha()) * 8.0 * self.scale)
    }

    /// Rect of module column `c`, row `r`, `cw` columns by `rh` rows, px.
    fn cell(&self, c: f32, r: f32, cw: f32, rh: f32) -> R {
        let s = self.scale;
        let (ox, oy) = self.origin();
        (ox + c * (U + G) * s, oy + r * (U + G) * s, (cw * U + (cw - 1.0) * G) * s, (rh * U + (rh - 1.0) * G) * s)
    }

    fn slider_row(&self, h: Hit) -> f32 {
        match (h, self.state.brightness.is_some()) {
            (Hit::Display, _) => 2.0,
            (_, true) => 3.0,
            _ => 2.0,
        }
    }

    /// The track inside a slider module.
    fn track(&self, h: Hit) -> R {
        let s = self.scale;
        let m = self.cell(0.0, self.slider_row(h), 4.0, 1.0);
        let x0 = m.0 + 38.0 * s;
        let x1 = m.0 + m.2 - 38.0 * s;
        (x0, m.1 + m.3 * 0.64 - TRACK * s / 2.0, x1 - x0, TRACK * s)
    }

    fn layout(&self) -> Vec<(Hit, R)> {
        let s = self.scale;
        let mut v = Vec::new();
        let wifi = self.cell(0.0, 0.0, 2.0, 1.0);
        let inset = 9.0 * s;
        let d = wifi.3 - 2.0 * inset;
        v.push((Hit::Wifi, (wifi.0 + inset, wifi.1 + inset, d, d)));
        v.push((Hit::WifiLabel, (wifi.0 + inset + d, wifi.1, wifi.2 - inset - d, wifi.3)));
        v.push((Hit::Bluetooth, self.cell(0.0, 1.0, 1.0, 1.0)));
        v.push((Hit::Dark, self.cell(1.0, 1.0, 1.0, 1.0)));
        let np = self.cell(2.0, 0.0, 2.0, 2.0);
        let b = 34.0 * s;
        let cy = np.1 + np.3 - b - 12.0 * s;
        let cx = np.0 + np.2 / 2.0;
        v.push((Hit::Previous, (cx - b * 1.6, cy, b, b)));
        v.push((Hit::PlayPause, (cx - b / 2.0, cy, b, b)));
        v.push((Hit::Next, (cx + b * 0.6, cy, b, b)));
        for h in [Hit::Display, Hit::Sound] {
            if h == Hit::Display && self.state.brightness.is_none() {
                continue;
            }
            let m = self.cell(0.0, self.slider_row(h), 4.0, 1.0);
            if h == Hit::Sound {
                v.push((Hit::SoundIcon, (m.0 + 8.0 * s, m.1 + m.3 * 0.4, 28.0 * s, m.3 * 0.5)));
            }
            let t = self.track(h);
            // The whole lower half of the module takes the drag.
            v.push((h, (t.0 - 6.0 * s, m.1 + m.3 * 0.4, t.2 + 12.0 * s, m.3 * 0.6)));
        }
        let last = self.rows() - 1.0;
        for (i, h) in [Hit::Shot, Hit::Calc, Hit::Settings, Hit::Lock].into_iter().enumerate() {
            v.push((h, self.cell(i as f32, last, 1.0, 1.0)));
        }
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
            Hit::SoundIcon => {
                st.muted = !st.muted;
                Some(ControlEvent::Muted(st.muted))
            }
            Hit::Shot => Some(ControlEvent::Screenshot),
            Hit::Calc => Some(ControlEvent::Calculator),
            Hit::Settings => Some(ControlEvent::Settings("")),
            Hit::Lock => Some(ControlEvent::Lock),
            Hit::Display | Hit::Sound => None,
        };
        self.events.extend(ev);
    }

    fn glass(&self, pm: &mut Pixmap, ctx: &Ctx, r: R, radius: f32) {
        let s = self.scale;
        let th = ctx.theme;
        match &self.backdrop {
            Some(b) => b.draw(pm, Shape { x: r.0, y: r.1, w: r.2, h: r.3, r: radius }, &GlassStyle::panel(th.dark, s)),
            None => {
                if let Some(p) = gfx::squircle(r.0, r.1, r.2, r.3, radius) {
                    gfx::fill(pm, &p, th.panel());
                }
            }
        }
    }

    /// Text and glyph colour on the glass: light over dark content, dark over light.
    fn ink(&self, ctx: &Ctx) -> (Color, Color) {
        let light_content = match &self.backdrop {
            Some(b) => b.luma > 0.62 && !ctx.theme.dark,
            None => !ctx.theme.dark,
        };
        if light_content { (rgba(0, 0, 0, 0.85), rgba(0, 0, 0, 0.5)) } else { (rgba(255, 255, 255, 1.0), rgba(255, 255, 255, 0.62)) }
    }

    /// A round toggle: solid white with an accent glyph when on, glass when off.
    fn toggle(&self, pm: &mut Pixmap, ctx: &Ctx, r: R, on: bool, glass: bool, ink: Color) -> Color {
        let (cx, cy, rad) = (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0);
        if on {
            if let Some(p) = gfx::circle(cx, cy, rad) {
                gfx::fill(pm, &p, rgba(255, 255, 255, 1.0));
            }
            ctx.theme.accent()
        } else {
            if glass {
                self.glass(pm, ctx, r, rad);
            } else if let Some(p) = gfx::circle(cx, cy, rad) {
                gfx::fill(pm, &p, gfx::fade(ink, 0.16));
            }
            ink
        }
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(Color::TRANSPARENT);
        let s = self.scale;
        let st = &self.state;
        let (ink, ink2) = self.ink(ctx);
        let l = self.layout();
        let rect = |h: Hit| l.iter().find(|(k, _)| *k == h).map(|(_, r)| *r).unwrap_or_default();
        let lit = |h: Hit| self.pressed == Some(h) && self.hover == Some(h);
        let fs = 13.0 * s;
        let small = 12.0 * s;
        let module_r = 0.42 * U * s;

        // Wi-Fi capsule.
        let wifi = self.cell(0.0, 0.0, 2.0, 1.0);
        self.glass(pm, ctx, wifi, wifi.3 / 2.0);
        let on = st.wifi.as_ref().is_some_and(|w| w.0);
        let r = rect(Hit::Wifi);
        let c = self.toggle(pm, ctx, r, on, false, ink);
        let g = 22.0 * s;
        glyphs::wifi(pm, r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0, g, c, true);
        let sub = match &st.wifi {
            None | Some((true, None)) => ctx.strings.not_connected.to_string(),
            Some((false, _)) => ctx.strings.off.to_string(),
            Some((true, Some(n))) => n.clone(),
        };
        let tx = r.0 + r.2 + 10.0 * s;
        let maxw = wifi.0 + wifi.2 - tx - 12.0 * s;
        let mid = wifi.1 + wifi.3 / 2.0;
        ctx.text.draw(pm, ctx.strings.wifi, tx, mid - 2.0 * s, fs, Weight::Bold, ink, None);
        let sub = ctx.text.ellipsize(&sub, small, Weight::Regular, maxw);
        ctx.text.draw(pm, &sub, tx, mid + 14.0 * s, small, Weight::Regular, ink2, None);

        // Bluetooth and appearance.
        for (h, on) in [(Hit::Bluetooth, st.bluetooth.unwrap_or(false)), (Hit::Dark, st.dark)] {
            let r = rect(h);
            let c = self.toggle(pm, ctx, r, on, true, ink);
            if lit(h)
                && let Some(p) = gfx::circle(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0)
            {
                gfx::fill(pm, &p, rgba(0, 0, 0, 0.12));
            }
            let g = 24.0 * s;
            let (gx, gy) = (r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0);
            if h == Hit::Bluetooth {
                glyphs::bluetooth(pm, gx, gy, g, c);
            } else {
                glyphs::moon(pm, gx, gy, g, c);
            }
        }

        // Now Playing.
        let np = self.cell(2.0, 0.0, 2.0, 2.0);
        self.glass(pm, ctx, np, module_r);
        let pad = 14.0 * s;
        let art = 0.62 * U * s;
        let (ax, ay) = (np.0 + pad, np.1 + pad);
        let text_w = np.2 - 2.0 * pad;
        match &st.playing {
            Some(p) => {
                match &p.art {
                    Some(img) => img.draw(pm, ax, ay, art, 1.0, false),
                    None => art_placeholder(pm, ax, ay, art, ink),
                }
                let title = ctx.text.ellipsize(&p.title, fs, Weight::Bold, text_w);
                ctx.text.draw(pm, &title, ax, ay + art + 22.0 * s, fs, Weight::Bold, ink, None);
                let artist = ctx.text.ellipsize(&p.artist, small, Weight::Regular, text_w);
                ctx.text.draw(pm, &artist, ax, ay + art + 39.0 * s, small, Weight::Regular, ink2, None);
            }
            None => {
                art_placeholder(pm, ax, ay, art, ink);
                let t = ctx.text.ellipsize(ctx.strings.not_playing, fs, Weight::Bold, text_w);
                ctx.text.draw(pm, &t, ax, ay + art + 22.0 * s, fs, Weight::Bold, ink2, None);
            }
        }
        let ctl = if st.playing.is_some() { ink } else { ink2 };
        for h in [Hit::Previous, Hit::PlayPause, Hit::Next] {
            let r = rect(h);
            if lit(h)
                && let Some(p) = gfx::circle(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0)
            {
                gfx::fill(pm, &p, gfx::fade(ink, 0.15));
            }
            let g = if h == Hit::PlayPause { 24.0 * s } else { 22.0 * s };
            let (gx, gy) = (r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0);
            match h {
                Hit::Previous => glyphs::skip(pm, gx, gy, g, ctl, true),
                Hit::Next => glyphs::skip(pm, gx, gy, g, ctl, false),
                _ if st.playing.as_ref().is_some_and(|p| p.playing) => glyphs::pause(pm, gx, gy, g, ctl),
                _ => glyphs::play(pm, gx, gy, g, ctl),
            }
        }

        // Display and Sound.
        let mut sliders = Vec::new();
        if let Some(b) = st.brightness {
            sliders.push((Hit::Display, ctx.strings.display, b));
        }
        sliders.push((Hit::Sound, ctx.strings.sound, if st.muted { 0.0 } else { st.volume.unwrap_or(0.0) }));
        for (h, label, v) in sliders {
            let m = self.cell(0.0, self.slider_row(h), 4.0, 1.0);
            self.glass(pm, ctx, m, module_r);
            ctx.text.draw(pm, label, m.0 + 16.0 * s, m.1 + 24.0 * s, fs, Weight::Bold, ink, None);
            let t = self.track(h);
            if let Some(p) = gfx::squircle(t.0, t.1, t.2, t.3, t.3 / 2.0) {
                gfx::fill(pm, &p, gfx::fade(ink, 0.22));
            }
            let fw = (t.2 * v.clamp(0.0, 1.0)).max(t.3);
            if let Some(p) = gfx::squircle(t.0, t.1, fw, t.3, t.3 / 2.0) {
                gfx::fill(pm, &p, gfx::fade(ink, 0.9));
            }
            let g = 15.0 * s;
            let gy = t.1 + t.3 / 2.0 - g / 2.0;
            if h == Hit::Display {
                glyphs::sun(pm, m.0 + 14.0 * s, gy + 2.0 * s, g * 0.75, ink);
                glyphs::sun(pm, t.0 + t.2 + 10.0 * s, gy - 1.0 * s, g * 1.1, ink);
            } else {
                let muted = st.muted || v < 0.01;
                glyphs::speaker(pm, m.0 + 13.0 * s, gy, g, ink, 0);
                glyphs::speaker(pm, t.0 + t.2 + 8.0 * s, gy, g * 1.1, if muted { ink2 } else { ink }, 2);
            }
        }

        // Shortcuts.
        for h in [Hit::Shot, Hit::Calc, Hit::Settings, Hit::Lock] {
            let r = rect(h);
            self.glass(pm, ctx, r, r.2 / 2.0);
            if lit(h)
                && let Some(p) = gfx::circle(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0)
            {
                gfx::fill(pm, &p, rgba(0, 0, 0, 0.12));
            }
            let g = 24.0 * s;
            let (gx, gy) = (r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0);
            match h {
                Hit::Shot => glyphs::screenshot(pm, gx, gy, g, ink),
                Hit::Calc => glyphs::calculator_glyph(pm, gx, gy, g, ink),
                Hit::Settings => glyphs::gear(pm, gx, gy, g, ink),
                _ => glyphs::lock(pm, gx, gy, g, ink),
            }
        }
        crate::panel::fade_all(pm, self.reveal.alpha());
    }
}

fn art_placeholder(pm: &mut Pixmap, x: f32, y: f32, size: f32, ink: Color) {
    if let Some(p) = gfx::squircle(x, y, size, size, size * 0.2) {
        gfx::fill(pm, &p, gfx::fade(ink, 0.14));
    }
    let g = size * 0.5;
    glyphs::music_note(pm, x + (size - g) / 2.0, y + (size - g) / 2.0, g, gfx::fade(ink, 0.6));
}
