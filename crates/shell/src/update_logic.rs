//! Platform-neutral parts of the updater: version order, release selection,
//! checksum files and the download URL allowlist. No I/O, so it runs in tests
//! on any host; the Windows side lives in `win/update.rs`.
#![cfg_attr(not(windows), allow(dead_code))]

use std::cmp::Ordering;

pub const EXE_ASSET: &str = "macrust.exe";
pub const SHA_ASSET: &str = "macrust.exe.sha256";

/// Hosts a download may come from, including the hosts GitHub redirects to.
pub const ALLOWED_HOSTS: [&str; 4] =
    ["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

// ---------------------------------------------------------------------------
// Versions

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub core: [u64; 3],
    /// Pre-release suffix after `-` (`rc1`); sorts below the plain release.
    pub pre: Option<String>,
}

impl Version {
    /// `v0.3.0`, `0.3.0`, `0.3`, `1.2.3-rc1`. Build metadata (`+x`) is ignored.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let s = s.split('+').next().unwrap_or(s);
        let (nums, pre) = match s.split_once('-') {
            Some((n, p)) if !p.is_empty() => (n, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut core = [0u64; 3];
        let mut count = 0;
        for part in nums.split('.') {
            if count == 3 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) || part.len() > 9 {
                return None;
            }
            core[count] = part.parse().ok()?;
            count += 1;
        }
        if count < 2 {
            return None;
        }
        Some(Version { core, pre })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.core[0], self.core[1], self.core[2])?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `macrust-0.4.0.exe`, the name of a downloaded update.
pub fn update_file_name(v: &Version) -> String {
    format!("macrust-{v}.exe")
}

pub fn parse_update_file_name(name: &str) -> Option<Version> {
    let inner = name.strip_prefix("macrust-")?.strip_suffix(".exe")?;
    Version::parse(inner)
}

// ---------------------------------------------------------------------------
// URLs

pub use crate::net_logic::Url;

/// [`crate::net_logic::allowed_url`] with the updater's hosts.
pub fn allowed_url(url: &str) -> Option<Url> {
    crate::net_logic::allowed_url(url, &ALLOWED_HOSTS)
}

/// Resolves a `Location` header against the URL that returned it.
#[cfg(test)]
pub fn resolve_redirect(base: &Url, location: &str) -> Option<Url> {
    crate::net_logic::resolve_redirect(base, location, &ALLOWED_HOSTS)
}

// ---------------------------------------------------------------------------
// Checksum files

/// `<64 hex>  macrust.exe` (also `*macrust.exe`, or the bare hash).
pub fn parse_sha256_file(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut parts = line.split_whitespace();
    let hash = parts.next()?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if let Some(name) = parts.next()
        && (name.strip_prefix('*').unwrap_or(name) != EXE_ASSET || parts.next().is_some())
    {
        return None;
    }
    Some(hash.to_ascii_lowercase())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Release selection

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub exe: Url,
    pub sha: Url,
}

/// The highest non-draft release (pre-releases included) that carries both
/// assets on allowed URLs. `Ok(None)` when there is none.
pub fn select_release(json: &str) -> Result<Option<Release>, String> {
    let root = Json::parse(json)?;
    let Json::Arr(list) = root else { return Err("releases: not an array".into()) };
    let mut best: Option<Release> = None;
    for rel in &list {
        if rel.get("draft").and_then(Json::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(version) = rel.get("tag_name").and_then(Json::as_str).and_then(Version::parse) else { continue };
        if best.as_ref().is_some_and(|b| b.version >= version) {
            continue;
        }
        let Some(Json::Arr(assets)) = rel.get("assets") else { continue };
        let find = |name: &str| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(Json::as_str) == Some(name))
                .and_then(|a| a.get("browser_download_url"))
                .and_then(Json::as_str)
                .and_then(allowed_url)
        };
        if let (Some(exe), Some(sha)) = (find(EXE_ASSET), find(SHA_ASSET)) {
            best = Some(Release { version, exe, sha });
        }
    }
    Ok(best)
}

// ---------------------------------------------------------------------------
// Minimal JSON reader

#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        let v = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err("json: trailing data".into());
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

