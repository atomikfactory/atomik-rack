//! The spectrum behind the EQ's curve in the app core.
//!
//! The EQ owns a scope tap that carries its output (after the output gain). The tap costs the
//! audio thread a copy per batch while it is live, so the core keeps it live only for an EQ that
//! shows the spectrum: its panel is on screen ([`atomik_ui::RackView::visible_panels`], so not
//! folded, not scrolled out and not on the back face), its `spectrum.on` switch is on and the
//! device is not bypassed. Once a frame it switches the tap on for the devices that qualify and
//! off for the ones that stopped (the same flag the console strips use for their meter panels,
//! [`EngineSink::set_device_scope_live`]), reads the newest 4096 frames of each live tap
//! ([`EngineSink::read_device_scope`], lock-free), runs the FFT of the Spectrum device
//! ([`atomik_dsp::spectrum`]: Hann window, averaged over 120 ms, smoothed to a sixth of an octave,
//! 240 log-frequency columns) and hands the rack view the trace
//! ([`atomik_ui::analyser::AnalyserData`]) for the graph. The FFT never runs on the audio thread.
//!
//! The switch is sent when it changes, and again after an engine restart or for a device that was
//! added anew (a new engine id), and a device that stops qualifying has its trace cleared, so the
//! graph shows nothing stale when it comes back. A tap that did not move since the last frame (a
//! stopped engine) is not analysed again.

use super::*;
use atomik_dsp::spectrum::{ChannelMode, Settings, Smoothing, Spectrum, Weighting, WindowKind};
use atomik_ui::analyser::{AnalyserData, TRACE_COLUMNS};
use atomik_ui::fx::FxKind;
use std::collections::BTreeMap;

/// Frames of the tap the FFT reads.
pub(super) const EQ_SPECTRUM_FRAMES: usize = 4096;

/// The analysis of one EQ whose tap is live.
struct Live {
    id: DeviceInstanceId,
    spectrum: Spectrum,
    /// The tap head the trace was computed at.
    head: u64,
    fed: bool,
    data: AnalyserData,
}

/// The EQs whose tap the core switched on, with their analysis.
#[derive(Default)]
pub(super) struct EqSpectra {
    live: BTreeMap<DeviceKey, Live>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl EqSpectra {
    /// A new engine knows nothing of what was sent, and its taps start from frame 0.
    pub(super) fn restart(&mut self) {
        self.live.clear();
    }
}

/// How the EQ spectrum is analysed.
fn settings() -> Settings {
    Settings {
        size: EQ_SPECTRUM_FRAMES,
        window: WindowKind::Hann,
        channel: ChannelMode::Sum,
        bands: false,
        averaging_ms: 120.0,
        hold_ms: 0.0,
        decay_db_per_s: 15.0,
        tilt_db_per_oct: 0.0,
        smoothing: Smoothing::Sixth,
        weighting: Weighting::Flat,
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Switches the taps of the EQs and feeds their spectra to the rack view (module docs); once
    /// per frame from [`Self::frame`].
    pub(super) fn feed_eq_spectra(&mut self) {
        let shown = self.view.visible_panels(FxKind::Eq);
        if shown.is_empty() && self.eq_spectra.live.is_empty() {
            return;
        }
        // The EQs that want the tap: on screen, spectrum on, not bypassed.
        let mut wanted: Vec<(DeviceKey, DeviceInstanceId)> = Vec::new();
        for key in shown {
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let Some(dev) = self.doc().resolve(&key) else {
                continue;
            };
            let Some(desc) = self.registry.descriptor(&dev.type_id) else {
                continue;
            };
            let value = |k: &str| {
                desc.params
                    .iter()
                    .find(|p| p.key == k)
                    .map_or(0.0, |p| atomik_ui::widget::current_value(dev, p))
            };
            if value("spectrum.on") >= 0.5 && value("bypass") < 0.5 {
                wanted.push((key, id));
            }
        }
        let mut state = std::mem::take(&mut self.eq_spectra);
        // Off for what stopped qualifying (a device that is gone or replaced is a refusal that
        // changes nothing).
        let mut stopped: Vec<DeviceKey> = Vec::new();
        state.live.retain(|key, live| {
            let keep = wanted.iter().any(|(k, id)| k == key && *id == live.id);
            if !keep {
                self.sink.set_device_scope_live(live.id, false);
                stopped.push(key.clone());
            }
            keep
        });
        for key in stopped {
            self.view.set_analyser(&key, &AnalyserData::default());
        }
        let sample_rate = self.doc().sample_rate as f32;
        let settings = settings();
        state.left.resize(EQ_SPECTRUM_FRAMES, 0.0);
        state.right.resize(EQ_SPECTRUM_FRAMES, 0.0);
        for (key, id) in wanted {
            if !state.live.contains_key(&key) {
                self.sink.set_device_scope_live(id, true);
                state.live.insert(
                    key.clone(),
                    Live {
                        id,
                        spectrum: Spectrum::new(),
                        head: 0,
                        fed: false,
                        data: AnalyserData::default(),
                    },
                );
            }
            let Some(live) = state.live.get_mut(&key) else {
                continue;
            };
            let Some((n, head)) =
                self.sink
                    .read_device_scope(id, &mut state.left, &mut state.right)
            else {
                continue;
            };
            if head == 0 || (head == live.head && live.fed) {
                continue;
            }
            let elapsed = if live.fed {
                head.saturating_sub(live.head)
            } else {
                0
            };
            live.head = head;
            live.fed = true;
            live.spectrum.update(
                &state.left[..n],
                &state.right[..n],
                elapsed,
                sample_rate,
                &settings,
                TRACE_COLUMNS,
            );
            live.data.trace.clear();
            live.data.trace.extend_from_slice(live.spectrum.trace());
            self.view.set_analyser(&key, &live.data);
        }
        self.eq_spectra = state;
    }

    /// Whether the tap of the EQ at `key` is switched on for its spectrum.
    pub fn eq_spectrum_live(&self, key: &DeviceKey) -> bool {
        self.eq_spectra.live.contains_key(key)
    }
}
