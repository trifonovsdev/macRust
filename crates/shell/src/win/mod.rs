//! Windows host for the macRust shell.

mod app;
mod capture;
mod catalog;
mod config;
mod files;
mod genie;
mod icons;
mod macicons;
mod media;
mod minbutton;
mod net;
mod notifications;
mod surface;
mod system;
mod tracker;
mod tray;
mod update;
mod util;
mod wifi;
mod winops;
mod workarea;

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONWARNING, MB_OK, MessageBoxW,
    DefWindowProcW, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW, MSG, PM_REMOVE, PeekMessageW,
    PostMessageW, RegisterClassExW, TranslateMessage, WM_APP, WM_HOTKEY, WM_QUIT, WM_TIMER, WNDCLASSEXW,
};
use windows::core::{PCWSTR, w};

use app::App;
use util::{log, wide};

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Win32 calls made while handling a message can send us another one
    // synchronously; the app is busy then and the nested message gets the default.
    let handled = APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().and_then(|app| app.handle(hwnd, msg, wp, lp)),
        Err(_) => {
            // Our own posted messages (window refresh, icons, catalog, app-bar
            // notifications) and timers must not be lost: post them again so
            // they arrive once the current handler has returned.
            if (msg >= WM_APP || msg == WM_TIMER || msg == WM_HOTKEY) && unsafe { PostMessageW(Some(hwnd), msg, wp, lp) }.is_ok() {
                return Some(LRESULT(0));
            }
            None
        }
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn register_class() {
    let class = wide(surface::CLASS);
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    // The file manager is its own process; it must not touch the taskbar
    // or anything else the shell owns.
    if let Some(i) = args.iter().position(|a| a == "--files") {
        files::run(&args[i + 1..]);
        return;
    }

    // Leaving the taskbar hidden after a crash would strand the user, so any
    // panic puts it back before the process aborts.
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        log(format!("panic in {thread}: {info}"));
        // Background threads catch their panics and carry on without the
        // feature; only the UI thread takes the shell down.
        if thread == "main" {
            system::restore_taskbar();
            genie::release_min_animate();
            alert(&format!(
                "macRust stopped because of an error:\n\n{info}\n\nDetails: {}",
                util::data_dir().join("log.txt").display()
            ));
        }
    }));

    if let Some(pid) = args.iter().position(|a| a == "--watch").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()) {
        system::watch(pid);
        workarea::release();
        genie::release_min_animate();
        return;
    }
    if has("--restore") {
        workarea::release();
        system::restore_taskbar();
        genie::release_min_animate();
        return;
    }
    if has("--install") {
        system::set_autostart(true);
    }
    if has("--uninstall") {
        system::set_autostart(false);
        system::restore_taskbar();
        return;
    }

    // A process started by the updater races the one it replaces for the
    // single-instance mutex, which the old one gives up by exiting.
    let updated_from = args.iter().position(|a| a == "--updated-from").and_then(|i| args.get(i + 1)).cloned();
    unsafe {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let handle = CreateMutexW(None, true, w!("Local\\macRust.shell"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                // The handle stays open for the life of the process.
                break;
            }
            if let Ok(h) = handle {
                let _ = CloseHandle(h);
            }
            if updated_from.is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            log("another instance is running");
            alert("macRust is already running.");
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    if let Some(from) = &updated_from {
        log(format!("updated from {from} to {}", env!("CARGO_PKG_VERSION")));
        // The old process's watchdog restores the taskbar when it sees that
        // process end; let it finish before this one hides the taskbar again.
        std::thread::sleep(Duration::from_millis(1500));
    } else if config::load_settings().auto_update && !has("--no-update") && update::apply_at_startup() {
        return;
    }
    update::cleanup_later();

    if has("--wallpaper") {
        let s = system::primary_screen();
        let pm = macrust_ui::wallpaper::procedural(s.w as u32, s.h as u32, system::system_dark());
        if !system::set_wallpaper(&pm) {
            log("could not set the wallpaper");
        }
    }

    // A previous run that was killed may have left the taskbar hidden; put it
    // back first, whatever the current settings say.
    if system::taskbar_state_saved() {
        system::restore_taskbar();
    }
    // The same for Windows' minimize animation, which the Genie turns off.
    genie::release_min_animate();
    log(format!("starting {}", env!("CARGO_PKG_VERSION")));
    register_class();
    let app = App::new();
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    log("started");

    let mut msg = MSG::default();
    let mut fast_flushes = 0u32;
    loop {
        let animating = with_app(|a| a.animating()).unwrap_or(false);
        if animating {
            // Animation: drain input, draw a frame, then wait for the next
            // composition (vsync) so frames line up with the display.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return finish();
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            with_app(|a| a.frame());
            let t = Instant::now();
            if unsafe { DwmFlush() }.is_err() || t.elapsed() < Duration::from_micros(1500) {
                // DwmFlush returned without waiting (no composition pending):
                // pace by hand so a spinning loop does not burn a core.
                fast_flushes += 1;
                if fast_flushes > 2 {
                    std::thread::sleep(Duration::from_millis(7));
                }
            } else {
                fast_flushes = 0;
            }
        } else {
            // Idle: block until something happens. No timers run while idle
            // except the clock and status ticks.
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 <= 0 {
                return finish();
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if with_app(|a| a.has_work()).unwrap_or(false) {
                with_app(|a| a.frame());
            }
        }
    }
}

fn alert(text: &str) {
    let t = wide(text);
    unsafe {
        MessageBoxW(None, PCWSTR(t.as_ptr()), w!("macRust"), MB_OK | MB_ICONWARNING);
    }
}

fn finish() {
    with_app(|a| a.shutdown());
    log("stopped");
    update::relaunch_if_pending();
}
