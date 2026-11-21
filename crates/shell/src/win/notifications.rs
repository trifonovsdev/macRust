//! Windows notification history reader.
//!
//! Windows keeps toasts in a per-user SQLite database that an unpackaged exe can read
//! (UserNotificationListener needs package identity). The database is owned by the
//! notification service, so it is opened read-only and, if that fails, from a copy.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::tracker;
use super::util::{hwnd_from_isize, hwnd_to_isize, log};

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: i64,
    pub app_id: String,
    pub title: String,
    pub body: String,
    pub image: Option<PathBuf>,
    pub arrival: SystemTime,
}

pub enum Update {
    /// The recent history on start (newest first, at most 50, last 3 days).
    Initial(Vec<Toast>),
    /// Notifications that arrived since the last poll (oldest first).
    New(Vec<Toast>),
    /// Ids no longer in the database (the user dismissed them in Windows).
    Removed(Vec<i64>),
}

pub struct Watcher {
    pub rx: Receiver<Update>,
}

const POLL: Duration = Duration::from_millis(1500);
const RETRY: Duration = Duration::from_secs(30);
const INITIAL_LIMIT: usize = 50;
const INITIAL_DAYS: u64 = 3;
const TRACKED_IDS: usize = 200;

impl Watcher {
    /// Starts a background thread; posts `notify_msg` to `notify` after sending updates.
    pub fn start(notify: HWND, notify_msg: u32) -> Watcher {
        let (tx, rx) = mpsc::channel();
        let hwnd = hwnd_to_isize(notify);
        let spawned = std::thread::Builder::new().name("notifications".into()).spawn(move || {
            // A panic here must not take the shell down; the feature just stops.
            let r = std::panic::catch_unwind(|| run(tx, hwnd, notify_msg));
            if r.is_err() {
                log("notifications: watcher thread panicked");
            }
        });
        if let Err(e) = spawned {
            log(format!("notifications: cannot start thread: {e}"));
        }
        Watcher { rx }
    }
}

static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Human app name for an AppUserModelID or exe path.
pub fn app_name(app_id: &str) -> String {
    let Ok(mut guard) = NAMES.lock() else { return app_id.to_string() };
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(n) = cache.get(app_id) {
        return n.clone();
    }
    let is_path = app_id.contains('\\') || app_id.contains('/') || app_id.to_ascii_lowercase().ends_with(".exe");
    let name = if is_path {
        tracker::app_name(&mut HashMap::new(), app_id, None)
    } else {
        tracker::display_name(&format!("shell:AppsFolder\\{app_id}")).unwrap_or_default()
    };
    let name = if name.trim().is_empty() { app_id.to_string() } else { name };
    cache.insert(app_id.to_string(), name.clone());
    name
}

fn db_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join(r"Microsoft\Windows\Notifications\wpndatabase.db"))
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// (len, mtime) of the database and its WAL; the WAL changes first.
fn stamp(db: &Path) -> Vec<Option<(u64, SystemTime)>> {
    [db.to_path_buf(), with_suffix(db, "-wal")]
        .iter()
        .map(|p| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().unwrap_or(UNIX_EPOCH))))
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Direct,
    Copy,
}

fn open(db: &Path, mode: Mode) -> Result<Connection, String> {
    let conn = match mode {
        Mode::Direct => {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
            Connection::open_with_flags(db, flags)
        }
        Mode::Copy => {
            let dir = std::env::temp_dir().join("macRust");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let dst = dir.join("wpndatabase.db");
            std::fs::copy(db, &dst).map_err(|e| e.to_string())?;
            let wal_dst = with_suffix(&dst, "-wal");
            match std::fs::copy(with_suffix(db, "-wal"), &wal_dst) {
                Ok(_) => {}
                Err(_) => {
                    // No WAL (or unreadable): a stale copy of it must not be paired with a fresh db.
                    let _ = std::fs::remove_file(&wal_dst);
                }
            }
            // Read-write so SQLite can create the -shm for the copied WAL.
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            Connection::open_with_flags(&dst, flags)
        }
    }
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_millis(200)).map_err(|e| e.to_string())?;
    Ok(conn)
}

