//! Platform-independent part of macRust: every shell component renders itself
//! into a premultiplied RGBA pixmap and keeps its own animation state, so the
//! Windows host only moves pixels to layered windows and feeds input back.

pub mod anim;
pub mod dock;
pub mod gfx;
pub mod glyphs;
pub mod menu;
pub mod menubar;
pub mod search;
pub mod spotlight;
pub mod strings;
pub mod text;
pub mod theme;
pub mod wallpaper;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub use tiny_skia;

use gfx::ShadowTex;
use strings::Strings;
use text::Text;
use theme::Theme;
use wallpaper::Vibrancy;

/// Shared drawing context: fonts, colors, copy and caches.
pub struct Ctx {
    pub text: Text,
    pub theme: Theme,
    pub strings: &'static Strings,
    pub vibrancy: Option<Vibrancy>,
    shadows: RefCell<HashMap<(u32, u32), Rc<ShadowTex>>>,
}

impl Ctx {
    pub fn new(text: Text, theme: Theme, strings: &'static Strings) -> Self {
        Self { text, theme, strings, vibrancy: None, shadows: RefCell::new(HashMap::new()) }
    }

    /// A cached shadow texture for a shape with corner `radius` blurred by `sigma` px.
    pub fn shadow(&self, radius: f32, sigma: f32) -> Rc<ShadowTex> {
        let key = ((radius * 4.0) as u32, (sigma * 4.0) as u32);
        self.shadows.borrow_mut().entry(key).or_insert_with(|| Rc::new(ShadowTex::new(radius, sigma))).clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    SelectAll,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A window's place on screen, in physical pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WinRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
