//! A channel or bus strip says it is quiescent only when a silent batch would change nothing but
//! what `idle_advance` advances, and skipping the batches of a silent stretch leaves every
//! output, every meter value and every state where processing them would: each stage of the
//! strip rests on its own rule (see the module docs of `channel`), so the strip is run through
//! the silence with each stage on.

use super::*;
use crate::test_support::{IdleRun, StageRig, harness, skip_matches_process};
use atomik_testkit::harness::DeviceHarness;

/// Every output the strip fills, the parallel pair included (it is written only while cabled).
const OUTS: [&str; 15] = [
    "insert_send_l",
    "insert_send_r",
    "send1_l",
    "send1_r",
    "send2_l",
    "send2_r",
    "send3_l",
    "send3_r",
    "send8_l",
    "send8_r",
    "out_l",
    "out_r",
    "gr_cv_out",
    "parallel_out_l",
    "parallel_out_r",
];

/// The settings of a strip with every section on and something to do in every stage.
const FULL: [(&str, f32); 26] = [
    ("input.gain", 3.0),
    ("hpf.on", 1.0),
    ("lpf.on", 1.0),
    ("lpf.freq", 9_000.0),
    ("comp.on", 1.0),
    ("comp.threshold", -30.0),
    ("comp.ratio", 6.0),
    ("comp.release", 120.0),
    ("comp.auto_makeup", 1.0),
    ("eq.on", 1.0),
    ("eq.hf.gain", 5.0),
    ("eq.hmf.gain", -4.0),
    ("eq.lmf.gain", 3.0),
    ("eq.lf.gain", 4.0),
    ("pcomp.on", 1.0),
    ("pcomp.mix", 0.4),
    ("latency.adjust", 120.0),
    ("width", 0.7),
    ("pan", -0.4),
    ("fader", 0.6),
    ("send1.on", 1.0),
    ("send2.on", 1.0),
    ("send2.pre", 1.0),
    ("send3.on", 1.0),
    ("send3.level", -6.0),
    ("send8.on", 1.0),
];

