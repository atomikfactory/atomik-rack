//! The audio and MIDI setup in the shell: when it opens, what its buttons do, the tone it plays
//! and how it closes.
//!
//! It opens by itself once, at the first start (no `settings.json` yet), before the start
//! screen, and whenever Options > Audio and MIDI Setup... asks. Each choice is applied at once
//! through the path Audio Settings uses (so the stream restarts on the new device and the meters
//! and the latency tell the truth) but nothing is written until Skip or Done: those write
//! `settings.json`, which is also what keeps the setup from opening by itself again. The update
//! check consent waits behind it (see `update_notice_reachable`).

use super::*;
use crate::audio_setup::{AudioSetup, TONE_RATE, chord_tone, key_tone};
use crate::audio_setup_view::SetupOutcome;
use crate::sink::EngineSink;

impl<B: AudioBackend> Shell<B> {
    /// The audio and MIDI setup, while it shows.
    pub fn audio_setup(&self) -> Option<&AudioSetup> {
        self.audio_setup.as_ref()
    }

    /// The start of the application window calls it once, before the start screen: on the first
    /// start it opens the setup and returns true. Later starts (a settings file exists), the
    /// headless sessions and the song windows never open it.
    pub fn first_run_setup(&mut self) -> bool {
        if !std::mem::take(&mut self.first_run) {
            return false;
        }
        self.open_audio_setup();
        self.audio_setup.is_some()
    }

    /// Options > Audio and MIDI Setup...: opens the panel on its first page (it stays open when it
    /// is open already).
    pub fn open_audio_setup(&mut self) {
        if self.audio_setup.is_some() {
            return;
        }
        let session = self.core.sink();
        let lists = DeviceLists::query(session.backend());
        // The tune belongs to the open project; the panel leaves it as it is.
        let mut current = session.settings().clone();
        current.master_tune_cents = self.core.doc().tune_cents;
        let mut setup = AudioSetup::new(current, lists);
        setup.start_listening(session.note_activity());
        self.audio_setup = Some(setup);
        self.close_menubars();
        self.sync_audio_setup();
        self.repaint = true;
    }

    /// Per-frame work of the panel: the meters, the latency, the lamp and the tone of a key.
    pub(super) fn sync_audio_setup(&mut self) {
        let Some(setup) = &mut self.audio_setup else {
            return;
        };
        let session = self.core.sink();
        let update = setup.update(
            &self.audio_status,
            session.io_peaks(),
            session.note_activity(),
        );
        if update.changed {
            self.repaint = true;
        }
        if let Some(key) = update.new_key {
            self.play_setup_tone(key_tone(key));
        }
    }

    /// Plays `samples` (mono at [`TONE_RATE`]) through the preview voice of the engine, the way
    /// the file browser plays a file. Without an engine nothing plays (the panel already says why).
    fn play_setup_tone(&mut self, samples: Vec<f32>) {
        if !self.audio_status.running {
            return;
        }
        let (mut feed, stream) = atomik_engine::preview_channel();
        feed.set_source_rate(TONE_RATE);
        feed.push(&[samples], 0);
        feed.finish();
        if let Err(e) = self.core.sink_mut().preview_start(stream, 1.0) {
            log::warn!("audio setup: the tone did not start: {e}");
        }
    }

    /// An event for the panel: it is modal and takes them all.
    pub(super) fn audio_setup_event(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        let size = self.size();
        let Some(setup) = &mut self.audio_setup else {
            return Vec::new();
        };
        match setup.handle(event, size) {
            SetupOutcome::Open => Vec::new(),
            SetupOutcome::Pick(choice) => {
                setup.choose(choice);
                let draft = setup.draft().clone();
                self.apply_settings_saving(draft, false);
                self.sync_audio_setup();
                Vec::new()
            }
            SetupOutcome::Test => {
                self.play_setup_tone(chord_tone());
                Vec::new()
            }
            SetupOutcome::TurnOnInput => {
                let requests = self.execute(AppCommand::InputChannels(2));
                self.poll_status();
                self.sync_audio_setup();
                requests
            }
            SetupOutcome::SetUpController => {
                self.open_controller_wizard();
                Vec::new()
            }
            SetupOutcome::Skip | SetupOutcome::Done => {
                self.finish_audio_setup();
                Vec::new()
            }
        }
    }

    /// Closes the panel: the settings are written (what was chosen, or the devices that opened
    /// before when the last choice did not open), so the setup never shows by itself again, and
    /// the start screen that waited for it opens.
    fn finish_audio_setup(&mut self) {
        let Some(setup) = self.audio_setup.take() else {
            return;
        };
        let mut to_save = self.core.sink().settings().clone();
        if !self.audio_status.running && self.audio_status.error.is_some() {
            to_save.use_devices(&LastGood::of(setup.opened_with()));
        }
        if let Some(path) = &self.settings_path
            && let Err(e) = settings::save(path, &to_save)
        {
            self.notify(format!("settings not saved: {e}"));
        }
        self.first_run = false;
        // The preview voice fades the tone out; a tone still playing is cut.
        let _ = self.core.sink_mut().preview_stop();
        if std::mem::take(&mut self.start_screen_waiting) {
            self.open_start_screen();
        }
        self.repaint = true;
    }
}
