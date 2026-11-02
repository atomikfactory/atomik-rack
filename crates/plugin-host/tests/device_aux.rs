//! The aux outputs of a plugin device: the pairs of `out_aux_<k>_l/r` sockets carry what the
//! processor writes to `PluginIo::aux_out`, fade out with the bypass crossfade, and are silent
//! when the batch has no wet signal. Nothing allocates (the harness runs `process` under the
//! guard).
// Justification: the processor writes exact constants and the tests compare them exactly.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{PluginIo, PluginProcessor, ProcessStatus};
use atomik_testkit::harness::DeviceHarness;
use support::*;

/// Writes the main output 0.5 and two aux pairs: (0.25, 0.125) and (0.0625, -0.0625).
struct Multi;

impl PluginProcessor for Multi {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        for ch in io.main_out.iter_mut() {
            *ch = [0.5; BATCH];
        }
        let levels = [0.25, 0.125, 0.0625, -0.0625];
        for (ch, level) in io.aux_out.iter_mut().zip(levels) {
            *ch = [level; BATCH];
        }
        ProcessStatus::Continue
    }
}

fn rig_with_aux(id: &str, pairs: u8) -> (DeviceHarness, Rest) {
    let mut layout = synth_layout();
    layout.aux_out = pairs;
    harness(rig(id, &layout, Box::new(Multi)))
}

fn all(h: &DeviceHarness, key: &str, level: f32) -> bool {
    h.output(key).iter().all(|s| *s == level)
}

#[test]
fn each_aux_pair_carries_its_own_channels() {
    let (mut h, _r) = rig_with_aux("aux.pairs", 2);
    h.process();
    assert!(all(&h, "out_l", 0.5) && all(&h, "out_r", 0.5));
    assert!(all(&h, "out_aux_1_l", 0.25) && all(&h, "out_aux_1_r", 0.125));
    assert!(all(&h, "out_aux_2_l", 0.0625) && all(&h, "out_aux_2_r", -0.0625));
}

#[test]
fn a_plugin_with_one_pair_leaves_the_second_processor_channels_unused() {
    let (mut h, _r) = rig_with_aux("aux.one", 1);
    h.process();
    assert!(all(&h, "out_aux_1_l", 0.25) && all(&h, "out_aux_1_r", 0.125));
    assert!(
        h.device()
            .descriptor()
            .socket_by_key("out_aux_2_l")
            .is_none()
    );
}

#[test]
fn bypass_fades_the_aux_outputs_to_silence_and_back() {
    let (mut h, _r) = rig_with_aux("aux.bypass", 1);
    h.process();
    assert!(all(&h, "out_aux_1_l", 0.25));
    h.set_param("bypass", 1.0);
    // The 5 ms crossfade is 240 frames at 48 kHz: a few batches, never a jump.
    h.process();
    let first = h.output("out_aux_1_l")[0];
    let last = h.output("out_aux_1_l")[BATCH - 1];
    assert!(first <= 0.25 && last < first, "fading: {first} then {last}");
    assert!(last > 0.0, "not cut in the first batch");
    for _ in 0..6 {
        h.process();
    }
    assert!(all(&h, "out_aux_1_l", 0.0) && all(&h, "out_aux_1_r", 0.0));
    h.set_param("bypass", 0.0);
    for _ in 0..6 {
        h.process();
    }
    assert!(all(&h, "out_aux_1_l", 0.25));
}

#[test]
fn a_failed_device_writes_silent_aux_outputs() {
    struct Fails;
    impl PluginProcessor for Fails {
        fn start_processing(&mut self) -> bool {
            true
        }
        fn stop_processing(&mut self) {}
        fn reset(&mut self) {}
        fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
            io.aux_out[0] = [0.9; BATCH];
            ProcessStatus::Error
        }
    }
    let mut layout = synth_layout();
    layout.aux_out = 1;
    let (mut h, _rest) = harness(rig("aux.fails", &layout, Box::new(Fails)));
    h.process();
    h.process();
    assert!(
        all(&h, "out_aux_1_l", 0.0),
        "nothing of a failed batch leaks"
    );
}
