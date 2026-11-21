//! HTTPS GET through WinHTTP for the updater and the icon fetcher. Each caller
//! brings its own host allowlist; redirects are followed by hand so every hop
//! is checked against it.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::PCWSTR;

use super::util::{log, wide};
use crate::net_logic::{Url, resolve_redirect};

const MAX_REDIRECTS: usize = 5;

pub struct Fetch<'a> {
    pub hosts: &'a [&'a str],
    pub agent: &'a str,
    /// Extra request headers, `\r\n`-separated; may be empty.
    pub headers: &'a str,
    pub deadline: Duration,
    /// Prefix of log lines.
    pub tag: &'a str,
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn handle(p: *mut c_void, what: &str) -> Result<Handle, String> {
    if p.is_null() { Err(format!("{what}: {}", windows::core::Error::from_thread())) } else { Ok(Handle(p)) }
}

/// GET with manual redirects (each hop must stay on `f.hosts`), a size cap
/// and timeouts. The body goes to `sink` in chunks.
pub fn get(f: &Fetch, url: &Url, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    let started = Instant::now();
    let mut url = url.clone();
    if !f.hosts.contains(&url.host.as_str()) {
        return Err("host is not allowed".into());
    }
    let agent = wide(f.agent);
    unsafe {
        let mut session = WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            // Automatic proxy detection needs Windows 8.1.
            session = WinHttpOpen(PCWSTR(agent.as_ptr()), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        }
        let session = handle(session, "winhttp open")?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000).map_err(|e| format!("timeouts: {e}"))?;

        for _ in 0..=MAX_REDIRECTS {
            let host = wide(&url.host);
            let path = wide(&url.path);
            let conn = handle(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0), "connect")?;
            let req = handle(
                WinHttpOpenRequest(
                    conn.0,
                    windows::core::w!("GET"),
                    PCWSTR(path.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_FLAG_SECURE,
                ),
                "open request",
            )?;
            WinHttpSetOption(Some(req.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&WINHTTP_DISABLE_REDIRECTS.to_ne_bytes()))
                .map_err(|e| format!("disable redirects: {e}"))?;
            let headers: Vec<u16> = f.headers.encode_utf16().collect();
            let headers = (!headers.is_empty()).then_some(headers.as_slice());
            WinHttpSendRequest(req.0, headers, None, 0, 0, 0).map_err(|e| format!("send: {e}"))?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("receive: {e}"))?;

            let mut status = 0u32;
            let mut len = size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                req.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut c_void),
                &mut len,
                std::ptr::null_mut(),
            )
            .map_err(|e| format!("status: {e}"))?;

            match status {
                200 => {
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut total = 0u64;
                    loop {
                        if started.elapsed() > f.deadline {
                            return Err("download took too long".into());
                        }
                        let mut read = 0u32;
                        WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                            .map_err(|e| format!("read: {e}"))?;
                        if read == 0 {
                            return Ok(());
                        }
                        total += read as u64;
                        if total > max {
                            return Err("response is larger than allowed".into());
                        }
                        sink(&buf[..read as usize])?;
                    }
                }
                301 | 302 | 303 | 307 | 308 => {
                    let mut loc = vec![0u16; 4096];
                    let mut bytes = (loc.len() * 2) as u32;
                    WinHttpQueryHeaders(
                        req.0,
                        WINHTTP_QUERY_LOCATION,
                        PCWSTR::null(),
                        Some(loc.as_mut_ptr() as *mut c_void),
                        &mut bytes,
                        std::ptr::null_mut(),
                    )
                    .map_err(|e| format!("redirect without usable location: {e}"))?;
                    let loc = String::from_utf16_lossy(&loc[..(bytes as usize / 2).min(loc.len())]);
                    let next = resolve_redirect(&url, &loc, f.hosts).ok_or("redirect to a host that is not allowed")?;
                    log(format!("{}: redirect {status} to {}", f.tag, next.host));
                    url = next;
                }
                s => return Err(format!("http {s}")),
            }
        }
    }
    Err("too many redirects".into())
}

/// The whole body of a GET, at most `max` bytes.
pub fn get_bytes(f: &Fetch, url: &Url, max: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    get(f, url, max, &mut |c| {
        body.extend_from_slice(c);
        Ok(())
    })?;
    Ok(body)
}
