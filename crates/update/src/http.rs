//! The one network seam of the program.
//!
//! The update check asks for one address through [`HttpGet`]. The real client ([`UreqGet`]) is a
//! small blocking HTTPS client on rustls; tests and every other build of the shell use a fake
//! ([`crate::FakeHttp`]) or [`Offline`], so no test reaches the network.

use std::time::Duration;

/// The longest answer read, in bytes. A list of releases is far smaller.
pub const MAX_BODY: u64 = 1 << 20;

/// How long a whole request may take, from the lookup of the name to the last byte.
pub const TIMEOUT: Duration = Duration::from_secs(8);

/// What is sent: an address and a User-Agent. Nothing else is added by the program (the client
/// adds the `Host` and `Accept` lines every request has): no cookie, no identifier, no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The address (HTTPS, no query).
    pub url: String,
    /// The generic User-Agent, `atomik-rack/<version>`.
    pub user_agent: String,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body (at most [`MAX_BODY`] bytes).
    pub body: Vec<u8>,
}

/// Why no answer came.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    /// The request could not be made or finished (no network, a timeout, a bad certificate).
    #[error("request failed: {0}")]
    Failed(String),
    /// The program has no network client (a build that never checks).
    #[error("no network client")]
    Offline,
}

/// A client that answers one GET request.
pub trait HttpGet: Send + Sync {
    /// Asks for `request.url` and returns the status and the body, or why there is none.
    fn get(&self, request: &Request) -> Result<Response, HttpError>;
}

/// A client that never leaves the machine: every request fails. The shell starts with this one,
/// and only the application window installs [`UreqGet`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Offline;

impl HttpGet for Offline {
    fn get(&self, _request: &Request) -> Result<Response, HttpError> {
        Err(HttpError::Offline)
    }
}

/// The real client: HTTPS only, rustls with the bundled root certificates, a timeout, no
/// redirect, no cookie store, no compression, a User-Agent that is the one of the request. It
/// follows the proxy settings of the environment like any program on the machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct UreqGet;

impl HttpGet for UreqGet {
    fn get(&self, request: &Request) -> Result<Response, HttpError> {
        let failed = |e: ureq::Error| HttpError::Failed(e.to_string());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(request.user_agent.as_str())
            .build()
            .into();
        let mut response = agent.get(request.url.as_str()).call().map_err(failed)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(failed)?;
        Ok(Response { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offline_client_never_answers() {
        let request = Request {
            url: "https://example.invalid/".into(),
            user_agent: "atomik-rack/0.0.0".into(),
        };
        assert_eq!(Offline.get(&request), Err(HttpError::Offline));
    }

    #[test]
    fn the_real_client_refuses_a_plain_http_address_before_any_connection() {
        // `https_only` makes the client refuse the address itself: this test opens no socket.
        let request = Request {
            url: "http://localhost:9/".into(),
            user_agent: "atomik-rack/0.0.0".into(),
        };
        assert!(matches!(UreqGet.get(&request), Err(HttpError::Failed(_))));
    }
}
