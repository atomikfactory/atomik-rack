//! The per-device CPU readout in the app core.
//!
//! The engine times every device's `process` call all the time ([`atomik_engine::timing`]; the
//! cost is about 0.5 % of a 64-frame callback for a hundred devices). Once per frame the core
//! reads each device's busy time next to the audio time the engine has rendered, and once that is
//! [`WINDOW_NS`] of audio later it turns the two samples into a load: the share of the audio time
//! the device used. The time base is the engine's own, so a paused engine keeps its last figure
//! and a test needs no clock. Each device has a [`Gauge`]: the load averaged over about a second
//! and a held peak. A container shows the sum of the devices inside it.
//!
//! What the readout feeds:
//! - the plates of the rack and the name plates of the console strips (the track's total: its
//!   instrument, its channel and its insert effects), while View > Show Device CPU is on;
//! - the tooltip of the status line, which lists the five heaviest devices with their tracks;
//! - the freeze hint ([`HintWatch`]): when the engine load stays high, the track that uses the
//!   most is suggested for freezing.

use super::*;
use crate::cpu_gauge::{Gauge, Hint, HintWatch};
use atomik_engine::timing::{CpuSample, load_percent};
use atomik_model::track_chain;

/// Audio time between two readings of a device, in nanoseconds (200 ms).
const WINDOW_NS: u64 = 200_000_000;
/// Loads under this many percent are left out of the heaviest-devices list.
const LIST_FLOOR: f32 = 0.05;

/// One device of the heaviest-devices list.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCpu {
    /// The device's name (a container's name for everything inside it).
    pub name: String,
    /// The track whose chain the device belongs to, when it is not the device's own name.
    pub track: Option<String>,
    /// The smoothed load in percent of the audio time.
    pub percent: f32,
    /// The held peak in percent of the audio time.
    pub peak: f32,
}

/// What one track's chain uses.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackCpu {
    /// The track (its instrument's key).
    pub track: DeviceKey,
    /// The track's console channel.
    pub channel: DeviceKey,
    /// The track name.
    pub name: String,
    /// The sum of its instrument, channel and insert effects, in percent of the audio time.
    pub percent: f32,
    /// Whether Freeze Track can take it.
    pub freezable: bool,
}

/// The state of the readout.
#[derive(Debug, Default)]
pub(super) struct CpuMeters {
    /// Whether the figures show (View > Show Device CPU). The engine measures either way.
    on: bool,
    /// The sample each device was last turned into a load from.
    last: BTreeMap<DeviceKey, CpuSample>,
    /// The figure of each flat device.
    gauges: BTreeMap<DeviceKey, Gauge>,
    /// The smoothed load per flat device, in percent of the audio time.
    load: BTreeMap<DeviceKey, f32>,
    /// What each track's chain uses, heaviest first.
    tracks: Vec<TrackCpu>,
    /// The track each top-level device belongs to, by name.
    owner: BTreeMap<DeviceKey, String>,
    /// The freeze hint watch.
    hint: HintWatch,
}

impl<S: EngineSink> AppCore<S> {
    /// Shows the figures on the plates and strips or hides them (View > Show Device CPU). The engine
    /// keeps measuring either way.
    pub fn set_cpu_meters(&mut self, on: bool) {
        if self.cpu.on == on {
            return;
        }
        self.cpu.on = on;
        self.show_cpu_loads();
    }

    /// Whether the figures show.
    pub fn cpu_meters_on(&self) -> bool {
        self.cpu.on
    }

    /// The share of the audio time device `key` used, in percent of one callback's budget
    /// (a container: the sum of the devices inside it); `None` while the readout is off or has
    /// no figure for it yet. Above 100 the device alone took longer than the time it had.
    pub fn cpu_load(&self, key: &DeviceKey) -> Option<f32> {
        if !self.cpu.on {
            return None;
        }
        atomik_ui::rack::load_under(&self.cpu.load, key)
    }

    /// The load of every device the readout has a figure for (flat keys), for the plates.
    pub fn cpu_loads(&self) -> &BTreeMap<DeviceKey, f32> {
        &self.cpu.load
    }

    /// The held peak of device `key` (flat key), in percent of the audio time.
    pub fn cpu_peak(&self, key: &DeviceKey) -> Option<f32> {
        self.cpu.gauges.get(key).map(Gauge::peak)
    }

    /// What each track's chain uses (instrument, channel and insert effects), heaviest first.
    pub fn track_cpu(&self) -> &[TrackCpu] {
        &self.cpu.tracks
    }

