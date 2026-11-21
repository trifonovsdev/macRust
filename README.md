<p align="center"><img src="docs/logo.png" alt="macRust" width="128"></p>

<h1 align="center">macRust</h1>

<p align="center">
  A macOS-style shell for Windows, written in Rust: a menu bar with tray icons, a Liquid Glass Dock, Control Center, Notification Center and search.<br>
  One small executable. No .NET, no Electron, no WebView.
</p>

<p align="center">
  <a href="https://github.com/trifonovsdev/macRust/actions/workflows/ci.yml"><img src="https://github.com/trifonovsdev/macRust/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4" alt="Windows 10 | 11">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License">
</p>

<p align="center">English | <a href="README.ru.md">Русский</a></p>

![Desktop](docs/desktop-light.png)

> [!WARNING]
> **Early preview.** It has not yet been tested widely on real hardware, so try it in a VM first.
> macRust hides the Windows taskbar while it runs and brings it back when it exits.
> The executable is not code-signed, so SmartScreen will warn you.

## Features

### Menu bar
- **Logo menu:** About This PC, System Settings, Microsoft Store and Force Quit (Task Manager). Also Sleep, Restart and Shut Down, each with a confirmation, plus Lock and Log Out.
- **Active app:** its name in bold, with a menu: Hide, Hide Others, Show All and Quit.
- **Window menu:** Minimize, Zoom, move to the left or right half of the screen (aligned to the visible frame, not the invisible resize border), Center and Bring All to Front.
- **Tray icons:** macRust hosts the notification area, so OBS's recording icon, Telegram's and Discord's unread badges and every other tray icon show up in the menu bar with their live state. Left and right clicks reach the app as they would on the taskbar.
- **Status items:** keyboard layout (click to switch), battery, network, search, Control Center and a clock. Clicking the clock opens Notification Center.
- **Privacy indicator:** an orange dot while an app records the microphone, green while one uses the camera.
- **No background:** as in macOS 26, the wallpaper shows through. Text turns dark or light depending on how bright the wallpaper is.
- **Menu switching:** while a menu is open, slide the cursor across the titles to switch menus, as on a Mac.

### Dock
![Dock](docs/dock-magnification.png)

- **Liquid Glass:** the Dock is a clear slab that bends the wallpaper along its rim and catches a specular highlight, as in macOS 26.
- **macOS icons:** for popular apps (Telegram, WhatsApp, Slack, Zoom, Teams, Word, Excel, PowerPoint, Outlook, OneNote, Firefox, Brave, OBS, VLC, qBittorrent) macRust downloads the app's own macOS icon on first use, from the Mac App Store or the app's source repository, and keeps it in `%LOCALAPPDATA%\macRust\icons`. Nothing is bundled with macRust.
- **Icon shapes:** other Windows icons that fill a square are cut to the macOS squircle; round logos and bare glyphs stand free, without a plate, sized to the macOS icon grid.

  <img src="docs/icon-shapes.png" alt="Icon shapes" width="280">
- **What it shows:** pinned apps and every running app, plus Downloads and the Recycle Bin. Apps are grouped like the taskbar groups them, by AppUserModelID or by executable, so Store apps work too. An app that closes to the tray (Telegram, Discord, OBS) keeps its icon and dot while it runs.
- **Magnification:** the point under the cursor stays put and the Dock grows on both sides of it.
- **Feedback:** the icon bounces while an app launches, a dot marks running apps, a label appears on hover and the icon darkens while pressed. Icons slide in and out when apps start and quit.
- **Click:** launches the app or brings its windows to the front. A click never hides an app: if all its windows are minimized, one comes back. A tray-only app is asked to show its window.
- **Right-click:** the app's windows, Keep in Dock or Remove from Dock, Show in Explorer, New Window and Quit. The Recycle Bin also has Empty.

### Control Center

<img src="docs/control-center.png" alt="Control Center" width="420">

- Wi-Fi and Bluetooth switches with the network name; click a name for its settings page.
- Now Playing for any app that reports media to Windows (Spotify, browsers, players): cover, title, artist, previous, play/pause and next.
- Dark Mode switches Windows itself between light and dark. Screenshot opens Snip & Sketch.
- Display brightness (external monitors over DDC/CI) and sound volume sliders; click the speaker to mute.

### Notification Center

<img src="docs/notification-center.png" alt="Notification Center" width="420">

- Notifications from every app (Telegram, Discord, mail…), read from the Windows notification history, with app icons and sender pictures. Click one to open the app, hover for the close button, or clear them all.
- A calendar widget with today's date.
- New notifications slide in as banners at the top right and hide after five seconds unless the pointer rests on them. Windows still shows its own toasts as well; turn on Do Not Disturb in Windows to see only macRust's banners (notifications are still saved to the history).

### Search (Alt+Space)
![Search](docs/spotlight-dark.png)

- **What it searches:** every app in `shell:AppsFolder` (Win32 and Store), Settings pages, and files and folders in Desktop, Documents, Downloads, Pictures, Music and Videos. Files are indexed in the background: up to 300k entries, refreshed every 30 minutes.
- **Fuzzy matching:** `vsc` finds Visual Studio Code, and Cyrillic matches without regard to case.
- **Calculator:** `2+2*2`, `(1,5+0,5)^3`. Press Enter to copy the result.
- **Web search:** the last row searches the web for the query.
- **Keys:** Enter opens the result and Ctrl+Enter shows it in Explorer. Esc clears the field, and a second Esc closes the panel. Ctrl+V, Ctrl+A, Ctrl+←/→ and Ctrl+Backspace work as usual.

