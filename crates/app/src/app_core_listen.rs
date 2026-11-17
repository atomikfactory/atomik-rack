//! Listen and grade in the app core: the glue between the engine's input tap, the open tab clip and
//! the practice bar. The pure parts are [`crate::listen`] (tracker, clock, passes) and
//! [`atomik_tab::grade`] (the grader); the tracker is [`atomik_dsp::listen`].
//!
//! # Flow
//!
//! * **On** (`practice.listen`): an engine record tap opens on the listening input (one interface
//!   channel, or a pair; [`AppCore::listen_input`]) with a 4 s ring the audio thread only copies
//!   into. Each UI frame ([`AppCore::feed_listen`]) the transport state is noted as an anchor, the
//!   ring is drained, the louder channel goes to the tracker, and finished passes are graded. The
//!   engine has one tap at a time: listening pauses while a take records or the SAMPLE button
//!   records, and starts again after.
//! * **Passes**: one lap of the loop within one run of the transport (the whole run when the loop
//!   is off). The expected notes are the open Tab Editor clip's notes on its track inside the
//!   graded range of the pass ([`crate::listen::graded_range`]). Without an open tab clip nothing
//!   is graded and the trainer counts loop wraps as before.
//! * **Results** (not saved, not in the project): the session ([`atomik_tab::grade::Session`]:
//!   history, streak), and the verdicts of the last graded pass, which the Tab Editor draws over
//!   the notes and the fretboard over the sounding ones. A new clip shows no verdicts.
//! * **Trainer**: while a pass is graded here, the trainer's pass count comes from the grade
//!   ([`PassJudge`] with [`AccuracyJudge`], default 90 %); loop wraps are not counted twice.
//! * **Latency**: a note played at input frame `h` was played against what the engine rendered
//!   `record_offset_frames` earlier (the audio settings' latencies and FIFO, plus their
//!   recording offset), and then the manual offset in milliseconds, which `practice.listen_calibrate`
//!   measures: it turns the click on, plays, and sets the offset from the median distance of the
//!   played notes to the beats. The offset is kept in `ui-state.json` (not in the project).
//!
//! The analysis runs on the UI thread like the tuner's; the audio thread only writes the ring.

use super::*;
use crate::listen::{
    AccuracyJudge, CLEAN_PERCENT_RANGE, CLEAN_PERCENT_STEP, ClosedPass, DEFAULT_CLEAN_PERCENT,
    Listening, MAX_OFFSET_MS, OFFSET_STEP_MS, TransportView, grade_closed,
};
use crate::practice::PracticeAction;
use atomik_core::PPQ;
use atomik_engine::{RecordTap, TapChannel, TapControls};
use atomik_tab::grade::{self, PassGrade, Session};
use atomik_ui::grading::{self, GradeMark};
use atomik_ui::{HISTORY_SHOWN, ListenState};

/// Seconds of input the listening tap's ring holds.
const TAP_SECONDS: usize = 4;
/// Frames between two tries to open the tap when the engine has none.
const RETRY_FRAMES: u32 = 90;
/// The notes a latency calibration waits for.
const CALIBRATION_NOTES: usize = 8;

/// The notes the open tab clip asks for, with the clip's place in the song.struct Expecting {    clip: ClipId,    /// Song tick the clip starts at.    start: i64,    /// Content tick shown at the clip's start.    offset: i64,    /// `(song tick, key)` of every note inside the clip.    notes: Vec<(i64, u8)>,}
/// The notes the open tab clip asks for, with the clip's place in the song.
struct Expecting {
    clip: ClipId,
    /// Song tick the clip starts at.
    start: i64,
    /// Content tick shown at the clip's start.
    offset: i64,
    /// `(song tick, key)` of every note inside the clip.
    notes: Vec<(i64, u8)>,
}

/// A latency calibration in progress.
struct Calibration {
    /// How far each note lay from its beat, milliseconds (positive = late).
    lateness_ms: Vec<f64>,
    /// The click was off before and goes off again.
    restore_click: bool,
    /// The transport was stopped before and stops again.
    restore_stop: bool,
    /// The transport has been seen running (its stop ends the calibration).
    saw_playing: bool,
}