    /// The `n` heaviest devices (a container counts as one device), heaviest first. Empty until
    /// the first readings are in.
    pub fn heaviest_devices(&self, n: usize) -> Vec<DeviceCpu> {
        let mut sums: BTreeMap<DeviceKey, (f32, f32)> = BTreeMap::new();
        for (flat, gauge) in &self.cpu.gauges {
            let top = flat.0.split('/').next().unwrap_or(&flat.0);
            let entry = sums.entry(DeviceKey(top.to_owned())).or_default();
            entry.0 += gauge.average();
            entry.1 += gauge.peak();
        }
        let mut rows: Vec<_> = sums
            .into_iter()
            .filter(|(_, (percent, _))| *percent >= LIST_FLOOR)
            .collect();
        rows.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
        rows.into_iter()
            .take(n)
            .map(|(key, (percent, peak))| {
                let name = self.device_label(&key);
                let track = self.cpu.owner.get(&key).filter(|t| **t != name).cloned();
                DeviceCpu {
                    name,
                    track,
                    percent,
                    peak,
                }
            })
            .collect()
    }

    /// The name of device `key` for a message (the key when it has gone).
    pub fn device_label(&self, key: &DeviceKey) -> String {
        self.doc()
            .device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    }

    /// Looks at the engine for the freeze hint: `engine_load` is the callback load in percent.
    /// `allowed` is false while something else is on screen that the hint must not cover; the
    /// watch then starts again. Returns the track to suggest when it is time (once per track
    /// per song).
    pub fn cpu_hint_poll(&mut self, engine_load: f32, allowed: bool) -> Option<Hint> {
        let playing = allowed && self.is_playing();
        let heaviest = self
            .cpu
            .tracks
            .iter()
            .find(|t| t.freezable && !self.cpu.hint.told(&t.track))
            .map(|t| (t.track.clone(), t.percent));
        let now = self.now_seconds;
        self.cpu.hint.step(now, playing, engine_load, heaviest)
    }

    /// Forgets the figures and the hints already given (a new song).
    pub(super) fn cpu_reset(&mut self) {
        let on = self.cpu.on;
        self.cpu = CpuMeters {
            on,
            ..CpuMeters::default()
        };
        self.show_cpu_loads();
    }

    /// Hands the figures to the views, or takes them away while the readout is off.
    fn show_cpu_loads(&mut self) {
        if !self.cpu.on {
            self.view.set_cpu_loads(None);
            self.console.set_cpu_loads(None);
            return;
        }
        self.view.set_cpu_loads(Some(&self.cpu.load));
        let strips: BTreeMap<DeviceKey, f32> = self
            .cpu
            .tracks
            .iter()
            .map(|t| (t.channel.clone(), t.percent))
            .collect();
        self.console.set_cpu_loads(Some(&strips));
    }

    /// Once per frame: turns the engine's samples into loads.
    pub(super) fn feed_cpu_loads(&mut self) {
        let mut moved = false;
        let cpu = &mut self.cpu;
        for (key, id) in &self.ids {
            let Some(now) = self.sink.read_device_cpu(*id) else {
                continue;
            };
            let Some(before) = cpu.last.get(key).copied() else {
                cpu.last.insert(key.clone(), now);
                continue;
            };
            if now.budget_ns < before.budget_ns {
                // The engine was rebuilt: its clock started again.
                cpu.last.insert(key.clone(), now);
                continue;
            }
            let window = now.budget_ns - before.budget_ns;
            if window < WINDOW_NS {
                continue;
            }
            if let Some(pct) = load_percent(before, now) {
                let gauge = cpu.gauges.entry(key.clone()).or_default();
                gauge.push(pct, window);
                cpu.load.insert(key.clone(), gauge.average());
                moved = true;
            }
            cpu.last.insert(key.clone(), now);
        }
        // A device that left the rack takes its figure with it.
        let ids = &self.ids;
        let before = cpu.gauges.len();
        cpu.gauges.retain(|k, _| ids.contains_key(k));
        cpu.load.retain(|k, _| ids.contains_key(k));
        cpu.last.retain(|k, _| ids.contains_key(k));
        if moved || cpu.gauges.len() != before {
            self.rebuild_track_cpu();
        }
        if self.cpu.on {
            self.show_cpu_loads();
        }
    }

    /// Sums every track's chain (see [`TrackCpu`]) from the device loads.
    fn rebuild_track_cpu(&mut self) {
        let doc = self.doc();
        let loads = &self.cpu.load;
        let under = |k: &DeviceKey| atomik_ui::rack::load_under(loads, k).unwrap_or(0.0);
        let mut tracks = Vec::new();
        let mut owner = BTreeMap::new();
        for t in &doc.song.tracks {
            let Ok(chain) = track_chain(doc, &self.registry, &t.device) else {
                continue;
            };
            let name = doc
                .device(&t.device)
                .map_or_else(|| t.device.0.clone(), |d| d.name.clone());
            let percent = under(&t.device)
                + under(&chain.channel)
                + chain.effects.iter().map(under).sum::<f32>();
            for key in std::iter::once(&t.device)
                .chain(std::iter::once(&chain.channel))
                .chain(chain.effects.iter())
            {
                owner.insert(key.clone(), name.clone());
            }
            let freezable = t.frozen.is_none()
                && !t.is_automation_only()
                && atomik_model::clips_span(t).is_some();
            tracks.push(TrackCpu {
                track: t.device.clone(),
                channel: chain.channel,
                name,
                percent,
                freezable,
            });
        }
        tracks.sort_by(|a, b| b.percent.total_cmp(&a.percent));
        self.cpu.tracks = tracks;
        self.cpu.owner = owner;
    }
}

