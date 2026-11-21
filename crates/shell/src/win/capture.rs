//! What is on screen under our glass, live: the primary output through the
//! DXGI Desktop Duplication API. Our own windows opt out of it
//! (`exclude_from_capture`), so the glass sees the windows and wallpaper
//! behind it and never itself. Frames stay in memory and are never saved.

use macrust_ui::WinRect;
use macrust_ui::genie;
use macrust_ui::tiny_skia::Pixmap;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::core::{BOOL, Interface, s, w};

use super::util::log;

pub struct Capture {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: Option<IDXGIOutputDuplication>,
    /// The two newest desktop frames, kept on the GPU, with GetTickCount at
    /// the poll that took each: a window that has just vanished can still be
    /// read from a frame taken before it went (genie.rs).
    frames: [Option<(ID3D11Texture2D, u32)>; 2],
    /// Index of the newest frame.
    cur: usize,
    /// Top-left of the output in virtual-screen coordinates.
    origin: (i32, i32),
    /// Staging textures by size: the glass surfaces and window snapshots
    /// alternate, and creating one per read is not free.
    staging: Vec<(u32, u32, ID3D11Texture2D)>,
    failures: u32,
}

impl Capture {
    pub fn new() -> Option<Capture> {
        let mut device = None;
        let mut ctx = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
        }
        .map_err(|e| log(format!("capture: no D3D11 device: {e}")))
        .ok()?;
        let mut c = Capture { device: device?, ctx: ctx?, dup: None, frames: [None, None], cur: 0, origin: (0, 0), staging: Vec::new(), failures: 0 };
        c.duplicate();
        c.dup.is_some().then_some(c)
    }

    fn duplicate(&mut self) {
        self.dup = None;
        let r: windows::core::Result<(IDXGIOutputDuplication, (i32, i32))> = (|| unsafe {
            let adapter = self.device.cast::<IDXGIDevice>()?.GetAdapter()?;
            let output = adapter.EnumOutputs(0)?;
            let desc = output.GetDesc()?;
            let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&self.device)?;
            Ok((dup, (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top)))
        })();
        match r {
            Ok((dup, origin)) => {
                self.dup = Some(dup);
                self.origin = origin;
                self.failures = 0;
            }
            Err(e) => {
                // Lost on desktop switches (UAC, lock screen) and mode changes;
                // retried on later polls, logged only the first times.
                self.failures += 1;
                if self.failures <= 3 {
                    log(format!("capture: duplicate output: {e}"));
                }
            }
        }
    }

    /// Takes the newest frame if the screen changed; true if it did.
    pub fn poll(&mut self) -> bool {
        let Some(dup) = self.dup.clone() else {
            // Back off: a duplication attempt is not free.
            self.failures += 1;
            if self.failures.is_multiple_of(30) {
                self.duplicate();
            }
            return false;
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(0, &mut info, &mut res) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return false,
            Err(e) => {
                log(format!("capture: frame: {e}"));
                self.dup = None;
                self.frames = [None, None];
                return false;
            }
        }
        // Only the pointer moved: the image is unchanged.
        let changed = info.LastPresentTime != 0;
        if changed && let Some(tex) = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
            self.keep(&tex);
        }
        unsafe {
            let _ = dup.ReleaseFrame();
        }
        changed
    }

    fn keep(&mut self, tex: &ID3D11Texture2D) {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        // The older slot takes the new frame; the current one becomes the older.
        let slot = 1 - self.cur;
        let fits = self.frames[slot].as_ref().is_some_and(|(f, _)| {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { f.GetDesc(&mut d) };
            d.Width == desc.Width && d.Height == desc.Height && d.Format == desc.Format
        });
        if !fits {
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = 0;
            desc.MiscFlags = 0;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            let mut t = None;
            if unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t)) }.is_err() {
                self.frames[slot] = None;
                return;
            }
            self.frames[slot] = t.map(|t| (t, 0));
        }
        if let Some((f, tick)) = &mut self.frames[slot] {
            unsafe { self.ctx.CopyResource(&*f, tex) };
            *tick = unsafe { GetTickCount() };
            self.cur = slot;
        }
    }

    /// The last frame's pixels under `r` (virtual-screen coordinates), opaque
    /// premultiplied RGBA. Parts outside the output stay black.
    pub fn region(&mut self, r: WinRect) -> Option<Pixmap> {
        let frame = self.frames[self.cur].as_ref()?.0.clone();
        let px = self.read(&frame, r, false)?;
        Pixmap::from_vec(px, macrust_ui::tiny_skia::IntSize::from_wh(r.w as u32, r.h as u32)?)
    }

    /// Like `region`, but opaque BGRA, from the newest frame taken strictly
    /// before GetTickCount `before` when given (a frame from the same tick
    /// may already miss a window that vanished in it).
    pub fn region_bgra(&mut self, r: WinRect, before: Option<u32>) -> Option<Vec<u8>> {
        let older = |t: u32| before.is_none_or(|b| (b.wrapping_sub(t) as i32) > 0);
        let pick = [self.cur, 1 - self.cur].into_iter().find_map(|i| self.frames[i].as_ref().filter(|f| older(f.1)));
        let frame = pick?.0.clone();
        self.read(&frame, r, true)
    }

    /// `genie::grid_samples` of the newest frame under `r` (BGR order),
    /// without reading the whole region back: what is on screen under our
    /// own windows, which the capture leaves out.
    pub fn sample(&mut self, r: WinRect, n: usize) -> Option<Vec<[u8; 3]>> {
        let frame = self.frames[self.cur].as_ref()?.0.clone();
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut d) };
        let (x0, y0) = (r.x - self.origin.0, r.y - self.origin.1);
        // Only a region inside the output: samples of black borders would
        // never match the window.
        if d.Format != DXGI_FORMAT_B8G8R8A8_UNORM
            || r.w < 8
            || r.h < 8
            || x0 < 0
            || y0 < 0
            || x0 + r.w > d.Width as i32
            || y0 + r.h > d.Height as i32
        {
            return None;
        }
        let (w, h) = (r.w as usize, r.h as usize);
        let staging = self.staging_for(r.w as u32, r.h as u32)?;
        let bx = D3D11_BOX {
            left: x0 as u32,
            top: y0 as u32,
            front: 0,
            right: (x0 + r.w) as u32,
            bottom: (y0 + r.h) as u32,
            back: 1,
        };
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &frame, 0, Some(&bx));
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
        }
        let pitch = m.RowPitch as usize;
        let src = unsafe { std::slice::from_raw_parts(m.pData as *const u8, pitch * h) };
        let mut out = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let o = genie::grid_at(j, h, n) * pitch + genie::grid_at(i, w, n) * 4;
                out.push([src[o], src[o + 1], src[o + 2]]);
            }
        }
        unsafe { self.ctx.Unmap(&staging, 0) };
        Some(out)
    }

    fn read(&mut self, frame: &ID3D11Texture2D, r: WinRect, bgra: bool) -> Option<Vec<u8>> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut d) };
        let (fw, fh) = (d.Width as i32, d.Height as i32);
        if d.Format != DXGI_FORMAT_B8G8R8A8_UNORM || r.w <= 0 || r.h <= 0 {
            return None;
        }
        let (w, h) = (r.w as u32, r.h as u32);
        let (x0, y0) = (r.x - self.origin.0, r.y - self.origin.1);
        let (l, t) = (x0.clamp(0, fw), y0.clamp(0, fh));
        let (rr, b) = ((x0 + r.w).clamp(0, fw), (y0 + r.h).clamp(0, fh));
        let mut out = vec![0u8; (w * h * 4) as usize];
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
        if rr <= l || b <= t {
            return Some(out);
        }
        let staging = self.staging_for(w, h)?;
        let bx = D3D11_BOX { left: l as u32, top: t as u32, front: 0, right: rr as u32, bottom: b as u32, back: 1 };
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx.CopySubresourceRegion(&staging, 0, (l - x0) as u32, (t - y0) as u32, 0, frame, 0, Some(&bx));
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
        }
        let pitch = m.RowPitch as usize;
        let src = unsafe { std::slice::from_raw_parts(m.pData as *const u8, pitch * h as usize) };
        let (c0, c1) = ((l - x0) as usize, (rr - x0) as usize);
        for y in (t - y0) as usize..(b - y0) as usize {
            let row = &src[y * pitch + c0 * 4..y * pitch + c1 * 4];
            let dst = &mut out[(y * w as usize + c0) * 4..(y * w as usize + c1) * 4];
            dst.copy_from_slice(row);
            for p in dst.chunks_exact_mut(4) {
                // Alpha is undefined in desktop frames.
                p[3] = 255;
                if !bgra {
                    p.swap(0, 2);
                }
            }
        }
        unsafe { self.ctx.Unmap(&staging, 0) };
        Some(out)
    }

    fn staging_for(&mut self, w: u32, h: u32) -> Option<ID3D11Texture2D> {
        if let Some(i) = self.staging.iter().position(|(sw, sh, _)| (*sw, *sh) == (w, h)) {
            let e = self.staging.remove(i);
            let t = e.2.clone();
            self.staging.insert(0, e);
            return Some(t);
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut t = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t)) }.ok()?;
        let t = t?;
        self.staging.insert(0, (w, h, t.clone()));
        // A few sizes in use at once (Dock, a panel, a window); the rest go.
        self.staging.truncate(4);
        Some(t)
    }
}

/// Keeps a window out of Desktop Duplication only (WCA_EXCLUDED_FROM_DDA),
/// so screenshots and screen recordings still show it.
pub fn exclude_from_capture(hwnd: HWND) {
    #[repr(C)]
    struct Data {
        attrib: u32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    type SetAttr = unsafe extern "system" fn(HWND, *mut Data) -> BOOL;
    const WCA_EXCLUDED_FROM_DDA: u32 = 24;
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return };
        let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return };
        // The export's signature is fixed since Windows 7 (documented in
        // the DWM WINDOWCOMPOSITIONATTRIB reference).
        let set: SetAttr = std::mem::transmute(f);
        let mut on = BOOL(1);
        let mut d = Data { attrib: WCA_EXCLUDED_FROM_DDA, data: &mut on as *mut BOOL as *mut _, size: size_of::<BOOL>() };
        let _ = set(hwnd, &mut d);
    }
}
