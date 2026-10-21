//! Tag names.

use serde::{Deserialize, Serialize};

/// The longest tag, in characters.
pub const MAX_TAG_CHARS: usize = 32;

/// A validated tag name: trimmed, 1 to [`MAX_TAG_CHARS`] characters, no `#`, `,` or control
/// characters. Inner whitespace runs collapse to one space. Two tags are the same tag when they
/// are equal ignoring case; the spelling stored is the first one used.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

/// Why a tag name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    /// Empty after trimming.
    #[error("a tag cannot be empty")]
    Empty,
    /// Longer than [`MAX_TAG_CHARS`].
    #[error("a tag can have at most 32 characters")]
    TooLong,
    /// Contains `#`, `,` or a control character.
    #[error("a tag cannot contain '#', ',' or control characters")]
    InvalidChar,
}

impl Tag {
    /// Validates and normalises `name`.
    pub fn new(name: &str) -> Result<Self, TagError> {
        let mut out = String::with_capacity(name.len());
        for word in name.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
        if out.is_empty() {
            return Err(TagError::Empty);
        }
        if out.chars().any(|c| c == '#' || c == ',' || c.is_control()) {
            return Err(TagError::InvalidChar);
        }
        if out.chars().count() > MAX_TAG_CHARS {
            return Err(TagError::TooLong);
        }
        Ok(Self(out))
    }

    /// The display spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The case-folded form used for comparison (per-character lower-casing).
    pub fn folded(&self) -> String {
        self.0.chars().flat_map(char::to_lowercase).collect()
    }
}

impl PartialEq for Tag {
    fn eq(&self, other: &Self) -> bool {
        self.folded() == other.folded()
    }
}

impl Eq for Tag {}

impl PartialOrd for Tag {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Tag {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.folded().cmp(&other.folded())
    }
}

impl TryFrom<String> for Tag {
    type Error = TagError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Tag::new(&s)
    }
}

impl From<Tag> for String {
    fn from(t: Tag) -> Self {
        t.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_whitespace() {
        assert_eq!(
            Tag::new("  hi   hat\t loop ").unwrap().as_str(),
            "hi hat loop"
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(Tag::new("").unwrap_err(), TagError::Empty);
        assert_eq!(Tag::new(" \t ").unwrap_err(), TagError::Empty);
    }

    #[test]
    fn rejects_forbidden_chars() {
        assert_eq!(Tag::new("a#b").unwrap_err(), TagError::InvalidChar);
        assert_eq!(Tag::new("a,b").unwrap_err(), TagError::InvalidChar);
        assert_eq!(Tag::new("a\u{7}b").unwrap_err(), TagError::InvalidChar);
    }

    #[test]
    fn length_counts_chars_not_bytes() {
        let ok = "é".repeat(MAX_TAG_CHARS);
        assert!(ok.len() > MAX_TAG_CHARS);
        assert!(Tag::new(&ok).is_ok());
        let long = "é".repeat(MAX_TAG_CHARS + 1);
        assert_eq!(Tag::new(&long).unwrap_err(), TagError::TooLong);
    }

    #[test]
    fn length_is_checked_after_collapsing() {
        let name = format!("a{}b", " ".repeat(40));
        assert_eq!(Tag::new(&name).unwrap().as_str(), "a b");
    }

    #[test]
    fn equality_and_order_ignore_case() {
        let a = Tag::new("808").unwrap();
        let b = Tag::new("Kick").unwrap();
        let c = Tag::new("KICK").unwrap();
        assert_eq!(b, c);
        assert_ne!(a, b);
        assert!(a < b);
        assert_eq!(b.cmp(&c), std::cmp::Ordering::Equal);
        assert_eq!(b.as_str(), "Kick");
        assert_eq!(b.folded(), "kick");
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let t = Tag::new("Snare").unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"Snare\"");
        let back: Tag = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_str(), "Snare");
        assert!(serde_json::from_str::<Tag>("\"a#b\"").is_err());
        assert!(serde_json::from_str::<Tag>("\"\"").is_err());
    }
}
