//! Against the real CLAP backend: state save and restore round trips, state
//! first and rack params after, failures keep the old state, and the capture points.
// Justification: exact copies and reference arithmetic are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{InstanceId, ManagerEvent, NativeParent, PluginManager};
use atomik_test_plugin_core as core;
use atomik_testkit::harness::DeviceHarness;
use support::*;

const PKG: &str = "atomik-test-clap";

fn open_with(
    m: &mut PluginManager,
    id: &str,
    state: Option<PluginStateBlob>,
) -> (DeviceHarness, InstanceId, Option<String>) {
    let mut req = request(id);
    req.state = state;
    let handle = m.instantiate(&req).expect("instantiates");
    (
        DeviceHarness::new(Box::new(handle.device), SR),
        handle.id,
        handle.restore_error,
    )
}

fn blob(bytes: Vec<u8>) -> PluginStateBlob {
    PluginStateBlob(bytes)
}

fn ramp(scale: f32) -> [f32; BATCH] {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 100.0;
    }
    b
}

fn reference_fx() -> core::GainFx {
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::GAIN, -7.0);
    fx.set_param(core::GainFx::LATENCY, 21.0);
    fx.set_param(core::GainFx::DUCK, 0.5);
    fx
}

#[test]
fn a_saved_state_restores_the_plugin_before_it_is_activated() {
    let mut m = manager_for(&[PKG]);
    let reference = reference_fx();
    let saved = blob(reference.save_state());
    let (mut h, id, err) = open_with(&mut m, "atomik.test.gain", Some(saved.clone()));
    assert!(err.is_none(), "{err:?}");
    // The restored latency is what the plugin reports at activation.
    assert_eq!(m.latency(id), Some(21));
    assert_eq!(m.slot(id).unwrap().latency(), 21);
    // And the output is the reference's with that state, bit for bit.
    let mut reference = reference;
    for n in 0..10 {
        let (l, r) = (ramp(1.0 + n as f32), ramp(-2.0));
        *h.input_mut("in_l") = l;
        *h.input_mut("in_r") = r;
        h.process();
        let mut out = [[0.0; BATCH]; 2];
        reference.process(&[l, r], None, &mut out);
        assert_eq!(&h.output("out_l")[..], &out[0][..], "batch {n}");
    }
    assert_eq!(
        m.capture_state(id).unwrap(),
        saved,
        "the same bytes come back"
    );
}

#[test]
fn state_first_and_the_racks_params_after() {
    let mut m = manager_for(&[PKG]);
    let (mut h, id, _) = open_with(
        &mut m,
        "atomik.test.gain",
        Some(blob(reference_fx().save_state())),
    );
    // The rack sets the gain the state would have overridden: the rack wins.
    h.set_param("p1", -3.0);
    *h.input_mut("in_l") = [1.0; BATCH];
    *h.input_mut("in_r") = [1.0; BATCH];
    for _ in 0..6 {
        h.process();
    }
    let want = 10f32.powf(-3.0 / 20.0);
    assert!((h.output("out_l")[BATCH - 1] - want).abs() < 1e-6);
    // The plugin's own state now carries the rack's value.
    let state = m.capture_state(id).unwrap();
    let mut fx = core::GainFx::new(SR);
    assert!(fx.load_state(&state.0));
    assert_eq!(fx.param(core::GainFx::GAIN), -3.0);
    assert_eq!(
        fx.param(core::GainFx::LATENCY),
        21.0,
        "the rest is the state's"
    );
}

#[test]
fn a_state_taken_from_one_instance_makes_another_identical() {
    let mut m = manager_for(&[PKG]);
    let (mut h, a, _) = open_with(&mut m, "atomik.test.synth", None);
    h.set_param("p1", 1.0);
    h.set_param("p2", 0.8);
    h.process();
    h.process();
    let saved = m.capture_state(a).unwrap();
    let (_h2, b, err) = open_with(&mut m, "atomik.test.synth", Some(saved.clone()));
    assert!(err.is_none());
    assert_eq!(m.capture_state(b).unwrap(), saved);
    let mut synth = core::TestSynth::new(SR);
    assert!(synth.load_state(&saved.0));
    assert_eq!(synth.param(core::TestSynth::WAVE), 1.0);
    assert_eq!(synth.param(core::TestSynth::LEVEL), 0.8f32 as f64);
}

