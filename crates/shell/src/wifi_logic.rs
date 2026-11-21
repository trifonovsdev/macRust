//! Platform-neutral part of the Wi-Fi menu's backend (`win/wifi.rs`): one
//! entry per network out of Windows' per-profile list, and the WLAN profile
//! that joins an open network. SSIDs are personal data: nothing here logs them.
#![cfg_attr(not(windows), allow(dead_code))]

/// A network as Windows lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub ssid: Vec<u8>,
    pub secure: bool,
    /// Signal quality 0..=100.
    pub signal: u8,
    /// Name of the WLAN profile that joins it, if Windows has one.
    pub profile: Option<String>,
    pub connected: bool,
}

/// Stable key of an SSID (64-bit FNV-1a): the menu passes it back on a click.
pub fn id_of(ssid: &[u8]) -> u64 {
    ssid.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

pub fn name_of(ssid: &[u8]) -> String {
    String::from_utf8_lossy(ssid).into_owned()
}

/// Windows lists a network once per profile plus once without one; the menu
/// shows each SSID once. Hidden networks (empty SSID) are left out: they are
/// joined through "Other…". The first entry's position is kept.
pub fn merge(seen: impl IntoIterator<Item = Seen>) -> Vec<Seen> {
    let mut out: Vec<Seen> = Vec::new();
    for s in seen {
        if s.ssid.is_empty() || s.ssid.iter().all(|&b| b == 0) {
            continue;
        }
        match out.iter_mut().find(|o| o.ssid == s.ssid) {
            Some(o) => {
                o.secure |= s.secure;
                o.signal = o.signal.max(s.signal);
                // The profile Windows is connected with wins over another one.
                if (s.connected && s.profile.is_some()) || o.profile.is_none() {
                    o.profile = s.profile.or(o.profile.take());
                }
                o.connected |= s.connected;
            }
            None => out.push(s),
        }
    }
    out
}

fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no escape for these; the hex element carries the real bytes.
            c if (c as u32) < 0x20 || c == '\u{fffd}' => out.push('_'),
            c => out.push(c),
        }
    }
    out
}

/// A temporary WLAN profile for an open (unsecured) network. The SSID goes
/// in as hex, so any bytes are joined exactly; the name is only a label.
pub fn open_profile(ssid: &[u8]) -> String {
    let hex: String = ssid.iter().map(|b| format!("{b:02X}")).collect();
    let name = xml_text(&name_of(ssid));
    format!(
        "<?xml version=\"1.0\"?>\
<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
<name>{name}</name>\
<SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>\
<connectionType>ESS</connectionType><connectionMode>manual</connectionMode>\
<MSM><security><authEncryption><authentication>open</authentication><encryption>none</encryption>\
<useOneX>false</useOneX></authEncryption></security></MSM>\
</WLANProfile>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(ssid: &str, signal: u8, profile: Option<&str>, connected: bool) -> Seen {
        Seen { ssid: ssid.as_bytes().to_vec(), secure: true, signal, profile: profile.map(Into::into), connected }
    }

    #[test]
    fn one_entry_per_ssid_keeping_the_profile_and_the_best_signal() {
        let merged = merge([
            seen("home", 40, None, false),
            seen("cafe", 70, None, false),
            seen("home", 60, Some("home"), true),
            seen("", 90, None, false),
            Seen { ssid: vec![0; 4], ..seen("x", 90, None, false) },
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], seen("home", 60, Some("home"), true));
        assert_eq!(merged[1].ssid, b"cafe");
    }

    #[test]
    fn the_connected_profile_wins() {
        let merged = merge([seen("w", 50, Some("old"), false), seen("w", 50, Some("used"), true)]);
        assert_eq!(merged[0].profile.as_deref(), Some("used"));
        let merged = merge([seen("w", 50, Some("used"), true), seen("w", 50, Some("other"), false)]);
        assert_eq!(merged[0].profile.as_deref(), Some("used"));
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(id_of(b"home"), id_of(b"home"));
        assert_ne!(id_of(b"home"), id_of(b"Home"));
        assert_ne!(id_of(b""), id_of(b"\0"));
    }

    #[test]
    fn open_profile_escapes_the_name_and_carries_the_bytes_as_hex() {
        let xml = open_profile(b"a<b>&\"c'\x01");
        assert!(xml.contains("<hex>613C623E2622632701</hex>"));
        assert!(xml.contains("<name>a&lt;b&gt;&amp;&quot;c&apos;_</name>"));
        assert!(!xml.contains("a<b"));
        assert!(xml.contains("<authentication>open</authentication><encryption>none</encryption>"));
        // Not UTF-8: still the exact bytes in hex, a lossy label.
        let xml = open_profile(&[0xff, 0x41]);
        assert!(xml.contains("<hex>FF41</hex>") && xml.contains("<name>_A</name>"));
    }
}
