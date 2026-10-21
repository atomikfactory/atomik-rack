//! Stable database keys for file paths.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The Windows path separator.
const BACKSLASH: char = '\u{5c}';

/// Whether `s` starts with a drive prefix such as `C:`.
fn has_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A normalised path used as the library database key.
///
/// Separators become `/`, `.` components are removed and `..` components are resolved
/// lexically (no disk access). On Windows the whole key is lower-cased (paths there are
/// case-insensitive), so `C:\Users\example\Kick.wav` and `c:/users/example/kick.WAV` are the same key.
/// Drive prefixes (`C:/x`) and UNC prefixes (`//server/share/x`) are kept, a trailing separator
/// is dropped, and `..` never climbs above the root (a relative path keeps leading `..`).
/// Serialised as a plain string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PathKey(String);

impl PathKey {
    /// Builds the key for `path`, using the current platform's case rule.
    pub fn new(path: &Path) -> Self {
        Self::with_case_folding(path, cfg!(windows))
    }

    /// Builds the key with an explicit case rule (for tests of both platforms).
    pub fn with_case_folding(path: &Path, fold_case: bool) -> Self {
        let text = path.to_string_lossy().replace(BACKSLASH, "/");
        let mut rest: &str = &text;
        if let Some(after) = rest.strip_prefix("//?/")
            && has_drive(after)
        {
            rest = after;
        }

        let mut prefix = String::new();
        let mut rooted = false;
        let mut unc = false;
        if has_drive(rest) {
            prefix.push_str(&rest[..2]);
            rest = &rest[2..];
            rooted = rest.starts_with('/');
        } else if rest.starts_with("//") && !rest.starts_with("///") {
            unc = true;
            rooted = true;
        } else if rest.starts_with('/') {
            rooted = true;
        }

        let mut parts = rest.split('/').filter(|p| !p.is_empty());
        if unc {
            prefix.push_str("//");
            if let Some(server) = parts.next() {
                prefix.push_str(server);
            }
            if let Some(share) = parts.next() {
                prefix.push('/');
                prefix.push_str(share);
            }
        }

        let mut stack: Vec<&str> = Vec::new();
        for part in parts {
            match part {
                "." => {}
                ".." => match stack.last() {
                    Some(&last) if last != ".." => {
                        stack.pop();
                    }
                    _ if rooted => {}
                    _ => stack.push(".."),
                },
                other => stack.push(other),
            }
        }

        let mut key = prefix;
        if rooted && (!unc || !stack.is_empty()) {
            key.push('/');
        }
        key.push_str(&stack.join("/"));
        if fold_case {
            key = key.to_lowercase();
        }
        Self(key)
    }

    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(p: &str, fold: bool) -> String {
        PathKey::with_case_folding(Path::new(p), fold)
            .as_str()
            .to_string()
    }

    #[test]
    fn converts_separators_and_collapses_repeats() {
        assert_eq!(key(r"a\b//c///d", false), "a/b/c/d");
    }

    #[test]
    fn drops_dots_and_strips_trailing_separator() {
        assert_eq!(key("/a/./b/./", false), "/a/b");
        assert_eq!(key("./a", false), "a");
    }

    #[test]
    fn resolves_parent_components_lexically() {
        assert_eq!(key("/a/b/../c", false), "/a/c");
        assert_eq!(key("/a/b/../../..", false), "/");
        assert_eq!(key("/../x", false), "/x");
        assert_eq!(key("a/../..", false), "..");
    }

    #[test]
    fn root_is_kept() {
        assert_eq!(key("/", false), "/");
        assert_eq!(key("", false), "");
    }

    #[test]
    fn windows_drive_prefix() {
        assert_eq!(
            key(r"C:\Users\example\Kick.wav", false),
            "C:/Users/example/Kick.wav"
        );
        assert_eq!(
            key(r"C:\Users\example\Kick.wav", true),
            "c:/users/example/kick.wav"
        );
        assert_eq!(key(r"C:\", true), "c:/");
        assert_eq!(key(r"C:\Windows\a\..\..\..\b", false), "C:/b");
        assert_eq!(key(r"C:rel\x", false), "C:rel/x");
    }

    #[test]
    fn verbatim_drive_prefix_is_removed() {
        assert_eq!(key(r"\\?\C:\Windows\y", true), "c:/windows/y");
    }

    #[test]
    fn unc_prefix() {
        assert_eq!(key(r"\\Server\Share\a\b", false), "//Server/Share/a/b");
        assert_eq!(key(r"\\Server\Share\a\b", true), "//server/share/a/b");
        assert_eq!(key("//server/share/..", false), "//server/share");
        assert_eq!(key("//server/share/", false), "//server/share");
    }

    #[test]
    fn case_is_kept_without_folding() {
        assert_ne!(key("/A/b", false), key("/a/b", false));
        assert_eq!(key("/A/b", true), key("/a/b", true));
    }

    #[test]
    fn mixed_spellings_are_one_key() {
        assert_eq!(
            PathKey::with_case_folding(Path::new(r"C:\Users\example\Kick.wav"), true),
            PathKey::with_case_folding(Path::new("c:/users/example/./x/../kick.WAV"), true)
        );
    }

    #[test]
    fn serialises_as_plain_string() {
        let k = PathKey::with_case_folding(Path::new("/a/b"), false);
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"/a/b\"");
    }

    #[test]
    fn new_uses_platform_rule() {
        let k = PathKey::new(Path::new("/A"));
        assert_eq!(k.as_str(), if cfg!(windows) { "/a" } else { "/A" });
    }
}
