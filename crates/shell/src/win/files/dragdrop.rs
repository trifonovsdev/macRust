//! Dropping files into a file manager window (IDropTarget): copy or move
//! into the folder under the pointer, with the shell's drag images.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};

const MK_ALT: u32 = 0x20;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDropTargetHelper};
use windows::core::{Ref, Result, implement};

use super::super::util::{hwnd_from_isize, hwnd_to_isize};
use super::{shellops, trace, window};

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: isize,
    helper: Option<IDropTargetHelper>,
    sources: RefCell<Vec<String>>,
    target: RefCell<Option<String>>,
}

impl DropTarget {
    pub fn create(hwnd: HWND) -> IDropTarget {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        DropTarget { hwnd: hwnd_to_isize(hwnd), helper, sources: RefCell::new(Vec::new()), target: RefCell::new(None) }.into()
    }

    fn hwnd(&self) -> HWND {
        hwnd_from_isize(self.hwnd)
    }

    fn root(p: &str) -> String {
        let p = p.to_lowercase();
        if let Some(rest) = p.strip_prefix("\\\\") {
            let mut parts = rest.split('\\');
            return format!("\\\\{}\\{}", parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        }
        p.chars().take(2).collect()
    }

    /// The effect Explorer would pick: Ctrl copies, Shift moves, otherwise
    /// a move on the same volume and a copy across volumes.
    fn effect(&self, keys: MODIFIERKEYS_FLAGS, allowed: DROPEFFECT) -> DROPEFFECT {
        let target = self.target.borrow();
        let sources = self.sources.borrow();
        let Some(target) = target.as_ref() else { return DROPEFFECT_NONE };
        if sources.is_empty() {
            return DROPEFFECT_NONE;
        }
        let parent = |p: &str| p.trim_end_matches('\\').rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default();
        let into_self = sources.iter().any(|s| macrust_ui::files::model::same_path(s, target) || target.to_lowercase().starts_with(&format!("{}\\", s.to_lowercase())));
        if into_self {
            return DROPEFFECT_NONE;
        }
        let same_folder = sources.iter().all(|s| macrust_ui::files::model::same_path(&parent(s), target));
        let want = if keys.0 & MK_CONTROL.0 != 0 || keys.0 & MK_ALT != 0 {
            DROPEFFECT_COPY
        } else if keys.0 & MK_SHIFT.0 != 0 || sources.iter().all(|s| Self::root(s) == Self::root(target)) {
            DROPEFFECT_MOVE
        } else {
            DROPEFFECT_COPY
        };
        if want == DROPEFFECT_MOVE && same_folder {
            return DROPEFFECT_NONE;
        }
        if allowed.0 & want.0 != 0 {
            want
        } else if allowed.0 & DROPEFFECT_COPY.0 != 0 && !same_folder {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        }
    }

    fn client(&self, pt: &POINTL) -> (f32, f32) {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd(), &mut p);
        }
        (p.x as f32, p.y as f32)
    }

    fn over(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, allowed: DROPEFFECT) -> DROPEFFECT {
        let (x, y) = self.client(pt);
        *self.target.borrow_mut() = window::drag_over(self.hwnd(), x, y);
        self.effect(keys, allowed)
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let data = data.ok()?;
        *self.sources.borrow_mut() = shellops::dropped_paths(data);
        let e = self.over(keys, pt, unsafe { *effect });
        unsafe { *effect = e };
        if let Some(h) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = h.DragEnter(self.hwnd(), data, &p, e);
            }
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let e = self.over(keys, pt, unsafe { *effect });
        unsafe { *effect = e };
        if let Some(h) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = h.DragOver(&p, e);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        window::drag_leave(self.hwnd());
        self.sources.borrow_mut().clear();
        if let Some(h) = &self.helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let data = data.ok()?;
        if self.sources.borrow().is_empty() {
            *self.sources.borrow_mut() = shellops::dropped_paths(data);
        }
        let e = self.over(keys, pt, unsafe { *effect });
        if let Some(h) = &self.helper {
            let p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = h.Drop(data, &p, e);
            }
        }
        window::drag_leave(self.hwnd());
        let target = self.target.borrow().clone();
        let sources = std::mem::take(&mut *self.sources.borrow_mut());
        match (target, e) {
            (Some(t), e) if e != DROPEFFECT_NONE => {
                let mv = e == DROPEFFECT_MOVE;
                trace(format!("drop: {} items, {}", sources.len(), if mv { "move" } else { "copy" }));
                window::transfer(self.hwnd(), sources, t, mv);
                // We move the files ourselves (an optimized move): the source
                // must not delete anything, so report no effect for moves.
                unsafe { *effect = if mv { DROPEFFECT_NONE } else { DROPEFFECT_COPY } };
            }
            _ => unsafe { *effect = DROPEFFECT_NONE },
        }
        Ok(())
    }
}
