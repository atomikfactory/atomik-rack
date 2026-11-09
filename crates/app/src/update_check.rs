//! The update check of the shell: when a request is made, on which thread, and what is kept.
//!
//! The rules (the details are in `atomik-update`, which owns the request itself):
//!
//! - Nothing is asked before the user has answered "Check for new versions once a day?" with Yes
//!   ([`Updater::start_automatic`] needs [`Consent::Yes`]; Help > Check for Updates asks the
//!   question first while it is open).
//! - At most one request a day, counted from the last request whether it worked or not.
//! - The request runs on its own short-lived worker, `atomik-update`; the shell polls for the
//!   answer on its frame. A failure is logged at debug level and told to nobody, except when the
//!   user asked for the check by hand.
//! - The client starts as [`Offline`]: only the application window installs the real one
//!   ([`Updater::set_http`]), so no test and no headless session can reach the network.
//! - What is kept (the time of the last request, the version the user skipped) is the small file
//!   `updates.json` next to the settings. It is read again before each write, so two song windows
//!   do not undo each other.

use atomik_update::{
    CheckError, Consent, HttpError, HttpGet, Offline, Release, UpdateState, Version, check,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The version of this build.
pub const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// The answer of one check.
pub struct Finished {
    /// The user asked for it (Help > Check for Updates), as opposed to the daily one at the start.
    pub manual: bool,
    /// The release to offer, `None` when this build is the newest, or why there is no answer.
    pub result: Result<Option<Release>, CheckError>,
}

/// A request in flight.
struct Flight {
    manual: bool,
    inbox: Receiver<Result<Option<Release>, CheckError>>,
}

/// The update check of one shell.
pub struct Updater {
    http: Arc<dyn HttpGet>,
    running: Version,
    state: UpdateState,
    state_path: Option<PathBuf>,
    flight: Option<Flight>,
}

impl Updater {
    /// An updater for this build, keeping its facts in `state_path` (`None`: not saved). The
    /// client is [`Offline`] until [`Self::set_http`].
    pub fn new(state_path: Option<PathBuf>) -> Self {
        let state = state_path
            .as_deref()
            .map(UpdateState::load)
            .unwrap_or_default();
        Self {
            http: Arc::new(Offline),
            // A build whose version cannot be read never offers anything: 0.0.0 is the oldest,
            // so it would offer everything. Use the newest possible instead.
            running: Version::parse(RUNNING).unwrap_or_else(|_| {
                Version::parse("999999.0.0").expect("a constant version parses")
            }),
            state,
            state_path,
            flight: None,
        }
    }

    /// Pretends to be another version (tests).
    #[cfg(test)]
    pub(crate) fn pretend(&mut self, version: &str) {
        self.running = Version::parse(version).expect("a test version");
    }

    /// Installs the client that makes the requests.
    pub fn set_http(&mut self, http: Arc<dyn HttpGet>) {
        self.http = http;
    }

    /// The version this build is.
    pub fn running(&self) -> &Version {
        &self.running
    }

    /// The facts kept.
    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Whether a request is in flight.
    pub fn busy(&self) -> bool {
        self.flight.is_some()
    }

    /// The daily check at the start: makes the request when the answer is Yes, a day has passed
    /// since the last one and none is in flight. Returns whether a request was started.
    pub fn start_automatic(&mut self, consent: Consent, now: u64) -> bool {
        if consent != Consent::Yes {
            return false;
        }
        self.state = self.fresh();
        if !self.state.due(now) {
            return false;
        }
        self.start(false, now)
    }

    /// A check the user asked for: no waiting for the day. The caller has the user's yes (an
    /// answer of Yes, or the click on Yes of the question). Returns whether a request was started.
    pub fn start_manual(&mut self, now: u64) -> bool {
        self.start(true, now)
    }

    fn start(&mut self, manual: bool, now: u64) -> bool {
        if self.flight.is_some() {
            return false;
        }
        let (send, inbox) = channel();
        let http = Arc::clone(&self.http);
        let running = self.running.clone();
        let spawned = std::thread::Builder::new()
            .name("atomik-update".into())
            .spawn(move || {
                // The shell may have gone by the time the answer is there: nobody to tell then.
                let _ = send.send(check(http.as_ref(), &running));
            });
        if let Err(e) = spawned {
            log::debug!("update check: no worker: {e}");
            return false;
        }
        self.remember(|s| s.checked(now));
        self.flight = Some(Flight { manual, inbox });
        true
    }

    /// The answer, once it is there (call it on every frame).
    pub fn poll(&mut self) -> Option<Finished> {
        let flight = self.flight.as_ref()?;
        let result = match flight.inbox.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(CheckError::Http(HttpError::Failed(
                "the worker ended without an answer".into(),
            ))),
        };
        let manual = flight.manual;
        self.flight = None;
        if let Err(e) = &result {
            log::debug!("update check failed: {e}");
        }
        Some(Finished { manual, result })
    }

    /// Whether `release` is shown: always after a check by hand, and after the daily one unless
    /// the user skipped exactly this version.
    pub fn offer(&mut self, release: &Release, manual: bool) -> bool {
        self.state = self.fresh();
        manual || !self.state.skips(&release.version)
    }

    /// Skips `release`: the daily check no longer shows it (a later version still shows).
    pub fn skip(&mut self, release: &Release) {
        self.remember(|s| s.skip(&release.version));
    }

    /// The facts as the file has them now (the shell's own copy when there is no file).
    fn fresh(&self) -> UpdateState {
        self.state_path
            .as_deref()
            .map_or_else(|| self.state.clone(), UpdateState::load)
    }

    /// Changes the facts and writes them (a failed write is logged, never shown).
    fn remember(&mut self, change: impl FnOnce(&mut UpdateState)) {
        let mut state = self.fresh();
        change(&mut state);
        if let Some(path) = &self.state_path
            && let Err(e) = state.save(path)
        {
            log::debug!("update check: could not save {}: {e}", path.display());
        }
        self.state = state;
    }
}

#[cfg(test)]
#[path = "update_check_tests.rs"]
mod tests;
