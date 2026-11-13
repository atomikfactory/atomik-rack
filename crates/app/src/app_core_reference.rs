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
//! follows the playhead: it starts at the file position equal to the song position in seconds, and
//! while it is heard a seek of the playhead moves it to the new song position
//! ([`AppCore::reference_after_seek`]: a new stream from there, the old one cut, no gap in the
//! settings). A reference with a cue is not time-aligned with the song and stays where it is.
//!
//! The same goes for the **loop**: while the transport plays with the loop on and the playhead
//! wraps at the loop end (the engine counts the laps), the reference that follows the playhead
//! jumps back with it ([`AppCore::reference_loop_frame`], the same new stream as after a seek), so the reference
//! and the mix stay on the same bar from one lap to the next.
//!
//! # Level match
//!
//! While any reference exists the engine measures the mix (K-weighted short-term loudness, 3 s,
//! before the reference voice). The reference's gain is the difference between that and the
//! short-term loudness of the *loudest section* of the reference (the maximum over its file, see
//! [`measure_loudest`]), so the two sit at the same level in their strong parts. The gain is
//! updated slowly while the mix plays: at most 0.5 dB every quarter of a second, the first
//! reading at once, and only from readings above -60 LUFS (a stopped song keeps the last gain).
//! The gain is shown ([`ReferenceInfo::gain_db`]); the matched part is never stored in the project.
//!
//! # Trim
//!
//! Each reference has a **trim** of up to 12 dB either way in 0.1 dB steps ([`Reference::trim_tenths`],
//! saved with the reference), added to the matched gain (and to 0 dB with the match off), so a
//! preference such as "the reference a little quieter" survives the slow match.
//! [`AppCore::set_reference_trim`] edits it as one undo step.
//!
//! # Matching over a section
//!
//! [`AppCore::match_reference_section`] measures the integrated loudness of the mix and of the
//! reference over the time selection (or the loop locators), the reference from the same seconds
//! (from its cue, counted from the start of the section, when it has one), and sets the matched
//! gain to the difference. The mix is rendered offline on a worker (`atomik-reference-match`,
//! the master over that range, progress and cancel as the other analysis workers); the reference
//! is measured at once from the decoded file. The slow match of the loudest section stops while
//! the section gain is held; turning **Match Reference Level** off and on again resumes it.
//!
//! # Band and side solo
//!
//! [`AppCore::set_reference_band`] and [`AppCore::set_reference_solo`] limit the mix and the
//! reference to the same band and stereo part; the engine applies identical filters to both.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_audio_file::{AudioData, decode_file_with};
use atomik_devices::mastering::loudness::LoudnessMeter;
use atomik_dsp::spectrum::{Settings, Spectrum};
use atomik_engine::{
    PreviewFeed, ReferenceBand, ReferenceSettings, ReferenceSolo, preview_channel,
};
use atomik_model::audio_trim::TempoMap;
use atomik_model::{Command, MAX_REFERENCES, Reference};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    /// Its long-term average spectrum in dB, one value per column of the meter panels (the
    /// reference curve of a spectrum panel).
    trace: Vec<f32>,
}

/// A measurement of the mix over a section that runs on a worker.
struct SectionJob {
    result: Receiver<Result<Option<f32>, String>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    /// The loudness of the reference over its part of the section, in LUFS.
    reference_lufs: f32,
    /// The reference the job measured (a changed selection drops the result).
    path: PathBuf,
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
    /// The file frame the reference stream last started from, while it is heard.
    from_frame: Option<usize>,
    /// The matched gain was set from a section and the slow match leaves it alone.
    section_held: bool,
    /// The section measurement that runs.
    section: Option<SectionJob>,
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
            from_frame: None,
            section_held: false,
            section: None,
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
    /// The trim of the selected reference in dB (0 when there is none).
    pub trim_db: f32,
    /// The matched gain was set from a section of the song and is held there.
    pub section_held: bool,
    /// How far the measurement of a section is, `0.0..=1.0`, while one runs.
    pub matching: Option<f32>,
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
    let trace = average_spectrum(&audio);
    Ok(Loaded {
        path: path.to_path_buf(),
        audio: Arc::new(audio),
        loudest,
        trace,
    })
}

