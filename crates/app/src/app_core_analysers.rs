//! The analyser devices (`atomik.spectrum`, `atomik.phase-meter`) in the app core.
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
use atomik_dsp::spectrum::{ChannelMode, Settings, Spectrum, WindowKind, band_centres};
use atomik_model::{DeviceKey, ExpandedRack, RackDevice};
use atomik_ui::analyser::{AnalyserData, SCOPE_READ_FRAMES, TRACE_COLUMNS};
use atomik_ui::fx::FxKind;
use std::collections::BTreeMap;

/// The device types of the analyser feed.
fn analyser_kind(type_id: &str) -> Option<FxKind> {
    FxKind::of(type_id).filter(|k| matches!(k, FxKind::Spectrum | FxKind::PhaseMeter))
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
        bands: choice("mode", 1) == 1,
        averaging_ms: param(desc, dev, "averaging"),
        hold_ms: param(desc, dev, "hold"),
        decay_db_per_s: param(desc, dev, "decay"),
        tilt_db_per_oct: TILT_DB_PER_OCT[choice("tilt", TILT_DB_PER_OCT.len() - 1)],
    }
}

/// The analysis state of one device.
struct State {
    spectrum: Spectrum,
    /// The tap head the data was computed at.
    head: u64,
    /// The data was computed at least once.
    fed: bool,
    data: AnalyserData,
}

impl State {
    fn new() -> Self {
        Self {
            spectrum: Spectrum::new(),
            head: 0,
            fed: false,
            data: AnalyserData::default(),
        }
    }

    /// Computes the display data of a read of the tap that ended at `head`.
    fn apply(
        &mut self,
        kind: FxKind,
        settings: &Settings,
        (left, right): (&[f32], &[f32]),
        head: u64,
        sample_rate: f32,
    ) {
        let elapsed = head.saturating_sub(self.head);
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
            }
            _ => {
                d.scope_l.clear();
                d.scope_l.extend(left.iter().step_by(2));
                d.scope_r.clear();
                d.scope_r.extend(right.iter().step_by(2));
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
            let want = if *kind == FxKind::Spectrum {
                settings.size
            } else {
                SCOPE_READ_FRAMES
            };
            a.left.resize(want, 0.0);
            a.right.resize(want, 0.0);
            let Some((n, head)) = self.sink.read_device_scope(id, &mut a.left, &mut a.right) else {
                continue;
            };
            let state = a.states.entry(key.clone()).or_insert_with(State::new);
            if head == 0 || (head == state.head && state.fed) {
                continue;
            }
            let frames = (&a.left[..n], &a.right[..n]);
            state.apply(*kind, &settings, frames, head, sample_rate);
            self.view.set_analyser(key, &state.data);
        }
        self.analysers = a;
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
            state.apply(
                kind,
                &settings,
                (&probe.left, &probe.right),
                probe.head,
                sample_rate,
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
