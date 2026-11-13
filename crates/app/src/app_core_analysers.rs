//! The analyser devices (`atomik.spectrum`, `atomik.phase-meter`, `atomik.tuner`) in the app core.
//!
//! # Live feed
//!
//! [`AppCore::feed_device_scopes`] runs once per frame after the meter feed. For every analyser
//! device of the flat rack it reads the newest frames of the device's scope tap through
//! [`EngineSink::read_device_scope`] (lock-free; the audio thread only writes the ring) and
//! hands the rack view an [`AnalyserData`]:
//!
//! * **Spectrum:** an FFT of the device's `size` frames with its window and channel, averaged,
//!   mapped to 240 log-frequency columns or the 31 one-third-octave bands, with the held peaks
//!   and the loudest-bin readout (`atomik_dsp::spectrum`; the settings are read from the
//!   document, so a knob turn shows on the next frame).
//! * **Phase meter:** the newest 2048 frames, every second one, for the goniometer. Its numbers
//!   (correlation, levels, loudness) come through the ordinary meter values.
//! * **Tuner:** the pitch of the louder channel ([`atomik_dsp::tuner`]) against the device's `ref`
//!   (A4) shifted by the project's master tune, smoothed and held as the audio track's tuner is.
//!
//! Beyond the trace the spectrum feed keeps three more things per device. The **spectrogram**
//! (display mode 2): the fine trace appended as a row of an 8-bit image every 75 ms of audio
//! (`atomik_dsp::spectrogram`), 80 rows, newest first. The **reference curve**: a stored set of
//! 31 band levels (`spectrum-reference.json` next to the library file, [`crate::spectrum_reference`])
//! that a rising edge of the device's `reference.capture` fills from the signal now on screen,
//! drawn over the trace as the `reference` choice says (as stored, or level matched). The phase
//! meter keeps a **loudness history**: its short-term loudness once a second for the graph and
//! the loudness range since the last reset (`atomik_dsp::loudness_history`), skipping the first
//! three seconds after a start or a reset while the meter's windows fill.
//!
//! The FFT and the drawing therefore cost the audio thread nothing. A device whose tap did not
//! move since the last feed (a paused engine) is skipped, so an idle project costs no view work;
//! a bypassed device is skipped too, so its panel freezes on what it last showed.
//!
//! # Headless readouts
//!
//! A session has no audio stream, so nothing fills a tap. [`AppCore::probe_meters`] renders the
//! document offline up to a position (the code of a bounce), takes the final meter values and
//! scope frames of every analyser device, runs the same analysis as the live feed, feeds the
//! view (the panels then show the values in `session_texts` and screenshots) and returns the
//! numbers as [`MeterReadout`]s.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_device_sdk::DeviceDescriptor;
use atomik_devices::phase_meter::value;
use atomik_devices::spectrum::{CHANNEL_NAMES, SIZES, TILT_DB_PER_OCT};
use atomik_dsp::loudness_history::LoudnessHistory;
use atomik_dsp::spectrogram::Spectrogram;
use atomik_dsp::spectrum::{
    BAND_COUNT, ChannelMode, FLOOR_DB, Settings, Smoothing, Spectrum, Weighting, WindowKind,
    band_centres, reference_curve,
};
use atomik_model::{DeviceKey, ExpandedRack, RackDevice};
use atomik_ui::analyser::{AnalyserData, SCOPE_READ_FRAMES, TRACE_COLUMNS};
use atomik_ui::fx::FxKind;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Columns of the spectrogram image (the trace has 240).
const SPECTROGRAM_COLS: usize = 120;
/// Rows of the spectrogram image.
const SPECTROGRAM_ROWS: usize = 80;
/// Milliseconds of audio per spectrogram row: 80 rows are 6 seconds.
const SPECTROGRAM_PERIOD_MS: f32 = 75.0;
/// Seconds after a start or a reset that the loudness history skips.
const HISTORY_SETTLE_S: u64 = 3;

/// The device types of the analyser feed.
fn analyser_kind(type_id: &str) -> Option<FxKind> {
    FxKind::of(type_id)
        .filter(|k| matches!(k, FxKind::Spectrum | FxKind::PhaseMeter | FxKind::Tuner))
}