/// The most analysis windows of the long-term spectrum.
const TRACE_WINDOWS: usize = 400;

/// The long-term average spectrum of `audio` in dB, one value per column of the meter panels:
/// the power of up to [`TRACE_WINDOWS`] evenly spread 4096-point windows averaged, silent windows
/// skipped. Empty for a file shorter than one window or with nothing audible.
pub fn average_spectrum(audio: &AudioData) -> Vec<f32> {
    use atomik_ui::analyser::TRACE_COLUMNS;
    const SIZE: usize = 4096;
    let Some(left) = audio.channels.first() else {
        return Vec::new();
    };
    let right = audio.channels.get(1).unwrap_or(left);
    let len = left.len().min(right.len());
    if len < SIZE {
        return Vec::new();
    }
    let rate = audio.sample_rate as f32;
    let step = ((len - SIZE) / TRACE_WINDOWS).max(SIZE);
    let settings = Settings {
        size: SIZE,
        averaging_ms: 0.0,
        hold_ms: 0.0,
        ..Settings::default()
    };
    let mut spectrum = Spectrum::new();
    let mut power = vec![0.0f64; TRACE_COLUMNS];
    let mut windows = 0u32;
    let mut at = 0;
    while at + SIZE <= len {
        spectrum.update(
            &left[at..at + SIZE],
            &right[at..at + SIZE],
            SIZE as u64,
            rate,
            &settings,
            TRACE_COLUMNS,
        );
        let trace = spectrum.trace();
        if trace.len() == TRACE_COLUMNS && trace.iter().any(|v| *v > -90.0) {
            for (p, v) in power.iter_mut().zip(trace) {
                *p += 10f64.powf(f64::from(*v) / 10.0);
            }
            windows += 1;
        }
        at += step;
    }
    if windows == 0 {
        return Vec::new();
    }
    power
        .iter()
        .map(|p| (10.0 * (p / f64::from(windows)).log10()) as f32)
        .collect()
}

/// The integrated loudness in LUFS of `frames` frames of `audio` from frame `from`, `None` for
/// silence or fewer frames than the 400 ms the loudness gate needs.
pub fn section_lufs(audio: &AudioData, from: usize, frames: usize) -> Option<f32> {
    let left = audio.channels.first()?;
    let right = audio.channels.get(1).unwrap_or(left);
    let end = (from + frames).min(left.len()).min(right.len());
    if end <= from || ((end - from) as f64) < f64::from(audio.sample_rate) * 0.4 {
        return None;
    }
    let mut meter = LoudnessMeter::new(f64::from(audio.sample_rate));
    let mut at = from;
    while at < end {
        let next = (at + 4_096).min(end);
        meter.process(&left[at..next], &right[at..next]);
        at = next;
    }
    let lufs = meter.integrated();
    (lufs > -70.0).then_some(lufs)
}