fn run(tx: Sender<Update>, hwnd: isize, msg: u32) {
    let Some(db) = db_path() else {
        log("notifications: LOCALAPPDATA is not set");
        return;
    };
    let send = |u: Update| -> bool {
        if tx.send(u).is_err() {
            return false;
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd_from_isize(hwnd)), msg, WPARAM(0), LPARAM(0));
        }
        true
    };

    let mut mode = Mode::Direct;
    let mut logged = false;
    let mut last_max: Option<i64> = None;
    let mut known: Vec<i64> = Vec::new();
    let mut last_stamp = Vec::new();

    loop {
        let st = stamp(&db);
        if last_max.is_none() || st != last_stamp {
            match poll(&db, &mut mode, &mut last_max, &mut known) {
                Ok(updates) => {
                    logged = false;
                    last_stamp = st;
                    for u in updates {
                        if !send(u) {
                            return;
                        }
                    }
                }
                Err(e) => {
                    if !logged {
                        log(format!("notifications: {e}"));
                        logged = true;
                    }
                    std::thread::sleep(RETRY);
                    continue;
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn poll(db: &Path, mode: &mut Mode, last_max: &mut Option<i64>, known: &mut Vec<i64>) -> Result<Vec<Update>, String> {
    let conn = match open(db, *mode) {
        Ok(c) => c,
        Err(e) if *mode == Mode::Direct => {
            // Read-only WAL access needs the -shm, which the service may deny us.
            log(format!("notifications: direct open failed ({e}), reading a copy"));
            *mode = Mode::Copy;
            open(db, *mode)?
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    match *last_max {
        None => {
            let cutoff = SystemTime::now()
                .checked_sub(Duration::from_secs(INITIAL_DAYS * 86_400))
                .unwrap_or(UNIX_EPOCH);
            let mut toasts = query(&conn, "n.Type = 'toast'", None, INITIAL_LIMIT * 4)?;
            toasts.retain(|t| t.arrival >= cutoff);
            toasts.sort_by_key(|t| std::cmp::Reverse(t.id));
            toasts.truncate(INITIAL_LIMIT);
            *last_max = Some(max_id(&conn)?);
            *known = toasts.iter().map(|t| t.id).collect();
            out.push(Update::Initial(toasts));
        }
        Some(prev) => {
            let mut fresh = query(&conn, "n.Type = 'toast' AND n.Id > ?1", Some(prev), usize::MAX)?;
            fresh.sort_by_key(|t| t.id);
            if let Some(m) = max_id(&conn).ok().filter(|m| *m > prev) {
                *last_max = Some(m);
            }
            if !known.is_empty() {
                let present = present_ids(&conn, known.iter().copied().min().unwrap_or(0))?;
                let gone: Vec<i64> = known.iter().copied().filter(|i| !present.contains(i)).collect();
                if !gone.is_empty() {
                    known.retain(|i| present.contains(i));
                    out.push(Update::Removed(gone));
                }
            }
            if !fresh.is_empty() {
                known.extend(fresh.iter().map(|t| t.id));
                if known.len() > TRACKED_IDS {
                    known.sort_unstable();
                    let cut = known.len() - TRACKED_IDS;
                    known.drain(..cut);
                }
                out.push(Update::New(fresh));
            }
        }
    }
    Ok(out)
}

fn max_id(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COALESCE(MAX(Id), 0) FROM Notification", [], |r| r.get(0)).map_err(|e| e.to_string())
}

fn present_ids(conn: &Connection, min_id: i64) -> Result<HashSet<i64>, String> {
    let mut st = conn.prepare("SELECT Id FROM Notification WHERE Id >= ?1").map_err(|e| e.to_string())?;
    let rows = st.query_map([min_id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// `filter` may reference `?1` (bound to `after`) only when `after` is Some.
fn query(conn: &Connection, filter: &str, after: Option<i64>, limit: usize) -> Result<Vec<Toast>, String> {
    let sql = format!(
        "SELECT n.Id, h.PrimaryId, n.Payload, n.ArrivalTime \
         FROM Notification n LEFT JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
         WHERE {filter} ORDER BY n.Id DESC LIMIT {}",
        limit.min(i64::MAX as usize)
    );
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let map = |r: &rusqlite::Row| -> rusqlite::Result<Option<Toast>> {
        let id: i64 = r.get(0)?;
        let app_id: String = r.get::<_, Option<String>>(1)?.unwrap_or_default();
        let payload: Vec<u8> = match r.get_ref(2)? {
            rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => b.to_vec(),
            _ => Vec::new(),
        };
        let ft: i64 = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
        Ok(toast_from(id, app_id, &payload, ft))
    };
    let rows = match after {
        Some(a) => st.query_map([a], map),
        None => st.query_map([], map),
    }
    .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok().flatten()).collect())
}

fn toast_from(id: i64, app_id: String, payload: &[u8], filetime: i64) -> Option<Toast> {
    if app_id.trim().is_empty() {
        return None;
    }
    let xml = String::from_utf8_lossy(payload);
    let (mut texts, image) = parse_toast(xml.trim_start_matches('\u{feff}'));
    if texts.is_empty() {
        return None;
    }
    let title = texts.remove(0);
    Some(Toast { id, app_id, title, body: texts.join("\n"), image, arrival: filetime_to_system(filetime) })
}

/// FILETIME (100 ns since 1601) to SystemTime; values before the Unix epoch clamp to it.
fn filetime_to_system(ft: i64) -> SystemTime {
    let secs = (ft.max(0) / 10_000_000) - 11_644_473_600;
    if secs <= 0 { UNIX_EPOCH } else { UNIX_EPOCH + Duration::from_secs(secs as u64) }
}

/// Visible texts (attribution lines skipped) and the preferred image path.
fn parse_toast(xml: &str) -> (Vec<String>, Option<PathBuf>) {
    let mut texts = Vec::new();
    let mut logo: Option<PathBuf> = None;
    let mut first_img: Option<PathBuf> = None;
    let mut pos = 0;
    while let Some(rel) = xml[pos..].find('<') {
        let start = pos + rel;
        let rest = &xml[start..];
        if rest.starts_with("<!--") {
            pos = rest.find("-->").map_or(xml.len(), |e| start + e + 3);
            continue;
        }
        let Some(end) = tag_end(rest) else { break };
        let tag = &rest[..=end];
        pos = start + end + 1;
        if is_tag(tag, "text") {
            if tag.ends_with("/>") {
                continue;
            }
            let close = xml[pos..].find("</text>").map(|c| pos + c).unwrap_or(xml.len());
            let content = flatten(&xml[pos..close]);
            pos = (close + "</text>".len()).min(xml.len());
            let placement = attr(tag, "placement");
            if placement.as_deref() == Some("attribution") {
                continue;
            }
            let content = content.trim().to_string();
            if !content.is_empty() {
                texts.push(content);
            }
        } else if is_tag(tag, "image") {
            let placement = attr(tag, "placement");
            if placement.as_deref() == Some("hero") {
                continue;
            }
            let path = attr(tag, "src").and_then(|s| src_to_path(&s));
            if placement.as_deref() == Some("appLogoOverride") {
                logo = logo.or(path);
            } else {
                first_img = first_img.or(path);
            }
        }
    }
    (texts, logo.or(first_img))
}

/// Index of the closing `>` of the tag at the start of `s`, skipping quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, b) in s.bytes().enumerate() {
        match (quote, b) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn is_tag(tag: &str, name: &str) -> bool {
    tag.strip_prefix('<')
        .and_then(|t| t.strip_prefix(name))
        .is_some_and(|t| t.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/'))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = tag[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = tag[..at].ends_with(|c: char| c.is_whitespace());
        let after = tag[from..].trim_start();
        let Some(after) = after.strip_prefix('=') else { continue };
        if !before_ok {
            continue;
        }
        let after = after.trim_start();
        let q = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let val = &after[1..];
        let close = val.find(q)?;
        return Some(decode_entities(&val[..close]));
    }
    None
}

/// Text content with CDATA unwrapped, nested tags dropped and entities decoded.
fn flatten(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..i]));
        rest = &rest[i..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            match body.find("]]>") {
                Some(e) => {
                    out.push_str(&body[..e]);
                    rest = &body[e + 3..];
                }
                None => {
                    out.push_str(body);
                    return out;
                }
            }
        } else {
            match tag_end(rest) {
                Some(e) => rest = &rest[e + 1..],
                None => return out,
            }
        }
    }
    out.push_str(&decode_entities(rest));
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|e| *e <= 10).and_then(|e| {
            let c = match &rest[1..e] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                n => {
                    let num = n.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Accepts `file:///C:/x.png` and plain drive paths; remote or relative sources are ignored.
fn src_to_path(src: &str) -> Option<PathBuf> {
    let raw = match src.get(..8).filter(|p| p.eq_ignore_ascii_case("file:///")) {
        Some(_) => percent_decode(&src[8..]),
        None => src.to_string(),
    };
    let b = raw.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\');
    let unc = raw.starts_with("\\\\");
    (drive || unc).then(|| PathBuf::from(raw.replace('/', "\\")))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%').then(|| b.get(i + 1..i + 3)).flatten();
        if let Some(v) = hex.and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_texts_and_logo() {
        let xml = r#"<toast launch="a&amp;b"><visual><binding template="ToastGeneric">
            <image placement="appLogoOverride" hint-crop="circle" src="file:///C:/Users/a%20b/x.png"/>
            <text>Ann &amp; Bob</text><text>line &lt;1&gt; &#33; &#x41;</text><text><![CDATA[c <2>]]></text>
            <text placement="attribution">via X</text></binding></visual></toast>"#;
        let (t, img) = parse_toast(xml);
        assert_eq!(t, vec!["Ann & Bob", "line <1> ! A", "c <2>"]);
        assert_eq!(img, Some(PathBuf::from(r"C:\Users\a b\x.png")));
    }

    #[test]
    fn empty_toast_is_skipped() {
        assert!(toast_from(1, "app".into(), b"<toast><visual/></toast>", 0).is_none());
        assert!(toast_from(1, String::new(), b"<toast><text>x</text></toast>", 0).is_none());
    }

    #[test]
    fn filetime() {
        assert_eq!(filetime_to_system(116_444_736_000_000_000 + 10_000_000), UNIX_EPOCH + Duration::from_secs(1));
        assert_eq!(filetime_to_system(0), UNIX_EPOCH);
    }
}
