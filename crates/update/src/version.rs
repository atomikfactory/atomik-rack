//! Version numbers and their order.
//!
//! A release tag reads `v0.4.0` or `v0.4.0-rc.2`: three numbers, then an optional pre-release part
//! of dot-separated identifiers. The order is the one of semantic versioning: the numbers compare
//! first; a version with a pre-release part is older than the same numbers without one
//! (`0.4.0-rc.2` < `0.4.0`); identifiers compare one by one, numbers by value (`rc.2` < `rc.10`) and
//! before words, words by their letters (`beta` < `rc`), and a shorter list is older when the
//! start is the same (`rc` < `rc.1`). Build metadata after a `+` is read and ignored.

use std::cmp::Ordering;
use std::fmt;

/// Why a text is not a version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// The text is empty, or a part is.
    #[error("empty version part")]
    Empty,
    /// The numbers are not exactly three whole numbers.
    #[error("a version has three numbers: major.minor.patch")]
    Numbers,
    /// A number that starts with a zero, or is too big.
    #[error("bad number in the version")]
    BadNumber,
    /// A character a version cannot hold.
    #[error("bad character in the version")]
    BadCharacter,
}

/// One identifier of a pre-release part.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    /// A number (compares by value, before every word).
    Number(u64),
    /// A word.
    Word(String),
}

/// A version: `major.minor.patch` and an optional pre-release part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Ident>,
}

fn number(text: &str) -> Result<u64, VersionError> {
    if text.is_empty() {
        return Err(VersionError::Empty);
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(VersionError::BadCharacter);
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(VersionError::BadNumber);
    }
    text.parse().map_err(|_| VersionError::BadNumber)
}

impl Version {
    /// Reads `1.2.3`, `v1.2.3`, `1.2.3-rc.1` or `1.2.3+build` (a leading `v` is allowed because
    /// tags have one).
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let text = text.trim();
        let text = text
            .strip_prefix('v')
            .or_else(|| text.strip_prefix('V'))
            .unwrap_or(text);
        if text.is_empty() {
            return Err(VersionError::Empty);
        }
        let without_build = text.split('+').next().unwrap_or(text);
        let (core, pre) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(VersionError::Numbers);
        };
        let mut idents = Vec::new();
        if let Some(pre) = pre {
            for part in pre.split('.') {
                if part.is_empty() {
                    return Err(VersionError::Empty);
                }
                if !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return Err(VersionError::BadCharacter);
                }
                idents.push(if part.bytes().all(|b| b.is_ascii_digit()) {
                    Ident::Number(number(part)?)
                } else {
                    Ident::Word(part.to_string())
                });
            }
        }
        Ok(Self {
            major: number(major)?,
            minor: number(minor)?,
            patch: number(patch)?,
            pre: idents,
        })
    }

    /// Whether this is a pre-release (`0.4.0-rc.1`).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // No pre-release part is the newer one.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, ident) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match ident {
                Ident::Number(n) => write!(f, "{n}")?,
                Ident::Word(w) => f.write_str(w)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_tag_and_a_bare_version_read_the_same() {
        assert_eq!(v("v0.4.0"), v("0.4.0"));
        assert_eq!(v("V1.2.3"), v("1.2.3"));
        assert_eq!(v(" 1.2.3 \n"), v("1.2.3"));
        assert_eq!(v("0.4.0-rc.1").to_string(), "0.4.0-rc.1");
        assert_eq!(v("v10.20.30").to_string(), "10.20.30");
    }

    #[test]
    fn numbers_compare_by_value_not_by_text() {
        assert!(v("0.9.0") < v("0.10.0"));
        assert!(v("0.4.9") < v("0.4.10"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("2.0.0") > v("1.99.99"));
    }

    #[test]
    fn a_release_candidate_is_older_than_its_release() {
        assert!(v("0.4.0-rc.1") < v("0.4.0"));
        assert!(v("0.4.0-rc.9") < v("0.4.0"));
        assert!(v("0.3.9") < v("0.4.0-rc.1"));
        assert!(v("0.4.0-rc.1").is_prerelease());
        assert!(!v("0.4.0").is_prerelease());
    }

    #[test]
    fn release_candidates_are_in_order_by_number_and_by_word() {
        assert!(v("0.4.0-rc.1") < v("0.4.0-rc.2"));
        assert!(v("0.4.0-rc.2") < v("0.4.0-rc.10"));
        assert!(v("0.4.0-alpha.3") < v("0.4.0-beta.1"));
        assert!(v("0.4.0-beta.9") < v("0.4.0-rc.1"));
        assert!(v("0.4.0-rc") < v("0.4.0-rc.1"));
        assert!(v("0.4.0-rc.1") < v("0.4.0-rc.1.1"));
        assert!(v("0.4.0-1") < v("0.4.0-alpha"));
    }

    #[test]
    fn build_metadata_does_not_count() {
        assert_eq!(v("1.2.3+abc"), v("1.2.3+def"));
        assert_eq!(v("1.2.3-rc.1+abc"), v("1.2.3-rc.1"));
        assert_eq!(v("1.2.3+abc").cmp(&v("1.2.3")), Ordering::Equal);
    }

    #[test]
    fn a_text_that_is_not_a_version_is_refused() {
        for bad in [
            "",
            "v",
            "1",
            "1.2",
            "1.2.3.4",
            "a.b.c",
            "1.2.x",
            "01.2.3",
            "1.02.3",
            "1.2.3-",
            "1.2.3-rc..1",
            "1.2.3-rc.01",
            "1.2.3-r_c",
            "latest",
            "nightly",
            "-1.2.3",
            "99999999999999999999.0.0",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }
}
