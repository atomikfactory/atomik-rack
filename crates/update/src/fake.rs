//! A fake client for tests: canned answers, and a record of every request it was asked for.

use crate::http::{HttpError, HttpGet, Request, Response};
use std::sync::Mutex;

/// A client that answers from a script and remembers the requests. Cloning shares nothing: wrap
/// it in an `Arc` to keep a handle for the assertions.
#[derive(Debug)]
pub struct FakeHttp {
    answer: Mutex<Result<Response, HttpError>>,
    seen: Mutex<Vec<Request>>,
}

impl FakeHttp {
    /// A client that answers every request with status 200 and `body`.
    pub fn ok(body: &str) -> Self {
        Self::with(Ok(Response {
            status: 200,
            body: body.as_bytes().to_vec(),
        }))
    }

    /// A client that answers every request with `status` and `body`.
    pub fn status(status: u16, body: &str) -> Self {
        Self::with(Ok(Response {
            status,
            body: body.as_bytes().to_vec(),
        }))
    }

    /// A client whose every request fails (no network, a timeout).
    pub fn failing() -> Self {
        Self::with(Err(HttpError::Failed("connection refused".into())))
    }

    fn with(answer: Result<Response, HttpError>) -> Self {
        Self {
            answer: Mutex::new(answer),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Replaces the answer of the next requests.
    pub fn set_answer(&self, answer: Result<Response, HttpError>) {
        if let Ok(mut slot) = self.answer.lock() {
            *slot = answer;
        }
    }

    /// The requests made so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// How many requests were made.
    pub fn count(&self) -> usize {
        self.requests().len()
    }
}

impl HttpGet for FakeHttp {
    fn get(&self, request: &Request) -> Result<Response, HttpError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request.clone());
        }
        self.answer
            .lock()
            .map_or(Err(HttpError::Offline), |a| a.clone())
    }
}