/// The analyser devices of an expansion, in its order.
pub(super) fn analyser_keys(expanded: &ExpandedRack) -> Vec<(DeviceKey, FxKind)> {
    expanded
        .devices
        .iter()
        .filter(|d| !d.frozen)
        .filter_map(|d| analyser_kind(&d.type_id).map(|k| (d.key.clone(), k)))
        .collect()
}

fn param(desc: &DeviceDescriptor, dev: &RackDevice, key: &str) -> f32 {
    desc.params
        .iter()
        .find(|p| p.key == key)
        .map_or(0.0, |p| atomik_ui::widget::current_value(dev, p))
}

/// The analysis settings of a spectrum device, from its params.
pub fn spectrum_settings(desc: &DeviceDescriptor, dev: &RackDevice) -> Settings {
    let choice = |key: &str, max: usize| (param(desc, dev, key).round().max(0.0) as usize).min(max);
    Settings {
        size: SIZES[choice("size", SIZES.len() - 1)],
        window: if choice("window", 1) == 1 {
            WindowKind::BlackmanHarris
        } else {
            WindowKind::Hann
        },
        channel: match choice("channel", CHANNEL_NAMES.len() - 1) {
            0 => ChannelMode::Left,
            1 => ChannelMode::Right,
            2 => ChannelMode::Mid,
            3 => ChannelMode::Side,
            _ => ChannelMode::Sum,
        },
        // Mode 2, the spectrogram, shows the fine trace as an image.
        bands: choice("mode", 2) == 1,
        averaging_ms: param(desc, dev, "averaging"),
        hold_ms: param(desc, dev, "hold"),
        decay_db_per_s: param(desc, dev, "decay"),
        tilt_db_per_oct: TILT_DB_PER_OCT[choice("tilt", TILT_DB_PER_OCT.len() - 1)],
        smoothing: match choice("smoothing", 3) {
            1 => Smoothing::Twelfth,
            2 => Smoothing::Sixth,
            3 => Smoothing::Third,
            _ => Smoothing::Off,
        },
        weighting: if choice("weighting", 1) == 1 {
            Weighting::K
        } else {
            Weighting::Flat
        },
    }
}

/// What a spectrum device shows besides the trace itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpectrumView {
    /// Display mode 2: the spectrogram image instead of the trace.
    spectrogram: bool,
    /// The `reference` choice: 0 off, 1 as stored, 2 shifted to the level of the signal.
    reference: usize,
}

fn spectrum_view(desc: &DeviceDescriptor, dev: &RackDevice) -> SpectrumView {
    let choice = |key: &str, max: usize| (param(desc, dev, key).round().max(0.0) as usize).min(max);
    SpectrumView {
        spectrogram: choice("mode", 2) == 2,
        reference: choice("reference", 2),
    }
}

/// The A4 a tuner device reads against: its `ref` shifted by the master tune. The second value is
/// what the panel shows instead of its own `ref`, 0 while the project is in tune.
fn tuner_reference(desc: &DeviceDescriptor, dev: &RackDevice, tune_cents: f32) -> (f32, f32) {
    let own = param(desc, dev, "ref");
    let own = if own > 0.0 {
        own
    } else {
        atomik_dsp::tuner::REF_DEFAULT_HZ
    };
    if tune_cents.to_bits() == 0 {
        return (own, 0.0);
    }
    let shifted = (f64::from(own) * atomik_model::tune_ratio(tune_cents)) as f32;
    (shifted, shifted)
}

/// The analysis state of one device.
struct State {
    spectrum: Spectrum,
    tuner: atomik_dsp::tuner::Tuner,
    /// The tap head the data was computed at.
    head: u64,
    /// The data was computed at least once.
    fed: bool,
    data: AnalyserData,
    /// The spectrogram image.
    image: Spectrogram,
    /// The loudness graph and range of a phase meter.
    history: LoudnessHistory,
    /// Audio frames since the history started or was reset.
    history_age: u64,
    /// The integrated loudness of the previous feed (a fall to silence is a reset).
    integrated: f32,
    /// `reference.capture` was on at the previous feed.
    capture_on: bool,
    /// What the reference of `data` was built for: the choice, the stored curve's serial and
    /// whether the spectrogram hides it.
    reference_key: Option<(usize, u64, bool)>,
    /// The analysis of the strip a Spectrum device draws as its second curve.
    second: Spectrum,
    /// The tap head `second` was computed at.
    second_head: u64,
    /// The source `second` was built for.
    second_from: Option<atomik_model::CurveFrom>,
}