/// The listen-and-grade state of the core.
pub(super) struct Listen {
    on: bool,
    /// The chosen input (`None`: the first the interface offers).
    input: Option<SamplingInput>,
    tap: Option<(RecordTap, SamplingInput)>,
    listening: Option<Listening>,
    planes: Vec<Vec<f32>>,
    session: Session,
    /// The accuracy a clean pass needs, percent.
    threshold: u8,
    offset_ms: i32,
    offset_changed: Option<i32>,
    calibration: Option<Calibration>,
    /// The verdicts of the last graded pass and the clip they belong to.
    grades: Option<(ClipId, Arc<Vec<GradeMark>>)>,
    retry: u32,
    paused_said: bool,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            on: false,
            input: None,
            tap: None,
            listening: None,
            planes: Vec::new(),
            session: Session::default(),
            threshold: DEFAULT_CLEAN_PERCENT,
            offset_ms: 0,
            offset_changed: None,
            calibration: None,
            grades: None,
            retry: 0,
            paused_said: false,
        }
    }
}

/// The inputs listening offers: each channel alone (a guitar is one signal), then the stereo pairs.
pub fn listen_inputs(channels: u16) -> Vec<SamplingInput> {
    let singles = (0..channels).map(|first| SamplingInput { first, mono: true });
    let pairs = (0..channels.saturating_sub(1))
        .step_by(2)
        .map(|first| SamplingInput { first, mono: false });
    singles.chain(pairs).collect()
}

impl<S: EngineSink> AppCore<S> {
    /// Whether listen and grade is on.
    pub fn listen_on(&self) -> bool {
        self.listen.on
    }

    /// The session of graded passes (history, streak).
    pub fn listen_session(&self) -> &Session {
        &self.listen.session
    }

    /// The manual latency offset, milliseconds.
    pub fn listen_offset_ms(&self) -> i32 {
        self.listen.offset_ms
    }

    /// Sets the manual offset (the shell, from the user's saved state).
    pub fn set_listen_offset_ms(&mut self, ms: i32) {
        self.listen.offset_ms = ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
        self.listen.offset_changed = None;
    }

    /// The offset when it was changed since the last call (the shell saves it).
    pub fn take_listen_offset_change(&mut self) -> Option<i32> {
        self.listen.offset_changed.take()
    }

    /// The accuracy a clean pass needs, percent.
    pub fn listen_threshold(&self) -> u8 {
        self.listen.threshold
    }

    /// The input listening uses: the chosen one if the interface still has it, else the first.
    pub fn listen_input(&self) -> Option<SamplingInput> {
        let options = listen_inputs(self.doc().input_channels);
        self.listen
            .input
            .filter(|i| options.contains(i))
            .or_else(|| options.first().copied())
    }

    /// What the practice bar shows of listening.
    pub(super) fn listen_view(&self) -> ListenState {
        let l = &self.listen;
        let mut history = [0u8; HISTORY_SHOWN];
        let passes = l.session.history();
        let shown = &passes[passes.len().saturating_sub(HISTORY_SHOWN)..];
        for (slot, p) in history.iter_mut().zip(shown) {
            *slot = p.percent;
        }
        ListenState {
            on: l.on,
            calibrating: l.calibration.is_some(),
            input: self.listen_input().map(|i| (i.first, i.mono)),
            last_percent: l.session.last().map(|p| p.percent),
            streak: l.session.streak(),
            best: l.session.best_streak(),
            threshold: l.threshold,
            offset_ms: i16::try_from(l.offset_ms).unwrap_or(0),
            history_len: shown.len() as u8,
            history,
        }
    }