fn noise(state: &mut u32) -> [f32; BATCH] {
    let mut b = [0.0; BATCH];
    for x in &mut b {
        *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *x = (*state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
    }
    b
}

fn feed(h: &mut DeviceHarness, ins: &[&str]) {
    let mut s = 5;
    for key in ins {
        *h.input_mut(key) = noise(&mut s);
    }
    h.process();
}

fn silence(h: &mut DeviceHarness, ins: &[&str]) {
    for key in ins {
        *h.input_mut(key) = [0.0; BATCH];
    }
    h.process();
}

/// A strip with every section on. `bus` builds the output bus instead of the channel.
fn full(bus: bool) -> DeviceHarness {
    let mut h = harness(if bus {
        Box::new(crate::bus::new())
    } else {
        Box::new(Channel::new())
    });
    for (k, v) in FULL {
        h.set_param(k, v);
    }
    for key in ["parallel_out_l", "parallel_out_r"] {
        h.set_connected(key, true);
    }
    h
}

fn ins(bus: bool) -> [&'static str; 2] {
    if bus {
        ["bus_l", "bus_r"]
    } else {
        ["in_l", "in_r"]
    }
}

#[test]
fn it_is_quiescent_only_after_every_stage_has_run_dry() {
    for bus in [false, true] {
        let ins = ins(bus);
        let mut h = full(bus);
        assert!(!h.device().quiescent(), "nothing has run");
        silence(&mut h, &ins);
        let mut batches = 0;
        while !h.device().quiescent() {
            silence(&mut h, &ins);
            batches += 1;
            assert!(batches < 20_000, "bus {bus}: a fresh strip never rested");
        }
        for _ in 0..30 {
            feed(&mut h, &ins);
            assert!(!h.device().quiescent(), "bus {bus}: sound");
        }
        let mut batches = 0;
        while !h.device().quiescent() {
            silence(&mut h, &ins);
            batches += 1;
            assert!(
                batches < 20_000,
                "bus {bus}: it never rested after the sound"
            );
        }
        // The latency line's ring is 8,192 samples (128 batches): the strip waits for all of it.
        assert!(batches >= 128, "bus {bus}: {batches} batches");
        // Any param change is heard first.
        h.set_param("fader", 0.5);
        silence(&mut h, &ins);
        assert!(
            !h.device().quiescent(),
            "bus {bus}: the fader is still moving"
        );
    }
}

#[test]
fn a_busy_strip_is_never_quiescent() {
    let mut h = full(false);
    for _ in 0..200 {
        feed(&mut h, &["in_l", "in_r"]);
        assert!(!h.device().quiescent());
    }
}

/// What keeps a strip from resting: a gate that is on (its closed gain shows on `gr_cv_out`),
/// polarity (a negative gain makes `-0.0`), the hard right pan (its gain is a hair below zero),
/// a CV that is cabled and the look-ahead (its delay lines are not watched). Each is run through
/// silence: the strip never skips, and it is still the same bits.
#[test]
fn what_cannot_prove_rest_keeps_running() {
    type Setup = (&'static str, fn(&mut DeviceHarness));
    let cases: [Setup; 6] = [
        ("gate", |h| {
            h.set_param("gate.on", 1.0);
            h.set_param("gate.threshold", -40.0);
        }),
        ("phase", |h| h.set_param("input.phase", 1.0)),
        ("phase right", |h| h.set_param("input.phase_r", 1.0)),
        ("hard right pan", |h| h.set_param("pan", 1.0)),
        ("fader cv", |h| h.set_connected("fader_cv_in", true)),
        ("look-ahead", |h| h.set_param("comp.lookahead", 4.0)),
    ];
    for (name, setup) in cases {
        let skipped = skip_matches_process(
            &|| {
                let mut h = full(false);
                setup(&mut h);
                h
            },
            &IdleRun {
                ins: &["in_l", "in_r"],
                outs: &OUTS,
                batches: 700,
                sound: &|n| n < 30 || (500..520).contains(&n),
                edit: &|_, _| false,
            },
        );
        assert_eq!(skipped, 0, "{name} never rests");
    }
}

#[test]
fn skipping_each_stage_in_silence_leaves_what_processing_the_silence_would() {
    for bus in [false, true] {
        let ins = ins(bus);
        let skipped = skip_matches_process(
            &|| full(bus),
            &IdleRun {
                ins: &ins,
                outs: &OUTS,
                batches: 12_000,
                sound: &|n| n < 40 || (9_000..9_040).contains(&n) || (10_000..10_010).contains(&n),
                edit: &|n, h| match n {
                    4_000 => {
                        h.set_param("fader", 0.45);
                        true
                    }
                    5_000 => {
                        h.set_param("hpf.freq", 140.0);
                        h.set_param("eq.hf.gain", -3.0);
                        true
                    }
                    6_000 => {
                        h.set_param("mute", 1.0);
                        true
                    }
                    6_500 => {
                        h.set_param("mute", 0.0);
                        h.set_param("send2.level", -12.0);
                        true
                    }
                    7_000 => {
                        h.set_param("comp.threshold", -24.0);
                        h.set_param("pcomp.mix", 0.7);
                        true
                    }
                    7_500 => {
                        h.set_param("latency.adjust", -64.0);
                        true
                    }
                    _ => false,
                },
            },
        );
        assert!(
            skipped > 5_000,
            "bus {bus}: the scenario skips a lot: {skipped}"
        );
    }
}

/// The routing variants: dynamics after the EQ, inserts before the dynamics, the filters in the
/// side-chain, a key input, and the insert return (with its own sound).
#[test]
fn the_routing_variants_rest_and_skip_exactly() {
    type Setup = (
        &'static str,
        fn(&mut DeviceHarness),
        &'static [&'static str],
    );
    const MAIN: &[&str] = &["in_l", "in_r"];
    let cases: [Setup; 5] = [
        (
            "dyn after eq",
            |h| h.set_param("route.dyn_post_eq", 1.0),
            MAIN,
        ),
        ("insert pre", |h| h.set_param("route.insert_pre", 1.0), MAIN),
        (
            "filters to sc",
            |h| h.set_param("route.filters_to_sc", 1.0),
            MAIN,
        ),
        (
            "key",
            |h| {
                h.set_param("key.on", 1.0);
                h.set_param("pcomp.key", 1.0);
            },
            &["in_l", "in_r", "key_in"],
        ),
        (
            "insert return",
            |h| h.set_param("route.insert_pre", 1.0),
            &["in_l", "in_r", "insert_return_l", "insert_return_r"],
        ),
    ];
    for (name, setup, ins) in cases {
        let skipped = skip_matches_process(
            &|| {
                let mut h = full(false);
                setup(&mut h);
                h
            },
            &IdleRun {
                ins,
                outs: &OUTS,
                batches: 12_000,
                sound: &|n| n < 40 || (9_000..9_040).contains(&n),
                edit: &|n, h| {
                    if n == 5_000 {
                        h.set_param("fader", 0.5);
                        true
                    } else {
                        false
                    }
                },
            },
        );
        assert!(skipped > 5_000, "{name}: {skipped}");
    }
}

fn set(rig: &mut StageRig, key: &str, v: f32) {
    let id = rig.device.descriptor().param_by_key(key).unwrap().id;
    rig.device.set_param(id, v);
}

/// One stage at a time, the way the engine calls them: the first sound of a return after the
/// strip came to rest arrives while stage 0 is skipped and stage 1 runs, so stage 1 reads what a
/// silent stage 0 hands over; the strip then carries on as the one that ran both stages.
#[test]
fn a_skipped_first_stage_leaves_silence_for_the_second() {
    let make = || {
        let mut rig = StageRig::new(Box::new(Channel::new()));
        for (k, v) in FULL {
            set(&mut rig, k, v);
        }
        for key in ["in_l", "in_r", "insert_return_l", "insert_return_r"] {
            rig.set_input(key, 0.0);
        }
        rig
    };
    let (mut a, mut b) = (make(), make());
    // Both come to rest with sound that goes through stage 0 (the same batches for both).
    for key in ["in_l", "in_r"] {
        a.set_input(key, 0.4);
        b.set_input(key, 0.4);
    }
    for _ in 0..10 {
        for rig in [&mut a, &mut b] {
            rig.stage(0);
            rig.stage(1);
        }
    }
    for key in ["in_l", "in_r"] {
        a.set_input(key, 0.0);
        b.set_input(key, 0.0);
    }
    let mut batches = 0;
    while !b.device.quiescent() {
        for rig in [&mut a, &mut b] {
            rig.stage(0);
            rig.stage(1);
        }
        batches += 1;
        assert!(batches < 20_000, "it never rested");
    }
    let keys = [
        "insert_send_l",
        "out_l",
        "out_r",
        "send1_l",
        "send2_r",
        "gr_cv_out",
    ];
    for k in 0..40 {
        for key in ["insert_return_l", "insert_return_r"] {
            let v = if k < 20 { 0.3 - 0.01 * k as f32 } else { 0.0 };
            a.set_input(key, v);
            b.set_input(key, v);
        }
        a.stage(0);
        a.stage(1);
        if k == 0 {
            // Stage 0 sees silence on a quiescent strip: skipped. Stage 1 has sound: it runs.
            b.device.idle_advance_stage(0, BATCH);
        } else {
            b.stage(0);
        }
        b.stage(1);
        for key in keys {
            let (x, y) = (a.get(key), b.get(key));
            assert!(
                x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()),
                "batch {k}: {key} differs"
            );
        }
    }
}