/// What the display of one device needs besides the frames.
struct Extra<'a> {
    /// The tuner's `(reference Hz, shown Hz)`.
    reference: (f32, f32),
    /// The spectrum's extra display state.
    view: SpectrumView,
    /// The stored reference curve.
    stored: Option<[f32; BAND_COUNT]>,
    /// The device's published meter values (the phase meter's loudness).
    meters: &'a [f32],
    /// The serial of the stored curve (see `Analysers::reference_serial`).
    serial: u64,
    /// A headless probe: one reading, so the image and the history are filled from it.
    probe: bool,
}

impl State {
    fn new() -> Self {
        Self {
            spectrum: Spectrum::new(),
            tuner: atomik_dsp::tuner::Tuner::new(),
            head: 0,
            fed: false,
            data: AnalyserData::default(),
            image: Spectrogram::new(SPECTROGRAM_COLS, SPECTROGRAM_ROWS, SPECTROGRAM_PERIOD_MS),
            history: LoudnessHistory::new(),
            history_age: 0,
            integrated: FLOOR_DB,
            capture_on: false,
            reference_key: None,
            second: Spectrum::new(),
            second_head: 0,
            second_from: None,
        }
    }

    /// Rebuilds the reference curve of the display from the stored one (nothing in the
    /// spectrogram, which has no level axis for it).
    fn refresh_reference(
        &mut self,
        view: SpectrumView,
        stored: Option<&[f32; BAND_COUNT]>,
        serial: u64,
        tilt_db_per_oct: f32,
    ) {
        self.data.reference.clear();
        if let Some(stored) = stored.filter(|_| view.reference > 0 && !view.spectrogram) {
            let now = self.spectrum.band_levels();
            self.data.reference.extend(reference_curve(
                stored,
                &now,
                view.reference == 2,
                tilt_db_per_oct,
            ));
        }
        self.reference_key = Some((view.reference, serial, view.spectrogram));
    }

    /// Adds the loudness the phase meter publishes to the history graph and the range (module
    /// docs). A fall of the integrated loudness to silence is a reset of the meter.
    fn feed_history(&mut self, extra: &Extra<'_>, elapsed: u64, first: bool, sample_rate: f32) {
        use atomik_devices::phase_meter::value;
        let (Some(short), Some(integrated)) = (
            extra.meters.get(value::SHORT_TERM).copied(),
            extra.meters.get(value::INTEGRATED).copied(),
        ) else {
            return;
        };
        if integrated <= -119.9 && self.integrated > -119.9 {
            self.history.reset();
            self.history_age = 0;
        }
        self.integrated = integrated;
        let rate = f64::from(sample_rate) as u64;
        if extra.probe {
            self.history.push(short, rate, sample_rate);
        } else {
            let elapsed = if first { 0 } else { elapsed };
            self.history_age = self.history_age.saturating_add(elapsed);
            if self.history_age >= HISTORY_SETTLE_S * rate {
                self.history.push(short, elapsed, sample_rate);
            }
        }
        self.history.graph(&mut self.data.loudness_history);
        self.data.lra = self.history.lra();
    }

