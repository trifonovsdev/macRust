//! Which "open in Explorer" requests the file manager takes over. Pure string
//! logic, so it is tested on any platform; the Windows side checks the disk.
#![cfg_attr(not(windows), allow(dead_code))]

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Explorer with no arguments: a window on the default folder.
    Home,
    /// A folder to open, if it turns out to be one on disk.
    Folder(String),
    /// Explorer's `/select,path`: open the parent and select the item.
    Select { dir: String, name: String },
}

/// A drive or UNC path (not `\\?\`, not `shell:`, not a URL).
fn pathlike(p: &str) -> bool {
    let b = p.as_bytes();
    ((b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || p.starts_with("\\\\")) && !p.starts_with("\\\\?\\")
}

/// `target`/`args` as given to ShellExecute; None leaves them to Windows.
pub fn route(target: &str, args: &str) -> Option<Route> {
    let t = target.trim().trim_matches('"');
    let lower = t.to_lowercase();
    let explorer = lower == "explorer" || lower == "explorer.exe" || lower.ends_with("\\explorer.exe");
    if !explorer {
        return pathlike(t).then(|| Route::Folder(t.to_string()));
    }
    let a = args.trim();
    if a.is_empty() {
        return Some(Route::Home);
    }
    if let Some(rest) = a.strip_prefix("/select,") {
        let p = rest.trim().trim_matches('"').trim_end_matches('\\');
        let (dir, name) = p.rsplit_once('\\')?;
        if !pathlike(p) || name.is_empty() {
            return None;
        }
        let dir = if dir.ends_with(':') { format!("{dir}\\") } else { dir.to_string() };
        return Some(Route::Select { dir, name: name.to_string() });
    }
    let p = a.trim_matches('"');
    pathlike(p).then(|| Route::Folder(p.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_requests() {
        assert_eq!(route("C:\\Windows\\explorer.exe", ""), Some(Route::Home));
        assert_eq!(route("explorer.exe", "/select,\"C:\\Users\\a\\b.txt\""), Some(Route::Select { dir: "C:\\Users\\a".into(), name: "b.txt".into() }));
        assert_eq!(route("explorer.exe", "/select,\"D:\\x\""), Some(Route::Select { dir: "D:\\".into(), name: "x".into() }));
        assert_eq!(route("explorer.exe", "\"C:\\Users\""), Some(Route::Folder("C:\\Users".into())));
        assert_eq!(route("explorer.exe", "shell:RecycleBinFolder"), None);
        assert_eq!(route("explorer.exe", "/e,::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), None);
    }

    #[test]
    fn plain_targets() {
        assert_eq!(route("C:\\Users\\a\\Downloads", ""), Some(Route::Folder("C:\\Users\\a\\Downloads".into())));
        assert_eq!(route("\\\\nas\\share", ""), Some(Route::Folder("\\\\nas\\share".into())));
        assert_eq!(route("shell:RecycleBinFolder", ""), None);
        assert_eq!(route("https://example.com", ""), None);
        assert_eq!(route("ms-settings:", ""), None);
        assert_eq!(route("\\\\?\\C:\\x", ""), None);
    }
}