#[cfg(test)]
mod core_tests {
    use super::*;
    use crate::sink::RecordingSink;
    use atomik_model::Command;

    fn sample(busy_ms: u64, budget_ms: u64) -> CpuSample {
        CpuSample {
            busy_ns: busy_ms * 1_000_000,
            budget_ns: budget_ms * 1_000_000,
        }
    }

    fn core_with_synth() -> (AppCore<RecordingSink>, DeviceKey, DeviceInstanceId) {
        let mut c = AppCore::new(RecordingSink::new());
        let key = DeviceKey("synth".into());
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key.clone()),
        })
        .unwrap();
        let id = c.device_id(&key).unwrap();
        (c, key, id)
    }

    #[test]
    fn the_engine_measures_from_the_start_and_the_switch_only_shows_the_figures() {
        let (mut c, key, id) = core_with_synth();
        assert!(c.sink().cpu_timing, "measuring is always on");
        c.sink_mut().cpu.insert(id, sample(0, 1000));
        c.frame();
        c.sink_mut().cpu.insert(id, sample(25, 1250));
        c.frame();
        assert_eq!(c.cpu_load(&key), None, "hidden while the switch is off");
        assert!(c.cpu_loads().contains_key(&key), "but measured");
        c.set_cpu_meters(true);
        assert!(c.cpu_meters_on());
        assert!((c.cpu_load(&key).unwrap() - 10.0).abs() < 1e-3);
        c.set_cpu_meters(false);
        assert!(c.sink().cpu_timing, "the switch does not stop the engine");
        assert_eq!(c.cpu_load(&key), None);
    }

    #[test]
    fn a_load_appears_after_a_window_of_audio_and_is_smoothed() {
        let (mut c, key, id) = core_with_synth();
        c.set_cpu_meters(true);
        c.sink_mut().cpu.insert(id, sample(0, 1000));
        c.frame();
        // 100 ms of audio is less than a window: nothing yet.
        c.sink_mut().cpu.insert(id, sample(10, 1100));
        c.frame();
        assert_eq!(c.cpu_load(&key), None);
        // 250 ms since the first reading, 25 ms busy: 10 %.
        c.sink_mut().cpu.insert(id, sample(25, 1250));
        c.frame();
        let first = c.cpu_load(&key).unwrap();
        assert!((first - 10.0).abs() < 1e-3, "{first}");
        // The next window used 50 %: the figure moves part of the way (about a second of
        // audio averages it) and the peak holds the new window.
        c.sink_mut().cpu.insert(id, sample(25 + 125, 1500));
        c.frame();
        let second = c.cpu_load(&key).unwrap();
        let alpha = 1.0 - (-0.25f32).exp();
        assert!((second - (10.0 + 40.0 * alpha)).abs() < 1e-2, "{second}");
        assert!((c.cpu_peak(&key).unwrap() - 50.0).abs() < 1e-3);
    }

    #[test]
    fn a_rebuilt_engine_starts_the_readout_again() {
        let (mut c, key, id) = core_with_synth();
        c.set_cpu_meters(true);
        c.sink_mut().cpu.insert(id, sample(0, 1000));
        c.frame();
        c.sink_mut().cpu.insert(id, sample(50, 1500));
        c.frame();
        assert!(c.cpu_load(&key).is_some());
        // The engine clock went back: that reading only restarts the window.
        c.sink_mut().cpu.insert(id, sample(1, 10));
        c.frame();
        let shown = c.cpu_load(&key).unwrap();
        c.sink_mut().cpu.insert(id, sample(1, 400));
        c.frame();
        assert!(c.cpu_load(&key).unwrap() < shown, "the new window is idle");
    }

    #[test]
    fn a_new_song_forgets_the_figures_and_keeps_the_switch() {
        let (mut c, key, id) = core_with_synth();
        c.set_cpu_meters(true);
        c.sink_mut().cpu.insert(id, sample(0, 1000));
        c.frame();
        c.sink_mut().cpu.insert(id, sample(50, 1500));
        c.frame();
        assert!(c.cpu_load(&key).is_some());
        c.new_document();
        assert!(c.cpu_meters_on());
        assert!(c.cpu_loads().is_empty());
        assert!(c.track_cpu().is_empty());
    }
}
