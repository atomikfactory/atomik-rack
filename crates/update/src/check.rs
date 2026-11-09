//! The check itself: one request, one answer, one decision.

use crate::http::{HttpError, HttpGet, MAX_BODY, Request};
use crate::releases::{Release, ReleaseError, newest};
use crate::version::Version;

/// The one address the program asks, and only when the user allowed it: the list of releases of
/// the project, newest first. No query, no identifier.
pub const RELEASES_URL: &str = "https://api.github.com/repos/atomikfactory/atomik-rack/releases";

/// Why a check gave no answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckError {
    /// The request failed.
    #[error("{0}")]
    Http(#[from] HttpError),
    /// The server answered with a status other than 200.
    #[error("the server answered with status {0}")]
    Status(u16),
    /// The answer was too big or not a list of releases.
    #[error("unreadable answer: {0}")]
    Malformed(#[from] ReleaseError),
}

/// The User-Agent of a build: generic, the version only.
pub fn user_agent(running: &Version) -> String {
    format!("atomik-rack/{running}")
}

/// Asks `http` for the list of releases and returns the one to offer: the newest eligible release
/// when it is newer than `running`, otherwise `None`. A pre-release is eligible only when
/// `running` is one itself.
pub fn check(http: &dyn HttpGet, running: &Version) -> Result<Option<Release>, CheckError> {
    let request = Request {
        url: RELEASES_URL.to_string(),
        user_agent: user_agent(running),
    };
    let response = http.get(&request)?;
    if response.status != 200 {
        return Err(CheckError::Status(response.status));
    }
    if response.body.len() as u64 > MAX_BODY {
        return Err(CheckError::Malformed(ReleaseError::Malformed));
    }
    let best = newest(&response.body, running.is_prerelease())?;
    Ok(best.filter(|release| release.version > *running))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeHttp;

    const LIST: &str = r#"[
        {"tag_name":"v0.5.0-rc.1","prerelease":true,"draft":false},
        {"tag_name":"v0.4.0","prerelease":false,"draft":false},
        {"tag_name":"v0.3.0","prerelease":false,"draft":false}
    ]"#;

    fn running(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_newer_release_is_found() {
        let http = FakeHttp::ok(LIST);
        let found = check(&http, &running("0.3.0")).unwrap().unwrap();
        assert_eq!(found.tag, "v0.4.0");
        assert_eq!(found.version, running("0.4.0"));
    }

    #[test]
    fn the_same_version_is_up_to_date() {
        let http = FakeHttp::ok(LIST);
        assert_eq!(check(&http, &running("0.4.0")), Ok(None));
    }

    #[test]
    fn an_older_list_is_up_to_date() {
        let http = FakeHttp::ok(LIST);
        assert_eq!(check(&http, &running("1.0.0")), Ok(None));
    }

    #[test]
    fn a_release_candidate_sees_a_later_candidate_and_the_release() {
        let http = FakeHttp::ok(LIST);
        assert_eq!(
            check(&http, &running("0.4.0-rc.1")).unwrap().unwrap().tag,
            "v0.5.0-rc.1"
        );
        let released = FakeHttp::ok(r#"[{"tag_name":"v0.4.0","prerelease":false}]"#);
        assert_eq!(
            check(&released, &running("0.4.0-rc.2"))
                .unwrap()
                .unwrap()
                .tag,
            "v0.4.0"
        );
    }

    #[test]
    fn a_release_does_not_see_the_candidates() {
        let http = FakeHttp::ok(r#"[{"tag_name":"v0.5.0-rc.1","prerelease":true}]"#);
        assert_eq!(check(&http, &running("0.4.0")), Ok(None));
    }

    #[test]
    fn a_malformed_answer_is_an_error() {
        for body in ["", "<html>rate limited</html>", "{}", "[1,"] {
            let http = FakeHttp::ok(body);
            assert!(
                matches!(
                    check(&http, &running("0.4.0")),
                    Err(CheckError::Malformed(_))
                ),
                "{body:?}"
            );
        }
    }

    #[test]
    fn an_error_status_is_an_error() {
        for status in [301, 403, 404, 429, 500, 503] {
            let http = FakeHttp::status(status, LIST);
            assert_eq!(
                check(&http, &running("0.3.0")),
                Err(CheckError::Status(status))
            );
        }
    }

    #[test]
    fn a_failed_request_is_an_error() {
        let http = FakeHttp::failing();
        assert!(matches!(
            check(&http, &running("0.3.0")),
            Err(CheckError::Http(HttpError::Failed(_)))
        ));
    }

    #[test]
    fn an_answer_over_the_limit_is_refused() {
        let big = format!("[{}]", " ".repeat(MAX_BODY as usize + 1));
        let http = FakeHttp::ok(&big);
        assert!(matches!(
            check(&http, &running("0.3.0")),
            Err(CheckError::Malformed(_))
        ));
    }

    #[test]
    fn the_request_is_the_address_and_a_generic_user_agent_and_nothing_more() {
        let http = FakeHttp::ok(LIST);
        check(&http, &running("0.4.0-rc.1")).unwrap();
        let seen = http.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0],
            Request {
                url: "https://api.github.com/repos/atomikfactory/atomik-rack/releases".into(),
                user_agent: "atomik-rack/0.4.0-rc.1".into(),
            }
        );
        assert!(!seen[0].url.contains('?'));
        assert!(seen[0].url.starts_with("https://"));
    }
}
