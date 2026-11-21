//! Control Center: Wi-Fi and Bluetooth toggles, Now Playing, appearance,
//! screenshot, display brightness and sound volume.

use std::rc::Rc;

use tiny_skia::Pixmap;

use crate::gfx::{self, Image};
use crate::panel::{self, MARGIN, Reveal, inside};
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
    /// Open a settings page: "wifi", "bluetooth", "sound", "display".
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
    Display,
    Sound,
    SoundIcon,
}

type R = (f32, f32, f32, f32);

// Points.
const W: f32 = 320.0;
const PAD: f32 = 12.0;
const GAP: f32 = 10.0;
const ROW_A: f32 = 136.0;
const ROW_B: f32 = 58.0;
const ROW_S: f32 = 62.0;
const RADIUS: f32 = 24.0;
const TILE_R: f32 = 18.0;
const SLIDER_H: f32 = 22.0;

pub struct ControlCenter {
    scale: f32,
    state: ControlState,
    reveal: Reveal,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    dragging: Option<Hit>,
    pub events: Vec<ControlEvent>,
}

impl ControlCenter {
    pub fn new(state: ControlState, scale: f32) -> Self {
        Self { scale, state, reveal: Reveal::new(), hover: None, pressed: None, dragging: None, events: Vec::new() }
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

    fn body_h(&self) -> f32 {
        let mut h = PAD + ROW_A + GAP + ROW_B + GAP + ROW_S + PAD;
        if self.state.brightness.is_some() {
            h += ROW_S + GAP;
        }
        h
    }

    pub fn body_size(&self) -> (f32, f32) {
        (W * self.scale, self.body_h() * self.scale)
    }

    pub fn window_size(&self) -> (i32, i32) {
        let s = self.scale;
        (((W + 2.0 * MARGIN) * s).ceil() as i32, ((self.body_h() + 2.0 * MARGIN) * s).ceil() as i32)
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

    /// Body origin in window px.
    fn origin(&self) -> (f32, f32) {
        let m = MARGIN * self.scale;
        (m, m - (1.0 - self.reveal.alpha()) * 8.0 * self.scale)
    }

    fn layout(&self) -> Vec<(Hit, R)> {
        let s = self.scale;
        let (ox, oy) = self.origin();
        let half = (W - 2.0 * PAD - GAP) / 2.0;
        let (lx, rx) = (ox + PAD * s, ox + (PAD + half + GAP) * s);
        let mut v = Vec::new();
        let ya = oy + PAD * s;
        for (i, (btn, label)) in [(Hit::Wifi, Hit::WifiLabel), (Hit::Bluetooth, Hit::BluetoothLabel)].into_iter().enumerate() {
            let cy = ya + (14.0 + 60.0 * i as f32) * s;
            v.push((btn, (lx + 12.0 * s, cy, 36.0 * s, 36.0 * s)));
            v.push((label, (lx + 52.0 * s, cy, (half - 56.0) * s, 36.0 * s)));
        }
        let cy = ya + (ROW_A - 38.0) * s;
        let bw = 34.0 * s;
        let cx = rx + half * s / 2.0;
        v.push((Hit::Previous, (cx - bw * 1.6, cy, bw, bw)));
        v.push((Hit::PlayPause, (cx - bw / 2.0, cy, bw, bw)));
        v.push((Hit::Next, (cx + bw * 0.6, cy, bw, bw)));
        let yb = ya + (ROW_A + GAP) * s;
        v.push((Hit::Dark, (lx, yb, half * s, ROW_B * s)));
        v.push((Hit::Shot, (rx, yb, half * s, ROW_B * s)));
        let mut ys = yb + (ROW_B + GAP) * s;
        let sw = (W - 2.0 * PAD - 24.0) * s;
        if self.state.brightness.is_some() {
            v.push((Hit::Display, (lx + 12.0 * s, ys + 30.0 * s, sw, SLIDER_H * s)));
            ys += (ROW_S + GAP) * s;
        }
        v.push((Hit::SoundIcon, (lx + 12.0 * s, ys + 30.0 * s, SLIDER_H * s * 1.4, SLIDER_H * s)));
        v.push((Hit::Sound, (lx + 12.0 * s, ys + 30.0 * s, sw, SLIDER_H * s)));
        v
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Hit, R)> {
        let l = self.layout();
        // The speaker glyph sits on the sound slider: test it first.
        l.iter().find(|(h, r)| *h == Hit::SoundIcon && inside(*r, x, y)).or_else(|| l.iter().find(|(_, r)| inside(*r, x, y))).copied()
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
        let Some((_, r)) = self.layout().into_iter().find(|(k, _)| *k == h) else { return };
        let v = panel::slider_value(r.0, r.2, r.3, x);
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
            Hit::SoundIcon => {
                st.muted = !st.muted;
                Some(ControlEvent::Muted(st.muted))
            }
            Hit::Display | Hit::Sound => None,
        };
        self.events.extend(ev);
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let s = self.scale;
        let th = ctx.theme;
        let st = &self.state;
        let (ox, oy) = self.origin();
        let (bw, bh) = self.body_size();
        panel::body(pm, ctx, ox, oy, bw, bh, RADIUS * s, s);
        let half = (W - 2.0 * PAD - GAP) / 2.0;
        let (lx, rx) = (ox + PAD * s, ox + (PAD + half + GAP) * s);
        let ya = oy + PAD * s;
        let l = self.layout();
        let rect = |h: Hit| l.iter().find(|(k, _)| *k == h).map(|(_, r)| *r).unwrap_or_default();
        let lit = |h: Hit| self.pressed == Some(h) && self.hover == Some(h);
        let fs = 13.0 * s;
        let small = 11.5 * s;

        // Connectivity.
        panel::tile(pm, th, lx, ya, half * s, ROW_A * s, TILE_R * s, false);
        let wifi_on = st.wifi.as_ref().is_some_and(|w| w.0);
        let wifi_sub = match &st.wifi {
            None => ctx.strings.not_connected.to_string(),
            Some((false, _)) => ctx.strings.off.to_string(),
            Some((true, Some(n))) => n.clone(),
            Some((true, None)) => ctx.strings.not_connected.to_string(),
        };
        let bt_on = st.bluetooth.unwrap_or(false);
        let bt_sub = match st.bluetooth {
            None => ctx.strings.not_connected,
            Some(true) => ctx.strings.on,
            Some(false) => ctx.strings.off,
        };
        for (btn, name, sub, on) in [
            (Hit::Wifi, ctx.strings.wifi, wifi_sub, wifi_on),
            (Hit::Bluetooth, ctx.strings.bluetooth, bt_sub.to_string(), bt_on),
        ] {
            let r = rect(btn);
            let c = panel::toggle(pm, th, r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0, on, lit(btn));
            let g = 18.0 * s;
            let (gx, gy) = (r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0);
            if btn == Hit::Wifi {
                glyphs::wifi(pm, gx, gy, g, c, true);
            } else {
                glyphs::bluetooth(pm, gx, gy, g, c);
            }
            let tx = r.0 + r.2 + 9.0 * s;
            let maxw = lx + half * s - tx - 8.0 * s;
            ctx.text.draw(pm, name, tx, r.1 + 15.0 * s, fs, Weight::Medium, th.label(), None);
            let sub = ctx.text.ellipsize(&sub, small, Weight::Regular, maxw);
            ctx.text.draw(pm, &sub, tx, r.1 + 31.0 * s, small, Weight::Regular, th.secondary(), None);
        }

        // Now Playing.
        panel::tile(pm, th, rx, ya, half * s, ROW_A * s, TILE_R * s, false);
        let art = 46.0 * s;
        let (ax, ay) = (rx + 12.0 * s, ya + 12.0 * s);
        let text_w = half * s - 24.0 * s;
        match &st.playing {
            Some(p) => {
                match &p.art {
                    Some(img) => img.draw(pm, ax, ay, art, 1.0, false),
                    None => art_placeholder(pm, ctx, ax, ay, art),
                }
                let title = ctx.text.ellipsize(&p.title, fs, Weight::Bold, text_w);
                ctx.text.draw(pm, &title, ax, ay + art + 18.0 * s, fs, Weight::Bold, th.label(), None);
                let artist = ctx.text.ellipsize(&p.artist, small, Weight::Regular, text_w);
                ctx.text.draw(pm, &artist, ax, ay + art + 33.0 * s, small, Weight::Regular, th.secondary(), None);
            }
            None => {
                art_placeholder(pm, ctx, ax, ay, art);
                let t = ctx.text.ellipsize(ctx.strings.not_playing, fs, Weight::Medium, text_w);
                ctx.text.draw(pm, &t, ax, ay + art + 18.0 * s, fs, Weight::Medium, th.secondary(), None);
            }
        }
        let ctl = if st.playing.is_some() { th.label() } else { th.tertiary() };
        for h in [Hit::Previous, Hit::PlayPause, Hit::Next] {
            let r = rect(h);
            if lit(h)
                && let Some(p) = gfx::circle(r.0 + r.2 / 2.0, r.1 + r.3 / 2.0, r.2 / 2.0)
            {
                gfx::fill(pm, &p, th.separator());
            }
            let g = if h == Hit::PlayPause { 20.0 * s } else { 18.0 * s };
            let (gx, gy) = (r.0 + (r.2 - g) / 2.0, r.1 + (r.3 - g) / 2.0);
            match h {
                Hit::Previous => glyphs::skip(pm, gx, gy, g, ctl, true),
                Hit::Next => glyphs::skip(pm, gx, gy, g, ctl, false),
                _ if st.playing.as_ref().is_some_and(|p| p.playing) => glyphs::pause(pm, gx, gy, g, ctl),
                _ => glyphs::play(pm, gx, gy, g, ctl),
            }
        }

        // Small tiles.
        for (h, label, on) in [(Hit::Dark, ctx.strings.dark_mode, st.dark), (Hit::Shot, ctx.strings.screenshot, false)] {
            let r = rect(h);
            panel::tile(pm, th, r.0, r.1, r.2, r.3, TILE_R * s, lit(h));
            let rad = 15.0 * s;
            let (cx, cy) = (r.0 + 12.0 * s + rad, r.1 + r.3 / 2.0);
            let c = panel::toggle(pm, th, cx, cy, rad, on, false);
            let g = 16.0 * s;
            if h == Hit::Dark {
                glyphs::moon(pm, cx - g / 2.0, cy - g / 2.0, g, c);
            } else {
                glyphs::screenshot(pm, cx - g / 2.0, cy - g / 2.0, g, c);
            }
            let tx = cx + rad + 9.0 * s;
            let lines = ctx.text.wrap(label, fs, Weight::Medium, r.0 + r.2 - tx - 8.0 * s, 2);
            let lh = 15.0 * s;
            let top = cy - lh * lines.len() as f32 / 2.0 + ctx.text.cap_height(fs) + 2.0 * s;
            for (i, line) in lines.iter().enumerate() {
                ctx.text.draw(pm, line, tx, top + i as f32 * lh, fs, Weight::Medium, th.label(), None);
            }
        }

        // Sliders.
        let full = (W - 2.0 * PAD) * s;
        let mut sliders = Vec::new();
        if let Some(b) = st.brightness {
            sliders.push((Hit::Display, ctx.strings.display, b));
        }
        sliders.push((Hit::Sound, ctx.strings.sound, if st.muted { 0.0 } else { st.volume.unwrap_or(0.0) }));
        for (h, label, v) in sliders {
            let r = rect(h);
            panel::tile(pm, th, lx, r.1 - 30.0 * s, full, ROW_S * s, TILE_R * s, false);
            ctx.text.draw(pm, label, r.0, r.1 - 10.0 * s, fs, Weight::Medium, th.label(), None);
            panel::slider(pm, ctx, r.0, r.1, r.2, r.3, v);
            let g = 14.0 * s;
            let gc = th.secondary();
            let (gx, gy) = (r.0 + 5.0 * s, r.1 + (r.3 - g) / 2.0);
            if h == Hit::Display {
                glyphs::sun(pm, gx, gy, g, gc);
            } else {
                let waves = if st.muted || v < 0.01 { 0 } else if v < 0.5 { 1 } else { 2 };
                glyphs::speaker(pm, gx, gy, g, gc, waves);
            }
        }
        panel::fade_all(pm, self.reveal.alpha());
    }
}

fn art_placeholder(pm: &mut Pixmap, ctx: &Ctx, x: f32, y: f32, size: f32) {
    let th = ctx.theme;
    if let Some(p) = gfx::squircle(x, y, size, size, size * 0.2) {
        gfx::fill(pm, &p, if th.dark { gfx::rgba(255, 255, 255, 0.1) } else { gfx::rgba(0, 0, 0, 0.06) });
    }
    let g = size * 0.5;
    glyphs::music_note(pm, x + (size - g) / 2.0, y + (size - g) / 2.0, g, th.tertiary());
}
