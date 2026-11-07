//! Reference tracks: comparing the mix with finished songs.
//!
//! A song keeps up to [`atomik_model::MAX_REFERENCES`] reference files ([`RackDoc::references`]);
//! the list is edited with one undo step per change. One of them is *selected*. The reference key
//! ([`AppCore::toggle_reference`]) switches between the mix and the selected reference with a
//! 30 ms crossfade, without a click, and works on a stopped song too.
//!
//! # Where it plays
//!
//! The reference is a stream into the engine's reference voice (`atomik_engine::reference`): it
//! plays after the master chain on interface outputs 1 and 2, so none of the user's master
//! processing touches it, and live only: a bounce, an export, a freeze or an offline render never
//! gets one. The file is decoded and measured once on a worker (`atomik-reference`) when it is
//! selected; a feeder thread then fills the stream's ring from the position the switch starts at.
//!
//! # Where it starts
//!
//! A reference with a cue ([`atomik_model::Reference::cue_ms`]) always starts at its cue, so the
//! chorus of the reference can be set against the chorus of the mix. A reference without one
//! follows the playhead: it starts at the file position equal to the song position in seconds. The
//! start is taken when the key is pressed; the reference does not follow later seeks.
//!
//! # Level match
//!
//! While any reference exists the engine measures the mix (K-weighted short-term loudness, 3 s,
//! before the reference voice). The reference's gain is the difference between that and the
//! short-term loudness of the *loudest section* of the reference (the maximum over its file, see
//! [`measure_loudest`]), so the two sit at the same level in their strong parts. The gain is
//! updated slowly while the mix plays: at most 0.5 dB every quarter of a second, the first
//! reading at once, and only from readings above -60 LUFS (a stopped song keeps the last gain).
//! The gain is shown ([`ReferenceInfo::gain_db`]); it is never stored in the project.
//!
//! # Band and side solo
//!
//! [`AppCore::set_reference_band`] and [`AppCore::set_reference_solo`] limit the mix and the
//! reference to the same band and stereo part; the engine applies identical filters to both.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_audio_file::{AudioData, decode_file_with};
use atomik_devices::mastering::loudness::LoudnessMeter;
use atomik_engine::{
    PreviewFeed, ReferenceBand, ReferenceSettings, ReferenceSolo, preview_channel,
};
use atomik_model::audio_trim::TempoMap;
use atomik_model::{Command, MAX_REFERENCES, Reference};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// Largest reference held in memory, in samples over all channels (about 15 minutes of stereo
/// at 96 kHz).
const MAX_REFERENCE_SAMPLES: u64 = 180_000_000;
/// UI frames between two level-match updates (a quarter of a second at 60 frames a second).
const MATCH_EVERY_FRAMES: u32 = 15;
/// The largest step of the matched gain per update, in dB.
const MATCH_STEP_DB: f32 = 0.5;
/// Mix readings at or below this are silence and never move the gain.
const AUDIBLE_LUFS: f32 = -60.0;
/// The matched gain stays within these dB.
const GAIN_RANGE_DB: (f32, f32) = (-40.0, 20.0);

/// A reference file after decoding and measuring.
struct Loaded {
    path: PathBuf,
    audio: Arc<AudioData>,
    /// Short-term loudness of its loudest section in LUFS (`None` for a silent file).
    loudest: Option<f32>,
}

/// A decode that runs on the worker.
struct Loading {
    path: PathBuf,
    result: Receiver<Result<Loaded, String>>,
}

/// The reference session state of one song (not saved).
pub(crate) struct ReferenceState {
    selected: usize,
    listening: bool,
    band: ReferenceBand,
    solo: ReferenceSolo,
    level_match: bool,
    loaded: Option<Loaded>,
    loading: Option<Loading>,
    error: Option<String>,
    /// The gain the voice was told, in dB; `None` until the first reading.
    gain_db: Option<f32>,
    /// The mix reading the gain was last set from.
    mix_lufs: Option<f32>,
    frames: u32,
    /// The settings the engine holds, to send only changes.
    sent: Option<ReferenceSettings>,
}

impl ReferenceState {
    pub(crate) fn new() -> Self {
        Self {
            selected: 0,
            listening: false,
            band: ReferenceBand::Full,
            solo: ReferenceSolo::Off,
            level_match: true,
            loaded: None,
            loading: None,
            error: None,
            gain_db: None,
            mix_lufs: None,
            frames: 0,
            // A new engine holds the default settings: only a change is ever sent.
            sent: Some(ReferenceSettings::default()),
        }
    }
}

