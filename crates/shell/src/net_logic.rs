//! Platform-neutral URL checks for the HTTPS client in `win/net.rs`: every
//! caller passes its own host allowlist, and each redirect is checked again.
#![cfg_attr(not(windows), allow(dead_code))]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Url {
    #[cfg(test)]
    pub fn as_string(&self) -> String {
        format!("https://{}{}", self.host, self.path)
    }
}

/// Accepts only `https://<host in hosts>[:443]/...` without credentials.
pub fn allowed_url(url: &str, hosts: &[&str]) -> Option<Url> {
    let url = url.trim();
    if url.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\') {
        return None;
    }
    let rest = url.get(..8).filter(|p| p.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_string()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_string()),
    };
    if authority.contains('@') {
        return None;
    }
    let host = match authority.split_once(':') {
        Some((h, "443")) => h,
        Some(_) => return None,
        None => authority,
    };
    let host = host.to_ascii_lowercase();
    hosts.contains(&host.as_str()).then_some(Url { host, path })
}

/// Resolves a `Location` header against the URL that returned it.
pub fn resolve_redirect(base: &Url, location: &str, hosts: &[&str]) -> Option<Url> {
    let location = location.trim();
    if location.starts_with("//") {
        return None;
    }
    if location.starts_with('/') {
        return allowed_url(&format!("https://{}{}", base.host, location), hosts);
    }
    allowed_url(location, hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_caller_has_its_own_hosts() {
        let a = ["a.example"];
        assert!(allowed_url("https://a.example/x", &a).is_some());
        assert!(allowed_url("https://b.example/x", &a).is_none());
        assert!(allowed_url("https://a.example/x", &[]).is_none());
        let base = allowed_url("https://a.example/x", &a).unwrap();
        assert_eq!(resolve_redirect(&base, "/y?z=1", &a).unwrap().as_string(), "https://a.example/y?z=1");
        assert!(resolve_redirect(&base, "https://b.example/y", &a).is_none());
    }
}
