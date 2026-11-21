//! Colors of the light and dark appearance.

use tiny_skia::Color;

use crate::gfx::rgba;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
}

impl Theme {
    pub fn accent(&self) -> Color {
        if self.dark { rgba(10, 132, 255, 1.0) } else { rgba(0, 122, 255, 1.0) }
    }
    pub fn label(&self) -> Color {
        if self.dark { rgba(255, 255, 255, 0.88) } else { rgba(0, 0, 0, 0.85) }
    }
    pub fn secondary(&self) -> Color {
        if self.dark { rgba(235, 235, 245, 0.55) } else { rgba(60, 60, 67, 0.6) }
    }
    pub fn tertiary(&self) -> Color {
        if self.dark { rgba(235, 235, 245, 0.3) } else { rgba(60, 60, 67, 0.3) }
    }
    pub fn separator(&self) -> Color {
        if self.dark { rgba(255, 255, 255, 0.12) } else { rgba(0, 0, 0, 0.1) }
    }
    /// Opaque-ish material of menus and panels that float over windows.
    pub fn panel(&self) -> Color {
        if self.dark { rgba(38, 38, 40, 1.0) } else { rgba(244, 244, 246, 1.0) }
    }
    pub fn panel_border(&self) -> Color {
        if self.dark { rgba(0, 0, 0, 0.55) } else { rgba(0, 0, 0, 0.16) }
    }
    /// Inner highlight line that gives dark panels their edge.
    pub fn panel_rim(&self) -> Color {
        if self.dark { rgba(255, 255, 255, 0.14) } else { rgba(255, 255, 255, 0.55) }
    }
    pub fn shadow(&self) -> Color {
        if self.dark { rgba(0, 0, 0, 0.5) } else { rgba(0, 0, 0, 0.26) }
    }
    /// Tint laid over the blurred wallpaper of the dock.
    pub fn dock_tint(&self) -> Color {
        if self.dark { rgba(28, 28, 30, 0.42) } else { rgba(255, 255, 255, 0.34) }
    }
    pub fn dock_fallback(&self) -> Color {
        if self.dark { rgba(40, 40, 42, 0.86) } else { rgba(236, 236, 238, 0.82) }
    }
    pub fn dock_border(&self) -> Color {
        if self.dark { rgba(0, 0, 0, 0.35) } else { rgba(0, 0, 0, 0.08) }
    }
    pub fn dock_rim(&self) -> Color {
        if self.dark { rgba(255, 255, 255, 0.16) } else { rgba(255, 255, 255, 0.45) }
    }
    pub fn indicator(&self) -> Color {
        if self.dark { rgba(255, 255, 255, 0.62) } else { rgba(0, 0, 0, 0.5) }
    }
    pub fn tooltip(&self) -> Color {
        if self.dark { rgba(44, 44, 46, 0.94) } else { rgba(240, 240, 242, 0.94) }
    }
    pub fn field_placeholder(&self) -> Color {
        if self.dark { rgba(235, 235, 245, 0.32) } else { rgba(60, 60, 67, 0.32) }
    }
}