/// What the Reference panel shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceInfo {
    /// The files, as listed.
    pub items: Vec<Reference>,
    /// The selected one.
    pub selected: usize,
    /// True while the reference is what is heard.
    pub listening: bool,
    /// True while the selected file is being decoded and measured.
    pub loading: bool,
    /// The loudness of the loudest section of the selected file, once measured.
    pub loudest_lufs: Option<f32>,
    /// The mix's short-term loudness the gain was set from.
    pub mix_lufs: Option<f32>,
    /// The matched gain in dB (`None` before the first reading, or with the match off).
    pub gain_db: Option<f32>,
    /// Whether the level match is on.
    pub level_match: bool,
    /// The band solo.
    pub band: ReferenceBand,
    /// The stereo solo.
    pub solo: ReferenceSolo,
    /// Why the selected file could not be used, if so.
    pub error: Option<String>,
}

/// The short-term loudness of the loudest section of `audio`: the maximum of the K-weighted 3 s
/// window (every 100 ms) over the file, from the first full window on. A file shorter than 3 s
/// has no full window and reads its gated integrated loudness. `None` for silence.
pub fn measure_loudest(audio: &AudioData) -> Option<f32> {
    let left = audio.channels.first()?;
    let right = audio.channels.get(1).unwrap_or(left);
    let rate = f64::from(audio.sample_rate);
    let mut meter = LoudnessMeter::new(rate);
    let window = (rate * 3.0) as usize;
    let chunk = 4_096;
    let mut best: Option<f32> = None;
    let mut done = 0;
    while done < left.len() {
        let end = (done + chunk).min(left.len());
        meter.process(&left[done..end], &right[done..end]);
        done = end;
        if done >= window {
            let st = meter.short_term();
            if st > -70.0 && best.is_none_or(|b| st > b) {
                best = Some(st);
            }
        }
    }
    best.or_else(|| {
        let i = meter.integrated();
        (i > -70.0).then_some(i)
    })
}

/// Decodes and measures `path` (on the worker).
fn load_reference(path: &Path) -> Result<Loaded, String> {
    let decoded = decode_file_with(path, MAX_REFERENCE_SAMPLES).map_err(|e| e.to_string())?;
    let audio = decoded.audio;
    if audio.channels.first().is_none_or(Vec::is_empty) {
        return Err("the file has no audio".to_owned());
    }
    let loudest = measure_loudest(&audio);
    Ok(Loaded {
        path: path.to_path_buf(),
        audio: Arc::new(audio),
        loudest,
    })
}

