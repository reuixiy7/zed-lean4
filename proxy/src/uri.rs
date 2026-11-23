//! Minimal `file://` URI helpers.

use std::path::{Path, PathBuf};

pub fn from_path(path: &Path) -> String {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        // Windows drive path: C:/foo -> /C:/foo
        s.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(b);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let mut s = String::from_utf8(decoded).ok()?;
    // /C:/foo -> C:/foo
    if s.len() > 2 && s.as_bytes()[0] == b'/' && s.as_bytes()[2] == b':' {
        s.remove(0);
    }
    Some(PathBuf::from(s))
}

/// Path relative to the working directory (Zed starts servers in the worktree root).
pub fn display_name(uri: &str) -> String {
    let Some(path) = to_path(uri) else {
        return uri.to_owned();
    };
    let relative = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf));
    relative
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_unicode_and_spaces() {
        let p = std::env::temp_dir().join("My Proj/Λ.lean");
        let uri = from_path(&p);
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.ends_with("/My%20Proj/%CE%9B.lean"), "{uri}");
        assert_eq!(to_path(&uri).unwrap(), p);
    }

    #[test]
    fn windows_drive() {
        assert_eq!(
            to_path("file:///C%3A/x/A.lean").unwrap(),
            PathBuf::from("C:/x/A.lean")
        );
    }
}