    /// Runs a listening command of the practice bar, menu and palette.
    pub(super) fn listen_action(&mut self, action: PracticeAction) {
        match action {
            PracticeAction::ToggleListen => self.set_listening(!self.listen.on),
            PracticeAction::ListenInput => {
                let options = listen_inputs(self.doc().input_channels);
                let Some(current) = self.listen_input() else {
                    return self
                        .reject("The audio interface has no inputs to listen to".to_owned());
                };
                let at = options.iter().position(|o| *o == current).unwrap_or(0);
                let next = options[(at + 1) % options.len()];
                self.listen.input = Some(next);
                self.report(format!("Listening to {}", next.label()));
            }
            PracticeAction::ListenClear => self.clear_listen_results(),
            PracticeAction::ListenOffset(up) => {
                let step = if up { OFFSET_STEP_MS } else { -OFFSET_STEP_MS };
                self.listen.offset_ms =
                    (self.listen.offset_ms + step).clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
                self.listen.offset_changed = Some(self.listen.offset_ms);
                self.report(format!("Listening offset {} ms", self.listen.offset_ms));
            }
            PracticeAction::ListenThreshold(up) => {
                let (lo, hi) = CLEAN_PERCENT_RANGE;
                let t = if up {
                    self.listen.threshold.saturating_add(CLEAN_PERCENT_STEP)
                } else {
                    self.listen.threshold.saturating_sub(CLEAN_PERCENT_STEP)
                };
                self.listen.threshold = t.clamp(lo, hi);
                self.install_listen_judge();
                self.report(format!("A pass is clean from {}%", self.listen.threshold));
            }
            PracticeAction::ListenCalibrate => self.toggle_calibration(),
            _ => {}
        }
    }

    fn install_listen_judge(&mut self) {
        if self.listen.on {
            self.set_pass_judge(Box::new(AccuracyJudge::new(self.listen.threshold)));
        } else {
            self.set_pass_judge(Box::new(crate::practice::EveryPass));
        }
    }

    fn set_listening(&mut self, on: bool) {
        if on == self.listen.on {
            return;
        }
        if on && self.listen_input().is_none() {
            return self.reject("The audio interface has no inputs to listen to".to_owned());
        }
        self.listen.on = on;
        self.listen.calibration = None;
        self.listen.paused_said = false;
        self.listen.retry = 0;
        if !on {
            self.stop_listen_tap();
            self.listen.listening = None;
        }
        self.install_listen_judge();
        self.last_pass_loops = None;
        self.show_transport();
    }

    /// Whether the open tab clip is graded now: listening is on with a tap running and a tab
    /// clip open (the trainer then counts graded passes instead of loop wraps).
    pub(super) fn listen_grades_passes(&self) -> bool {
        self.listen.on && self.listen.listening.is_some() && self.tab_clip.is_some()
    }

    fn clear_listen_results(&mut self) {
        self.listen.session.clear();
        self.listen.grades = None;
        if let Some(l) = &mut self.listen.listening {
            l.forget_passes();
        }
        self.show_transport();
    }

    fn stop_listen_tap(&mut self) {
        if self.listen.tap.take().is_some() {
            self.sink.stop_audio_record();
        }
    }

    /// A take or a sampling needs the engine's tap: listening lets go of it (it starts again
    /// afterwards).
    pub(super) fn listen_yield_tap(&mut self) {
        if self.listen.tap.is_some() {
            self.stop_listen_tap();
            self.listen.listening = None;
        }
    }

    /// The engine was rebuilt: its tap is gone.
    pub(super) fn listen_engine_lost(&mut self) {
        self.listen.tap = None;
        self.listen.listening = None;
        self.listen.retry = 0;
    }

    /// The tap is the engine's only one: whether a take or a sampling holds it.
    fn tap_busy(&self) -> bool {
        self.audio.recording.is_some() || self.audio.rec_pending > 0 || self.sampling_running()
    }

    /// Opens the tap when listening is on and none is running. False while it cannot.
    fn ensure_listen_tap(&mut self) -> bool {
        let Some(input) = self.listen_input() else {
            return false;
        };
        if let Some((_, tapped)) = &self.listen.tap {
            if *tapped == input {
                return true;
            }
            // The input changed: open again on the new one.
            self.stop_listen_tap();
        }
        if self.tap_busy() {
            if !self.listen.paused_said {
                self.listen.paused_said = true;
                self.report("Listening is paused while a take or a sample records");
            }
            return false;
        }
        if self.listen.retry > 0 {
            self.listen.retry -= 1;
            return false;
        }
        let rate = self.doc().sample_rate.max(1) as usize;
        let channels: Vec<TapChannel> = if input.mono {
            vec![TapChannel::host(usize::from(input.first), 0)]
        } else {
            let first = usize::from(input.first);
            vec![TapChannel::host(first, 0), TapChannel::host(first + 1, 0)]
        };
        let controls = Arc::new(TapControls::new(1));
        match self
            .sink
            .start_audio_record(&channels, TAP_SECONDS * rate, controls)
        {
            Some(tap) => {
                self.listen.tap = Some((tap, input));
                self.listen.listening = Some(Listening::new(rate as u32));
                self.listen.paused_said = false;
                true
            }
            None => {
                self.listen.retry = RETRY_FRAMES;
                false
            }
        }
    }