/// The analyser tap is fed the silent block a skipped strip makes, where the processed one
/// would have pushed it: the ring and its write position are the same.
#[test]
fn the_analyser_tap_sees_the_same_silence_skipped_or_processed() {
    for pre in [false, true] {
        let make = || {
            let h = full(false);
            let tap = h.device().scope_tap().unwrap();
            tap.reserve();
            tap.set_live(true);
            tap.set_pre(pre);
            (h, tap)
        };
        let (mut a, tap_a) = make();
        let (mut b, tap_b) = make();
        let ins = ["in_l", "in_r"];
        for _ in 0..40 {
            feed(&mut a, &ins);
            feed(&mut b, &ins);
        }
        let mut skipped = 0;
        for _ in 0..8_000 {
            silence(&mut a, &ins);
            if b.device().quiescent() {
                b.device_mut().idle_advance(BATCH);
                skipped += 1;
            } else {
                silence(&mut b, &ins);
            }
        }
        assert!(skipped > 3_000, "pre {pre}: {skipped}");
        assert_eq!(tap_a.head(), tap_b.head(), "pre {pre}: the write position");
        let read = |t: &Arc<ScopeTap>| {
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            let (n, _) = t.read_latest(&mut l, &mut r);
            (l.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), n)
        };
        assert_eq!(read(&tap_a), read(&tap_b), "pre {pre}: the ring");
    }
}

/// The bus's follow stage publishes its gain every batch while `follow_out` is cabled, so it
/// is asked to run while the strip rests; the gain is the fader's.
#[test]
fn a_bus_with_a_follow_output_publishes_its_gain_while_it_rests() {
    let mut h = full(true);
    h.set_connected("follow_out", true);
    let ins = ins(true);
    for _ in 0..40 {
        feed(&mut h, &ins);
    }
    let mut batches = 0;
    while !h.device().quiescent() {
        silence(&mut h, &ins);
        batches += 1;
        assert!(batches < 20_000);
    }
    assert!(
        h.device().idle_stage_runs(0),
        "stage 0 runs while the bus rests"
    );
    assert!(!h.device().idle_stage_runs(1));
    assert!(!h.device().idle_stage_runs(2));
    silence(&mut h, &ins);
    let gain = h.output("follow_out")[0];
    assert!(gain > 0.0, "the bus gain is published");
    let channel = full(false);
    assert!(
        !channel.device().idle_stage_runs(0),
        "a channel has no such stage"
    );
    assert_eq!(
        h.device().idle_ignored_inputs(),
        &[SocketId(S_FOLLOW_IN)],
        "the gain of the bus above only scales the sends"
    );
}