    /// Computes the display data of a read of the tap that ended at `head`.
    fn apply(
        &mut self,
        kind: FxKind,
        settings: &Settings,
        (left, right): (&[f32], &[f32]),
        head: u64,
        sample_rate: f32,
        extra: &Extra<'_>,
    ) {
        let reference = extra.reference;
        let elapsed = head.saturating_sub(self.head);
        let first = !self.fed;
        self.head = head;
        self.fed = true;
        let d = &mut self.data;
        match kind {
            FxKind::Spectrum => {
                self.spectrum
                    .update(left, right, elapsed, sample_rate, settings, TRACE_COLUMNS);
                d.trace.clear();
                d.trace.extend_from_slice(self.spectrum.trace());
                d.peaks.clear();
                if settings.hold_ms > 0.0 {
                    d.peaks.extend_from_slice(self.spectrum.peaks());
                }
                d.bands = settings.bands;
                d.peak_hz = self.spectrum.peak_hz();
                d.peak_db = self.spectrum.peak_db();
                if extra.view.spectrogram {
                    let step =
                        (f64::from(SPECTROGRAM_PERIOD_MS) * 0.001 * f64::from(sample_rate)) as u64;
                    let frames = if extra.probe {
                        step * SPECTROGRAM_ROWS as u64
                    } else if first {
                        0
                    } else {
                        elapsed
                    };
                    self.image.push(self.spectrum.trace(), frames, sample_rate);
                    self.image.rows_newest_first(&mut d.spectrogram);
                    d.spectrogram_cols = self.image.cols();
                    d.spectrogram_rows = self.image.rows();
                    d.spectrogram_span_s =
                        self.image.rows() as f32 * self.image.period_ms() * 0.001;
                } else if !d.spectrogram.is_empty() {
                    self.image.clear();
                    d.spectrogram.clear();
                    (d.spectrogram_cols, d.spectrogram_rows) = (0, 0);
                    d.spectrogram_span_s = 0.0;
                }
                self.refresh_reference(
                    extra.view,
                    extra.stored.as_ref(),
                    extra.serial,
                    settings.tilt_db_per_oct,
                );
            }
            FxKind::Tuner => {
                // The louder channel: a mono source may sit on either input.
                let mono = if atomik_dsp::tuner::rms(left) >= atomik_dsp::tuner::rms(right) {
                    left
                } else {
                    right
                };
                let elapsed = if first { 0 } else { elapsed };
                d.tuner = self.tuner.update(mono, sample_rate, elapsed, reference.0);
                d.tuner_on = true;
                d.tuner_ref_hz = reference.1;
            }
            _ => {
                d.scope_l.clear();
                d.scope_l.extend(left.iter().step_by(2));
                d.scope_r.clear();
                d.scope_r.extend(right.iter().step_by(2));
                self.feed_history(extra, elapsed, first, sample_rate);
            }
        }
    }
}

/// The analysis state of every analyser device of the flat rack.
#[derive(Default)]
pub(super) struct Analysers {
    devices: Vec<(DeviceKey, FxKind)>,
    states: BTreeMap<DeviceKey, State>,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The stored reference curve (31 band levels in dB).
    reference: Option<[f32; BAND_COUNT]>,
    /// Counts the changes of `reference`, so a display knows its copy is stale.
    reference_serial: u64,
    /// Where the curve is kept; `None` keeps it in memory only (tests).
    reference_path: Option<PathBuf>,
    /// The file was written by a newer version or cannot be read: never overwrite it.
    reference_locked: bool,
}

impl Analysers {
    /// Sets the devices of the new expansion and forgets the state of the ones that left.
    pub(super) fn set_devices(&mut self, devices: Vec<(DeviceKey, FxKind)>) {
        self.states
            .retain(|k, _| devices.iter().any(|(key, _)| key == k));
        self.devices = devices;
    }

    /// Forgets all state (a new engine: every tap starts again from frame 0).
    pub(super) fn restart(&mut self) {
        self.states.clear();
    }
}

/// What a headless probe read from one analyser device.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterReadout {
    /// The device's flat key.
    pub key: DeviceKey,
    /// Its type.
    pub type_id: String,
    /// Named values: for the phase meter every published value, for the spectrum the loudest bin.
    pub values: Vec<(&'static str, f32)>,
    /// The spectrum's one-third-octave levels as `(centre Hz, dB)` (empty for the phase meter).
    pub bands: Vec<(f32, f32)>,
}

/// The names of the phase meter's published values, in order.
pub const PHASE_VALUE_NAMES: [&str; 16] = [
    "peak_l",
    "peak_r",
    "hold_l",
    "hold_r",
    "rms_l",
    "rms_r",
    "sample_peak_dbfs",
    "true_peak_dbtp",
    "correlation",
    "balance",
    "width",
    "mid_rms",
    "side_rms",
    "momentary_lufs",
    "short_term_lufs",
    "integrated_lufs",
];

const _: () = assert!(value::INTEGRATED + 1 == PHASE_VALUE_NAMES.len());

