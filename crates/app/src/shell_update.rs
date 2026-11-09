//! The update check in the shell: the question of the first start, Help > Check for Updates, the
//! notice of a newer version and what its buttons do.
//!
//! The rules are the ones of [`crate::update_check`]: no request before a Yes, one a day, on a
//! worker, failures silent unless the user asked. The shell adds the words: the question and the
//! news are the notice in the lower left corner ([`UpdateNotice`]); a check by hand that finds
//! nothing, or fails, says so in the status line. Nothing is downloaded or installed: "What's new"
//! only opens the release page in the browser.

use super::*;
use crate::update_notice::{Choice, Notice, NoticeOutcome, UpdateNotice};
use atomik_update::{Consent, HttpGet, Release};

/// The seconds since 1970 now (the clock the daily throttle counts in).
fn unix_now() -> u64 {
    // The throttle is wall-clock by nature; nothing in the engine or a device reads it.
    #[allow(clippy::disallowed_methods)]
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl<B: AudioBackend> Shell<B> {
    /// The answer to "Check for new versions once a day?".
    pub fn update_consent(&self) -> Consent {
        Consent::from_saved(self.workflow.update_check)
    }

    /// The notice of the update check, while it shows.
    pub fn update_notice(&self) -> Option<&UpdateNotice> {
        self.update_notice.as_ref()
    }

    /// Installs the client the update check makes its request with. Only the application window
    /// calls it: a shell without one cannot reach the network.
    pub fn set_update_http(&mut self, http: std::sync::Arc<dyn HttpGet>) {
        self.updater.set_http(http);
    }

    /// The version this build pretends to be (tests).
    #[cfg(test)]
    pub(super) fn pretend_version(&mut self, version: &str) {
        self.updater.pretend(version);
    }

    /// The check of the start (only the application window calls it, once): with no answer yet
    /// the question shows and nothing is asked; after a Yes the daily check runs when a day has
    /// passed; after a No nothing happens.
    pub fn start_update_check(&mut self) {
        match self.update_consent() {
            Consent::Unasked => self.show_notice(Notice::Ask { then_check: false }),
            Consent::Yes => {
                if self.updater.start_automatic(Consent::Yes, unix_now()) {
                    log::debug!("update check: asking for the newest release");
                }
            }
            Consent::No => {}
        }
    }

    /// Help > Check for Updates. While the question is open it asks the question instead of
    /// making a request; otherwise it checks now, whatever the day.
    pub(super) fn check_for_updates_command(&mut self) {
        if self.update_consent() == Consent::Unasked {
            self.show_notice(Notice::Ask { then_check: true });
            return;
        }
        if self.updater.busy() {
            self.notify("Already checking for new versions");
        } else if self.updater.start_manual(unix_now()) {
            self.notify("Checking for new versions...");
        } else {
            self.notify("The check for new versions could not start");
        }
    }

    fn show_notice(&mut self, notice: Notice) {
        self.update_notice = Some(UpdateNotice::new(notice));
        self.repaint = true;
    }

    /// Whether the notice takes events and paints: while nothing modal is open, and over the
    /// start screen (where the question first shows).
    pub(super) fn update_notice_reachable(&self) -> bool {
        self.update_notice.is_some() && (self.start_screen.is_some() || !self.modal_open())
    }

    /// Per-frame work: the answer of the request, and closing a question that was answered in
    /// Settings.
    pub(super) fn poll_updates(&mut self) {
        if matches!(
            self.update_notice.as_ref().map(UpdateNotice::notice),
            Some(Notice::Ask { .. })
        ) && self.update_consent() != Consent::Unasked
        {
            self.update_notice = None;
            self.repaint = true;
        }
        let Some(done) = self.updater.poll() else {
            return;
        };
        match done.result {
            Ok(Some(release)) => {
                if self.updater.offer(&release, done.manual) {
                    let notice = Notice::News {
                        version: release.version.to_string(),
                        running: self.updater.running().to_string(),
                    };
                    self.update_release = Some(release);
                    self.show_notice(notice);
                }
            }
            Ok(None) if done.manual => {
                let text = format!(
                    "Atomik Rack {} is the newest version",
                    self.updater.running()
                );
                self.notify(text);
            }
            Err(_) if done.manual => {
                self.notify("Could not check for new versions; try again later");
            }
            // The daily check says nothing when there is nothing to say or it did not work.
            Ok(None) | Err(_) => {}
        }
    }

    /// An event for the notice: `Some` when it took the event (with the requests it makes).
    pub(super) fn update_notice_event(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Option<Vec<ShellRequest>> {
        if window != WindowKind::Main || !self.update_notice_reachable() || self.capture.is_some() {
            return None;
        }
        let size = self.size();
        let outcome = self.update_notice.as_mut()?.handle(event, size);
        match outcome {
            NoticeOutcome::Pass => None,
            NoticeOutcome::Taken => {
                self.repaint = true;
                Some(Vec::new())
            }
            NoticeOutcome::Chose(choice) => {
                self.repaint = true;
                Some(self.update_choice(choice))
            }
        }
    }

    /// What a button of the notice does.
    fn update_choice(&mut self, choice: Choice) -> Vec<ShellRequest> {
        let then_check = matches!(
            self.update_notice.as_ref().map(UpdateNotice::notice),
            Some(Notice::Ask { then_check: true })
        );
        let release: Option<Release> = self.update_release.clone();
        self.update_notice = None;
        match choice {
            Choice::Yes => {
                self.workflow.update_check = Some(true);
                self.save_workflow();
                // The first Yes also asks now (a check by hand never waits for the day; the
                // question at the start is the daily check itself).
                let started = if then_check {
                    self.updater.start_manual(unix_now())
                } else {
                    self.updater.start_automatic(Consent::Yes, unix_now())
                };
                if started && then_check {
                    self.notify("Checking for new versions...");
                }
                Vec::new()
            }
            Choice::No => {
                self.workflow.update_check = Some(false);
                self.save_workflow();
                Vec::new()
            }
            Choice::WhatsNew => release
                .map(|r| vec![ShellRequest::OpenRelease(r.page())])
                .unwrap_or_default(),
            Choice::Skip => {
                if let Some(release) = release {
                    self.updater.skip(&release);
                    self.notify(format!(
                        "Version {} will not be shown again; a later version will",
                        release.version
                    ));
                }
                Vec::new()
            }
            Choice::Later => Vec::new(),
        }
    }
}
