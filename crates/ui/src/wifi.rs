//! The Wi-Fi status menu of macOS Tahoe 26: a header with the switch, the
//! known networks in range, a collapsible Other Networks list and
//! "Wi-Fi Settings…". Sizes are measured in `out/ref/TAHOE-WIFI.md`.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use tiny_skia::{Color, PathBuilder, Pixmap};

use crate::gfx::{self, rgba};
use crate::glass::{Backdrop, Shape};
use crate::menu::{self, MARGIN_BOTTOM, MARGIN_TOP, MARGIN_X};
use crate::text::Weight;
use crate::{Ctx, WinRect, glyphs};

#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    /// Stable key of the SSID; the host maps it back to the network.
    pub id: u64,
    pub name: String,
    pub secure: bool,
    /// Signal quality 0..=100.
    pub signal: u8,
    /// Windows has a profile for it.
    pub known: bool,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Radio {
    /// No Wi-Fi adapter.
    #[default]
    Missing,
    Off,
    On,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WifiState {
    pub radio: Radio,
    pub networks: Vec<Network>,
    pub scanning: bool,
    /// The WLAN service cannot be reached: no list, only the switch.
    pub unavailable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WifiEvent {
    Power(bool),
    Join(u64),
    Disconnect(u64),
    /// Other Networks was opened: the list should be fresh.
    Scan,
    /// "Other…": join a hidden network.
    Hidden,
    Settings,
}

// Points, measured on Tahoe 26 (TAHOE-WIFI.md section 2).
const WIDTH: f32 = 308.0;
const VPAD: f32 = 6.0;
const HEADER_H: f32 = 31.0;
const SEP_H: f32 = 11.0;
const LABEL_H: f32 = 22.0;
const NET_H: f32 = 32.0;
const ITEM_H: f32 = 22.0;
const RADIUS: f32 = 15.0;
const TEXT_X: f32 = 15.0;
const SEP_INSET: f32 = 14.0;
const DISC_X: f32 = 14.0;
const DISC: f32 = 26.0;
const GLYPH_W: f32 = 13.7;
const NAME_X: f32 = 48.5;
const SWITCH_W: f32 = 53.5;
const SWITCH_H: f32 = 23.5;
const SWITCH_RIGHT: f32 = 14.5;
const KNOB_W: f32 = 31.5;
const KNOB_INSET: f32 = 2.0;
const LOCK_W: f32 = 8.3;
const LOCK_H: f32 = 12.0;
const LOCK_RIGHT: f32 = 17.0;
const CHEVRON_W: f32 = 5.4;
const CHEVRON_H: f32 = 9.5;
const CHEVRON_RIGHT: f32 = 15.4;
const FONT: f32 = 13.0;
const LABEL_FONT: f32 = 12.0;
const HOVER_INSET: f32 = 5.0;
const HOVER_RADIUS: f32 = 6.5;
/// Other Networks shows at most this many, strongest first.
pub const MAX_OTHER: usize = 15;
/// Measured menu close (menu.rs); menus open at once.
const FADE_OUT: f32 = 0.2;
const SWITCH_SLIDE: f32 = 0.2;
/// A switch flip overrides the reported radio state until it agrees or this passes.
const PENDING_POWER: f32 = 6.0;

/// Bars lit in the network glyph for a signal quality 0..=100.
pub fn signal_bars(quality: u8) -> u8 {
    match quality {
        60.. => 3,
        35..=59 => 2,
        _ => 1,
    }
}

/// Known networks (the connected one first, then by signal) and the others
/// (by signal, then name), as the menu lists them.
pub fn groups(nets: &[Network]) -> (Vec<Network>, Vec<Network>) {
    let order = |a: &Network, b: &Network| {
        b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    };
    let mut known: Vec<Network> = nets.iter().filter(|n| n.known || n.connected).cloned().collect();
    let mut other: Vec<Network> = nets.iter().filter(|n| !n.known && !n.connected).cloned().collect();
    known.sort_by(order);
    other.sort_by(order);
    other.truncate(MAX_OTHER);
    (known, other)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Msg {
    Searching,
    None,
    NoAdapter,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Header,
    Sep,
    KnownLabel,
    /// Index into `WifiMenu::list`.
    Net(usize),
    OtherHead,
    Message(Msg),
    Hidden,
    Settings,
}

impl Row {
    fn height(self) -> f32 {
        match self {
            Row::Header => HEADER_H,
            Row::Sep => SEP_H,
            Row::Net(_) => NET_H,
            Row::KnownLabel => LABEL_H,
            _ => ITEM_H,
        }
    }

    fn hoverable(self) -> bool {
        matches!(self, Row::Net(_) | Row::OtherHead | Row::Hidden | Row::Settings)
    }
}

pub struct WifiMenu {
    state: WifiState,
    /// Known networks, then the others.
    list: Vec<Network>,
    known_n: usize,
    /// The user opened or closed Other Networks; by default it is open only
    /// when no known network is in range.
    expanded: Option<bool>,
    scale: f32,
    hover: Option<usize>,
    closing: Option<f32>,
    /// Switch knob 0 (off) ..= 1 (on).
    knob: f32,
    pending: Option<(bool, f32)>,
    spin: f32,
    backdrop: Option<Backdrop>,
    pub events: Vec<WifiEvent>,
}

impl WifiMenu {
    pub fn new(state: WifiState, scale: f32) -> Self {
        let knob = if state.radio == Radio::On { 1.0 } else { 0.0 };
        let mut m = Self {
            state: WifiState::default(),
            list: Vec::new(),
            known_n: 0,
            expanded: None,
            scale,
            hover: None,
            closing: None,
            knob,
            pending: None,
            spin: 0.0,
            backdrop: None,
            events: Vec::new(),
        };
        m.set_state(state);
        m
    }

    pub fn set_state(&mut self, mut st: WifiState) {
        if let Some((on, _)) = self.pending {
            if (st.radio == Radio::On) == on || st.radio == Radio::Missing {
                self.pending = None;
            } else {
                st.radio = if on { Radio::On } else { Radio::Off };
            }
        }
        let (known, other) = groups(&st.networks);
        self.known_n = known.len();
        self.list = known;
        self.list.extend(other);
        self.state = st;
        self.hover = None;
    }

    pub fn set_backdrop(&mut self, behind: &Pixmap) {
        self.backdrop = Some(Backdrop::new(behind));
    }

    fn on(&self) -> bool {
        self.state.radio == Radio::On
    }

    fn expanded(&self) -> bool {
        self.expanded.unwrap_or(self.known_n == 0)
    }

    fn rows(&self) -> Vec<Row> {
        let mut r = vec![Row::Header, Row::Sep];
        match self.state.radio {
            Radio::Missing => r.extend([Row::Message(Msg::NoAdapter), Row::Sep]),
            Radio::Off => {}
            Radio::On if self.state.unavailable => r.extend([Row::Message(Msg::Unavailable), Row::Sep]),
            Radio::On => {
                if self.known_n > 0 {
                    r.push(Row::KnownLabel);
                    r.extend((0..self.known_n).map(Row::Net));
                    r.push(Row::Sep);
                }
                r.push(Row::OtherHead);
                if self.expanded() {
                    if self.list.len() > self.known_n {
                        r.extend((self.known_n..self.list.len()).map(Row::Net));
                    } else {
                        r.push(Row::Message(if self.state.scanning { Msg::Searching } else { Msg::None }));
                    }
                    r.push(Row::Hidden);
                }
                r.push(Row::Sep);
            }
        }
        r.push(Row::Settings);
        r
    }

    /// Rows with their top edge, in points from the body top.
    fn layout(&self) -> Vec<(Row, f32)> {
        let mut y = VPAD;
        self.rows()
            .into_iter()
            .map(|row| {
                let top = y;
                y += row.height();
                (row, top)
            })
            .collect()
    }

    fn body_h_pt(&self) -> f32 {
        VPAD * 2.0 + self.rows().iter().map(|r| r.height()).sum::<f32>()
    }

    pub fn body_size(&self) -> (f32, f32) {
        let s = self.scale;
        ((WIDTH * s).ceil(), (self.body_h_pt() * s).ceil())
    }

    pub fn window_size(&self) -> (i32, i32) {
        let s = self.scale;
        let (bw, bh) = self.body_size();
        ((bw + 2.0 * MARGIN_X * s).ceil() as i32, (bh + (MARGIN_TOP + MARGIN_BOTTOM) * s).ceil() as i32)
    }

    pub fn body_offset(&self) -> (i32, i32) {
        ((MARGIN_X * self.scale).round() as i32, (MARGIN_TOP * self.scale).round() as i32)
    }

    /// Tahoe hangs status menus from the left end of the item's capsule,
    /// flush with the bar's bottom edge; the menu stays on screen.
    pub fn window_rect(&self, anchor_x: f32, bar_bottom: i32, screen_w: i32) -> WinRect {
        let s = self.scale;
        let (bw, _) = self.body_size();
        let bx = anchor_x.min(screen_w as f32 - bw - 4.0 * s).max(4.0 * s).round() as i32;
        let (ox, oy) = self.body_offset();
        let (w, h) = self.window_size();
        WinRect { x: bx - ox, y: bar_bottom - oy, w, h }
    }

    /// The body in screen pixels for a window at `win`.
    pub fn body_rect(&self, win: WinRect) -> WinRect {
        let (ox, oy) = self.body_offset();
        let (bw, bh) = self.body_size();
        WinRect { x: win.x + ox, y: win.y + oy, w: bw as i32, h: bh as i32 }
    }

    pub fn close(&mut self) {
        if self.closing.is_none() {
            self.closing = Some(0.0);
        }
    }

    pub fn closing(&self) -> bool {
        self.closing.is_some()
    }

    pub fn finished(&self) -> bool {
        self.closing.is_some_and(|t| t >= FADE_OUT)
    }

    fn knob_target(&self) -> f32 {
        if self.on() { 1.0 } else { 0.0 }
    }

    fn spinning(&self) -> bool {
        self.on() && self.state.scanning && self.closing.is_none() && self.rows().contains(&Row::OtherHead)
    }

    pub fn animating(&self) -> bool {
        self.closing.is_some_and(|t| t < FADE_OUT) || (self.knob - self.knob_target()).abs() > 1e-3 || self.spinning()
    }

    pub fn update(&mut self, dt: f32) {
        if let Some(t) = &mut self.closing {
            *t += dt;
        }
        let target = self.knob_target();
        let step = dt / SWITCH_SLIDE;
        self.knob = if self.knob < target { (self.knob + step).min(target) } else { (self.knob - step).max(target) };
        if let Some((_, age)) = &mut self.pending {
            *age += dt;
            if *age > PENDING_POWER {
                self.pending = None;
            }
        }
        if self.spinning() {
            self.spin = (self.spin + dt) % 1.0;
        }
    }

    fn row_at(&self, x: f32, y: f32) -> Option<(usize, Row, f32)> {
        let (ox, oy) = self.body_offset();
        let s = self.scale;
        let (bx, by) = ((x - ox as f32) / s, (y - oy as f32) / s);
        if !(0.0..WIDTH).contains(&bx) {
            return None;
        }
        self.layout().into_iter().enumerate().find(|(_, (row, top))| by >= *top && by < top + row.height()).map(|(i, (row, top))| (i, row, top))
    }

    fn on_switch(&self, x: f32, y: f32) -> bool {
        let (ox, oy) = self.body_offset();
        let s = self.scale;
        let (bx, by) = ((x - ox as f32) / s, (y - oy as f32) / s);
        let x0 = WIDTH - SWITCH_RIGHT - SWITCH_W;
        let cy = VPAD + HEADER_H / 2.0;
        // A few points of slop around the switch, as for any small control.
        bx >= x0 - 4.0 && bx <= WIDTH - SWITCH_RIGHT + 4.0 && (by - cy).abs() <= SWITCH_H / 2.0 + 4.0
    }

    /// Opens or closes Other Networks as a click on it would, without the scan.
    pub fn set_expanded(&mut self, open: bool) {
        self.expanded = Some(open);
    }

    /// Window point at the middle of a network's name, for previews.
    pub fn network_point(&self, id: u64) -> Option<(f32, f32)> {
        let i = self.list.iter().position(|n| n.id == id)?;
        let (_, top) = self.layout().into_iter().find(|(r, _)| *r == Row::Net(i))?;
        let (ox, oy) = self.body_offset();
        Some((ox as f32 + (NAME_X + 20.0) * self.scale, oy as f32 + (top + NET_H / 2.0) * self.scale))
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if self.closing.is_some() {
            return;
        }
        self.hover = self.row_at(x, y).filter(|(_, row, _)| row.hoverable()).map(|(i, _, _)| i);
    }

    pub fn mouse_leave(&mut self) {
        self.hover = None;
    }

    /// Handles a click; returns true when it landed on the menu body.
    pub fn mouse_up(&mut self, x: f32, y: f32) -> bool {
        if self.closing.is_some() {
            return true;
        }
        if self.on_switch(x, y) {
            if self.state.radio != Radio::Missing {
                let on = !self.on();
                self.pending = Some((on, 0.0));
                self.state.radio = if on { Radio::On } else { Radio::Off };
                self.state.scanning = on;
                self.hover = None;
                self.events.push(WifiEvent::Power(on));
            }
            return true;
        }
        let Some((_, row, _)) = self.row_at(x, y) else { return false };
        let (ox, _) = self.body_offset();
        let bx = (x - ox as f32) / self.scale;
        match row {
            Row::Net(i) => {
                let n = &self.list[i];
                if n.connected {
                    // Tahoe: clicking the Wi-Fi icon left of the connected
                    // network disconnects it; its name does nothing.
                    if bx <= DISC_X + DISC + 2.0 {
                        self.events.push(WifiEvent::Disconnect(n.id));
                        self.close();
                    }
                } else {
                    self.events.push(WifiEvent::Join(n.id));
                    self.close();
                }
            }
            Row::OtherHead => {
                let open = !self.expanded();
                self.expanded = Some(open);
                self.hover = None;
                if open {
                    self.events.push(WifiEvent::Scan);
                }
            }
            Row::Hidden => {
                self.events.push(WifiEvent::Hidden);
                self.close();
            }
            Row::Settings => {
                self.events.push(WifiEvent::Settings);
                self.close();
            }
            _ => {}
        }
        true
    }

    pub fn draw(&self, pm: &mut Pixmap, ctx: &Ctx) {
        pm.fill(Color::TRANSPARENT);
        let a = self.closing.map_or(1.0, |t| 1.0 - (t / FADE_OUT).clamp(0.0, 1.0));
        if a <= 0.0 {
            return;
        }
        let s = self.scale;
        let dark = ctx.theme.dark;
        let (ox, oy) = self.body_offset();
        let (bx, by) = (ox as f32, oy as f32);
        let (bw, bh) = self.body_size();
        menu::sheet(pm, ctx, self.backdrop.as_ref(), Shape { x: bx, y: by, w: bw, h: bh, r: RADIUS * s }, s);

        let primary = menu::text_color(dark);
        // Section titles, the lock and the chevron: 0.93 of the primary ink
        // in Tahoe (Sequoia drew them at 0.6, a plain secondary).
        let heading = gfx::fade(primary, 0.93);
        let secondary = menu::secondary(dark);
        let cap = |size: f32| ctx.text.cap_height(size * s);
        for (i, (row, top)) in self.layout().into_iter().enumerate() {
            let y = by + top * s;
            let h = row.height() * s;
            let cy = y + h / 2.0;
            if self.hover == Some(i) && let Some(p) = gfx::squircle(bx + HOVER_INSET * s, y.round(), bw - 2.0 * HOVER_INSET * s, h.round(), HOVER_RADIUS * s) {
                gfx::fill(pm, &p, if dark { rgba(255, 255, 255, 0.1) } else { rgba(0, 0, 0, 0.07) });
            }
            let text = |pm: &mut Pixmap, t: &str, x: f32, size: f32, w: Weight, c: Color, max: f32| {
                let t = ctx.text.ellipsize(t, size * s, w, max);
                ctx.text.draw(pm, &t, (bx + x * s).round(), (cy + cap(size) / 2.0).round(), size * s, w, c, None);
            };
            match row {
                Row::Header => {
                    text(pm, ctx.strings.wifi, TEXT_X, FONT, Weight::Bold, primary, bw);
                    let x = bx + (WIDTH - SWITCH_RIGHT - SWITCH_W) * s;
                    let enabled = self.state.radio != Radio::Missing;
                    switch(pm, ctx, x, cy, s, self.knob, enabled);
                }
                Row::Sep => {
                    let t = s.max(1.0).round();
                    gfx::fill_rect(pm, bx + SEP_INSET * s, (cy - t / 2.0).round(), bw - 2.0 * SEP_INSET * s, t, menu::separator(dark));
                }
                Row::KnownLabel => {
                    let label = if self.known_n == 1 { ctx.strings.wifi_known_one } else { ctx.strings.wifi_known };
                    text(pm, label, TEXT_X, LABEL_FONT, Weight::Bold, heading, bw);
                }
                Row::Net(n) => {
                    let net = &self.list[n];
                    let dcx = bx + (DISC_X + DISC / 2.0) * s;
                    let (disc_c, glyph_c) = if net.connected {
                        (ctx.theme.accent(), rgba(255, 255, 255, 1.0))
                    } else if dark {
                        (rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.92))
                    } else {
                        (rgba(0, 0, 0, 0.115), rgba(0, 0, 0, 0.85))
                    };
                    if let Some(p) = gfx::circle(dcx, cy, DISC * s / 2.0) {
                        gfx::fill(pm, &p, disc_c);
                    }
                    let gw = GLYPH_W * s;
                    glyphs::wifi_bars(pm, dcx - gw / 2.0, cy - gw * 0.74 / 2.0, gw, glyph_c, signal_bars(net.signal));
                    let right = if net.secure { LOCK_RIGHT + LOCK_W + 8.0 } else { 15.0 };
                    text(pm, &net.name, NAME_X, FONT, Weight::Regular, primary, (WIDTH - NAME_X - right) * s);
                    if net.secure {
                        padlock(pm, bx + (WIDTH - LOCK_RIGHT - LOCK_W) * s, cy - LOCK_H * s / 2.0, s, heading);
                    }
                }
                Row::OtherHead => {
                    text(pm, ctx.strings.wifi_other_networks, TEXT_X, LABEL_FONT, Weight::Bold, heading, bw);
                    let ccx = bx + (WIDTH - CHEVRON_RIGHT - CHEVRON_W / 2.0) * s;
                    chevron(pm, ccx, cy, s, heading, self.expanded());
                    if self.spinning() {
                        spinner(pm, ccx - 18.0 * s, cy, s, secondary, self.spin);
                    }
                }
                Row::Message(m) => {
                    let st = ctx.strings;
                    let t = match m {
                        Msg::Searching => st.wifi_searching,
                        Msg::None => st.wifi_none,
                        Msg::NoAdapter => st.wifi_no_adapter,
                        Msg::Unavailable => st.wifi_unavailable,
                    };
                    text(pm, t, TEXT_X, FONT, Weight::Regular, menu::disabled(dark), bw - 2.0 * TEXT_X * s);
                }
                Row::Hidden => text(pm, ctx.strings.wifi_hidden, TEXT_X, FONT, Weight::Regular, primary, bw),
                Row::Settings => text(pm, ctx.strings.wifi_settings, TEXT_X, FONT, Weight::Regular, primary, bw),
            }
        }
        crate::panel::fade_all(pm, a);
    }
}

/// Tahoe's switch: a 53.5 x 23.5 pt capsule, accent when on, with a white
/// pill knob 31.5 x 19.5 pt inset 2 pt; `knob` runs 0 (off) ..= 1 (on).
fn switch(pm: &mut Pixmap, ctx: &Ctx, x: f32, cy: f32, s: f32, knob: f32, enabled: bool) {
    let dark = ctx.theme.dark;
    let (w, h) = (SWITCH_W * s, SWITCH_H * s);
    let y = cy - h / 2.0;
    let off = if dark { rgba(255, 255, 255, 0.16) } else { rgba(0, 0, 0, 0.09) };
    let track = gfx::mix(off, ctx.theme.accent(), knob);
    let track = if enabled { track } else { gfx::fade(off, 0.6) };
    if let Some(p) = gfx::squircle(x, y, w, h, h / 2.0) {
        gfx::fill(pm, &p, track);
    }
    let (kw, kh) = (KNOB_W * s, h - 2.0 * KNOB_INSET * s);
    let travel = w - 2.0 * KNOB_INSET * s - kw;
    let kx = x + KNOB_INSET * s + travel * knob;
    let kc = if enabled { rgba(255, 255, 255, 1.0) } else { rgba(255, 255, 255, if dark { 0.35 } else { 0.7 }) };
    if let Some(p) = gfx::squircle(kx, y + KNOB_INSET * s, kw, kh, kh / 2.0) {
        ctx.shadow(kh / 2.0, 1.5 * s).draw(pm, kx, y + KNOB_INSET * s + 0.5 * s, kw, kh, rgba(0, 0, 0, 0.18));
        gfx::fill(pm, &p, kc);
    }
}

/// SF `lock.fill` at its menu size: 8.3 x 12 pt, `(x, y)` its top-left.
fn padlock(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let (w, h) = (LOCK_W * s, LOCK_H * s);
    let body_h = h * 0.58;
    if let Some(p) = gfx::squircle(x, y + h - body_h, w, body_h, 1.6 * s) {
        gfx::fill(pm, &p, c);
    }
    let lw = 1.45 * s;
    let r = w * 0.5 - lw * 0.5 - 0.95 * s;
    let (cx, top) = (x + w / 2.0, y + lw / 2.0 + r);
    let mut pb = PathBuilder::new();
    pb.move_to(cx - r, y + h - body_h + 0.5 * s);
    pb.line_to(cx - r, top);
    let n = 12;
    for i in 1..=n {
        let a = PI + PI * i as f32 / n as f32;
        pb.line_to(cx + r * a.cos(), top + r * a.sin());
    }
    pb.line_to(cx + r, y + h - body_h + 0.5 * s);
    if let Some(p) = pb.finish() {
        gfx::stroke(pm, &p, c, lw);
    }
}

/// The disclosure chevron of Other Networks, 5.4 x 9.5 pt pointing right;
/// turned down while the list is open.
fn chevron(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, down: bool) {
    let (hw, hh) = (CHEVRON_W * s / 2.0, CHEVRON_H * s / 2.0);
    let lw = 1.5 * s;
    let (hw, hh) = (hw - lw / 2.0, hh - lw / 2.0);
    let pts = if down { [(-hh, -hw), (0.0, hw), (hh, -hw)] } else { [(-hw, -hh), (hw, 0.0), (-hw, hh)] };
    let mut pb = PathBuilder::new();
    pb.move_to(cx + pts[0].0, cy + pts[0].1);
    pb.line_to(cx + pts[1].0, cy + pts[1].1);
    pb.line_to(cx + pts[2].0, cy + pts[2].1);
    if let Some(p) = pb.finish() {
        gfx::stroke_round(pm, &p, c, lw);
    }
}

/// The classic macOS progress spinner: eight spokes, the brightest one
/// turning once a second (`phase` 0..1).
fn spinner(pm: &mut Pixmap, cx: f32, cy: f32, s: f32, c: Color, phase: f32) {
    let (r0, r1) = (2.3 * s, 5.0 * s);
    let lead = (phase * 8.0).floor();
    for i in 0..8 {
        let a = -FRAC_PI_2 + TAU * i as f32 / 8.0;
        let age = (lead - i as f32).rem_euclid(8.0);
        let mut pb = PathBuilder::new();
        pb.move_to(cx + r0 * a.cos(), cy + r0 * a.sin());
        pb.line_to(cx + r1 * a.cos(), cy + r1 * a.sin());
        if let Some(p) = pb.finish() {
            gfx::stroke_round(pm, &p, gfx::fade(c, 1.0 - age / 9.0), 1.3 * s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(id: u64, name: &str, signal: u8, known: bool, connected: bool) -> Network {
        Network { id, name: name.into(), secure: true, signal, known, connected }
    }

    fn on(nets: Vec<Network>) -> WifiState {
        WifiState { radio: Radio::On, networks: nets, scanning: false, unavailable: false }
    }

    /// Screen point (window px) inside row `i` at body x `bx` pt.
    fn at(m: &WifiMenu, row: Row, bx: f32) -> (f32, f32) {
        let (ox, oy) = m.body_offset();
        let (_, top) = m.layout().into_iter().find(|(r, _)| *r == row).unwrap();
        (ox as f32 + bx * m.scale, oy as f32 + (top + row.height() / 2.0) * m.scale)
    }

    #[test]
    fn signal_maps_to_three_bars() {
        assert_eq!([0, 34, 35, 59, 60, 100].map(signal_bars), [1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn known_networks_lead_with_the_connected_one_and_others_sort_by_signal() {
        let nets = vec![
            net(1, "b-other", 40, false, false),
            net(2, "home", 50, true, false),
            net(3, "office", 90, true, true),
            net(4, "A-other", 40, false, false),
            net(5, "strong", 95, false, false),
        ];
        let (known, other) = groups(&nets);
        assert_eq!(known.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 2]);
        assert_eq!(other.iter().map(|n| n.id).collect::<Vec<_>>(), [5, 4, 1]);
        let many: Vec<Network> = (0..40).map(|i| net(i, "x", i as u8, false, false)).collect();
        assert_eq!(groups(&many).1.len(), MAX_OTHER);
    }

    #[test]
    fn measured_layout_of_one_connected_known_network() {
        // MacPaw's Tahoe menu: header, Known Network, Other Networks closed,
        // Wi-Fi Settings; 174.8 pt measured.
        let m = WifiMenu::new(on(vec![net(1, "Apple", 80, true, true), net(2, "x", 30, false, false)]), 1.0);
        assert_eq!(m.rows(), vec![Row::Header, Row::Sep, Row::KnownLabel, Row::Net(0), Row::Sep, Row::OtherHead, Row::Sep, Row::Settings]);
        assert_eq!(m.body_h_pt(), 174.0);
        let off = WifiMenu::new(WifiState { radio: Radio::Off, ..Default::default() }, 1.0);
        assert_eq!(off.rows(), vec![Row::Header, Row::Sep, Row::Settings]);
    }

    #[test]
    fn other_networks_open_by_themselves_only_without_known_ones() {
        let mut m = WifiMenu::new(on(vec![net(2, "x", 30, false, false)]), 1.0);
        assert!(m.rows().contains(&Row::Net(0)) && m.rows().contains(&Row::Hidden));
        m.set_state(WifiState { scanning: true, ..on(vec![]) });
        assert!(m.rows().contains(&Row::Message(Msg::Searching)));
        assert!(m.animating(), "the spinner turns while scanning");
    }

    #[test]
    fn clicks_do_what_tahoe_does() {
        let nets = vec![net(1, "home", 80, true, true), net(2, "cafe", 60, true, false), net(3, "x", 30, false, false)];
        let mut m = WifiMenu::new(on(nets.clone()), 2.0);
        // The connected network's name does nothing, its icon disconnects.
        let (x, y) = at(&m, Row::Net(0), NAME_X + 20.0);
        assert!(m.mouse_up(x, y));
        assert!(m.events.is_empty() && !m.closing());
        // Other Networks opens in place and asks for a scan.
        let (x, y) = at(&m, Row::OtherHead, 100.0);
        m.mouse_up(x, y);
        assert_eq!(m.events, [WifiEvent::Scan]);
        assert!(m.rows().contains(&Row::Net(2)) && !m.closing());
        m.events.clear();
        let (x, y) = at(&m, Row::Net(1), NAME_X + 20.0);
        m.mouse_up(x, y);
        assert_eq!(m.events, [WifiEvent::Join(2)]);
        assert!(m.closing());

        let mut m = WifiMenu::new(on(nets), 2.0);
        let (x, y) = at(&m, Row::Net(0), DISC_X + DISC / 2.0);
        m.mouse_up(x, y);
        assert_eq!(m.events, [WifiEvent::Disconnect(1)]);
    }

    #[test]
    fn the_switch_flips_at_once_and_holds_until_windows_agrees() {
        let mut m = WifiMenu::new(on(vec![net(1, "home", 80, true, true)]), 1.0);
        let (ox, oy) = m.body_offset();
        let (x, y) = (ox as f32 + WIDTH - SWITCH_RIGHT - 10.0, oy as f32 + VPAD + HEADER_H / 2.0);
        assert!(m.mouse_up(x, y));
        assert_eq!(m.events, [WifiEvent::Power(false)]);
        assert!(!m.closing() && m.animating());
        // A stale "on" report does not flip the switch back.
        m.set_state(on(vec![]));
        assert_eq!(m.rows(), vec![Row::Header, Row::Sep, Row::Settings]);
        m.update(1.0);
        assert_eq!(m.knob, 0.0);
        m.set_state(WifiState { radio: Radio::Off, ..Default::default() });
        assert!(m.pending.is_none());
    }

    #[test]
    fn opens_at_once_and_fades_out() {
        let mut m = WifiMenu::new(WifiState { radio: Radio::Off, ..Default::default() }, 1.0);
        assert!(!m.animating());
        m.close();
        m.update(0.1);
        assert!(m.animating() && !m.finished());
        m.update(0.11);
        assert!(m.finished());
    }
}