impl<S: EngineSink> AppCore<S> {
    /// The device, its descriptor and bypass state for the analyser at `key`.
    fn analyser_device(&self, key: &DeviceKey) -> Option<(&RackDevice, &DeviceDescriptor)> {
        let dev = self.doc().resolve(key)?;
        let desc = self.registry.descriptor(&dev.type_id)?;
        Some((dev, desc))
    }

    /// Feeds the rack view the analysis of every analyser device's scope tap (module docs).
    pub fn feed_device_scopes(&mut self) {
        if self.analysers.devices.is_empty() {
            return;
        }
        let mut a = std::mem::take(&mut self.analysers);
        let sample_rate = self.doc().sample_rate as f32;
        let tune_cents = self.doc().tune_cents;
        let mut captured: Option<[f32; BAND_COUNT]> = None;
        for (key, kind) in &a.devices {
            let Some(id) = self.ids.get(key).copied() else {
                continue;
            };
            let Some((dev, desc)) = self.analyser_device(key) else {
                continue;
            };
            if param(desc, dev, "bypass") >= 0.5 {
                continue;
            }
            let settings = spectrum_settings(desc, dev);
            let reference = tuner_reference(desc, dev, tune_cents);
            let view = spectrum_view(desc, dev);
            let capture = param(desc, dev, "reference.capture") >= 0.5;
            let want = match kind {
                FxKind::Spectrum => settings.size,
                FxKind::Tuner => atomik_dsp::tuner::frames_wanted(sample_rate),
                _ => SCOPE_READ_FRAMES,
            };
            a.left.resize(want, 0.0);
            a.right.resize(want, 0.0);
            let Some((n, head)) = self.sink.read_device_scope(id, &mut a.left, &mut a.right) else {
                continue;
            };
            let state = a.states.entry(key.clone()).or_insert_with(State::new);
            if *kind == FxKind::Spectrum {
                if self.feed_second_curve(key, state, &settings, view, sample_rate) {
                    self.view.set_analyser(key, &state.data);
                }
                if capture && !state.capture_on && state.fed {
                    captured = Some(state.spectrum.band_levels());
                }
                state.capture_on = capture;
                // A new choice or a new stored curve shows at once, even with the engine paused.
                let want_key = Some((view.reference, a.reference_serial, view.spectrogram));
                if state.fed && state.reference_key != want_key {
                    state.refresh_reference(
                        view,
                        a.reference.as_ref(),
                        a.reference_serial,
                        settings.tilt_db_per_oct,
                    );
                    self.view.set_analyser(key, &state.data);
                }
            }
            if head == 0 || (head == state.head && state.fed) {
                continue;
            }
            let frames = (&a.left[..n], &a.right[..n]);
            let mut meters = [0.0f32; atomik_devices::phase_meter::PHASE_METERS];
            let meter_count = if *kind == FxKind::PhaseMeter {
                self.sink
                    .read_device_meter_values(id, &mut meters)
                    .map_or(0, |(n, _)| n)
            } else {
                0
            };
            let extra = Extra {
                reference,
                view,
                stored: a.reference,
                meters: &meters[..meter_count],
                serial: a.reference_serial,
                probe: false,
            };
            state.apply(*kind, &settings, frames, head, sample_rate, &extra);
            self.view.set_analyser(key, &state.data);
        }
        self.analysers = a;
        if let Some(bands) = captured {
            self.store_reference(bands);
        }
    }