const MAX_DEPTH: usize = 64;

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err("json: too deep".into());
        }
        self.ws();
        match self.s.get(self.i) {
            None => Err("json: unexpected end".into()),
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return Err("json: expected ':'".into());
                    }
                    m.push((k, self.value(depth + 1)?));
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Obj(m));
                    }
                    return Err("json: expected ',' or '}'".into());
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Arr(a));
                    }
                    return Err("json: expected ',' or ']'".into());
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while matches!(self.s.get(self.i), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                t.parse().map(Json::Num).map_err(|_| "json: bad number".to_string())
            }
            Some(_) => Err("json: unexpected character".into()),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.s.get(self.i..self.i + 4).ok_or("json: short \\u escape")?;
        let h = std::str::from_utf8(h).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(h, 16).map_err(|_| "json: bad \\u escape".to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        if !self.eat("\"") {
            return Err("json: expected string".into());
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else { return Err("json: unterminated string".into()) };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else { return Err("json: bad escape".into()) };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat("\\u") {
                                    return Err("json: lone surrogate".into());
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("json: bad surrogate pair".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(cp).ok_or("json: bad code point")?
                        }
                        _ => return Err("json: bad escape".into()),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return Err("json: control character in string".into()),
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| "json: invalid utf-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn version_parse_and_order() {
        assert_eq!(v("v0.3.0"), v("0.3.0"));
        assert_eq!(v("1.2").core, [1, 2, 0]);
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("0.3.1") > v("0.3.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("1.0.0-rc1") < v("1.0.0"));
        assert!(v("1.0.0-rc2") > v("1.0.0-rc1"));
        assert!(v("1.0.1-rc1") > v("1.0.0"));
        assert_eq!(v("1.2.3+build5"), v("1.2.3"));
        assert_eq!(v("0.4.0").to_string(), "0.4.0");
        for bad in ["", "v", "1", "a.b.c", "1..2", "1.2.3.4", "1.2.3-", "-1.2.3", "1.2.x", "99999999999.1.1"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn update_file_names() {
        let ver = v("0.4.0");
        assert_eq!(update_file_name(&ver), "macrust-0.4.0.exe");
        assert_eq!(parse_update_file_name("macrust-0.4.0.exe"), Some(ver));
        assert_eq!(parse_update_file_name("macrust-0.4.0.exe.sha256"), None);
        assert_eq!(parse_update_file_name("macrust.exe"), None);
        assert_eq!(parse_update_file_name("macrust-0.4.0.exe.part"), None);
    }

    #[test]
    fn url_allowlist() {
        let ok = allowed_url("https://github.com/trifonovsdev/macRust/releases/download/v1/macrust.exe").unwrap();
        assert_eq!(ok.host, "github.com");
        assert_eq!(ok.path, "/trifonovsdev/macRust/releases/download/v1/macrust.exe");
        assert!(allowed_url("HTTPS://API.GITHUB.COM/repos/x?per_page=10").is_some());
        assert_eq!(allowed_url("https://api.github.com?x=1").unwrap().path, "/?x=1");
        assert!(allowed_url("https://objects.githubusercontent.com:443/a?b=c#frag").is_some());
        assert_eq!(allowed_url("https://github.com/a#frag").unwrap().path, "/a");
        for bad in [
            "http://github.com/a",
            "ftp://github.com/a",
            "https://evilgithub.com/a",
            "https://github.com.evil.com/a",
            "https://evil.com/github.com",
            "https://github.com@evil.com/a",
            "https://evil.com@github.com/a",
            "https://github.com:8443/a",
            "https://github.com:80/a",
            "https://raw.githubusercontent.com/a",
            "https://sub.github.com/a",
            "https://github.com\\@evil.com/a",
            "https://github.com/a b",
            "//github.com/a",
            "/relative",
            "",
        ] {
            assert!(allowed_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn redirects_are_resolved_and_checked() {
        let base = allowed_url("https://github.com/x/y/releases/download/v1/macrust.exe").unwrap();
        let r = resolve_redirect(&base, "https://objects.githubusercontent.com/abc?sig=1").unwrap();
        assert_eq!(r.host, "objects.githubusercontent.com");
        assert_eq!(resolve_redirect(&base, "/other/path").unwrap().as_string(), "https://github.com/other/path");
        assert!(resolve_redirect(&base, "https://evil.com/x").is_none());
        assert!(resolve_redirect(&base, "http://github.com/x").is_none());
        assert!(resolve_redirect(&base, "//evil.com/x").is_none());
        assert!(resolve_redirect(&base, "relative/path").is_none());
    }

    #[test]
    fn sha256_files() {
        let h = "a".repeat(32) + &"0F".repeat(16);
        assert_eq!(parse_sha256_file(&format!("{h}  macrust.exe\n")), Some(h.to_ascii_lowercase()));
        assert!(parse_sha256_file(&format!("\u{feff}{h} *macrust.exe\r\n")).is_some());
        assert!(parse_sha256_file(&h).is_some());
        assert!(parse_sha256_file(&format!("\n\n{h}  macrust.exe")).is_some());
        assert!(parse_sha256_file(&format!("{h}  other.exe")).is_none());
        assert!(parse_sha256_file(&format!("{h}  macrust.exe extra")).is_none());
        assert!(parse_sha256_file(&"g".repeat(64)).is_none());
        assert!(parse_sha256_file(&h[..63]).is_none());
        assert!(parse_sha256_file("").is_none());
        assert_eq!(hex(&[0, 1, 0xab, 0xff]), "0001abff");
    }

    fn release(tag: &str, draft: bool, assets: &[(&str, &str)]) -> String {
        let a: Vec<String> =
            assets.iter().map(|(n, u)| format!(r#"{{"name":"{n}","browser_download_url":"{u}","size":1}}"#)).collect();
        format!(r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":true,"assets":[{}]}}"#, a.join(","))
    }

    fn full(tag: &str) -> Vec<(String, String)> {
        let base = format!("https://github.com/trifonovsdev/macRust/releases/download/{tag}");
        vec![("macrust.exe".into(), format!("{base}/macrust.exe")), ("macrust.exe.sha256".into(), format!("{base}/macrust.exe.sha256"))]
    }

    fn rel(tag: &str, draft: bool) -> String {
        let f = full(tag);
        let refs: Vec<(&str, &str)> = f.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        release(tag, draft, &refs)
    }

    #[test]
    fn picks_highest_non_draft_with_assets() {
        let json = format!("[{},{},{},{}]", rel("v0.3.0", false), rel("v0.10.0", false), rel("v0.11.0", true), rel("v0.9.0", false));
        let r = select_release(&json).unwrap().unwrap();
        assert_eq!(r.version, v("0.10.0"));
        assert_eq!(r.exe.path, "/trifonovsdev/macRust/releases/download/v0.10.0/macrust.exe");
        assert_eq!(r.sha.host, "github.com");
    }

    #[test]
    fn skips_releases_without_valid_assets() {
        let none = release("v0.5.0", false, &[]);
        let only_exe = release("v0.4.0", false, &[("macrust.exe", "https://github.com/a/b")]);
        let evil = release(
            "v0.6.0",
            false,
            &[("macrust.exe", "https://evil.com/macrust.exe"), ("macrust.exe.sha256", "https://github.com/a/b")],
        );
        let json = format!("[{none},{only_exe},{evil},{},{{\"tag_name\":\"nightly\",\"assets\":[]}}]", rel("v0.3.0", false));
        assert_eq!(select_release(&json).unwrap().unwrap().version, v("0.3.0"));
        assert_eq!(select_release("[]").unwrap(), None);
        assert!(select_release("{}").is_err());
        assert!(select_release("[").is_err());
    }

    #[test]
    fn json_edge_cases() {
        let j = Json::parse(r#"{"a":"xé😀\n\"","b":[1,-2.5e1,null,true],"c":{}} "#).unwrap();
        assert_eq!(j.get("a").and_then(Json::as_str), Some("x\u{e9}\u{1f600}\n\""));
        assert!(matches!(j.get("b"), Some(Json::Arr(a)) if a.len() == 4));
        assert_eq!(j.get("c"), Some(&Json::Obj(vec![])));
        for bad in ["", "{", r#"{"a"}"#, r#"{"a":1,}"#, "[1,]", r#""\ud800""#, r#""\x""#, "tru", "[1] x", "\"a\nb\""] {
            assert!(Json::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Json::parse(&"[".repeat(200)).is_err());
    }
}
