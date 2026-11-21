//! A layered top-level window that shows a premultiplied pixmap with per-pixel
//! alpha. Fully transparent pixels let clicks through to what is underneath.

use macrust_ui::WinRect;
use macrust_ui::tiny_skia::Pixmap;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WINDOW_EX_STYLE, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

use super::util::wide;

pub const CLASS: &str = "macRustSurface";

pub struct Surface {
    pub hwnd: HWND,
    pub pm: Pixmap,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    size: (i32, i32),
    pub shown: bool,
    tracking: bool,
    pub dirty: bool,
    pub rect: WinRect,
}

impl Surface {
    pub fn new(extra: WINDOW_EX_STYLE, title: &str) -> Self {
        let class = wide(CLASS);
        let title = wide(title);
        let hwnd = unsafe {
            CreateWindowExW(
                extra | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
                None,
            )
            .expect("create layered window")
        };
        let dc = unsafe { CreateCompatibleDC(None) };
        Self {
            hwnd,
            pm: Pixmap::new(1, 1).expect("1x1"),
            dc,
            bmp: HBITMAP::default(),
            old: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            size: (0, 0),
            shown: false,
            tracking: false,
            dirty: true,
            rect: WinRect::default(),
        }
    }

    /// Sizes the pixmap (and the DIB behind it) for the next frame.
    pub fn prepare(&mut self, r: WinRect) {
        self.rect = r;
        let (w, h) = (r.w.max(1), r.h.max(1));
        if self.size == (w, h) {
            return;
        }
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp.into());
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            self.bmp = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).expect("DIB section");
            self.old = SelectObject(self.dc, self.bmp.into());
            self.bits = bits as *mut u32;
        }
        self.size = (w, h);
        self.pm = Pixmap::new(w as u32, h as u32).expect("surface pixmap");
    }

    /// Copies the pixmap to the window and moves it to `self.rect`.
    pub fn present(&mut self) {
        let (w, h) = self.size;
        if self.bits.is_null() {
            return;
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(self.bits, (w * h) as usize) };
        for (d, s) in dst.iter_mut().zip(self.pm.data().chunks_exact(4)) {
            // Premultiplied RGBA -> premultiplied BGRA.
            *d = u32::from_le_bytes([s[2], s[1], s[0], s[3]]);
        }
        let pt = POINT { x: self.rect.x, y: self.rect.y };
        let size = SIZE { cx: w, cy: h };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pt),
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            if !self.shown {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.shown = true;
            }
        }
        self.dirty = false;
    }

    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }

    /// Asks for WM_MOUSELEAVE; returns true if the cursor just entered.
    pub fn track_leave(&mut self) -> bool {
        if self.tracking {
            return false;
        }
        let mut t = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        unsafe {
            let _ = TrackMouseEvent(&mut t);
        }
        self.tracking = true;
        true
    }

    pub fn left(&mut self) {
        self.tracking = false;
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bmp.into());
            }
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