#[test]
fn a_large_state_round_trips() {
    let mut m = manager_for(&[PKG]);
    let (mut h, a, _) = open_with(&mut m, "atomik.test.params", None);
    h.set_param("p7", 0.25);
    h.set_param("p1500", 0.5);
    h.process();
    let saved = m.capture_state(a).unwrap();
    assert!(saved.0.len() > 16_000, "{}", saved.0.len());
    let (_h2, b, _) = open_with(&mut m, "atomik.test.params", Some(saved.clone()));
    assert_eq!(m.capture_state(b).unwrap(), saved);
}

#[test]
fn a_failed_restore_keeps_the_instance_and_its_old_state() {
    let mut m = manager_for(&[PKG]);
    let good = blob(reference_fx().save_state());
    let (_h, id, _) = open_with(&mut m, "atomik.test.gain", Some(good.clone()));
    let err = m.restore_state(id, &blob(vec![0xde, 0xad])).unwrap_err();
    assert!(err.to_string().contains("state"), "{err}");
    assert_eq!(
        m.capture_state(id).unwrap(),
        good,
        "never overwritten by a failed load"
    );
}

#[test]
fn a_bad_state_at_instantiation_is_reported_and_the_plugin_still_runs() {
    let mut m = manager_for(&[PKG]);
    let (mut h, _id, err) = open_with(&mut m, "atomik.test.gain", Some(blob(vec![1, 2, 3])));
    assert!(err.is_some(), "the caller keeps the blob and shows FAILED");
    *h.input_mut("in_l") = [0.5; BATCH];
    h.process();
    h.process();
    assert!(
        h.output("out_l").iter().any(|s| *s != 0.0),
        "defaults, not silence"
    );
}

#[test]
fn the_plugins_mark_dirty_becomes_an_event() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (_h, id, _) = open_with(&mut m, "atomik.test.gain", None);
    let native = m.native_handle(id).unwrap();
    assert_eq!(lib.command_raw(native, command::MARK_DIRTY, 0, 0), 1);
    assert_eq!(m.poll(), vec![ManagerEvent::StateDirty(id)]);
}

#[test]
fn closing_the_editor_captures_the_state() {
    let mut m = manager_for(&[PKG]);
    let (mut h, id, _) = open_with(&mut m, "atomik.test.gain", None);
    h.set_param("p1", -9.0);
    h.process();
    h.process();
    let size = m.open_gui(id, fake_parent(), 1.0).expect("opens");
    assert_eq!(size, (320, 200));
    let state = m
        .close_gui(id)
        .unwrap()
        .expect("the state is captured on close");
    let mut fx = core::GainFx::new(SR);
    assert!(fx.load_state(&state.0));
    assert_eq!(fx.param(core::GainFx::GAIN), -9.0);
}

#[test]
fn state_is_captured_before_an_engine_restart_and_survives_it() {
    let mut m = manager_for(&[PKG]);
    let (mut h, id, _) = open_with(&mut m, "atomik.test.gain", None);
    h.set_param("p1", -5.0);
    h.process();
    h.process();
    m.retain(id, true);
    let before = m.capture_state(id).unwrap();
    drop(h);
    m.poll();
    let handle = m.reattach(id, SR).unwrap();
    drop(handle);
    assert_eq!(
        m.capture_state(id).unwrap(),
        before,
        "the same instance, the same state"
    );
}

#[test]
fn the_state_of_a_removed_instance_can_be_taken_just_before() {
    let mut m = manager_for(&[PKG]);
    let (h, id, _) = open_with(&mut m, "atomik.test.synth", None);
    let last = m.capture_state(id).unwrap();
    drop(h);
    m.poll();
    assert!(m.capture_state(id).is_err(), "gone after the reaper");
    assert!(!last.0.is_empty());
}

/// A stand-in for a window handle: the test plugin only records it.
pub fn fake_parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0x1234)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0x1234)
    } else {
        NativeParent::X11(0x1234)
    }
}
