//! `atomik.bus` tests: the shared strip with a summing input.

use super::*;
use crate::channel::{DESCRIPTOR as CHANNEL, SENDS};
use crate::test_support::{harness, sine, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_device_sdk::Device;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

fn bus() -> DeviceHarness {
    harness(Box::new(new()))
}

fn channel() -> DeviceHarness {
    harness(Box::new(Channel::new()))
}

/// Outputs compared bit for bit: `out`, the gain-reduction CV and every send.
fn watched() -> Vec<String> {
    let mut v = vec!["out_l".to_string(), "out_r".into(), "gr_cv_out".into()];
    for k in 1..=SENDS {
        v.push(format!("send{k}_l"));
        v.push(format!("send{k}_r"));
    }
    v
}

/// Feeds a two-tone signal into `l`/`r` (only `l` when not `stereo`) and returns the bits of
/// every watched output.
fn feed(h: &mut DeviceHarness, l: &str, r: &str, batches: usize, stereo: bool) -> Vec<Vec<u32>> {
    let keys = watched();
    let mut out: Vec<Vec<u32>> = keys.iter().map(|_| Vec::new()).collect();
    let (fl, fr) = (sine(440.0, 0.4), sine(660.0, 0.3));
    for b in 0..batches {
        let (mut xl, mut xr) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        for i in 0..BATCH {
            xl[i] = fl(b * BATCH + i);
            xr[i] = fr(b * BATCH + i);
        }
        *h.input_mut(l) = xl;
        if stereo {
            *h.input_mut(r) = xr;
        }
        h.process();
        for (dst, k) in out.iter_mut().zip(&keys) {
            dst.extend(h.output(k).iter().map(|x| x.to_bits()));
        }
    }
    out
}

fn same_params(a: &mut DeviceHarness, b: &mut DeviceHarness, set: &[(&str, f32)]) {
    for (k, v) in set {
        a.set_param(k, *v);
        b.set_param(k, *v);
    }
}

#[test]
fn descriptor_validates_and_is_a_mixer_named_atomik_bus() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(DESCRIPTOR.type_id, "atomik.bus");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Mixer);
}

#[test]
fn sockets_are_the_channels_with_a_summing_input_in_place_of_in_l_r() {
    let want: Vec<&str> = CHANNEL
        .sockets
        .iter()
        .map(|s| match s.key {
            "in_l" => "bus_l",
            "in_r" => "bus_r",
            k => k,
        })
        .collect();
    let got: Vec<&str> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(got, want);
    for s in DESCRIPTOR.sockets {
        let c = CHANNEL.sockets.iter().find(|c| c.id == s.id).unwrap();
        assert_eq!((s.kind, s.id), (c.kind, c.id), "{}", s.key);
        if s.key.starts_with("bus_") {
            assert_eq!(s.direction, Direction::SummingInput);
        } else {
            assert_eq!(s.direction, c.direction, "{}", s.key);
        }
    }
    assert!(DESCRIPTOR.socket_by_key("in_l").is_none());
    assert!(DESCRIPTOR.socket_by_key("send8_r").is_some());
}

#[test]
fn params_are_the_channels_exactly_including_internal_ones() {
    assert_eq!(DESCRIPTOR.params.len(), CHANNEL.params.len());
    for (a, b) in DESCRIPTOR.params.iter().zip(CHANNEL.params) {
        assert_eq!(
            (a.id, a.key, a.min, a.max, a.default),
            (b.id, b.key, b.min, b.max, b.default)
        );
    }
    assert!(DESCRIPTOR.param_by_key("internal.solo_silence").is_some());
    assert!(DESCRIPTOR.param_by_key("send8.pre").is_some());
}

#[test]
fn stage_table_is_the_channels_with_the_bus_input_in_stage_0() {
    for s in DESCRIPTOR.sockets {
        assert_eq!(
            DESCRIPTOR.stage_of(s.id),
            CHANNEL.stage_of(s.id),
            "{}",
            s.key
        );
    }
    let id = |k: &str| DESCRIPTOR.socket_by_key(k).unwrap().id;
    assert_eq!(DESCRIPTOR.stage_of(id("bus_l")), 0);
    assert_eq!(DESCRIPTOR.stage_of(id("out_l")), 1);
}

#[test]
fn factory_creates_a_bus_and_the_channel_still_reports_its_own_descriptor() {
    assert_eq!((factory().create)().descriptor().type_id, "atomik.bus");
    assert_eq!(Channel::new().descriptor().type_id, "atomik.channel");
}

