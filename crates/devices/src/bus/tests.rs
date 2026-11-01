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
    // `follow_out` comes after the channel's sockets.
    assert_eq!(got.last(), Some(&"follow_out"));
    let got = &got[..got.len() - 1];
    assert_eq!(got, want);
    for s in &DESCRIPTOR.sockets[..CHANNEL.sockets.len()] {
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
fn stage_table_is_the_channels_moved_up_by_one_with_the_follow_gain_in_stage_0() {
    for s in &DESCRIPTOR.sockets[..CHANNEL.sockets.len()] {
        let want = if s.key == "follow_in" {
            0
        } else {
            CHANNEL.stage_of(s.id) + 1
        };
        assert_eq!(DESCRIPTOR.stage_of(s.id), want, "{}", s.key);
    }
    let id = |k: &str| DESCRIPTOR.socket_by_key(k).unwrap().id;
    assert_eq!(DESCRIPTOR.stage_of(id("follow_out")), 0);
    assert_eq!(DESCRIPTOR.stage_count(), 3);
    assert_eq!(DESCRIPTOR.stage_of(id("bus_l")), 1);
    assert_eq!(DESCRIPTOR.stage_of(id("out_l")), 2);
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

/// The last sample of each of `outs` after 20 batches (past the 5 ms ramps).
fn settled(h: &mut DeviceHarness, outs: &[&str]) -> Vec<f32> {
    h.render(20, outs)
        .iter()
        .map(|o| *o.last().unwrap())
        .collect()
}

#[test]
fn follow_out_is_the_fader_times_the_mute() {
    let mut h = bus();
    h.set_connected("follow_out", true);
    *h.input_mut("bus_l") = [0.5; BATCH];
    let o = settled(&mut h, &["follow_out"]);
    assert!((o[0] - atomik_dsp::fader_gain(0.75)).abs() < 1e-6, "{o:?}");
    h.set_param("fader", 0.4);
    let o = settled(&mut h, &["follow_out", "out_l"]);
    let want = atomik_dsp::fader_gain(0.4);
    assert!((o[0] - want).abs() < 1e-6, "{o:?}");
    // The audio stage applies the very gain the follow output carried (the pan is the only
    // other factor on the main output).
    let pan = std::f32::consts::FRAC_1_SQRT_2;
    assert!((o[1] - 0.5 * want * pan).abs() < 1e-5, "{o:?}");
    h.set_param("mute", 1.0);
    let o = settled(&mut h, &["follow_out"]);
    assert!(o[0].abs() < 1e-9, "a muted bus follows at 0: {o:?}");
    h.set_param("mute", 0.0);
    h.set_param("internal.solo_silence", 1.0);
    assert!(settled(&mut h, &["follow_out"])[0].abs() < 1e-9);
}

#[test]
fn follow_out_is_sample_aligned_with_the_main_output_while_the_fader_ramps() {
    let mut h = bus();
    h.set_connected("follow_out", true);
    h.set_param("pan", -1.0); // the left output carries no pan attenuation
    *h.input_mut("bus_l") = [1.0; BATCH];
    h.render(20, &["out_l"]);
    h.set_param("fader", 0.1);
    let o = h.render(2, &["follow_out", "out_l"]);
    for (i, (f, m)) in o[0].iter().zip(&o[1]).enumerate() {
        assert!((f - m).abs() < 1e-6, "sample {i}: {f} vs {m}");
    }
    assert!(o[0].first() != o[0].last(), "the fader really ramped");
}

#[test]
fn follow_out_cascades_the_bus_above() {
    let mut h = bus();
    h.set_connected("follow_out", true);
    dc_in(&mut h, "follow_in", 0.5);
    let o = settled(&mut h, &["follow_out"]);
    assert!(
        (o[0] - 0.5 * atomik_dsp::fader_gain(0.75)).abs() < 1e-6,
        "{o:?}"
    );
}

fn dc_in(h: &mut DeviceHarness, key: &str, v: f32) {
    *h.input_mut(key) = [v; BATCH];
}

#[test]
fn the_sends_of_a_bus_follow_the_bus_above_and_nothing_is_written_uncabled() {
    let mut h = bus();
    dc_in(&mut h, "bus_l", 0.5);
    h.set_param("send1.on", 1.0);
    dc_in(&mut h, "follow_in", 0.25);
    let o = settled(&mut h, &["send1_l", "follow_out"]);
    assert!((o[0] - 0.125).abs() < 1e-5, "{o:?}");
    assert_eq!(o[1], 0.0, "follow_out is not written while uncabled");
}

#[test]
fn processing_by_stages_matches_the_one_call() {
    use atomik_device_sdk::{ProcessContext, SocketKind};
    // The engine runs stage 0 (follow), 1 (input) and 2 (output) one after the other.
    let desc = DESCRIPTOR.sockets;
    let n = desc.len();
    let mut signals = vec![[0.0f32; BATCH]; n];
    let mut notes: Vec<_> = (0..n)
        .map(|_| atomik_core::NoteBuffer::with_capacity(0))
        .collect();
    let mut connected = vec![false; n];
    let id = |k: &str| DESCRIPTOR.socket_by_key(k).unwrap().id.0 as usize;
    connected[id("bus_l")] = true;
    connected[id("follow_out")] = true;
    signals[id("bus_l")] = [0.5; BATCH];
    assert!(desc.iter().all(|s| s.kind != SocketKind::Notes));
    let mut d = new();
    d.prepare(&atomik_device_sdk::PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(48_000.0).unwrap(),
        seed: 0,
    });
    for _ in 0..20 {
        for stage in 0..3u8 {
            let mut ctx = ProcessContext::new(
                atomik_core::TimeInfo::at(
                    0,
                    atomik_core::SampleRate::new(48_000.0).unwrap(),
                    atomik_core::Tempo::new(120.0).unwrap(),
                    true,
                ),
                &mut signals,
                &mut notes,
                &connected,
            );
            d.process_stage(stage, &mut ctx);
        }
    }
    let follow = signals[id("follow_out")][BATCH - 1];
    assert!(
        (follow - atomik_dsp::fader_gain(0.75)).abs() < 1e-6,
        "{follow}"
    );
    let out = signals[id("out_l")][BATCH - 1];
    assert!(out > 0.1, "the audio stages ran: {out}");
}