    /// The second curve of the Spectrum device `key` (module docs of `app_core_meter_panels`): the
    /// reference track's long-term spectrum or the analysis of another strip's tap, and the list
    /// of sources its REF button offers. Returns whether anything the display shows changed.
    fn feed_second_curve(
        &self,
        key: &DeviceKey,
        state: &mut State,
        settings: &Settings,
        view: SpectrumView,
        sample_rate: f32,
    ) -> bool {
        let strips: Vec<DeviceKey> = self
            .console
            .strip_names()
            .into_iter()
            .map(|(k, _, _)| k)
            .collect();
        // A strip that is gone is no source any more.
        let curve = self
            .doc()
            .spectrum_curves
            .iter()
            .find(|r| &r.device == key)
            .map(|r| r.from.clone())
            .filter(|from| match from {
                atomik_model::CurveFrom::Reference => true,
                atomik_model::CurveFrom::Strip(k) => strips.contains(k),
            });
        let (sources, chosen) = self.spectrum_curve_sources(key, curve.as_ref());
        let mut second: Vec<f32> = Vec::new();
        if state.second_from != curve {
            state.second = Spectrum::new();
            state.second_head = 0;
            state.second_from.clone_from(&curve);
        }
        if !view.spectrogram {
            match &curve {
                Some(atomik_model::CurveFrom::Reference) => {
                    let shift = self.reference_curve_shift();
                    if let Some(t) = self.reference_trace() {
                        second = t.iter().map(|v| v + shift).collect();
                    }
                }
                Some(atomik_model::CurveFrom::Strip(strip)) => {
                    if let Some(other) = self.ids.get(strip).copied() {
                        let size = settings.size;
                        let mut left = vec![0.0f32; size];
                        let mut right = vec![0.0f32; size];
                        if let Some((n, head)) =
                            self.sink.read_device_scope(other, &mut left, &mut right)
                            && head != 0
                            && head != state.second_head
                        {
                            let elapsed = head.saturating_sub(state.second_head);
                            state.second_head = head;
                            state.second.update(
                                &left[..n],
                                &right[..n],
                                elapsed,
                                sample_rate,
                                settings,
                                TRACE_COLUMNS,
                            );
                        }
                        second = state.second.trace().to_vec();
                    }
                }
                None => {}
            }
        }
        let d = &mut state.data;
        if d.second == second && d.second_sources == sources && d.second_source == chosen {
            return false;
        }
        d.second = second;
        d.second_sources = sources;
        d.second_source = chosen;
        true
    }

    /// The Spectrum devices whose second curve is a strip, with the window size they read it at:
    /// the taps of those strips must be live.
    pub(super) fn spectrum_curve_taps(&self) -> Vec<(DeviceKey, usize)> {
        let mut out = Vec::new();
        for (key, kind) in &self.analysers.devices {
            if *kind != FxKind::Spectrum {
                continue;
            }
            let Some(atomik_model::CurveFrom::Strip(strip)) = self
                .doc()
                .spectrum_curves
                .iter()
                .find(|r| &r.device == key)
                .map(|r| &r.from)
            else {
                continue;
            };
            let Some((dev, desc)) = self.analyser_device(key) else {
                continue;
            };
            if param(desc, dev, "bypass") >= 0.5 || spectrum_view(desc, dev).spectrogram {
                continue;
            }
            out.push((strip.clone(), spectrum_settings(desc, dev).size));
        }
        out
    }

    /// Keeps `bands` as the reference curve and writes it to the user's file.
    fn store_reference(&mut self, bands: [f32; BAND_COUNT]) {
        if bands.iter().all(|v| *v <= FLOOR_DB + 50.0) {
            self.report("There is no signal to capture as a reference curve");
            return;
        }
        let a = &mut self.analysers;
        a.reference = Some(bands);
        a.reference_serial += 1;
        let mut message = "Spectrum reference curve captured".to_string();
        if let Some(path) = a.reference_path.clone() {
            if a.reference_locked {
                message = "Spectrum reference captured for this session; the stored curve was \
                           written by a newer version and is left alone"
                    .to_string();
            } else if let Err(e) = crate::spectrum_reference::save(&path, &bands) {
                log::warn!("writing {}: {e}", path.display());
                message = format!("Spectrum reference captured but not saved: {e}");
            }
        }
        self.report(message);
    }

    /// Where the stored spectrum reference curve lives (`spectrum-reference.json` next to the
    /// library file); `None` keeps it in memory only (tests). Reads it now: a damaged file is
    /// ignored until the next capture, a file from a newer version is never overwritten.
    pub fn set_reference_path(&mut self, path: Option<PathBuf>) {
        use crate::spectrum_reference::{Loaded, load};
        let a = &mut self.analysers;
        a.reference = None;
        a.reference_locked = false;
        if let Some(p) = &path {
            match load(p) {
                Loaded::Curve(bands) => a.reference = Some(bands),
                Loaded::Missing => {}
                Loaded::Unusable { why, newer } => {
                    log::warn!(
                        "the spectrum reference curve {} is not used: {why}",
                        p.display()
                    );
                    a.reference_locked = newer;
                }
            }
        }
        a.reference_path = path;
        a.reference_serial += 1;
    }