    /// Once a frame while listening: notes the transport, drains the tap into the tracker and
    /// grades the passes that are over (module docs). `state` is the engine's transport state.
    pub(super) fn feed_listen(&mut self, state: &TransportState) {
        if !self.listen.on || !self.ensure_listen_tap() {
            return;
        }
        let view = TransportView {
            playing: state.playing,
            engine_frame: state.engine_frame,
            tick: state.tick,
            loop_count: state.loop_count,
            loop_range: state
                .loop_enabled
                .then_some((state.loop_start, state.loop_end)),
        };
        let mut planes = std::mem::take(&mut self.listen.planes);
        let mut chunks: Vec<(u64, Vec<f32>)> = Vec::new();
        if let Some((tap, _)) = &mut self.listen.tap {
            loop {
                for p in &mut planes {
                    p.clear();
                }
                let Some((first, n)) = tap.drain(&mut planes) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                chunks.push((first, louder(&planes)));
            }
        }
        self.listen.planes = planes;
        self.listen_consume(&view, chunks);
    }

    /// The part of [`Self::feed_listen`] after the tap: the transport view and the mono input
    /// chunks `(first frame, samples)` drained this frame.
    pub(super) fn listen_consume(&mut self, view: &TransportView, chunks: Vec<(u64, Vec<f32>)>) {
        let comp = self.sink.record_offset_frames();
        let speed = self.practice.speed();
        let offset = self.listen.offset_ms;
        let mut located = Vec::new();
        let closed = {
            let Some(l) = &mut self.listen.listening else {
                return;
            };
            l.set_compensation(comp, offset);
            l.anchor(view, speed);
            for (first, mono) in &chunks {
                located.extend(l.samples(*first, mono));
            }
            l.take_closed()
        };
        if self.listen.calibration.is_some() {
            self.calibrate_with(&located, view.playing);
            return;
        }
        for pass in closed {
            self.grade_pass(&pass);
        }
    }

    /// The notes the open tab clip asks for.
    fn expected_notes(&self) -> Option<Expecting> {
        let id = self.tab_clip?;
        let (track, clip) = self.doc().song.clip_or_take(id)?;
        let (from, to) = (clip.start, clip.start + clip.length);
        let notes = atomik_model::song::flatten_notes(track)
            .into_iter()
            .filter(|(tick, ..)| (from..to).contains(tick))
            .map(|(tick, _, key, _)| (tick, key))
            .collect();
        Some(Expecting {
            clip: id,
            start: clip.start,
            offset: clip.offset,
            notes,
        })
    }

    fn grade_pass(&mut self, pass: &ClosedPass) {
        let Some(Expecting {
            clip,
            start,
            offset,
            notes,
        }) = self.expected_notes()
        else {
            return;
        };
        let graded = grade_closed(pass, &notes, grade::DEFAULT_WINDOW_MS);
        let accuracy = graded.as_ref().and_then(|(g, _)| g.accuracy());
        if let Some((g, _)) = &graded {
            self.listen.session.record(g, pass.speed);
            let marks = marks_of(g, start, offset);
            self.listen.grades = Some((clip, Arc::new(grading::sorted(marks))));
        }
        if self
            .practice
            .pass_graded(self.pass_judge.as_mut(), accuracy)
            .is_some()
        {
            self.send_practice_speed();
        }
        self.show_transport();
    }

