//! The Chord Trainer pane in the app core.
//!
//! [`Pane::ChordTrainer`](crate::Pane::ChordTrainer) shows [`ChordTrainerView`] in place of the
//! arrangement, as the chord sheet does, and opens from View > Editors > Chord Trainer
//! (`view.toggle_chord_trainer`). It is a pane, not a detachable window: it needs no clip and no
//! platform window of its own, and it sits where the Tab Editor and the Chord Sheet sit, so the
//! three switch with one click. The view keeps the setup and the drill; this file is the glue
//! with the rest of the app:
//!
//! * **The clock.** [`AppCore::chord_trainer_frame`] runs every frame (from
//!   [`AppCore::transport_frame`]) with the frame's time and ticks the view. The drill does not
//!   depend on the song transport: it is never started or stopped by it, and it runs while the
//!   song is stopped.
//! * **The click.** A timed drill clicks through the engine's preview voice
//!   ([`crate::trainer_click`]): one continuous stream of samples, so the beats keep the audio
//!   clock. It starts with the drill and stops with it (a new session, a changed tempo, the end,
//!   closing the pane); the transport's own click is untouched.
//! * **Listening.** A drill that listens shares the engine's one input tap with listen and grade
//!   (`feed_listen` drains it once a frame): the chunks go to the trainer's own
//!   [`Listener`], whose onsets (with their chroma) are placed on the frame clock, less the
//!   latency compensation, and handed to the view, which matches the chroma against the chord.
//!   The input is the practice bar's (`practice.listen_input`), and the tap is let go when
//!   neither listening needs it.
//! * **Scores.** The best scores per chord pair are user settings (`ui-state.json`), not the
//!   project: the shell hands them in at startup and saves what
//!   [`AppCore::take_trainer_scores_change`] reports. Nothing the pane does touches the song.

use super::*;
use crate::trainer_click::ClickStream;
use atomik_dsp::listen::{Listener, PlayedNote};
use atomik_tab::trainer::Scores;

/// The level of the click in the preview voice.
const CLICK_GAIN: f32 = 0.7;

/// What the trainer holds of the audio side.
#[derive(Default)]
pub(super) struct TrainerAudio {
    /// The click stream and the session of the drill it was started for.
    click: Option<(ClickStream, u64)>,
    /// The tracker of the input.
    listener: Option<Listener>,
}

impl<S: EngineSink> AppCore<S> {
    /// The chord trainer view.
    pub fn chord_trainer(&self) -> &ChordTrainerView {
        &self.chord_trainer
    }

    /// The chord trainer view, mutably (bounds, scale, focus).
    pub fn chord_trainer_mut(&mut self) -> &mut ChordTrainerView {
        &mut self.chord_trainer
    }

    /// Whether the chord trainer pane is open.
    pub fn chord_trainer_open(&self) -> bool {
        self.trainer_open
    }

    /// Opens the chord trainer pane. The other editors close.
    pub fn open_chord_trainer(&mut self) {
        self.close_piano_roll();
        self.close_chord_sheet();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_audio_editor();
        self.close_seq_editor();
        self.close_container_editor();
        self.trainer_open = true;
        let label = self.listen_input().map(|i| i.label());
        self.chord_trainer.set_input(label);
        self.chord_trainer.sync(self.document.doc(), &self.registry);
    }

    /// Closes the chord trainer pane (the arrangement shows again). A running drill stops.
    pub fn close_chord_trainer(&mut self) {
        if !self.trainer_open {
            return;
        }
        self.chord_trainer.leave();
        self.trainer_open = false;
        self.trainer_release();
    }

    /// Hands the view the best scores saved with the user's settings (not a change to save).
    pub fn set_trainer_scores(&mut self, scores: Scores) {
        self.chord_trainer.set_scores(scores);
    }

    /// Seeds the random order of the drills (the shell, once per run).
    pub fn set_trainer_seed(&mut self, seed: u64) {
        self.chord_trainer.set_seed(seed);
    }

    /// The best scores when a session or a reset changed them since the last call.
    pub fn take_trainer_scores_change(&mut self) -> Option<Scores> {
        self.chord_trainer.take_scores_change()
    }