#[test]
fn default_bus_is_bit_identical_to_a_channel_fed_the_same_stereo_signal() {
    let (mut b, mut c) = (bus(), channel());
    let ob = feed(&mut b, "bus_l", "bus_r", 40, true);
    let oc = feed(&mut c, "in_l", "in_r", 40, true);
    assert_eq!(ob, oc);
    assert!(ob[0].iter().any(|x| f32::from_bits(*x) != 0.0));
}

#[test]
fn mono_fallback_matches_the_channel() {
    let (mut b, mut c) = (bus(), channel());
    assert_eq!(
        feed(&mut b, "bus_l", "bus_r", 20, false),
        feed(&mut c, "in_l", "in_r", 20, false)
    );
}

#[test]
fn a_fully_loaded_bus_is_bit_identical_to_a_channel() {
    let set = [
        ("hpf.on", 1.0),
        ("eq.on", 1.0),
        ("eq.hf.gain", 4.0),
        ("comp.on", 1.0),
        ("comp.threshold", -24.0),
        ("gate.on", 1.0),
        ("gate.threshold", -60.0),
        ("send1.on", 1.0),
        ("send1.level", -6.0),
        ("send2.on", 1.0),
        ("send2.pre", 1.0),
        ("pan", -0.3),
        ("fader", 0.6),
        ("input.gain", 3.0),
    ];
    let (mut b, mut c) = (bus(), channel());
    same_params(&mut b, &mut c, &set);
    assert_eq!(
        feed(&mut b, "bus_l", "bus_r", 60, true),
        feed(&mut c, "in_l", "in_r", 60, true)
    );
}

#[test]
fn sends_carry_the_bus_signal() {
    let mut h = bus();
    h.set_param("send3.on", 1.0);
    h.set_param("send3.level", 0.0);
    h.set_param("send3.pre", 1.0);
    for _ in 0..30 {
        *h.input_mut("bus_l") = [0.5; BATCH];
        *h.input_mut("bus_r") = [0.25; BATCH];
        h.process();
    }
    assert!((h.output("send3_l")[BATCH - 1] - 0.5).abs() < 1e-5);
    assert!((h.output("send3_r")[BATCH - 1] - 0.25).abs() < 1e-5);
    assert_eq!(h.output("send4_l")[BATCH - 1], 0.0);
}

#[test]
fn latency_adjust_is_reported_like_the_channel() {
    for (adj, want) in [(-480.0, 480), (0.0, 0), (240.0, 0)] {
        let mut h = bus();
        h.set_param("latency.adjust", adj);
        assert_eq!(h.device_mut().latency_frames(), want);
    }
}

#[test]
fn set_param_every_param_min_max_does_not_panic() {
    let mut h = bus();
    for k in ["bus_l", "bus_r", "key_in", "insert_return_l"] {
        *h.input_mut(k) = [0.3; BATCH];
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn process_is_allocation_free() {
    let mut h = bus();
    for (i, k) in [
        "bus_l",
        "bus_r",
        "key_in",
        "insert_return_l",
        "insert_return_r",
    ]
    .iter()
    .enumerate()
    {
        *h.input_mut(k) = [0.1 * i as f32; BATCH];
    }
    for i in 0..200 {
        let p = &DESCRIPTOR.params[i % DESCRIPTOR.params.len()];
        let v = p.from_normalized((i as f32 * 0.37).fract());
        assert_no_alloc(|| h.set_param(p.key, v));
        h.process();
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.process();
}

#[test]
fn reset_restores_a_fresh_bus() {
    let setup = |h: &mut DeviceHarness| {
        h.set_param("comp.on", 1.0);
        h.set_param("send1.on", 1.0);
    };
    let mut h = bus();
    setup(&mut h);
    feed(&mut h, "bus_l", "bus_r", 30, true);
    h.device_mut().reset();
    let after = feed(&mut h, "bus_l", "bus_r", 10, true);
    let mut fresh = bus();
    setup(&mut fresh);
    assert_eq!(after, feed(&mut fresh, "bus_l", "bus_r", 10, true));
}

#[test]
fn same_seed_gives_the_same_output() {
    let run = || {
        let mut h = bus();
        h.set_param("eq.on", 1.0);
        feed(&mut h, "bus_l", "bus_r", 20, true)
    };
    assert_eq!(run(), run());
}

#[test]
fn output_does_not_depend_on_the_length_of_the_run() {
    // The strip only sees BATCH-sized blocks: a shorter run is a prefix of a longer one.
    let long = feed(&mut bus(), "bus_l", "bus_r", 40, true);
    let short = feed(&mut bus(), "bus_l", "bus_r", 25, true);
    for (l, s) in long.iter().zip(&short) {
        assert_eq!(&l[..s.len()], &s[..]);
    }
}
