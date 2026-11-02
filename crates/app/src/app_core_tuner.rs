//! The audio track tuner in the app core.
//!
//! The TUNER button of an audio track's rack panel emits [`UiAction::SetTuner`]; the shown state is
//! UI state (not stored, not undoable), kept here per track. While a track's tuner shows:
//!
//! * **The track is told** with `internal.tuner_live` = 1 ([`AppCore::push_tuner_live`], like
//!   `internal.monitor_live`), which makes the device fade its output out when its `tuner_mute`
//!   param is on. The value is sent again after an engine restart and when the device is added
//!   again (a new engine id), and only when it changed.
//! * **The input is read** once per frame by [`AppCore::feed_tuners`] from the device's scope tap
//!   (`EngineSink::read_device_scope`; the audio thread only writes the ring). The tap holds the
//!   input *after* the track's input gain, armed or not, so the tuner hears what the take would
//!   get; the louder of the two channels is analysed ([`atomik_dsp::tuner`], on this thread) at
//!   the project's rate with the track's `tuner_ref` as A4, and the smoothed reading goes to the
//!   rack panel through [`atomik_ui::RackView::set_analyser`]. A tap that did not move since the
//!   last feed (a paused engine) is skipped, so an idle project costs nothing.
//!
//! A track that is removed, or is no audio track, drops out of the set.

use super::*;
use atomik_dsp::tuner::{self, Tuner};
use atomik_ui::analyser::AnalyserData;
use std::collections::BTreeMap;

/// The `internal.tuner_live` param of the audio track device.
const TUNER_LIVE_PARAM: &str = "internal.tuner_live";
/// The reference pitch param of the audio track device.
const TUNER_REF_PARAM: &str = "tuner_ref";

/// What one shown tuner remembers between frames.
#[derive(Debug, Default)]
struct Shown {
    tuner: Tuner,
    /// The tap head the last analysis was made at.
    head: u64,
    /// An analysis was made at least once with this engine.
    fed: bool,
    /// Scratch for the two channels, reused.
    left: Vec<f32>,
    right: Vec<f32>,
}

/// The tuners that are showing and what the engine was told.
#[derive(Debug, Default)]
pub(super) struct Tuners {
    shown: BTreeMap<DeviceKey, Shown>,
    /// `internal.tuner_live` as the engine has it: the device's engine id and the value.
    sent: BTreeMap<DeviceKey, (DeviceInstanceId, bool)>,
}

impl Tuners {
    /// A new engine: every tap starts again from frame 0 and knows nothing of what was sent.
    pub(super) fn restart(&mut self) {
        self.sent.clear();
        for s in self.shown.values_mut() {
            s.head = 0;
            s.fed = false;
        }
    }

    /// Whether the tuner of `key` shows.
    pub(super) fn is_shown(&self, key: &DeviceKey) -> bool {
        self.shown.contains_key(key)
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Shows or hides the tuner of an audio track ([`UiAction::SetTuner`]). A device that is not
    /// an audio track is ignored (its panel is put back to hidden).
    pub(super) fn set_tuner(&mut self, device: DeviceKey, on: bool) {
        let audio = self.doc().is_audio_track(&device, &self.registry);
        if on && audio {
            if self
                .tuners
                .shown
                .insert(device.clone(), Shown::default())
                .is_none()
            {
                let lit = AnalyserData {
                    tuner_on: true,
                    tuner_ref_hz: self.tuner_reference(&device).1,
                    ..AnalyserData::default()
                };
                self.view.set_analyser(&device, &lit);
            }
        } else {
            self.tuners.shown.remove(&device);
            self.view.set_analyser(&device, &AnalyserData::default());
        }
        self.push_tuner_live();
    }

    /// The A4 the tuner of `key` reads against: the track's `tuner_ref` shifted by the master
    /// tune. The second value is what the panel shows instead of its own `tuner_ref`, 0 while the
    /// project is in tune.
    pub(super) fn tuner_reference(&self, key: &DeviceKey) -> (f32, f32) {
        let own = self
            .doc()
            .param_value(key, TUNER_REF_PARAM, &self.registry)
            .unwrap_or(tuner::REF_DEFAULT_HZ);
        let cents = self.doc().tune_cents;
        if cents.to_bits() == 0 {
            return (own, 0.0);
        }
        let shifted = (f64::from(own) * atomik_model::tune_ratio(cents)) as f32;
        (shifted, shifted)
    }

    /// Whether the tuner of an audio track is showing.
    pub fn tuner_shown(&self, device: &DeviceKey) -> bool {
        self.tuners.is_shown(device)
    }

    /// Sends `internal.tuner_live` to the audio tracks whose value the engine does not have yet
    /// (module docs).
    pub(super) fn push_tuner_live(&mut self) {
        let Some(pid) = self
            .registry
            .get(atomik_model::song::AUDIO_TRACK_TYPE)
            .and_then(|f| f.descriptor.param_by_key(TUNER_LIVE_PARAM))
            .map(|p| p.id)
        else {
            return;
        };
        let tracks: Vec<DeviceKey> = self
            .doc()
            .song
            .tracks
            .iter()
            .filter(|t| self.doc().is_audio_track(&t.device, &self.registry))
            .map(|t| t.device.clone())
            .collect();
        self.tuners.shown.retain(|k, _| tracks.contains(k));
        self.tuners.sent.retain(|k, _| tracks.contains(k));
        for key in tracks {
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let want = self.tuners.is_shown(&key);
            match self.tuners.sent.get(&key) {
                Some(&(sent_id, v)) if sent_id == id && v == want => continue,
                // Never sent and not wanted: a new device starts at 0.
                None if !want => {
                    self.tuners.sent.insert(key, (id, false));
                    continue;
                }
                _ => {}
            }
            let result = self.sink.set_param(id, pid, if want { 1.0 } else { 0.0 });
            self.engine_result(result);
            self.tuners.sent.insert(key, (id, want));
        }
    }

    /// Reads the tap of every shown tuner and feeds its panel (module docs); once per frame from
    /// [`Self::frame`].
    pub(super) fn feed_tuners(&mut self) {
        if self.tuners.shown.is_empty() && self.tuners.sent.values().all(|(_, v)| !v) {
            return;
        }
        self.push_tuner_live();
        let sample_rate = self.doc().sample_rate as f32;
        let want = tuner::frames_wanted(sample_rate);
        let keys: Vec<DeviceKey> = self.tuners.shown.keys().cloned().collect();
        for key in keys {
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let (ref_hz, shown_ref) = self.tuner_reference(&key);
            let Some(mut shown) = self.tuners.shown.remove(&key) else {
                continue;
            };
            shown.left.resize(want, 0.0);
            shown.right.resize(want, 0.0);
            let read = self
                .sink
                .read_device_scope(id, &mut shown.left, &mut shown.right);
            if let Some((n, head)) = read
                && head != 0
                && !(shown.fed && head == shown.head)
            {
                let elapsed = if shown.fed { head - shown.head } else { 0 };
                shown.head = head;
                shown.fed = true;
                let (l, r) = (&shown.left[..n], &shown.right[..n]);
                // The louder channel: a mono source may sit on either input.
                let mono = if tuner::rms(l) >= tuner::rms(r) { l } else { r };
                let reading = shown.tuner.update(mono, sample_rate, elapsed, ref_hz);
                let data = AnalyserData {
                    tuner_on: true,
                    tuner: reading,
                    tuner_ref_hz: shown_ref,
                    ..AnalyserData::default()
                };
                self.view.set_analyser(&key, &data);
            }
            self.tuners.shown.insert(key, shown);
        }
    }
}
