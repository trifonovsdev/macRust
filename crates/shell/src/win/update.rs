//! Self-update from GitHub releases.
//!
//! A background thread asks the GitHub API for releases (after 60 s, then every
//! 6 h), downloads `macrust.exe` of a newer one into
//! `%LOCALAPPDATA%\macRust\update` and checks it against the published SHA-256.
//! The swap itself (`swap`) runs from the logo menu or, with `auto_update`, at
//! the next start before any UI exists. Windows lets a running exe be renamed,
//! so the current file becomes `macrust.old.exe`, the new one takes its path,
//! and the new process is started once this one has left.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider,
    BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData, BCryptOpenAlgorithmProvider,
};
use windows::Win32::UI::Shell::FOLDERID_LocalAppData;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::PCWSTR;

use super::net;
use super::util::{hwnd_from_isize, hwnd_to_isize, known_folder, log};
use crate::update_logic::{self as logic, Release, Version, allowed_url};

const RELEASES_URL: &str = "https://api.github.com/repos/trifonovsdev/macRust/releases?per_page=10";
const USER_AGENT: &str = "macRust-updater";
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30 * 60);
const MAX_JSON: u64 = 2 << 20;
const MAX_EXE: u64 = 256 << 20;
const MAX_SHA: u64 = 4 << 10;
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    Downloading(String),
    UpToDate,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub version: Version,
    pub path: PathBuf,
}

struct Shared {
    status: Status,
    ready: Option<Ready>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { status: Status::Idle, ready: None });
static CHECK_NOW: OnceLock<mpsc::Sender<()>> = OnceLock::new();
/// Exe to start once this process has finished its normal shutdown.
static RELAUNCH: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version { core: [0, 0, 0], pre: None })
}

pub fn status() -> Status {
    SHARED.lock().map(|s| s.status.clone()).unwrap_or(Status::Idle)
}

/// Version string of a downloaded, not yet installed update.
pub fn ready_version() -> Option<String> {
    SHARED.lock().ok().and_then(|s| s.ready.as_ref().map(|r| r.version.to_string()))
}

fn set_status(s: Status) {
    if let Ok(mut g) = SHARED.lock() {
        g.status = s;
    }
}

