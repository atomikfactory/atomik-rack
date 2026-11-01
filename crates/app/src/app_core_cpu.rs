//! The per-device CPU readout in the app core.
//!
//! While the readout is on (View > CPU Meters) the engine times every device's `process`
//! ([`atomik_engine::timing`]). Once per frame the core reads each device's cumulative busy
//! time next to the audio time the engine has rendered, and once that is [`WINDOW_NS`] of audio
//! later it turns the two samples into a load: the share of the audio time the device used.
//! The time base is the engine's own, so a paused engine keeps its last figure and a test needs
//! no clock. Readings are smoothed so a plate does not flicker. A container shows the sum of
//! the devices inside it.

use super::*;
use atomik_engine::timing::{CpuSample, load_percent};

/// Audio time between two readings of a device, in nanoseconds (200 ms).
const WINDOW_NS: u64 = 200_000_000;
/// How far a new reading moves the shown figure (0..=1).
const SMOOTHING: f32 = 0.5;

/// The state of the readout.
#[derive(Debug, Default)]
pub(super) struct CpuMeters {
    on: bool,
    /// The sample each device was last turned into a load from.
    last: BTreeMap<DeviceKey, CpuSample>,
    /// The shown load per flat device, in percent of the audio time.
    load: BTreeMap<DeviceKey, f32>,
}

impl<S: EngineSink> AppCore<S> {
    /// Switches the readout on or off (View > CPU Meters). Off, the engine stops timing and the
    /// figures go.
    pub fn set_cpu_meters(&mut self, on: bool) {
        if self.cpu.on == on {
            return;
        }
        self.cpu.on = on;
        self.sink.set_cpu_timing(on);
        self.cpu.last.clear();
        self.cpu.load.clear();
        self.view.set_cpu_loads(None);
    }

    /// Whether the readout is on.
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

    /// Once per frame: turns the engine's samples into loads. Does nothing while the readout is
    /// off.
    pub(super) fn feed_cpu_loads(&mut self) {
        if !self.cpu.on {
            return;
        }
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
            if now.budget_ns - before.budget_ns < WINDOW_NS {
                continue;
            }
            if let Some(pct) = load_percent(before, now) {
                let shown = match cpu.load.get(key) {
                    Some(old) => old + (pct - old) * SMOOTHING,
                    None => pct,
                };
                cpu.load.insert(key.clone(), shown);
            }
            cpu.last.insert(key.clone(), now);
        }
        // A device that left the rack takes its figure with it.
        let ids = &self.ids;
        cpu.load.retain(|k, _| ids.contains_key(k));
        cpu.last.retain(|k, _| ids.contains_key(k));
        self.view.set_cpu_loads(Some(&self.cpu.load));
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
    fn the_switch_turns_the_engine_timing_on_and_off() {
        let (mut c, key, id) = core_with_synth();
        assert!(!c.sink().cpu_timing);
        c.sink_mut().cpu.insert(id, sample(1, 100));
        c.set_cpu_meters(true);
        assert!(c.sink().cpu_timing && c.cpu_meters_on());
        assert_eq!(c.cpu_load(&key), None, "no figure before two readings");
        c.set_cpu_meters(false);
        assert!(!c.sink().cpu_timing);
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
        // The next window used 50 %: halfway between the old figure and the new one.
        c.sink_mut().cpu.insert(id, sample(25 + 125, 1500));
        c.frame();
        let second = c.cpu_load(&key).unwrap();
        assert!((second - 30.0).abs() < 1e-3, "{second}");
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
        // The engine's clock went back: that reading only restarts the window.
        c.sink_mut().cpu.insert(id, sample(1, 10));
        c.frame();
        let shown = c.cpu_load(&key).unwrap();
        c.sink_mut().cpu.insert(id, sample(1, 400));
        c.frame();
        assert!(c.cpu_load(&key).unwrap() < shown, "the new window is idle");
    }
}