> The screenshots come from the same rendering code (`cargo run -p macrust-ui --example preview`). The Dock icons in them are placeholders; on Windows the Dock shows real app icons.

## Install

1. Download `macrust.exe` from the [latest release](https://github.com/trifonovsdev/macRust/releases).
2. Run it. To start it at sign-in, run `macrust.exe --install` once.
3. To quit, open the logo menu in the menu bar and choose **Quit macRust**.

| Flag | What it does |
|---|---|
| `--install` | start at sign-in (HKCU Run key) and start now |
| `--uninstall` | remove from sign-in and bring the taskbar back |
| `--restore` | only bring the taskbar back |
| `--wallpaper` | set the bundled gradient as the desktop picture |

**The taskbar cannot get stuck hidden.** Its previous state is saved to
`%APPDATA%\macRust\taskbar-state.txt`. A tiny watchdog process
(`macrust.exe --watch <pid>`) restores it if the shell crashes or is ended from
Task Manager, and the next start restores it first too. To restore it by hand,
press Win+R and run `macrust.exe --restore`.

## Settings

The settings live in `%APPDATA%\macRust`. Restart macRust after editing them.

- `settings.txt`: `icon_size`, `magnified_size`, `magnification`, `hide_taskbar`, `reserve_dock_space`, `appearance` (`auto` | `light` | `dark`), `tray_icons`, `banners`
- `dock.txt`: pinned apps, one per line as `Name|exe|C:\path\to.exe` or `Name|aumid|AppUserModelID`. Pinning from the Dock's context menu is easier.
- `log.txt`: errors, because the shell has no console.
- `icons\`: your own Dock icons as PNG, named after the program file in lower case (`chrome.png`, `telegram.png`) or, for Store apps, after the package family name. They are used as they are and win over every other icon.

## Build

```powershell
# on Windows
cargo build --release -p macrust
```

```sh
# cross-compile from Linux (needs mingw-w64)
rustup target add x86_64-pc-windows-gnu
cargo build --release -p macrust --target x86_64-pc-windows-gnu
```

You can look at the UI on any OS:

```sh
cargo run --release -p macrust-ui --example preview -- out
cargo test -p macrust-ui
```

## How it works

```
crates/ui      rendering, animation and state of every component; no OS code
crates/shell   Windows host: layered windows, input, window tracking, shell icons, system
```

- **Windows:** each component is a layered window (`UpdateLayeredWindow`) with per-pixel alpha. Fully transparent pixels let clicks through, so the Dock's window can be wider than the Dock.
- **Rendering:** `tiny-skia` draws on the CPU with SIMD. Text is Inter, rasterized with `ab_glyph`, with a glyph cache and quarter-pixel horizontal positioning. Corners are continuous-curvature squircles rather than circular arcs.
- **Motion:** every animation is a spring, as in SwiftUI. An interrupted animation keeps its velocity instead of jumping.
- **Idle cost:** frames are drawn only while something moves, paced to the display with `DwmFlush`. At rest the process sleeps in `GetMessage`; the only timers are the clock and a one-second status tick.
- **Glass:** the menu bar and the Dock reserve their strips of the work area, so the desktop is always behind them. The Dock refracts a lightly blurred copy of the wallpaper: a rounded-rectangle distance field gives the rim's bend and highlight, only the rim is computed per pixel, and the flat middle is copied from a cache.
- **Tray:** a hidden window of the class `Shell_TrayWnd` sits above Explorer's, so `Shell_NotifyIcon` calls land in macRust first; each one is also forwarded to Explorer, which stays consistent if macRust exits.
- **Notifications and media:** notifications are read from the per-user Windows notification database; Now Playing, volume, radios and brightness come from WinRT and Core Audio on a worker thread.
- **Shadows and icons:** shadows are pre-blurred nine-slice textures, and each icon's shadow is baked into the icon. Icons keep mip levels, so a 256 px icon drawn at 52 px stays sharp.
- **Window tracking:** WinEvent hooks report window changes instead of polling, and bursts of events collapse into one refresh.

Frame cost at 2x scale on a small cloud VM (a desktop CPU is faster):

| | ms per frame |
|---|---|
| Dock while magnifying (2236×304 px) | 6.2 |
| Search panel opening | 3.6 |
| Search results (8 rows) | 5.8 |
| Menu bar | 0.2 |

## Roadmap

- A Finder-style file browser
- Dock auto-hide, dragging icons, minimized windows in the Dock (via DWM thumbnails)
- Mission Control, Launchpad
- App menus in the menu bar (Win32 apps expose them through `HMENU`)
- Multiple monitors (the menu bar and the Dock are on the primary one for now)
- CJK fallback fonts for file names

## License

MIT or Apache-2.0, at your option: see [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
The bundled Inter typeface is licensed under the SIL Open Font License; see
[crates/ui/fonts/Inter-LICENSE.txt](crates/ui/fonts/Inter-LICENSE.txt).

Inspired by [WinRust](https://github.com/Lemxy/WinRust). Not affiliated with Apple; macOS is a trademark of Apple Inc.
