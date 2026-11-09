//! Reading the list of releases the project page answers with, and choosing the newest one.

use crate::version::Version;
use serde_json::Value;

/// Where a release has its page: the tag is appended.
pub const RELEASE_PAGE: &str = "https://github.com/atomikfactory/atomik-rack/releases/tag/";

/// The longest tag the program will put in an address.
const MAX_TAG: usize = 64;

/// Why an answer is not a list of releases.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    /// The text is not JSON, or not a list.
    #[error("the answer is not a list of releases")]
    Malformed,
}

/// One release that can be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// Its tag, exactly as published (letters, digits and `.-_+` only).
    pub tag: String,
}

impl Release {
    /// The page of the release on the project site, where "What's new" leads. It is made from the
    /// tag, never read from the answer.
    pub fn page(&self) -> String {
        format!("{RELEASE_PAGE}{}", self.tag)
    }
}

/// A tag that can stand in an address without any escaping.
fn plain_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= MAX_TAG
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'))
}

/// The newest release of the list `body` that this build may be offered: never a draft; a
/// pre-release only when `include_prereleases` (the running build is one itself). An entry that is
/// not an object, has no readable tag or has a tag that is not a version is left out, so one odd
/// entry never hides the others. `Ok(None)` when nothing qualifies.
pub fn newest(body: &[u8], include_prereleases: bool) -> Result<Option<Release>, ReleaseError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ReleaseError::Malformed)?;
    let Value::Array(entries) = value else {
        return Err(ReleaseError::Malformed);
    };
    let mut best: Option<Release> = None;
    for entry in &entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        if object.get("draft").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(tag) = object.get("tag_name").and_then(Value::as_str) else {
            continue;
        };
        if !plain_tag(tag) {
            continue;
        }
        let Ok(version) = Version::parse(tag) else {
            continue;
        };
        let flagged = object.get("prerelease").and_then(Value::as_bool) == Some(true);
        if (flagged || version.is_prerelease()) && !include_prereleases {
            continue;
        }
        if best.as_ref().is_none_or(|b| version > b.version) {
            best = Some(Release {
                version,
                tag: tag.to_string(),
            });
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_of(body: &str, pre: bool) -> Option<String> {
        newest(body.as_bytes(), pre).unwrap().map(|r| r.tag)
    }

    #[test]
    fn the_highest_version_wins_whatever_the_order_of_the_list() {
        let body = r#"[{"tag_name":"v0.3.0"},{"tag_name":"v0.10.0"},{"tag_name":"v0.9.5"}]"#;
        assert_eq!(tag_of(body, false).as_deref(), Some("v0.10.0"));
    }

    #[test]
    fn drafts_are_never_offered() {
        let body = r#"[{"tag_name":"v9.0.0","draft":true},{"tag_name":"v0.4.0"}]"#;
        assert_eq!(tag_of(body, false).as_deref(), Some("v0.4.0"));
        assert_eq!(tag_of(body, true).as_deref(), Some("v0.4.0"));
    }

    #[test]
    fn a_pre_release_is_offered_only_to_a_pre_release() {
        let body = r#"[{"tag_name":"v0.5.0-rc.1","prerelease":true},{"tag_name":"v0.4.0"}]"#;
        assert_eq!(tag_of(body, false).as_deref(), Some("v0.4.0"));
        assert_eq!(tag_of(body, true).as_deref(), Some("v0.5.0-rc.1"));
    }

    #[test]
    fn a_pre_release_tag_counts_even_when_the_flag_is_missing() {
        let body = r#"[{"tag_name":"v0.5.0-rc.1"},{"tag_name":"v0.4.0"}]"#;
        assert_eq!(tag_of(body, false).as_deref(), Some("v0.4.0"));
    }

    #[test]
    fn a_release_beats_its_own_candidates() {
        let body = r#"[{"tag_name":"v0.5.0-rc.2","prerelease":true},{"tag_name":"v0.5.0"}]"#;
        assert_eq!(tag_of(body, true).as_deref(), Some("v0.5.0"));
    }

    #[test]
    fn odd_entries_are_skipped() {
        let body = r#"[1, null, "x", {"name":"no tag"}, {"tag_name":7},
            {"tag_name":"nightly"}, {"tag_name":"v1.0.0/../x"}, {"tag_name":"v0.4.1"}]"#;
        assert_eq!(tag_of(body, false).as_deref(), Some("v0.4.1"));
    }

    #[test]
    fn an_empty_list_has_no_newest() {
        assert_eq!(tag_of("[]", true), None);
        assert_eq!(
            tag_of(r#"[{"tag_name":"v1.0.0","draft":true}]"#, true),
            None
        );
    }

    #[test]
    fn an_answer_that_is_not_a_list_is_malformed() {
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"tag_name":"v1.0.0"}"#,
            "42",
            "null",
            "[1,",
        ] {
            assert_eq!(
                newest(bad.as_bytes(), true),
                Err(ReleaseError::Malformed),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_page_is_made_from_the_tag() {
        let r = newest(
            br#"[{"tag_name":"v0.4.0","html_url":"https://example.com/evil"}]"#,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            r.page(),
            "https://github.com/atomikfactory/atomik-rack/releases/tag/v0.4.0"
        );
    }

    #[test]
    fn a_tag_with_characters_an_address_must_escape_is_left_out() {
        for tag in ["v1.0.0?x=1", "v1.0.0#frag", "v1.0.0 beta", "v1.0.0%2f", ""] {
            let body = format!(r#"[{{"tag_name":"{tag}"}}]"#);
            assert_eq!(tag_of(&body, true), None, "{tag:?}");
        }
        let long = format!("v1.0.0-{}", "a".repeat(80));
        assert_eq!(tag_of(&format!(r#"[{{"tag_name":"{long}"}}]"#), true), None);
    }
}
