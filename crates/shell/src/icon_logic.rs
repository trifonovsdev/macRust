//! Platform-neutral parts of the dock's macOS icons: the catalog of apps with
//! an official macOS icon source, matching a running program to it, override
//! file names, and reading what the sources return (iTunes lookup JSON, ICNS,
//! PNG headers). No I/O; the Windows side lives in `win/macicons.rs`.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::net_logic::{Url, allowed_url};
use crate::update_logic::Json;

/// Hosts the icon fetcher may talk to, redirects included.
pub const ICON_HOSTS: [&str; 7] = [
    "itunes.apple.com",
    "is1-ssl.mzstatic.com",
    "is2-ssl.mzstatic.com",
    "is3-ssl.mzstatic.com",
    "is4-ssl.mzstatic.com",
    "is5-ssl.mzstatic.com",
    "raw.githubusercontent.com",
];
pub const MAX_ICON_BYTES: u64 = 4 << 20;
pub const MAX_LOOKUP_BYTES: u64 = 256 << 10;
pub const MIN_ICON_PX: u32 = 128;
/// Larger pictures are refused before decoding (a small PNG can inflate to gigabytes).
pub const MAX_ICON_PX: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Mac App Store id: the iTunes lookup API names the current artwork.
    AppStore(u64),
    /// A PNG file at a pinned commit of the app's own repository.
    Png(&'static str),
    /// An ICNS file; its largest PNG-encoded entry is used.
    Icns(&'static str),
}

#[derive(Debug)]
pub struct App {
    /// Cache and override file name.
    pub key: &'static str,
    /// Lower-case exe file names without `.exe`.
    pub exes: &'static [&'static str],
    /// AppUserModelID prefixes, compared without case.
    pub aumids: &'static [&'static str],
    pub origin: Origin,
}

/// Every source was checked to return the app's current macOS icon (1024 px
/// unless noted). Artwork is fetched on the user's PC, never shipped.
pub const CATALOG: &[App] = &[
    App {
        key: "telegram",
        exes: &["telegram"],
        aumids: &["Telegram.TelegramDesktop", "TelegramMessengerLLP.TelegramDesktop_"],
        origin: Origin::AppStore(747_648_890),
    },
    App { key: "whatsapp", exes: &["whatsapp", "whatsapp.root"], aumids: &["5319275A.WhatsAppDesktop_"], origin: Origin::AppStore(310_633_997) },
    App { key: "slack", exes: &["slack"], aumids: &["com.squirrel.slack.slack", "91750D7E.Slack_"], origin: Origin::AppStore(803_453_959) },
    App { key: "zoom", exes: &["zoom"], aumids: &["zoom.us"], origin: Origin::AppStore(546_505_307) },
    App { key: "teams", exes: &["ms-teams", "teams"], aumids: &["MSTeams_", "com.squirrel.Teams.Teams"], origin: Origin::AppStore(1_113_153_706) },
    App { key: "word", exes: &["winword"], aumids: &["Microsoft.Office.WINWORD.EXE"], origin: Origin::AppStore(462_054_704) },
    App { key: "excel", exes: &["excel"], aumids: &["Microsoft.Office.EXCEL.EXE"], origin: Origin::AppStore(462_058_435) },
    App { key: "powerpoint", exes: &["powerpnt"], aumids: &["Microsoft.Office.POWERPNT.EXE"], origin: Origin::AppStore(462_062_816) },
    App {
        key: "outlook",
        exes: &["outlook", "olk"],
        aumids: &["Microsoft.Office.OUTLOOK.EXE", "Microsoft.OutlookForWindows_"],
        origin: Origin::AppStore(985_367_838),
    },
    App {
        key: "onenote",
        exes: &["onenote", "onenoteim"],
        aumids: &["Microsoft.Office.ONENOTE.EXE", "Microsoft.Office.OneNote_"],
        origin: Origin::AppStore(784_801_555),
    },
    App {
        key: "firefox",
        exes: &["firefox"],
        // 308046B0AF4A39CB is the id of a default per-machine install.
        aumids: &["308046B0AF4A39CB", "Mozilla.Firefox_"],
        origin: Origin::Icns(concat!(
            "https://raw.githubusercontent.com",
            "/mozilla-firefox/firefox/b8cbe7118bdf7af47a632105e49472221b9efb32/browser/branding/official/firefox.icns"
        )),
    },
    App {
        key: "brave",
        exes: &["brave"],
        aumids: &["Brave"],
        origin: Origin::Icns(concat!(
            "https://raw.githubusercontent.com",
            "/brave/brave-core/c3c2240ce521c009276c66ccc625b7115c2124b3/app/theme/brave/mac/app.icns"
        )),
    },
    App {
        key: "obs",
        exes: &["obs64", "obs32", "obs"],
        aumids: &[],
        origin: Origin::Png(concat!(
            "https://raw.githubusercontent.com",
            "/obsproject/obs-studio/3047d4a4fd5cc32767338a6dd218ada532f51043",
            "/frontend/cmake/macos/Assets.xcassets/AppIcon.appiconset/icon_512x512@2x.png"
        )),
    },
    App {
        key: "vlc",
        exes: &["vlc"],
        aumids: &["VideoLAN.VLC"],
        // The macOS 26 icon; the repository has it at 256 px only.
        origin: Origin::Icns(concat!(
            "https://raw.githubusercontent.com",
            "/videolan/vlc/5c415c4ef816951c54ae09777469d3a2efd0aa46/modules/gui/macosx/Resources/App-Icons/VLC26.icns"
        )),
    },
    App {
        key: "qbittorrent",
        exes: &["qbittorrent"],
        aumids: &[],
        origin: Origin::Icns(concat!(
            "https://raw.githubusercontent.com",
            "/qbittorrent/qBittorrent/9213075dd5f7b352208361c6a84c1c115efc898e/dist/mac/qbittorrent_mac.icns"
        )),
    },
];