fn update_dir() -> Option<PathBuf> {
    let dir = known_folder(&FOLDERID_LocalAppData)?.join("macRust").join("update");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

// ---------------------------------------------------------------------------
// Background thread

pub fn start(notify: HWND, msg: u32) {
    if let Some(r) = find_ready(&current_version()) {
        log(format!("update: {} was already downloaded", logic::update_file_name(&r.version)));
        if let Ok(mut g) = SHARED.lock() {
            g.ready = Some(r);
        }
    }
    let (tx, rx) = mpsc::channel();
    let _ = CHECK_NOW.set(tx);
    let h = hwnd_to_isize(notify);
    let spawned = std::thread::Builder::new().name("updater".into()).spawn(move || {
        // A panic ends update checks only; the shell carries on.
        if std::panic::catch_unwind(|| run(rx, h, msg)).is_err() {
            log("update: thread panicked");
        }
    });
    if let Err(e) = spawned {
        log(format!("update: cannot start thread: {e}"));
    }
}

/// Asks the updater thread for a check right now.
pub fn check_now() {
    if let Some(tx) = CHECK_NOW.get() {
        let _ = tx.send(());
    }
}

fn run(rx: mpsc::Receiver<()>, hwnd: isize, msg: u32) {
    let post = || unsafe {
        let _ = PostMessageW(Some(hwnd_from_isize(hwnd)), msg, Default::default(), Default::default());
    };
    let mut wait = FIRST_CHECK;
    loop {
        match rx.recv_timeout(wait) {
            Ok(()) => log("update: manual check"),
            Err(RecvTimeoutError::Timeout) => log("update: scheduled check"),
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while rx.try_recv().is_ok() {}
        set_status(Status::Checking);
        post();
        wait = match check_once(&post) {
            Ok(()) => CHECK_EVERY,
            Err(e) => {
                log(format!("update: check failed: {e}"));
                set_status(Status::Failed);
                RETRY_AFTER_FAILURE
            }
        };
        post();
    }
}

fn check_once(post: &dyn Fn()) -> Result<(), String> {
    let url = allowed_url(RELEASES_URL).ok_or("releases url not allowed")?;
    let mut body = Vec::new();
    http_get(&url, MAX_JSON, &mut |c| {
        body.extend_from_slice(c);
        Ok(())
    })?;
    let text = String::from_utf8(body).map_err(|_| "releases: invalid utf-8")?;
    let current = current_version();
    let Some(release) = logic::select_release(&text)? else {
        log("update: no usable release found");
        set_status(Status::UpToDate);
        return Ok(());
    };
    if release.version <= current {
        log(format!("update: latest is {}, running {current}", release.version));
        set_status(Status::UpToDate);
        return Ok(());
    }
    log(format!("update: {} is available, running {current}", release.version));
    if let Some(r) = find_ready(&current)
        && r.version >= release.version
    {
        log(format!("update: {} is already downloaded", logic::update_file_name(&r.version)));
        if let Ok(mut g) = SHARED.lock() {
            g.ready = Some(r);
        }
        set_status(Status::Idle);
        return Ok(());
    }
    set_status(Status::Downloading(release.version.to_string()));
    post();
    let ready = download(&release)?;
    log(format!("update: {} downloaded and verified", logic::update_file_name(&ready.version)));
    if let Ok(mut g) = SHARED.lock() {
        g.ready = Some(ready);
    }
    set_status(Status::Idle);
    Ok(())
}

/// Downloads the exe and its checksum; the final file name appears only after
/// the hash matched, and the `.sha256` sidecar only after that.
fn download(r: &Release) -> Result<Ready, String> {
    let dir = update_dir().ok_or("no update folder")?;
    let name = logic::update_file_name(&r.version);
    let mut sha_text = Vec::new();
    http_get(&r.sha, MAX_SHA, &mut |c| {
        sha_text.extend_from_slice(c);
        Ok(())
    })?;
    let expected = logic::parse_sha256_file(&String::from_utf8_lossy(&sha_text)).ok_or("checksum file is malformed")?;
    log("update: downloading macrust.exe");

    let part = dir.join(format!("{name}.part"));
    let result = (|| -> Result<(), String> {
        let mut f = std::fs::File::create(&part).map_err(|e| format!("create {name}.part: {e}"))?;
        http_get(&r.exe, MAX_EXE, &mut |c| f.write_all(c).map_err(|e| format!("write {name}.part: {e}")))?;
        f.sync_all().map_err(|e| format!("flush {name}.part: {e}"))?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let actual = sha256_file(&part);
    let header_ok = std::fs::File::open(&part).and_then(|mut f| {
        let mut mz = [0u8; 2];
        f.read_exact(&mut mz).map(|_| mz == *b"MZ")
    });
    match (actual, header_ok) {
        (Ok(a), Ok(true)) if a == expected => {}
        (Ok(a), _) if a != expected => {
            let _ = std::fs::remove_file(&part);
            return Err("checksum mismatch, download discarded".into());
        }
        _ => {
            let _ = std::fs::remove_file(&part);
            return Err("downloaded file is not a valid executable".into());
        }
    }
    let final_path = dir.join(&name);
    std::fs::rename(&part, &final_path).map_err(|e| format!("rename {name}.part: {e}"))?;
    std::fs::write(sidecar(&final_path), format!("{expected}  {}\n", logic::EXE_ASSET)).map_err(|e| format!("write {name}.sha256: {e}"))?;
    Ok(Ready { version: r.version.clone(), path: final_path })
}

fn sidecar(exe: &Path) -> PathBuf {
    let mut s = exe.as_os_str().to_os_string();
    s.push(".sha256");
    PathBuf::from(s)
}

/// The newest downloaded update that is newer than `current` and has its checksum file.
fn find_ready(current: &Version) -> Option<Ready> {
    let dir = update_dir()?;
    let mut best: Option<Ready> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(version) = logic::parse_update_file_name(&name) else { continue };
        let path = e.path();
        if version <= *current || !sidecar(&path).is_file() || best.as_ref().is_some_and(|b| b.version >= version) {
            continue;
        }
        best = Some(Ready { version, path });
    }
    best
}

/// Deletes everything in the update folder except a pending newer update.
fn tidy_update_dir() {
    let (Some(dir), keep) = (update_dir(), find_ready(&current_version())) else { return };
    let keep_name = keep.map(|r| logic::update_file_name(&r.version));
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let kept = keep_name.as_deref().is_some_and(|k| name == k || name == format!("{k}.sha256"));
        if !kept && e.path().is_file() && std::fs::remove_file(e.path()).is_ok() {
            log(format!("update: removed {name}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Swap and restart

fn remove_ready(r: &Ready) {
    let _ = std::fs::remove_file(&r.path);
    let _ = std::fs::remove_file(sidecar(&r.path));
}

/// Replaces the running exe by the downloaded one. On success the exe path to
/// start is returned; on failure the installed files are as they were.
fn swap(exe: &Path, ready: &Ready) -> Result<(), String> {
    let dir = exe.parent().ok_or("exe has no folder")?;
    let stem = exe.file_stem().and_then(|s| s.to_str()).ok_or("exe has no name")?;
    // Program Files and similar: renaming there needs rights we do not have.
    let probe = dir.join(format!("{stem}.write-test"));
    std::fs::write(&probe, b"").map_err(|e| format!("folder is not writable: {e}"))?;
    let _ = std::fs::remove_file(&probe);

    let expected = std::fs::read_to_string(sidecar(&ready.path))
        .ok()
        .and_then(|t| logic::parse_sha256_file(&t))
        .ok_or("checksum file is missing or malformed")?;
    // Same volume as the target, so the final step is an atomic rename.
    let staged = dir.join(format!("{stem}.new.exe"));
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(&ready.path, &staged).map_err(|e| format!("copy update: {e}"))?;
    let result = (|| {
        if sha256_file(&staged)? != expected {
            remove_ready(ready);
            return Err("update file does not match its checksum, discarded".to_string());
        }
        let old = free_old_path(dir, stem);
        std::fs::rename(exe, &old).map_err(|e| format!("rename current exe: {e}"))?;
        log("update: current exe renamed to the old name");
        if let Err(e) = std::fs::rename(&staged, exe) {
            let back = std::fs::rename(&old, exe);
            return Err(format!("move new exe into place: {e}; rollback {}", if back.is_ok() { "ok" } else { "FAILED" }));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// `<stem>.old.exe`, or a numbered name when a locked leftover blocks it.
fn free_old_path(dir: &Path, stem: &str) -> PathBuf {
    let plain = dir.join(format!("{stem}.old.exe"));
    if !plain.exists() || std::fs::remove_file(&plain).is_ok() {
        return plain;
    }
    dir.join(format!("{stem}.old.{}.exe", std::process::id()))
}

/// Menu action: swaps in the downloaded update and queues the restart. The
/// caller then quits through the normal path; `relaunch_if_pending` starts
/// the new exe afterwards.
pub fn install_ready() -> Result<(), String> {
    let ready = SHARED.lock().ok().and_then(|s| s.ready.clone()).ok_or("no update is ready")?;
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    log(format!("update: installing {}", logic::update_file_name(&ready.version)));
    swap(&exe, &ready)?;
    remove_ready(&ready);
    let from = current_version().to_string();
    if let Ok(mut g) = RELAUNCH.lock() {
        *g = Some((exe, from));
    }
    if let Ok(mut g) = SHARED.lock() {
        g.ready = None;
    }
    Ok(())
}

/// Start of the process, mutex held, nothing shown yet: installs a downloaded
/// update and starts it. `true` when this process should exit.
pub fn apply_at_startup() -> bool {
    let Some(ready) = find_ready(&current_version()) else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    log(format!("update: applying {} at start", logic::update_file_name(&ready.version)));
    if let Err(e) = swap(&exe, &ready) {
        log(format!("update: not applied: {e}"));
        return false;
    }
    remove_ready(&ready);
    spawn_new(&exe, &current_version().to_string())
}

/// Called after the shell shut down cleanly (taskbar restored).
pub fn relaunch_if_pending() {
    let pending = RELAUNCH.lock().ok().and_then(|mut g| g.take());
    if let Some((exe, from)) = pending {
        spawn_new(&exe, &from);
    }
}

fn spawn_new(exe: &Path, from: &str) -> bool {
    // The new process waits for the single-instance mutex, which this one
    // releases by exiting.
    match std::process::Command::new(exe).arg("--updated-from").arg(from).spawn() {
        Ok(_) => {
            log("update: new version started");
            true
        }
        Err(e) => {
            log(format!("update: cannot start the new version: {e}"));
            false
        }
    }
}

/// Removes leftovers of earlier updates. The old exe can stay locked for a
/// moment (the watchdog copy of it exits only after its parent), so retry.
pub fn cleanup_later() {
    let _ = std::thread::Builder::new().name("update-cleanup".into()).spawn(|| {
        for delay in [0u64, 5, 30, 300] {
            std::thread::sleep(Duration::from_secs(delay));
            if clean_old_files() {
                break;
            }
        }
        tidy_update_dir();
    });
}

/// `true` when nothing is left.
fn clean_old_files() -> bool {
    let Ok(exe) = std::env::current_exe() else { return true };
    let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem().and_then(|s| s.to_str())) else { return true };
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    let mut left = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let stale = name == format!("{stem}.new.exe")
            || name == format!("{stem}.old.exe")
            || name
                .strip_prefix(&format!("{stem}.old."))
                .and_then(|r| r.strip_suffix(".exe"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !stale || e.path() == exe {
            continue;
        }
        match std::fs::remove_file(e.path()) {
            Ok(()) => log(format!("update: removed {name}")),
            Err(err) => {
                log(format!("update: cannot remove {name} yet: {err}"));
                left = true;
            }
        }
    }
    !left
}

// ---------------------------------------------------------------------------
// SHA-256 through CNG

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("open for hashing: {e}"))?;
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, PCWSTR::null(), BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0))
            .ok()
            .map_err(|e| format!("sha256 provider: {e}"))?;
        let mut hash = BCRYPT_HASH_HANDLE::default();
        let r = (|| {
            BCryptCreateHash(alg, &mut hash, None, None, 0).ok().map_err(|e| format!("sha256 create: {e}"))?;
            let mut buf = vec![0u8; 64 * 1024];
            let step = (|| loop {
                let n = f.read(&mut buf).map_err(|e| format!("read for hashing: {e}"))?;
                if n == 0 {
                    return Ok(());
                }
                BCryptHashData(hash, &buf[..n], 0).ok().map_err(|e| format!("sha256 update: {e}"))?;
            })();
            let mut out = [0u8; 32];
            let fin = step.and_then(|()| BCryptFinishHash(hash, &mut out, 0).ok().map_err(|e| format!("sha256 finish: {e}")));
            let _ = BCryptDestroyHash(hash);
            fin.map(|()| logic::hex(&out))
        })();
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        r
    }
}

// ---------------------------------------------------------------------------
// HTTPS

const FETCH: net::Fetch = net::Fetch {
    hosts: &logic::ALLOWED_HOSTS,
    agent: USER_AGENT,
    headers: "Accept: application/vnd.github+json, application/octet-stream\r\nX-GitHub-Api-Version: 2022-11-28",
    deadline: DOWNLOAD_DEADLINE,
    tag: "update",
};

fn http_get(url: &logic::Url, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    net::get(&FETCH, url, max, sink)
}