    /// Hands the tab editor the verdicts of the last graded pass (when they are of its clip).
    pub(super) fn push_listen_grades(&mut self) {
        let marks = match (&self.listen.grades, self.tab_clip) {
            (Some((clip, marks)), Some(open)) if *clip == open => Some(Arc::clone(marks)),
            _ => None,
        };
        self.tab_editor.set_grades(marks);
    }

    fn toggle_calibration(&mut self) {
        if let Some(c) = self.listen.calibration.take() {
            // A second press ends it with what was heard.
            self.listen.calibration = Some(c);
            return self.finish_calibration();
        }
        if !self.listen.on {
            self.set_listening(true);
            if !self.listen.on {
                return;
            }
        }
        let restore_click = !self.metronome;
        if restore_click {
            self.transport_action(TransportAction::ToggleMetronome);
        }
        let restore_stop = !self.playing;
        if restore_stop {
            self.transport_action(TransportAction::Play);
        }
        self.listen.calibration = Some(Calibration {
            lateness_ms: Vec::new(),
            restore_click,
            restore_stop,
            saw_playing: false,
        });
        self.report(format!(
            "Play along the click: {CALIBRATION_NOTES} notes set the latency offset"
        ));
        self.show_transport();
    }

    /// Takes the notes heard during a calibration: each one's distance to the beat. It ends with
    /// enough notes, or when the transport that was running stops.
    fn calibrate_with(&mut self, located: &[crate::listen::Located], playing: bool) {
        let Some(c) = &mut self.listen.calibration else {
            return;
        };
        for n in located {
            if n.ms_per_tick <= 0.0 {
                continue;
            }
            let ticks = grade::beat_offsets(&[n.played.tick], PPQ as f64);
            if let Some(t) = ticks.first() {
                c.lateness_ms.push(t * n.ms_per_tick);
            }
        }
        c.saw_playing |= playing;
        if c.lateness_ms.len() >= CALIBRATION_NOTES || (c.saw_playing && !playing) {
            self.finish_calibration();
        }
    }

    fn finish_calibration(&mut self) {
        let Some(c) = self.listen.calibration.take() else {
            return;
        };
        // What was played along the click is not a pass to grade.
        if let Some(l) = &mut self.listen.listening {
            l.forget_passes();
        }
        if c.restore_click && self.metronome {
            self.transport_action(TransportAction::ToggleMetronome);
        }
        if c.restore_stop && self.playing {
            self.transport_action(TransportAction::Stop);
        }
        match grade::calibration_offset_ms(&c.lateness_ms) {
            Some(correction) => {
                let ms = (f64::from(self.listen.offset_ms) + correction)
                    .round()
                    .clamp(-f64::from(MAX_OFFSET_MS), f64::from(MAX_OFFSET_MS))
                    as i32;
                self.listen.offset_ms = ms;
                self.listen.offset_changed = Some(ms);
                self.report(format!("Latency offset set to {ms} ms"));
            }
            None => self.report(
                "Not enough notes heard: play along the click on the listening input, then try again",
            ),
        }
        self.show_transport();
    }
}

/// The louder of the two channels of a drain (a mono source may sit on either input), or the
/// only one.
fn louder(planes: &[Vec<f32>]) -> Vec<f32> {
    match planes {
        [] => Vec::new(),
        [only] => only.clone(),
        [a, b, ..] => {
            if atomik_dsp::tuner::rms(a) >= atomik_dsp::tuner::rms(b) {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// The marks of a graded pass: one per key of each group, at the clip's content tick.
fn marks_of(grade: &PassGrade, clip_start: i64, clip_offset: i64) -> Vec<GradeMark> {
    grade
        .notes
        .iter()
        .flat_map(|n| {
            n.expected.keys.iter().map(move |key| GradeMark {
                tick: n.expected.tick - clip_start + clip_offset,
                key: *key,
                verdict: n.verdict,
            })
        })
        .collect()
}

#[cfg(test)]
impl<S: EngineSink> AppCore<S> {
    /// Tests: a listener without a tap (the recording sink has no engine to tap), fed by
    /// [`Self::listen_consume`].
    pub(super) fn listen_attach_for_test(&mut self, rate: u32) {
        self.listen.on = true;
        self.listen.listening = Some(Listening::new(rate));
        self.install_listen_judge();
    }
}
