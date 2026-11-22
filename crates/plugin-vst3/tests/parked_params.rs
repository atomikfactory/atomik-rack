//! A value set while an effect is parked by a long bypass, or powered off, is in the plugin's saved
//! state, as it is when the plugin runs: the device sends it in one call with silent input,
//! in both formats. (A value that waited for the next real block would be missing from a project
//! saved meanwhile.)

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_device_sdk::RunMode;
use atomik_test_plugin_core as core;
use support::*;

/// Blocks of the park time at 48 kHz (10 s).
const PARK: usize = 7_500;

enum Case {
    Running,
    Parked,
    PoweredOff,
}

/// The state captured after the gain was set to -6 dB in `case`, and how often the plugin was
/// called after the set.
fn captured(format: Format, case: &Case) -> (Vec<u8>, u64) {
    let _serial = serial();
    let k = kit(format);
    let lib = TestLib::open_path(&k.path);
    let mut rig = rig_for(&*k.backend, &k.plugin(Plug::Gain), RunMode::Realtime);
    let mut h = rig.harness();
    *h.input_mut("in_l") = [0.25; atomik_core::BATCH];
    *h.input_mut("in_r") = [0.25; atomik_core::BATCH];
    match case {
        Case::Running => {}
        Case::Parked => {
            h.set_param("bypass", 1.0);
            for _ in 0..PARK + 100 {
                h.process();
            }
        }
        Case::PoweredOff => {
            rig.manager.slot().set_powered(false);
            for _ in 0..50 {
                h.process();
            }
        }
    }
    let before = lib.probe(&*rig.instance, probe::PROCESS_CALLS);
    let (host, _) = k.host_value(Plug::Gain, core::GainFx::GAIN, -6.0);
    h.set_param(&format!("p{}", core::GainFx::GAIN), host as f32);
    for _ in 0..5 {
        h.process();
    }
    let calls = lib.probe(&*rig.instance, probe::PROCESS_CALLS) - before;
    let blob = rig.instance.capture_state().expect("the state is captured");
    (component_state(format, &blob), calls)
}

#[test]
fn a_value_set_while_parked_or_powered_off_is_in_the_saved_state_in_both_formats() {
    for format in FORMATS {
        let (running, _) = captured(format, &Case::Running);
        let (parked, calls) = captured(format, &Case::Parked);
        assert_eq!(parked, running, "{format:?}: parked");
        assert_eq!(calls, 1, "{format:?}: one call carries the value");
        let (off, calls) = captured(format, &Case::PoweredOff);
        assert_eq!(off, running, "{format:?}: powered off");
        assert_eq!(calls, 1, "{format:?}: one call carries the value");
    }
}
