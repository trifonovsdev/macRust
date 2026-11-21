//! Icons from the shell (IShellItemImageFactory), as premultiplied RGBA.

use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{Interface, PCWSTR};

use super::util::{hwnd_from_isize, hwnd_to_isize, wide};

#[derive(Clone)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub fn shell_item(parse_name: &str) -> Option<IShellItem> {
    let w = wide(parse_name);
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok() }
}

/// Loads the icon of a path, `shell:AppsFolder\…` id or other shell parsing name.
pub fn load(parse_name: &str, size: i32) -> Option<Rgba> {
    unsafe {
        let item = shell_item(parse_name)?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let hbmp = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let out = read_bitmap(hbmp);
        let _ = DeleteObject(hbmp.into());
        out
    }
}

unsafe fn read_bitmap(hbmp: HBITMAP) -> Option<Rgba> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bmi = BITMAPINFO {
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
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }
        // BGRA -> RGBA. The shell hands out either premultiplied or straight
        // alpha (and sometimes no alpha at all); tell them apart by the data.
        let mut any_alpha = false;
        let mut over = 0usize;
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
            if p[3] != 0 {
                any_alpha = true;
            }
            if p[0] > p[3] || p[1] > p[3] || p[2] > p[3] {
                over += 1;
            }
        }
        // A few stray pixels do not make a premultiplied image straight.
        let straight = over * 100 > (w * h) as usize;
        for p in px.chunks_exact_mut(4) {
            if !any_alpha {
                p[3] = 255;
            } else if straight {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            } else {
                let a = p[3];
                for c in &mut p[..3] {
                    *c = (*c).min(a);
                }
            }
        }
        Some(Rgba { w: w as u32, h: h as u32, data: px })
    }
}

pub struct Request {
    pub id: u64,
    pub parse_name: String,
    pub size: i32,
}

/// Loads icons on a background thread and wakes the UI thread with `notify_msg`.
pub struct IconWorker {
    tx: Sender<Request>,
    pub rx: Receiver<(u64, Option<Rgba>)>,
}

impl IconWorker {
    pub fn spawn(notify: HWND, notify_msg: u32) -> Self {
        let (tx, req_rx) = channel::<Request>();
        let (res_tx, rx) = channel();
        let target = hwnd_to_isize(notify);
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            while let Ok(r) = req_rx.recv() {
                let icon = load(&r.parse_name, r.size);
                if res_tx.send((r.id, icon)).is_err() {
                    break;
                }
                unsafe {
                    let _ = PostMessageW(Some(hwnd_from_isize(target)), notify_msg, WPARAM(0), LPARAM(0));
                }
            }
        });
        Self { tx, rx }
    }

    pub fn request(&self, r: Request) {
        let _ = self.tx.send(r);
    }
}
