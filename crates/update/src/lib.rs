//! The update check of Atomik Rack: the only network request the program makes, and only when
//! the user said yes.
//!
//! Once a day at the start (and on Help > Check for Updates) the program may ask the project page
//! for its list of releases. The request is one GET of a fixed address with a generic
//! User-Agent (`atomik-rack/<version>`): no identifier, no cookie, no query, no body, nothing
//! about the user or the machine. The answer is read, compared with the running version
//! ([`Version`]: release candidates come in the right order), and a newer one is offered; nothing
//! is downloaded or installed.
//!
//! The crate is pure except for [`UreqGet`], the real client. Everything goes through the
//! [`HttpGet`] trait, so tests use [`FakeHttp`] and no test reaches the network. The answer to
//! the question, the daily throttle and the skipped version are in [`UpdateState`].

#![warn(missing_docs)]

mod check;
mod fake;
mod http;
mod releases;
mod state;
mod version;

pub use check::{CheckError, RELEASES_URL, check, user_agent};
pub use fake::FakeHttp;
pub use http::{HttpError, HttpGet, MAX_BODY, Offline, Request, Response, TIMEOUT, UreqGet};
pub use releases::{RELEASE_PAGE, Release, ReleaseError, newest};
pub use state::{Consent, DAY_SECS, FILE, UpdateState};
pub use version::{Version, VersionError};