/// What the worker of a section match does: renders the master of `doc` over `span` and measures
/// its integrated loudness. `None`: it was stopped.
fn measure_mix_section(
    work: SectionWork,
    control: atomik_render::plugin_pass::RenderControl<'_>,
) -> Result<Option<f32>, String> {
    use atomik_device_sdk::RunMode;
    use atomik_render::plugins::MissingPlugins;
    use atomik_render::project::{PluginBounce, ProjectOptions};
    let SectionWork {
        doc,
        bundle_root,
        plugins,
        span,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host =
        crate::freeze_worker::plugin_host(&doc, plugins, &mut registry, "the reference match")?;
    atomik_model::register_stand_ins(&doc, &mut registry).map_err(|e| e.to_string())?;
    let options = ProjectOptions {
        host_block: std::num::NonZeroUsize::new(512).unwrap_or(std::num::NonZeroUsize::MIN),
        workers: 0,
        loop_count: None,
    };
    let mut bounce = PluginBounce {
        host: host.as_mut(),
        mode: RunMode::Offline,
        missing: MissingPlugins::Error,
    };
    let Some(audio) = atomik_render::energy::render_master_span(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        span,
        control,
    )
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let frames = audio.channels.first().map_or(0, Vec::len);
    section_lufs(&audio, 0, frames)
        .map(Some)
        .ok_or_else(|| "the mix is silent over the section".to_owned())
}

/// What the worker of a section match owns.
struct SectionWork {
    doc: atomik_model::RackDoc,
    bundle_root: Option<PathBuf>,
    plugins: crate::freeze_worker::PluginEnv,
    span: atomik_render::export::ExportSpan,
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
            trim_db: self.selected_reference().map_or(0.0, |r| r.trim_db()),
            section_held: st.section_held,
            matching: st
                .section
                .as_ref()
                .map(|j| f32::from_bits(j.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0)),
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
        // On again: the slow match of the loudest section takes over from a section gain.
        self.reference.section_held = false;
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
                self.reference.from_frame = None;
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
        self.reference.listening = true;
        if !self.start_reference_stream(&audio, from, wanted.name()) {
            self.reference.listening = false;
            return;
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

    /// Gives the engine a stream of `audio` from file frame `from` and the settings now in force
    /// (a stream that was loaded is cut). False when the engine refused.
    fn start_reference_stream(&mut self, audio: &Arc<AudioData>, from: usize, name: &str) -> bool {
        let len = audio.channels.first().map_or(0, Vec::len);
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(audio.sample_rate);
        // The first part is in the ring before the engine sees the stream: no gap at the start.
        let pushed = feed.push(&audio.channels, from);
        let settings = self.reference_settings();
        if let Err(e) = self.sink.reference_start(stream, settings) {
            self.report(format!("reference: {e}"));
            return false;
        }
        self.reference.sent = Some(settings);
        self.reference.from_frame = Some(from);
        let next = from + pushed;
        if next >= len {
            feed.finish();
            if from >= len {
                self.report(format!("The reference {name} ends before this position"));
            }
        } else {
            let audio = Arc::clone(audio);
            if std::thread::Builder::new()
                .name("atomik-reference-feed".into())
                .spawn(move || feeder(&audio, next, feed))
                .is_err()
            {
                log::warn!("reference: the feeder thread could not start");
            }
        }
        true
    }

    /// The playhead was moved ([`AppCore::locate`]): a reference that is heard and follows the
    /// playhead (it has no cue of its own) starts again from the new song position.
    pub(super) fn reference_after_seek(&mut self) {
        if !self.reference.listening {
            return;
        }
        let Some(wanted) = self.selected_reference().filter(|r| r.cue_ms.is_none()) else {
            return;
        };
        let Some(loaded) = self.selected_loaded() else {
            return;
        };
        let audio = Arc::clone(&loaded.audio);
        let len = audio.channels.first().map_or(0, Vec::len);
        let from = start_frame(
            None,
            self.playhead_seconds(),
            f64::from(audio.sample_rate),
            len,
        );
        self.start_reference_stream(&audio, from, wanted.name());
    }

    /// One frame of the transport: the engine counted another loop wrap while the song plays (the
    /// playhead went from the loop end back to the loop start), so a reference that follows the
    /// playhead goes back too (see the module docs). The count is the engine's, so a seek of
    /// the user never counts as a wrap and a wrap never counts twice.
    pub(super) fn reference_loop_frame(&mut self, playing: bool, loop_count: u64) {
        let seen = self.reference_loops_seen.replace(loop_count);
        if playing && seen.is_some_and(|s| loop_count > s) {
            self.reference_after_seek();
        }
    }

    /// The song position, in seconds, the reference stream last started from (`None` while the
    /// mix is heard).
    pub fn reference_start_seconds(&self) -> Option<f64> {
        let from = self
            .reference
            .from_frame
            .filter(|_| self.reference.listening)?;
        let rate = f64::from(self.selected_loaded()?.audio.sample_rate);
        Some(from as f64 / rate)
    }

    /// The long-term spectrum of the selected reference once it is decoded (the reference curve of
    /// a spectrum panel).
    pub(super) fn reference_trace(&self) -> Option<&Vec<f32>> {
        self.selected_loaded()
            .map(|l| &l.trace)
            .filter(|t| !t.is_empty())
    }

    /// How far the reference curve moves from its own level to the level it is heard at: the
    /// matched gain (when there is one) and the trim, in dB.
    pub(super) fn reference_curve_shift(&self) -> f32 {
        let st = &self.reference;
        let matched = if st.level_match {
            st.gain_db.unwrap_or(0.0)
        } else {
            0.0
        };
        matched + self.selected_reference().map_or(0.0, |r| r.trim_db())
    }

    /// Sets the trim of the selected reference to `tenths` of a dB (clamped to 12 dB either way).
    /// One undo step; nothing changes when it is the value already.
    pub fn set_reference_trim(&mut self, tenths: i16) {
        let limit = atomik_model::MAX_REFERENCE_TRIM_TENTHS;
        let tenths = tenths.clamp(-limit, limit);
        let mut list = self.doc().references.clone();
        let Some(r) = list.get_mut(self.reference.selected) else {
            self.report("Add a reference track first");
            return;
        };
        if r.trim_tenths == tenths {
            return;
        }
        r.trim_tenths = tenths;
        let message = format!("Reference {} trim {:+.1} dB", r.name(), r.trim_db());
        if self.apply_command(Command::SetReferences(list)).is_ok() {
            self.push_reference_settings();
            self.report(message);
        }
    }

    /// Moves the trim of the selected reference by `delta` tenths of a dB.
    pub fn step_reference_trim(&mut self, delta: i16) {
        let now = self
            .selected_reference()
            .map_or(0, |r| i32::from(r.trim_tenths));
        let next = (now + i32::from(delta)).clamp(
            -i32::from(atomik_model::MAX_REFERENCE_TRIM_TENTHS),
            i32::from(atomik_model::MAX_REFERENCE_TRIM_TENTHS),
        );
        self.set_reference_trim(next as i16);
    }

    /// The range a section match measures: the time selection of the arrangement, else the loop
    /// locators, as `(start, end)` ticks.
    pub fn reference_section(&self) -> Option<(i64, i64)> {
        self.export_selection().or_else(|| {
            let (a, b) = self.doc().song.loop_range;
            (a < b).then_some((a, b))
        })
    }

    /// The integrated loudness in LUFS of the decoded selected reference over `seconds` of the
    /// file from `from_seconds` (`None` before it is decoded, for silence, or under 0.4 s).
    pub fn reference_lufs_over(&self, from_seconds: f64, seconds: f64) -> Option<f32> {
        let audio = &self.selected_loaded()?.audio;
        let rate = f64::from(audio.sample_rate);
        section_lufs(
            audio,
            (from_seconds.max(0.0) * rate).round() as usize,
            (seconds.max(0.0) * rate).round() as usize,
        )
    }

    /// Matches the loudness of the selected reference to the mix over the time selection (or
    /// the loop locators), see the module docs.
    ///
    /// # Errors
    /// There is no reference or it is not decoded yet, there is no section or it is shorter
    /// than 0.4 s, the reference is silent over it, or a measurement already runs (the reason
    /// is also in the status line).
    pub fn match_reference_section(&mut self) -> Result<(), String> {
        let refusal = self.try_match_reference_section();
        if let Err(why) = &refusal {
            self.reject(why.clone());
        }
        refusal
    }

    fn try_match_reference_section(&mut self) -> Result<(), String> {
        if self.reference.section.is_some() {
            return Err("The reference is already being measured".to_owned());
        }
        let wanted = self
            .selected_reference()
            .ok_or_else(|| "Add a reference track first".to_owned())?;
        let (start, end) = self
            .reference_section()
            .ok_or_else(|| "Select a time range or set the loop locators first".to_owned())?;
        let (audio, path) = {
            let loaded = self
                .selected_loaded()
                .ok_or_else(|| format!("The reference {} is still loading", wanted.name()))?;
            (Arc::clone(&loaded.audio), loaded.path.clone())
        };
        let tempo = TempoMap::new(&self.doc().song.tempo);
        let (from_s, to_s) = (
            tempo.seconds_at_tick(start as f64),
            tempo.seconds_at_tick(end as f64),
        );
        // The reference part of the section: the same seconds, or from the cue.
        let ref_from_s = wanted.cue_ms.map_or(from_s, |ms| ms as f64 / 1000.0);
        let rate = f64::from(audio.sample_rate);
        let from = (ref_from_s.max(0.0) * rate).round() as usize;
        let frames = ((to_s - from_s).max(0.0) * rate).round() as usize;
        let reference_lufs = section_lufs(&audio, from, frames).ok_or_else(|| {
            format!(
                "The reference {} is silent or too short over that section",
                wanted.name()
            )
        })?;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let (tx, rx) = channel();
        let work = SectionWork {
            doc: self.doc().clone(),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: crate::freeze_worker::PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
            span: atomik_render::export::ExportSpan::Range(start, end),
        };
        let (stop, step) = (Arc::clone(&cancel), Arc::clone(&progress));
        std::thread::Builder::new()
            .name("atomik-reference-match".into())
            .spawn(move || {
                let report = |p: f32| step.store(p.to_bits(), Ordering::Relaxed);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    measure_mix_section(
                        work,
                        atomik_render::plugin_pass::RenderControl {
                            cancel: Some(&stop),
                            progress: Some(&report),
                        },
                    )
                }))
                .unwrap_or_else(|_| Err("a device failed while the mix was measured".to_owned()));
                let _ = tx.send(result);
            })
            .map_err(|e| format!("cannot start the measurement: {e}"))?;
        self.reference.section = Some(SectionJob {
            result: rx,
            cancel,
            progress,
            reference_lufs,
            path,
        });
        self.report("Measuring the mix over the section");
        Ok(())
    }

    /// Stops the measurement of a section.
    pub fn cancel_reference_match(&mut self) {
        if let Some(job) = &self.reference.section {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Takes the result of the section measurement when it is done.
    fn poll_reference_section(&mut self) {
        let Some(job) = &self.reference.section else {
            return;
        };
        let outcome = match job.result.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the measurement stopped".to_owned()),
        };
        let Some(job) = self.reference.section.take() else {
            return;
        };
        // The selection changed while it ran: the result is for another file.
        let same = self.selected_loaded().is_some_and(|l| l.path == job.path);
        match outcome {
            Err(e) => self.reject(format!("Reference match failed: {e}")),
            Ok(None) => self.reject("Reference match cancelled".to_owned()),
            Ok(Some(mix_lufs)) if same => {
                let gain = (mix_lufs - job.reference_lufs).clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1);
                self.reference.gain_db = Some(gain);
                self.reference.mix_lufs = Some(mix_lufs);
                self.reference.section_held = true;
                self.reference.level_match = true;
                self.push_reference_settings();
                self.report(format!(
                    "Reference matched over the section: {gain:+.1} dB (mix {mix_lufs:.1}, reference {:.1} LUFS)",
                    job.reference_lufs
                ));
            }
            Ok(Some(_)) => {}
        }
    }

    /// The settings the engine voice should hold now.
    fn reference_settings(&self) -> ReferenceSettings {
        let st = &self.reference;
        let matched = if st.level_match {
            st.gain_db.unwrap_or(0.0)
        } else {
            0.0
        };
        let gain_db = matched + self.selected_reference().map_or(0.0, |r| r.trim_db());
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
        self.reference.section_held = false;
        if let Some(job) = self.reference.section.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
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
        self.poll_reference_section();
        self.reference.frames += 1;
        if self.reference.frames >= MATCH_EVERY_FRAMES {
            self.reference.frames = 0;
            self.update_reference_gain();
        }
        self.push_reference_settings();
    }

    /// Moves the matched gain toward the loudest-section match.
    fn update_reference_gain(&mut self) {
        if !self.reference.level_match || self.reference.section_held {
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