    /// Per-frame work of the trainer at `now` (seconds, the frame clock): names the input, ticks
    /// the drill and keeps the click and the input in step with it.
    pub(super) fn chord_trainer_frame(&mut self, now: f64) {
        if !self.trainer_open {
            return;
        }
        let label = self.listen_input().map(|i| i.label());
        self.chord_trainer.set_input(label);
        self.chord_trainer.tick(now);
        // The click is the stream's: the beats and changes only drive the display.
        self.chord_trainer.take_events();
        self.trainer_click_sync();
        if !self.chord_trainer.wants_input() && self.trainer_audio.listener.is_some() {
            self.trainer_audio.listener = None;
            self.trainer_release_tap();
        }
    }

    /// Whether a chord drill needs the frame clock (running, or about to start): the platform keeps
    /// ticking frames while this is true, audio stream or not.
    pub fn chord_trainer_active(&self) -> bool {
        self.trainer_open && self.chord_trainer.wants_frames()
    }

    /// Whether the click stream of the drill is running.
    pub fn trainer_click_running(&self) -> bool {
        self.trainer_audio.click.is_some()
    }

    /// Whether the drill listens to the input now (the tap must run).
    pub(super) fn trainer_wants_input(&self) -> bool {
        self.trainer_open && self.chord_trainer.wants_input()
    }

    /// Starts, restarts or stops the click stream to match the drill.
    fn trainer_click_sync(&mut self) {
        let want = self.chord_trainer.wants_click();
        let session = self.chord_trainer.session();
        let current = self.trainer_audio.click.as_ref().map(|(_, s)| *s);
        match (current, want) {
            (Some(s), true) if s == session => {}
            (_, true) => {
                self.stop_trainer_click();
                self.start_trainer_click(session);
            }
            (Some(_), false) => self.stop_trainer_click(),
            (None, false) => {}
        }
    }

    fn start_trainer_click(&mut self, session: u64) {
        let cfg = *self.chord_trainer.config();
        let (feed, stream) = atomik_engine::preview_channel();
        let Some(click) = ClickStream::start(feed, cfg.bpm, cfg.beats) else {
            return self.report("The chord trainer click could not start");
        };
        match self.sink.preview_start(stream, CLICK_GAIN) {
            Ok(()) => self.trainer_audio.click = Some((click, session)),
            Err(e) => log::warn!("chord trainer: the click did not start: {e}"),
        }
    }

    fn stop_trainer_click(&mut self) {
        if self.trainer_audio.click.take().is_some() {
            let _ = self.sink.preview_stop();
        }
    }

    /// Lets go of the click, the tracker and the input tap.
    fn trainer_release(&mut self) {
        self.stop_trainer_click();
        self.trainer_audio.listener = None;
        self.trainer_release_tap();
    }

    fn trainer_release_tap(&mut self) {
        if !self.listen_on() {
            self.listen_yield_tap();
        }
    }

    /// The mono input chunks `(first frame, samples)` the tap gave this frame, for the drill.
    pub(super) fn trainer_consume(&mut self, chunks: &[(u64, Vec<f32>)]) {
        let rate = self.doc().sample_rate.max(1);
        let listener = self
            .trainer_audio
            .listener
            .get_or_insert_with(|| Listener::new(rate));
        let mut notes: Vec<(PlayedNote, u64)> = Vec::new();
        for (first, mono) in chunks {
            let end = first + mono.len() as u64;
            notes.extend(listener.feed(*first, mono).into_iter().map(|n| (n, end)));
        }
        for (note, end) in notes {
            self.trainer_heard(&note, end);
        }
    }

    /// Places one note of the tracker on the frame clock and gives it to the drill. `end` is the
    /// last input frame heard when it came out; the note's own frame lies before it.
    pub(super) fn trainer_heard(&mut self, note: &PlayedNote, end: u64) {
        let rate = f64::from(self.doc().sample_rate.max(1));
        let behind = end.saturating_sub(note.frame) as f64 / rate;
        let comp = self.sink.record_offset_frames() as f64 / rate
            + f64::from(self.listen_offset_ms()) / 1000.0;
        self.chord_trainer
            .hear(self.now_seconds - behind - comp, note.chroma);
    }
}