    /// The stored spectrum reference curve: 31 band levels in dB.
    pub fn spectrum_reference(&self) -> Option<&[f32; BAND_COUNT]> {
        self.analysers.reference.as_ref()
    }

    /// The analyser devices of the flat rack (flat key, kind).
    pub fn analyser_devices(&self) -> &[(DeviceKey, FxKind)] {
        &self.analysers.devices
    }

    /// Renders the document offline for `frames` frames and reads what every analyser device
    /// (or only `only`) shows at the end: feeds the rack view as the live feed would, and
    /// returns the numbers (module docs). Refused for a project with plugins.
    ///
    /// # Errors
    /// The renderer's message, or `only` is not an analyser device.
    pub fn probe_meters(
        &mut self,
        frames: usize,
        only: Option<&DeviceKey>,
    ) -> Result<Vec<MeterReadout>, String> {
        let keys: Vec<(DeviceKey, FxKind)> = self
            .analysers
            .devices
            .iter()
            .filter(|(k, _)| only.is_none_or(|o| o == k))
            .cloned()
            .collect();
        if keys.is_empty() {
            return Err(match only {
                Some(k) => format!("`{}` is not a spectrum or phase meter device", k.0),
                None => "the project has no spectrum or phase meter device".to_string(),
            });
        }
        let used = self.doc().plugin_devices();
        if !used.is_empty() {
            return Err(
                "this project uses third-party plugins, which a headless render cannot run"
                    .to_string(),
            );
        }
        let device_keys: Vec<DeviceKey> = keys.iter().map(|(k, _)| k.clone()).collect();
        let probes = atomik_render::project::probe_devices(
            self.doc(),
            &self.registry,
            &atomik_render::project::ProjectOptions::default(),
            self.bundle_root(),
            frames,
            &device_keys,
        )
        .map_err(|e| e.to_string())?;
        let sample_rate = self.doc().sample_rate as f32;
        let tune_cents = self.doc().tune_cents;
        let mut out = Vec::new();
        for probe in probes {
            let Some(kind) = keys.iter().find(|(k, _)| *k == probe.key).map(|(_, k)| *k) else {
                continue;
            };
            let Some((dev, desc)) = self.analyser_device(&probe.key) else {
                continue;
            };
            let settings = spectrum_settings(desc, dev);
            let type_id = dev.type_id.clone();
            let mut state = State::new();
            let reference = tuner_reference(desc, dev, tune_cents);
            let extra = Extra {
                reference,
                view: spectrum_view(desc, dev),
                stored: self.analysers.reference,
                meters: &probe.values,
                serial: self.analysers.reference_serial,
                probe: true,
            };
            state.apply(
                kind,
                &settings,
                (&probe.left, &probe.right),
                probe.head,
                sample_rate,
                &extra,
            );
            self.view.set_device_meter_values(&probe.key, &probe.values);
            self.view.set_analyser(&probe.key, &state.data);
            let mut readout = MeterReadout {
                key: probe.key.clone(),
                type_id,
                values: Vec::new(),
                bands: Vec::new(),
            };
            if kind == FxKind::PhaseMeter {
                for (name, v) in PHASE_VALUE_NAMES.iter().zip(&probe.values) {
                    readout.values.push((*name, *v));
                }
            } else if kind == FxKind::Tuner {
                if let Some(r) = state.data.tuner {
                    readout.values.push(("hz", r.hz));
                    readout.values.push(("key", r.note.key as f32));
                    readout.values.push(("cents", r.note.cents));
                }
            } else {
                readout.values.push(("peak_hz", state.data.peak_hz));
                readout.values.push(("peak_db", state.data.peak_db));
                // The bands, whatever the device displays: one fresh average of the last frames.
                let bands = Settings {
                    bands: true,
                    averaging_ms: 0.0,
                    hold_ms: 0.0,
                    tilt_db_per_oct: 0.0,
                    ..settings
                };
                let mut spectrum = Spectrum::new();
                spectrum.update(&probe.left, &probe.right, 0, sample_rate, &bands, 0);
                readout.bands = band_centres()
                    .into_iter()
                    .zip(spectrum.trace().iter().copied())
                    .collect();
            }
            out.push(readout);
        }
        Ok(out)
    }
}