// ---------------------------------------------------------------------------
// Matching

/// Lower-case file name of a program path without its extension.
pub fn exe_stem(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next()?.trim();
    let stem = match name.rsplit_once('.') {
        Some((s, ext)) if ext.eq_ignore_ascii_case("exe") => s,
        _ => name,
    };
    (!stem.is_empty()).then(|| stem.to_lowercase())
}

pub fn find(stem: Option<&str>, aumid: Option<&str>) -> Option<&'static App> {
    let aumid = aumid.map(str::to_lowercase);
    CATALOG.iter().find(|a| {
        stem.is_some_and(|s| a.exes.contains(&s))
            || aumid.as_deref().is_some_and(|id| a.aumids.iter().any(|p| id.starts_with(&p.to_lowercase())))
    })
}

/// `Family_publisherid` of a packaged app's `Family_publisherid!App` id.
pub fn package_family(aumid: &str) -> Option<&str> {
    let (family, app) = aumid.split_once('!')?;
    (family.contains('_') && !app.is_empty()).then_some(family)
}

/// A file-name-safe, lower-case form of an id.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .take(120)
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// Names (without `.png`) looked up in the user's override folder, best first.
pub fn override_names(stem: Option<&str>, aumid: Option<&str>, app: Option<&App>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        let s = s.trim_matches('.').to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    if let Some(s) = stem {
        push(sanitize(s));
    }
    if let Some(id) = aumid {
        if let Some(f) = package_family(id) {
            push(sanitize(f));
        }
        push(sanitize(id));
    }
    if let Some(a) = app {
        push(a.key.to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// Sources

pub fn lookup_url(id: u64) -> Option<Url> {
    allowed_url(&format!("https://itunes.apple.com/lookup?id={id}&country=us"), &ICON_HOSTS)
}

/// The 1024 px PNG of an iTunes lookup result's artwork. The artwork server
/// renders any size and format named in the last path segment.
pub fn artwork_url(json: &str) -> Option<Url> {
    let root = Json::parse(json).ok()?;
    let Some(Json::Arr(results)) = root.get("results") else { return None };
    let art = results.first()?.get("artworkUrl512")?.as_str()?;
    let (dir, last) = art.rsplit_once('/')?;
    if !last.contains("x") || !last.contains("bb.") {
        return None;
    }
    allowed_url(&format!("{dir}/1024x1024bb.png"), &ICON_HOSTS)
}

pub fn source_url(o: Origin) -> Option<Url> {
    match o {
        Origin::AppStore(id) => lookup_url(id),
        Origin::Png(u) | Origin::Icns(u) => allowed_url(u, &ICON_HOSTS),
    }
}

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Width and height from a PNG's header.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != PNG_SIG || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((w, h))
}

/// A PNG an app icon may come from: square within 10%, `MIN_ICON_PX`..=`MAX_ICON_PX`.
pub fn icon_png_ok(b: &[u8]) -> bool {
    png_size(b).is_some_and(|(w, h)| {
        let (lo, hi) = (w.min(h), w.max(h));
        lo >= MIN_ICON_PX && hi <= MAX_ICON_PX && (hi - lo) * 10 <= hi
    })
}

/// The largest PNG-encoded entry of an ICNS file (ic10, ic09, ic14, ...).
/// Older JPEG 2000 and RLE entries are skipped.
pub fn icns_largest_png(b: &[u8]) -> Option<&[u8]> {
    if b.len() < 8 || &b[..4] != b"icns" {
        return None;
    }
    let total = (u32::from_be_bytes(b[4..8].try_into().ok()?) as usize).min(b.len());
    let mut i = 8;
    let mut best: Option<(u32, &[u8])> = None;
    while i + 8 <= total {
        let len = u32::from_be_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        if len < 8 || i + len > total {
            break;
        }
        let data = &b[i + 8..i + len];
        if let Some((w, _)) = png_size(data)
            && best.is_none_or(|(bw, _)| w > bw)
        {
            best = Some((w, data));
        }
        i += len;
    }
    best.map(|(_, d)| d)
}

/// The icon PNG inside a downloaded body, if it is acceptable.
pub fn icon_from_body(o: Origin, body: &[u8]) -> Option<&[u8]> {
    let png = match o {
        Origin::Icns(_) => icns_largest_png(body)?,
        _ => body,
    };
    icon_png_ok(png).then_some(png)
}

/// Opaque corners mark full-bleed square art (iPad apps on the App Store),
/// which needs the squircle cut; macOS grid icons have transparent corners.
pub fn opaque_corners(w: u32, h: u32, rgba: &[u8]) -> bool {
    if w < 4 || h < 4 || rgba.len() < (w * h * 4) as usize {
        return false;
    }
    let at = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
    let (r, b) = (w - 2, h - 2);
    [at(1, 1), at(r, 1), at(1, b), at(r, b)].iter().all(|&a| a > 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_exe_or_aumid() {
        assert_eq!(find(Some("telegram"), None).unwrap().key, "telegram");
        assert_eq!(find(None, Some("telegram.telegramdesktop")).unwrap().key, "telegram");
        assert_eq!(find(None, Some("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App")).unwrap().key, "whatsapp");
        assert_eq!(find(None, Some("Microsoft.Office.WINWORD.EXE.15")).unwrap().key, "word");
        assert_eq!(find(Some("olk"), None).unwrap().key, "outlook");
        assert_eq!(find(Some("notepad"), Some("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App")).map(|a| a.key), None);
        assert_eq!(find(Some("telegram-x"), None).map(|a| a.key), None);
    }

    #[test]
    fn exe_stems() {
        assert_eq!(exe_stem(r"C:\Users\x\AppData\Roaming\Telegram Desktop\Telegram.exe").as_deref(), Some("telegram"));
        assert_eq!(exe_stem("C:/a/obs64.EXE").as_deref(), Some("obs64"));
        assert_eq!(exe_stem(r"C:\a\WhatsApp.Root.exe").as_deref(), Some("whatsapp.root"));
        assert_eq!(exe_stem(r"C:\a\"), None);
    }

    #[test]
    fn override_file_names() {
        let id = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App";
        let app = find(None, Some(id));
        assert_eq!(
            override_names(None, Some(id), app),
            ["5319275a.whatsappdesktop_cv1g1gvanyjgm", "5319275a.whatsappdesktop_cv1g1gvanyjgm_app", "whatsapp"]
        );
        assert_eq!(override_names(Some("chrome"), Some("Chrome"), None), ["chrome"]);
        assert_eq!(sanitize(r"..\..\evil/x y"), ".._.._evil_x_y");
        assert_eq!(override_names(None, Some(r"..\.."), None), ["_"]);
    }

    #[test]
    fn catalog_is_well_formed() {
        for a in CATALOG {
            assert!(source_url(a.origin).is_some(), "{}", a.key);
            assert_eq!(sanitize(a.key), a.key);
            assert!(a.exes.iter().all(|e| *e == e.to_lowercase()), "{}", a.key);
            if let Origin::Png(u) | Origin::Icns(u) = a.origin {
                // Pinned to a commit, so the picture cannot change under us.
                let path = u.strip_prefix("https://raw.githubusercontent.com").unwrap();
                let sha = path.split('/').nth(3).unwrap();
                assert!(sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()), "{}", a.key);
            }
        }
        let mut keys: Vec<_> = CATALOG.iter().map(|a| a.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), CATALOG.len());
    }

    #[test]
    fn artwork_from_lookup() {
        let j = r#"{"resultCount":1,"results":[{"trackId":747648890,"artworkUrl512":"https://is1-ssl.mzstatic.com/image/thumb/Purple221/v4/dd/AppIcon-0-0-85-220-0-0-6-0-2x-sRGB.png/512x512bb.png"}]}"#;
        assert_eq!(
            artwork_url(j).unwrap().as_string(),
            "https://is1-ssl.mzstatic.com/image/thumb/Purple221/v4/dd/AppIcon-0-0-85-220-0-0-6-0-2x-sRGB.png/1024x1024bb.png"
        );
        let jpg = r#"{"results":[{"artworkUrl512":"https://is3-ssl.mzstatic.com/image/thumb/a/b.png/512x512bb.jpg"}]}"#;
        assert!(artwork_url(jpg).unwrap().path.ends_with("/b.png/1024x1024bb.png"));
        assert!(artwork_url(r#"{"results":[]}"#).is_none());
        assert!(artwork_url(r#"{"results":[{"artworkUrl512":"https://evil.com/x/512x512bb.png"}]}"#).is_none());
        assert!(artwork_url("not json").is_none());
        assert_eq!(lookup_url(1).unwrap().as_string(), "https://itunes.apple.com/lookup?id=1&country=us");
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = PNG_SIG.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    fn entry(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = kind.to_vec();
        v.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    fn icns(entries: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = entries.concat();
        let mut v = b"icns".to_vec();
        v.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
        v.extend(body);
        v
    }

    #[test]
    fn png_checks() {
        assert_eq!(png_size(&png(512, 500)), Some((512, 500)));
        assert!(icon_png_ok(&png(1024, 1024)));
        assert!(icon_png_ok(&png(256, 240)));
        assert!(!icon_png_ok(&png(100, 100)));
        assert!(!icon_png_ok(&png(1024, 600)));
        assert!(!icon_png_ok(&png(60_000, 60_000)));
        assert!(!icon_png_ok(b"GIF89a................................"));
    }

    #[test]
    fn icns_takes_the_largest_png() {
        let jp2 = b"\0\0\0\x0cjP  \r\n\x87\n".to_vec();
        let file = icns(&[
            entry(b"TOC ", &[0; 16]),
            entry(b"ic07", &png(128, 128)),
            entry(b"ic10", &jp2),
            entry(b"ic09", &png(512, 512)),
            entry(b"ic13", &png(256, 256)),
            entry(b"is32", &[1, 2, 3]),
        ]);
        assert_eq!(png_size(icns_largest_png(&file).unwrap()), Some((512, 512)));
        assert_eq!(icon_from_body(Origin::Icns(""), &file).map(png_size), Some(Some((512, 512))));
        // Only legacy entries, a truncated entry, or not an ICNS at all.
        assert!(icns_largest_png(&icns(&[entry(b"ic10", &jp2)])).is_none());
        let mut cut = icns(&[entry(b"ic09", &png(512, 512))]);
        cut.truncate(20);
        assert!(icns_largest_png(&cut).is_none());
        assert!(icns_largest_png(&png(512, 512)).is_none());
        let mut bad_len = icns(&[entry(b"ic09", &png(512, 512))]);
        bad_len[12..16].copy_from_slice(&3u32.to_be_bytes());
        assert!(icns_largest_png(&bad_len).is_none());
    }

    #[test]
    fn corners() {
        let w = 8;
        let mut px = vec![255u8; (w * w * 4) as usize];
        assert!(opaque_corners(w, w, &px));
        px[3] = 0;
        px[((w + 1) * 4 + 3) as usize] = 0;
        assert!(!opaque_corners(w, w, &px));
        assert!(!opaque_corners(w, w, &px[..10]));
    }
}