/// Fills `feed` from `audio` starting at frame `from`, until the file ends or the engine lets go
/// of the stream.
fn feeder(audio: &Arc<AudioData>, mut from: usize, mut feed: PreviewFeed) {
    let len = audio.channels.first().map_or(0, Vec::len);
    while from < len {
        if feed.is_abandoned() {
            return;
        }
        let n = feed.push(&audio.channels, from);
        from += n;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    feed.finish();
}

/// The file frame a switch to the reference starts from: the cue when there is one, the song
/// position in seconds otherwise, never past the end of the file.
fn start_frame(cue_ms: Option<u64>, playhead_seconds: f64, rate: f64, len: usize) -> usize {
    let seconds = match cue_ms {
        Some(ms) => ms as f64 / 1000.0,
        None => playhead_seconds,
    };
    ((seconds.max(0.0) * rate).round() as usize).min(len)
}

fn linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

impl<S: EngineSink> AppCore<S> {
    /// The reference tracks of the song and the state of the session.
    pub fn reference_info(&self) -> ReferenceInfo {
        let st = &self.reference;
        let items = self.doc().references.clone();
        ReferenceInfo {
            selected: st.selected.min(items.len().saturating_sub(1)),
            items,
            listening: st.listening,
            loading: st.loading.is_some(),
            loudest_lufs: self.selected_loaded().and_then(|l| l.loudest),
            mix_lufs: st.mix_lufs,
            gain_db: if st.level_match { st.gain_db } else { None },
            level_match: st.level_match,
            band: st.band,
            solo: st.solo,
            error: st.error.clone(),
        }
    }

    /// What the Reference submenu of the Mixer menu shows.
    pub fn reference_menu(&self) -> crate::commands::ReferenceMenu {
        let info = self.reference_info();
        crate::commands::ReferenceMenu {
            count: info.items.len() as u8,
            selected: info.selected as u8,
            listening: info.listening,
            level_match: info.level_match,
            has_cue: info
                .items
                .get(info.selected)
                .is_some_and(|r| r.cue_ms.is_some()),
            band: info.band,
            solo: info.solo,
        }
    }

    /// True while the reference is what is heard.
    pub fn reference_listening(&self) -> bool {
        self.reference.listening
    }

    fn selected_reference(&self) -> Option<Reference> {
        self.doc().references.get(self.reference.selected).cloned()
    }

    fn selected_loaded(&self) -> Option<&Loaded> {
        let wanted = self.selected_reference()?;
        self.reference
            .loaded
            .as_ref()
            .filter(|l| l.path == Path::new(&wanted.path))
    }

    /// Adds `path` to the song's reference tracks, selects it and starts decoding it. One undo
    /// step.
    ///
    /// # Errors
    /// The model's refusal: the list is full, the file is listed already, or the path is not
    /// acceptable.
    pub fn add_reference(&mut self, path: &Path) -> Result<(), atomik_model::ModelError> {
        let mut list = self.doc().references.clone();
        list.push(Reference::new(path.to_string_lossy().into_owned()));
        let at = list.len() - 1;
        let name = list[at].name().to_owned();
        self.apply_command(Command::SetReferences(list))?;
        self.reference.selected = at;
        self.reference_forget_audio();
        self.begin_reference_load();
        self.report(format!(
            "Reference track {name} added ({} of {MAX_REFERENCES})",
            at + 1
        ));
        Ok(())
    }

    /// Removes reference `index` from the list. One undo step. A reference that is playing
    /// stops first.
    pub fn remove_reference(&mut self, index: usize) {
        let mut list = self.doc().references.clone();
        if index >= list.len() {
            return;
        }
        if self.reference.listening && index == self.reference.selected {
            self.set_reference_listening(false);
        }
        let gone = list.remove(index);
        if self.apply_command(Command::SetReferences(list)).is_err() {
            return;
        }
        let count = self.doc().references.len();
        if index < self.reference.selected || self.reference.selected >= count {
            self.reference.selected = self.reference.selected.saturating_sub(1);
        }
        self.reference_forget_audio();
        if count > 0 {
            self.begin_reference_load();
        }
        self.report(format!(
            "Reference track {} removed",
            Reference::name(&gone)
        ));
    }

    /// Selects reference `index` and starts decoding it; a reference that was playing stops.
    pub fn select_reference(&mut self, index: usize) {
        if index >= self.doc().references.len() || index == self.reference.selected {
            return;
        }
        if self.reference.listening {
            self.set_reference_listening(false);
        }
        self.reference.selected = index;
        self.reference_forget_audio();
        self.begin_reference_load();
    }

    /// Selects the next reference in the list (the first after the last).
    pub fn next_reference(&mut self) {
        let count = self.doc().references.len();
        if count > 1 {
            self.select_reference((self.reference.selected + 1) % count);
        }
    }

    /// Gives reference `index` the song position as its own cue (`from_playhead`), or takes the
    /// cue away so that it follows the playhead again. One undo step.
    pub fn set_reference_cue(&mut self, index: usize, from_playhead: bool) {
        let mut list = self.doc().references.clone();
        let Some(r) = list.get_mut(index) else { return };
        r.cue_ms = from_playhead.then(|| self.playhead_ms());
        let message = match r.cue_ms {
            Some(ms) => format!(
                "Reference {} starts at {}.{:03} s",
                r.name(),
                ms / 1000,
                ms % 1000
            ),
            None => format!("Reference {} follows the playhead", r.name()),
        };
        if self.apply_command(Command::SetReferences(list)).is_ok() {
            self.report(message);
        }
    }

    /// Turns the level match on or off (off: the reference plays at its own level).
    pub fn set_reference_level_match(&mut self, on: bool) {
        self.reference.level_match = on;
        self.push_reference_settings();
    }

    /// Sets the band both signals are limited to.
    pub fn set_reference_band(&mut self, band: ReferenceBand) {
        self.reference.band = band;
        self.push_reference_settings();
    }

    /// Sets the stereo part both signals are limited to.
    pub fn set_reference_solo(&mut self, solo: ReferenceSolo) {
        self.reference.solo = solo;
        self.push_reference_settings();
    }

    /// The reference key: the mix when the reference is heard, the reference otherwise.
    pub fn toggle_reference(&mut self) {
        self.set_reference_listening(!self.reference.listening);
    }

    /// Switches between the mix (`false`) and the selected reference (`true`).
    pub fn set_reference_listening(&mut self, on: bool) {
        if !on {
            if self.reference.listening {
                self.reference.listening = false;
                self.push_reference_settings();
                self.report("Listening to the mix");
            }
            return;
        }
        if self.reference.listening {
            return;
        }
        let Some(wanted) = self.selected_reference() else {
            self.report("Add a reference track first");
            return;
        };
        let Some(loaded) = self.selected_loaded() else {
            if self.reference.loading.is_none() && self.reference.error.is_none() {
                self.begin_reference_load();
            }
            let message = match &self.reference.error {
                Some(e) => format!("The reference {} cannot be used: {e}", wanted.name()),
                None => format!("The reference {} is still loading", wanted.name()),
            };
            self.report(message);
            return;
        };
        let audio = Arc::clone(&loaded.audio);
        let len = audio.channels.first().map_or(0, Vec::len);
        let rate = f64::from(audio.sample_rate);
        let from = start_frame(wanted.cue_ms, self.playhead_seconds(), rate, len);
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(audio.sample_rate);
        // The first part is in the ring before the engine sees the stream: no gap at the start.
        let pushed = feed.push(&audio.channels, from);
        self.reference.listening = true;
        let settings = self.reference_settings();
        if let Err(e) = self.sink.reference_start(stream, settings) {
            self.reference.listening = false;
            self.report(format!("reference: {e}"));
            return;
        }
        self.reference.sent = Some(settings);
        let next = from + pushed;
        if next >= len {
            feed.finish();
            if from >= len {
                self.report(format!(
                    "The reference {} ends before this position",
                    wanted.name()
                ));
            }
        } else if std::thread::Builder::new()
            .name("atomik-reference-feed".into())
            .spawn(move || feeder(&audio, next, feed))
            .is_err()
        {
            log::warn!("reference: the feeder thread could not start");
        }
        let gain = match (self.reference.level_match, self.reference.gain_db) {
            (true, Some(g)) => format!(", matched {g:+.1} dB"),
            _ => String::new(),
        };
        self.report(format!(
            "Listening to the reference {}{gain}",
            wanted.name()
        ));
    }

    /// The settings the engine voice should hold now.
    fn reference_settings(&self) -> ReferenceSettings {
        let st = &self.reference;
        let gain_db = if st.level_match {
            st.gain_db.unwrap_or(0.0)
        } else {
            0.0
        };
        ReferenceSettings {
            listening: st.listening,
            gain: linear(gain_db),
            band: st.band,
            solo: st.solo,
            measure: !self.doc().references.is_empty(),
        }
    }

    /// Sends the settings when they changed.
    fn push_reference_settings(&mut self) {
        let settings = self.reference_settings();
        if self.reference.sent == Some(settings) {
            return;
        }
        let result = self.sink.reference_set(settings);
        self.engine_result(result);
        self.reference.sent = Some(settings);
    }

    /// The song position in milliseconds (the playhead).
    fn playhead_ms(&self) -> u64 {
        (self.playhead_seconds() * 1000.0).round().max(0.0) as u64
    }

    /// The song position in seconds, through the tempo map.
    fn playhead_seconds(&self) -> f64 {
        TempoMap::new(&self.doc().song.tempo).seconds_at_tick(self.position as f64)
    }

    /// Forgets the decoded file (another file is selected, or the list changed).
    fn reference_forget_audio(&mut self) {
        self.reference.loaded = None;
        self.reference.loading = None;
        self.reference.error = None;
        self.reference.gain_db = None;
        self.reference.mix_lufs = None;
    }

    /// Starts decoding and measuring the selected file on a worker.
    fn begin_reference_load(&mut self) {
        let Some(wanted) = self.selected_reference() else {
            return;
        };
        let path = PathBuf::from(&wanted.path);
        let (tx, rx) = channel();
        let job = path.clone();
        let started = std::thread::Builder::new()
            .name("atomik-reference".into())
            .spawn(move || {
                let _ = tx.send(load_reference(&job));
            });
        if started.is_err() {
            self.reference.error = Some("the worker could not start".into());
            return;
        }
        self.reference.error = None;
        self.reference.loading = Some(Loading { path, result: rx });
    }

    /// A new engine has no reference voice: the song goes back to the mix and the engine is
    /// told to measure again.
    pub(super) fn reference_after_reset(&mut self) {
        self.reference.listening = false;
        self.reference.sent = Some(ReferenceSettings::default());
        self.push_reference_settings();
    }

    /// One frame of the reference: applies a finished decode, keeps the voice's measuring switch
    /// right and moves the level match.
    pub(super) fn reference_frame(&mut self) {
        let count = self.doc().references.len();
        if count == 0 {
            if self.reference.listening {
                self.reference.listening = false;
            }
            self.reference_forget_audio();
            self.reference.selected = 0;
            self.push_reference_settings();
            return;
        }
        if self.reference.selected >= count {
            self.reference.selected = count - 1;
        }
        // The list changed under the selection (undo, a cue edit keeps the path).
        let wanted = self.selected_reference().map(|r| PathBuf::from(r.path));
        let current: Option<PathBuf> = self
            .reference
            .loaded
            .as_ref()
            .map(|l| l.path.clone())
            .or_else(|| self.reference.loading.as_ref().map(|l| l.path.clone()));
        if current.is_some() && current != wanted {
            if self.reference.listening {
                self.reference.listening = false;
            }
            self.reference_forget_audio();
        }
        if current.is_none() && self.reference.error.is_none() && wanted.is_some() {
            self.begin_reference_load();
        }
        if let Some(loading) = &self.reference.loading {
            match loading.result.try_recv() {
                Ok(Ok(loaded)) => {
                    self.reference.loading = None;
                    self.reference.loaded = Some(loaded);
                }
                Ok(Err(e)) => {
                    let name = self.selected_reference().map(|r| r.name().to_owned());
                    self.reference.loading = None;
                    self.reference.error = Some(e.clone());
                    self.report(format!(
                        "The reference {} cannot be used: {e}",
                        name.unwrap_or_default()
                    ));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.reference.loading = None;
                    self.reference.error = Some("the decoder stopped".into());
                }
            }
        }
        self.reference.frames += 1;
        if self.reference.frames >= MATCH_EVERY_FRAMES {
            self.reference.frames = 0;
            self.update_reference_gain();
        }
        self.push_reference_settings();
    }

    /// Moves the matched gain toward the loudest-section match.
    fn update_reference_gain(&mut self) {
        if !self.reference.level_match {
            return;
        }
        let Some(loudest) = self.selected_loaded().and_then(|l| l.loudest) else {
            return;
        };
        let Some(mix) = self.sink.reference_level() else {
            return;
        };
        if mix <= AUDIBLE_LUFS {
            return;
        }
        let want = (mix - loudest).clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1);
        self.reference.mix_lufs = Some(mix);
        self.reference.gain_db = Some(match self.reference.gain_db {
            None => want,
            Some(now) if (want - now).abs() < 0.05 => now,
            Some(now) => now + (want - now).clamp(-MATCH_STEP_DB, MATCH_STEP_DB),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cue_wins_over_the_playhead_and_the_start_stays_inside_the_file() {
        assert_eq!(start_frame(None, 2.0, 48_000.0, 1_000_000), 96_000);
        assert_eq!(start_frame(Some(500), 2.0, 48_000.0, 1_000_000), 24_000);
        assert_eq!(start_frame(None, 99.0, 48_000.0, 1_000), 1_000);
        assert_eq!(start_frame(None, -1.0, 48_000.0, 1_000), 0);
    }

    #[test]
    fn the_loudest_section_is_what_a_reference_measures() {
        let rate = 48_000u32;
        let tone = |amp: f32, seconds: usize| -> Vec<f32> {
            (0..rate as usize * seconds)
                .map(|n| amp * (std::f32::consts::TAU * 1000.0 * n as f32 / rate as f32).sin())
                .collect()
        };
        let mut left = tone(0.05, 5);
        left.extend(tone(0.2, 5));
        left.extend(tone(0.05, 5));
        let audio = AudioData::new(rate, vec![left.clone(), left]).unwrap();
        let loudest = measure_loudest(&audio).expect("audible");
        // Amplitude 0.2 on both channels reads about -14 LUFS.
        assert!((loudest + 14.0).abs() < 0.5, "{loudest}");
        // Silence has no loudness.
        let quiet = AudioData::new(rate, vec![vec![0.0; 200_000]]).unwrap();
        assert_eq!(measure_loudest(&quiet), None);
        // A file under 3 s reads its integrated loudness.
        let short = tone(0.2, 2);
        let audio = AudioData::new(rate, vec![short.clone(), short]).unwrap();
        let loudest = measure_loudest(&audio).expect("audible");
        assert!((loudest + 14.0).abs() < 1.0, "{loudest}");
    }
}
