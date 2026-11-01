//! `atomik.master` tests: contract keys, return mixing, bus compressor, insert, monitoring.

use super::*;
use crate::test_support::{harness, sine_gain_db, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use proptest::prelude::*;

const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
const MAIN: [&str; 2] = ["main_l", "main_r"];
const MON: [&str; 2] = ["monitor_l", "monitor_r"];

fn master() -> DeviceHarness {
    harness(Box::new(Master::new()))
}

fn dc(h: &mut DeviceHarness, key: &str, v: f32) {
    *h.input_mut(key) = [v; BATCH];
}

fn settled(h: &mut DeviceHarness, outs: &[&str]) -> Vec<f32> {
    h.render(20, outs)
        .iter()
        .map(|o| *o.last().unwrap())
        .collect()
}

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn descriptor_keys_match_contract_exactly() {
    let mut params: Vec<String> = [
        "insert.pre_comp",
        "insert.bypass",
        "comp.on",
        "comp.threshold",
        "comp.ratio",
        "comp.attack",
        "comp.release",
        "comp.makeup",
        "comp.key",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for k in 1..=8 {
        for p in ["level", "mute", "pan"] {
            params.push(format!("return{k}.{p}"));
        }
    }
    for p in ["fader", "monitor.level", "monitor.dim", "monitor.source"] {
        params.push(p.into());
    }
    for k in 1..=8 {
        params.push(format!("send{k}.level"));
    }
    params.push("internal.tap_live".into());
    let mut sockets: Vec<String> = vec!["bus_l".into(), "bus_r".into()];
    for prefix in ["send_bus", "fx_send", "return"] {
        for k in 1..=8 {
            sockets.push(format!("{prefix}{k}_l"));
            sockets.push(format!("{prefix}{k}_r"));
        }
    }
    for s in [
        "insert_send_l",
        "insert_send_r",
        "insert_return_l",
        "insert_return_r",
        "key_in",
        "main_l",
        "main_r",
        "monitor_l",
        "monitor_r",
        "gr_cv_out",
    ] {
        sockets.push(s.into());
    }
    let p: Vec<&str> = params.iter().map(String::as_str).collect();
    let s: Vec<&str> = sockets.iter().map(String::as_str).collect();
    crate::test_support::assert_keys(&DESCRIPTOR, &p, &s);
    assert_eq!(DESCRIPTOR.type_id, "atomik.master");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Mixer);
}

#[test]
fn descriptor_socket_directions() {
    for s in DESCRIPTOR.sockets {
        let want = if s.key.starts_with("bus_") || s.key.starts_with("send_bus") {
            Direction::SummingInput
        } else if s.key.starts_with("return")
            || s.key == "key_in"
            || s.key.starts_with("insert_return")
        {
            Direction::Input
        } else {
            Direction::Output
        };
        assert_eq!(s.direction, want, "{}", s.key);
        let kind = if s.key == "gr_cv_out" {
            SocketKind::Cv
        } else {
            SocketKind::Audio
        };
        assert_eq!(s.kind, kind, "{}", s.key);
    }
}

#[test]
fn bus_passes_to_main_and_monitor_at_unity() {
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "bus_r", -0.25);
    let o = settled(&mut h, &["main_l", "main_r", "monitor_l", "monitor_r"]);
    assert!(
        approx(o[0], 0.5, 1e-6) && approx(o[1], -0.25, 1e-6),
        "{o:?}"
    );
    assert!(
        approx(o[2], 0.5, 1e-6) && approx(o[3], -0.25, 1e-6),
        "{o:?}"
    );
}

#[test]
fn fx_sends_pass_send_buses_through() {
    let mut h = master();
    for k in 1..=8 {
        dc(&mut h, &format!("send_bus{k}_l"), 0.1 * k as f32);
        dc(&mut h, &format!("send_bus{k}_r"), -0.1 * k as f32);
    }
    h.set_param("fader", 0.0);
    h.process();
    for k in 1..=8 {
        assert!(approx(
            h.output(&format!("fx_send{k}_l"))[5],
            0.1 * k as f32,
            1e-7
        ));
        assert!(approx(
            h.output(&format!("fx_send{k}_r"))[5],
            -0.1 * k as f32,
            1e-7
        ));
    }
}

#[test]
fn returns_mix_with_level_pan_and_mute() {
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "bus_r", 0.5);
    dc(&mut h, "return1_l", 0.2);
    dc(&mut h, "return1_r", 0.2);
    let o = settled(&mut h, &MAIN);
    assert!(approx(o[0], 0.5 + 0.2 * K, 1e-5), "centre pan −3 dB: {o:?}");
    h.set_param("return1.pan", -1.0);
    h.set_param("return1.level", -6.0);
    let o = settled(&mut h, &MAIN);
    let g = 10f32.powf(-0.3);
    assert!(
        approx(o[0], 0.5 + 0.2 * g, 1e-5) && approx(o[1], 0.5, 1e-5),
        "{o:?}"
    );
    h.set_param("return1.mute", 1.0);
    let o = settled(&mut h, &MAIN);
    assert!(approx(o[0], 0.5, 1e-6), "muted {o:?}");
    // A mono return (only L connected) feeds both sides; returns sum.
    let mut h = master();
    dc(&mut h, "return3_l", 0.2);
    dc(&mut h, "return8_l", 0.1);
    let o = settled(&mut h, &MAIN);
    assert!(approx(o[1], 0.3 * K, 1e-5), "{o:?}");
    h.set_param("return8.level", -60.0);
    let o = settled(&mut h, &MAIN);
    assert!(approx(o[1], 0.2 * K, 1e-5), "level minimum is off: {o:?}");
}

#[test]
fn bus_compressor_static_curve_and_makeup() {
    let mut h = master();
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -20.0);
    h.set_param("comp.ratio", 4.0);
    for level in [-40.0f64, -12.0, -6.0, 0.0] {
        let got = sine_gain_db(
            &mut h,
            &["bus_l", "bus_r"],
            "main_l",
            1_000.0,
            10f64.powf(level / 20.0),
            0.6,
        );
        let want = -f64::from(atomik_dsp::static_gain_reduction_db(
            level as f32,
            -20.0,
            4.0,
        ));
        assert!(
            (got - want).abs() < 0.2,
            "{level} dB: {got:.3} vs {want:.3}"
        );
    }
    h.set_param("comp.makeup", 6.0);
    let got = sine_gain_db(&mut h, &["bus_l", "bus_r"], "main_l", 1_000.0, 0.5, 0.6);
    let want = 6.0 - f64::from(atomik_dsp::static_gain_reduction_db(-6.0206, -20.0, 4.0));
    assert!((got - want).abs() < 0.2, "make-up: {got} vs {want}");
    let gr = h.output("gr_cv_out")[10];
    let g = 10f32.powf(-atomik_dsp::static_gain_reduction_db(-6.0206, -20.0, 4.0) / 20.0);
    assert!(approx(gr, 1.0 - g, 0.02), "gr cv {gr} vs {}", 1.0 - g);
    // Off: unity, no CV.
    h.set_param("comp.on", 0.0);
    let got = sine_gain_db(&mut h, &["bus_l", "bus_r"], "main_l", 1_000.0, 0.5, 0.2);
    assert!(got.abs() < 1e-3, "{got}");
    assert!(h.output("gr_cv_out").iter().all(|v| v.abs() < 1e-9));
}

#[test]
fn bus_compressor_key_input() {
    let mut h = master();
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -30.0);
    h.set_param("comp.ratio", 10.0);
    dc(&mut h, "key_in", 0.5);
    let quiet = sine_gain_db(&mut h, &["bus_l", "bus_r"], "main_l", 1_000.0, 0.01, 0.6);
    assert!(quiet.abs() < 0.05, "{quiet}");
    h.set_param("comp.key", 1.0);
    let keyed = sine_gain_db(&mut h, &["bus_l", "bus_r"], "main_l", 1_000.0, 0.01, 0.6);
    // DC key 0.5 reads √2·0.5 on the sine-calibrated RMS detector (−3.01 dB).
    let want = -f64::from(atomik_dsp::static_gain_reduction_db(-3.0103, -30.0, 10.0));
    assert!((keyed - want).abs() < 0.1, "{keyed} vs {want}");
}

#[test]
fn master_insert_before_or_after_the_compressor() {
    let ins = ["insert_send_l", "main_l"];
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "bus_r", 0.5);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -30.0);
    // Default: insert after the compressor (send carries the compressed bus).
    let o = settled(&mut h, &ins);
    assert!(
        o[0] < 0.2 && approx(o[0], o[1], 1e-6),
        "post-comp tap {o:?}"
    );
    h.set_param("insert.pre_comp", 1.0);
    let o = settled(&mut h, &ins);
    assert!(approx(o[0], 0.5, 1e-6) && o[1] < 0.2, "pre-comp tap {o:?}");
    // Connected return replaces; bypass passes through.
    h.set_param("comp.on", 0.0);
    dc(&mut h, "insert_return_l", 0.1);
    let o = settled(&mut h, &["main_l", "main_r"]);
    assert!(approx(o[0], 0.1, 1e-6) && approx(o[1], 0.1, 1e-6), "{o:?}");
    h.set_param("insert.bypass", 1.0);
    let o = settled(&mut h, &["main_l"]);
    assert!(approx(o[0], 0.5, 1e-6), "{o:?}");
}

#[test]
fn fader_and_dim_affect_only_their_outputs() {
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "bus_r", 0.5);
    h.set_param("monitor.dim", 1.0);
    let o = settled(&mut h, &["main_l", "monitor_l"]);
    assert!(approx(o[0], 0.5, 1e-6), "dim leaves main alone: {o:?}");
    assert!(approx(o[1], 0.05, 1e-6), "dim −20 dB on the monitor: {o:?}");
    h.set_param("monitor.dim", 0.0);
    h.set_param("monitor.level", -6.0);
    let o = settled(&mut h, &["main_l", "monitor_l"]);
    assert!(approx(o[0], 0.5, 1e-6) && approx(o[1], 0.5 * 10f32.powf(-0.3), 1e-5));
    h.set_param("monitor.level", 0.0);
    h.set_param("fader", 1.0);
    let o = settled(&mut h, &["main_l", "monitor_l"]);
    let top = 0.5 * atomik_dsp::fader_gain(1.0);
    assert!(
        approx(o[0], top, 1e-5) && approx(o[1], top, 1e-5),
        "monitor follows main {o:?}"
    );
    assert!((20.0 * (o[0] / 0.5).log10() - 7.4963).abs() < 1e-3);
}

#[test]
fn monitor_source_selects_fx_returns() {
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "return2_l", 0.2);
    h.set_param("return2.pan", -1.0);
    h.set_param("monitor.source", 1.0);
    let o = settled(&mut h, &["main_l", "monitor_l", "monitor_r"]);
    assert!(approx(o[0], 0.7, 1e-5), "{o:?}");
    assert!(
        approx(o[1], 0.2, 1e-5) && o[2].abs() < 1e-6,
        "returns only: {o:?}"
    );
    let o = settled(&mut h, &MON);
    assert!(approx(o[0], 0.2, 1e-5));
}

#[test]
fn set_param_every_param_min_max_does_not_panic() {
    let mut h = master();
    for k in ["bus_l", "bus_r", "key_in", "return1_l", "insert_return_r"] {
        dc(&mut h, k, 0.3);
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn process_is_allocation_free() {
    let mut h = master();
    for s in DESCRIPTOR.sockets {
        if s.direction.is_input() {
            dc(&mut h, s.key, 0.2);
        }
    }
    for i in 0..200 {
        let p = &DESCRIPTOR.params[i % DESCRIPTOR.params.len()];
        let v = p.from_normalized((i as f32 * 0.61).fract());
        assert_no_alloc(|| h.set_param(p.key, v));
        h.process();
    }
    assert_no_alloc(|| h.device_mut().reset());
    h.process();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn any_params_and_signals_give_finite_bounded_output(
        norms in prop::collection::vec(0.0f32..=1.0, PARAM_COUNT),
        seed in any::<u64>(),
    ) {
        let mut h = master();
        for (p, n) in DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        let mut rng = atomik_dsp::Rng::new(seed);
        let ins: Vec<&str> = DESCRIPTOR
            .sockets
            .iter()
            .filter(|s| s.direction.is_input() && rng.next_f32() < 0.7)
            .map(|s| s.key)
            .collect();
        let outs = crate::test_support::output_keys(&DESCRIPTOR);
        for _ in 0..30 {
            for k in &ins {
                for s in h.input_mut(k).iter_mut() {
                    *s = rng.next_f32() * 2.0 - 1.0;
                }
            }
            h.process();
            for k in &outs {
                for v in h.output(k) {
                    prop_assert!(v.is_finite() && v.abs() < 1e4, "{k}: {v}");
                }
            }
            prop_assert!(h.output("gr_cv_out").iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}

#[test]
fn stage_table_matches_plan_section_9() {
    let mut s0: Vec<String> = Vec::new();
    let mut s1: Vec<String> = vec!["bus_l".into(), "bus_r".into()];
    for k in 1..=8 {
        for side in ["l", "r"] {
            s0.push(format!("send_bus{k}_{side}"));
            s0.push(format!("fx_send{k}_{side}"));
            s1.push(format!("return{k}_{side}"));
        }
    }
    s1.push("insert_send_l".into());
    s1.push("insert_send_r".into());
    s1.push("key_in".into());
    crate::test_support::assert_stages(
        &DESCRIPTOR,
        &[
            s0.iter().map(String::as_str).collect(),
            s1.iter().map(String::as_str).collect(),
            vec![
                "insert_return_l",
                "insert_return_r",
                "main_l",
                "main_r",
                "monitor_l",
                "monitor_r",
                "gr_cv_out",
            ],
        ],
    );
}

#[test]
fn stages_run_separately_through_return_and_insert_loops() {
    use crate::test_support::StageRig;
    let mut rig = StageRig::new(Box::new(Master::new()));
    rig.set_input("bus_l", 0.5);
    rig.set_input("bus_r", 0.5);
    rig.set_input("send_bus1_l", 0.2);
    rig.set_input("send_bus1_r", 0.2);
    let id = |k: &str| DESCRIPTOR.socket_by_key(k).unwrap().id.0 as usize;
    for _ in 0..10 {
        rig.stage(0);
        // FX effect: return 1 = fx_send 1 (stage 0 output feeds a stage 1 input).
        for side in ["l", "r"] {
            rig.signals[id(&format!("return1_{side}"))] = *rig.get(&format!("fx_send1_{side}"));
            rig.connected[id(&format!("return1_{side}"))] = true;
        }
        rig.signals[id("main_l")] = [9.0; BATCH];
        rig.stage(1);
        assert!(
            rig.get("main_l").iter().all(|v| approx(*v, 9.0, 0.0)),
            "stage 1 wrote main"
        );
        // Insert effect: −6 dB.
        for side in ["l", "r"] {
            rig.signals[id(&format!("insert_return_{side}"))] =
                rig.get(&format!("insert_send_{side}")).map(|v| v * 0.5);
            rig.connected[id(&format!("insert_return_{side}"))] = true;
        }
        rig.stage(2);
    }
    let want = (0.5 + 0.2 * K) * 0.5;
    assert!(
        approx(rig.get("main_l")[BATCH - 1], want, 1e-5),
        "{}",
        rig.get("main_l")[0]
    );
    assert!(approx(rig.get("monitor_r")[BATCH - 1], want, 1e-5));
}

#[test]
fn process_equals_stage_by_stage() {
    use crate::test_support::StageRig;
    let mut rig = StageRig::new(Box::new(Master::new()));
    let mut h = master();
    for (k, v) in [
        ("comp.on", 1.0),
        ("comp.threshold", -30.0),
        ("comp.key", 1.0),
        ("return2.pan", 0.3),
    ] {
        let p = DESCRIPTOR.param_by_key(k).unwrap();
        rig.device.set_param(p.id, v);
        h.set_param(k, v);
    }
    let mut rng = atomik_dsp::Rng::new(9);
    for _ in 0..30 {
        for k in ["bus_l", "bus_r", "return2_l", "key_in", "send_bus3_r"] {
            let x: [f32; BATCH] = std::array::from_fn(|_| rng.next_bipolar());
            *h.input_mut(k) = x;
            let i = DESCRIPTOR.socket_by_key(k).unwrap().id.0 as usize;
            rig.signals[i] = x;
            rig.connected[i] = true;
        }
        h.process();
        for s in 0..3 {
            rig.stage(s);
        }
        for k in [
            "fx_send3_r",
            "insert_send_l",
            "main_l",
            "main_r",
            "monitor_l",
            "gr_cv_out",
        ] {
            assert_eq!(h.output(k), rig.get(k), "{k}");
        }
    }
}

#[test]
fn keyed_compressor_before_the_insert_reacts_in_the_same_batch() {
    // key_in is a stage-1 input: with the default routing (compressor before the insert,
    // both in stage 1) the key acts in the batch it arrives, with no one-batch lag.
    let mut h = master();
    h.set_param("comp.on", 1.0);
    h.set_param("comp.key", 1.0);
    h.set_param("comp.threshold", -30.0);
    h.set_param("comp.attack", 0.1);
    dc(&mut h, "bus_l", 0.01);
    dc(&mut h, "key_in", 0.0);
    h.render(10, &["main_l"]);
    assert!(h.output("gr_cv_out").iter().all(|v| v.abs() < 1e-6));
    dc(&mut h, "key_in", 1.0);
    h.process();
    let gr = h.output("gr_cv_out")[BATCH - 1];
    assert!(gr > 0.5, "reduction in the same batch: {gr}");
    let send = h.output("insert_send_l")[BATCH - 1];
    assert!(
        send < 0.005,
        "insert send carries the keyed reduction: {send}"
    );
}

#[test]
fn params_set_before_the_first_batch_apply_without_ramp() {
    use crate::test_support::assert_first_batch_settled;
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    dc(&mut h, "bus_r", -0.3);
    dc(&mut h, "return1_l", 0.2);
    for (k, v) in [
        ("fader", 0.5),
        ("return1.level", -6.0),
        ("return1.pan", 0.7),
        ("monitor.level", -12.0),
        ("monitor.dim", 1.0),
        ("comp.makeup", 6.0),
    ] {
        h.set_param(k, v);
    }
    assert_first_batch_settled(&mut h, &["main_l", "main_r", "monitor_l", "monitor_r"], 40);
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    h.set_param("fader", 0.0);
    h.process();
    assert!(
        h.output("main_l").iter().all(|v| v.abs() < 1e-9),
        "fader −∞ from sample 0"
    );
}

#[test]
fn param_changes_after_the_first_batch_still_ramp() {
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    h.process();
    h.set_param("fader", 0.0);
    h.process();
    let o = h.output("main_l");
    assert!(
        o[0] > 0.49 && o[BATCH - 1] > 0.3,
        "ramping: {} {}",
        o[0],
        o[BATCH - 1]
    );
}

#[test]
fn connected_insert_return_is_used_from_the_first_sample() {
    for pre_comp in [0.0, 1.0] {
        let mut h = master();
        h.set_param("insert.pre_comp", pre_comp);
        dc(&mut h, "bus_l", 0.5);
        dc(&mut h, "insert_return_l", 0.1);
        h.process();
        assert!(
            h.output("main_l").iter().all(|v| approx(*v, 0.1, 1e-7)),
            "dry leak (pre_comp {pre_comp}): {:?}",
            &h.output("main_l")[..4]
        );
        h.device_mut().reset();
        h.process();
        assert!(
            h.output("main_l").iter().all(|v| approx(*v, 0.1, 1e-7)),
            "after reset"
        );
    }
    let mut h = master();
    dc(&mut h, "bus_l", 0.5);
    h.process();
    dc(&mut h, "insert_return_l", 0.1);
    h.process();
    assert!(
        h.output("main_l")[0] > 0.45,
        "crossfades after the first batch"
    );
}

// -------------------------------------------------------------: old behaviour

/// FNV-1a over the bit patterns of every output of a deterministic scenario, so the master's
/// sound for the two original `monitor.source` choices (and level-0 sends) is pinned bit-exactly.
fn scenario_hash(source: f32) -> u64 {
    let mut h = master();
    h.set_param("monitor.source", source);
    h.set_param("comp.threshold", -24.0);
    h.set_param("comp.ratio", 4.0);
    h.set_param("comp.makeup", 3.0);
    h.set_param("monitor.level", -3.0);
    for k in 1..=8 {
        h.set_param(&format!("return{k}.level"), -2.0 * k as f32);
        h.set_param(&format!("return{k}.pan"), 0.25 * k as f32 - 1.0);
    }
    let mut rng = atomik_dsp::Rng::new(0x4d4d_0002);
    let ins: Vec<&str> = DESCRIPTOR
        .sockets
        .iter()
        .filter(|s| s.direction.is_input() && s.key != "insert_return_l")
        .map(|s| s.key)
        .collect();
    let outs = crate::test_support::output_keys(&DESCRIPTOR);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for _ in 0..40 {
        for k in &ins {
            for s in h.input_mut(k).iter_mut() {
                *s = (rng.next_f32() * 2.0 - 1.0) * 0.5;
            }
        }
        h.process();
        for k in &outs {
            for v in h.output(k) {
                for b in v.to_bits().to_le_bytes() {
                    hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
    }
    hash
}

#[test]
fn original_sources_and_zero_db_sends_keep_their_sound_bit_exactly() {
    // Recorded on the master as it was before the extra sources existed (the same scenario, same inputs).
    assert_eq!(scenario_hash(0.0), 0x0952_f898_8f22_0a51);
    assert_eq!(scenario_hash(1.0), 0x9ebf_a84c_4548_2fee);
}

#[test]
fn send_level_params_are_appended_after_the_monitor_source() {
    assert_eq!(DESCRIPTOR.params.len(), 37 + 8 + 1);
    let src = DESCRIPTOR.param_by_key("monitor.source").unwrap();
    assert_eq!(src.id.0, 36);
    for k in 1..=8u32 {
        let p = DESCRIPTOR.param_by_key(&format!("send{k}.level")).unwrap();
        assert_eq!(p.id.0, src.id.0 + k, "send{k}.level id");
        assert_eq!((p.min, p.max, p.default), (-60.0, 6.0, 0.0));
        assert_eq!(p.unit, Unit::Db);
        assert!(!p.discrete);
    }
    let ret = DESCRIPTOR.param_by_key("return1.level").unwrap();
    let send = DESCRIPTOR.param_by_key("send1.level").unwrap();
    assert_eq!(
        (ret.taper, ret.min, ret.max),
        (send.taper, send.min, send.max)
    );
}

#[test]
fn monitor_sources_are_appended_so_old_indices_keep_their_meaning() {
    let want: Vec<String> = ["Main", "FX returns"]
        .iter()
        .map(|s| s.to_string())
        .chain((1..=8).map(|k| format!("Send {k}")))
        .chain((1..=8).map(|k| format!("Return {k}")))
        .collect();
    assert_eq!(MONITOR_SOURCES.len(), 18);
    assert_eq!(MONITOR_SOURCES.to_vec(), want);
    let p = DESCRIPTOR.param_by_key("monitor.source").unwrap();
    assert_eq!((p.min, p.max, p.default), (0.0, 17.0, 0.0));
    assert!(p.discrete);
}

fn mono_in(h: &mut DeviceHarness, key: &str, v: f32) {
    dc(h, key, v);
}

#[test]
fn send_level_scales_the_fx_feed_and_zero_db_is_bit_exact() {
    let mut h = master();
    let mut rng = atomik_dsp::Rng::new(7);
    for k in 1..=8 {
        for s in ["l", "r"] {
            for v in h.input_mut(&format!("send_bus{k}_{s}")).iter_mut() {
                *v = rng.next_f32() * 2.0 - 1.0;
            }
        }
    }
    h.process();
    for k in 1..=8 {
        for s in ["l", "r"] {
            let a = *h.input_mut(&format!("send_bus{k}_{s}"));
            let b = *h.output(&format!("fx_send{k}_{s}"));
            assert!(
                a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()),
                "send {k} {s} not bit exact at 0 dB"
            );
        }
    }
    h.set_param("send3.level", 6.0);
    h.set_param("send5.level", -6.0);
    h.set_param("send7.level", -60.0);
    mono_in(&mut h, "send_bus3_l", 0.25);
    mono_in(&mut h, "send_bus5_l", 0.5);
    mono_in(&mut h, "send_bus7_l", 0.5);
    mono_in(&mut h, "send_bus1_l", 0.5);
    let o = settled(
        &mut h,
        &["fx_send3_l", "fx_send5_l", "fx_send7_l", "fx_send1_l"],
    );
    assert!(approx(o[0], 0.25 * db_to_gain(6.0), 1e-6), "{o:?}");
    assert!(approx(o[1], 0.5 * db_to_gain(-6.0), 1e-6), "{o:?}");
    assert_eq!(o[2], 0.0, "-60 dB is off");
    assert_eq!(o[3], 0.5, "other sends untouched");
}

#[test]
fn send_level_change_ramps_without_a_step() {
    let mut h = master();
    mono_in(&mut h, "send_bus2_l", 1.0);
    h.process();
    h.set_param("send2.level", -60.0);
    h.process();
    let o = h.output("fx_send2_l");
    assert!(
        o[0] < 1.0 && o[0] > 0.9,
        "starts from the old gain: {}",
        o[0]
    );
    assert!(o.windows(2).all(|w| w[1] <= w[0]));
}

/// Impulse level test signals: every tap carries a distinct constant.
fn tap_rig(source: f32) -> Vec<f32> {
    let mut h = master();
    h.set_param("monitor.source", source);
    for k in 1..=8 {
        mono_in(&mut h, &format!("send_bus{k}_l"), 0.01 * k as f32);
        mono_in(&mut h, &format!("send_bus{k}_r"), 0.01 * k as f32);
        mono_in(&mut h, &format!("return{k}_l"), 0.1 * k as f32);
        mono_in(&mut h, &format!("return{k}_r"), 0.1 * k as f32);
        h.set_param(&format!("return{k}.pan"), 0.0);
    }
    mono_in(&mut h, "bus_l", 0.5);
    mono_in(&mut h, "bus_r", 0.5);
    settled(&mut h, &["monitor_l", "monitor_r"])
}

#[test]
fn monitor_send_k_and_return_k_reach_the_monitor_only_for_their_source() {
    // Return k at 0 dB centre pan: constant-power law gives K on each side.
    for k in 1..=8usize {
        let o = tap_rig(1.0 + k as f32); // Send k
        let want = 0.01 * k as f32;
        assert!(
            approx(o[0], want, 1e-6) && approx(o[1], want, 1e-6),
            "send {k}: {o:?}"
        );
        let o = tap_rig(9.0 + k as f32); // Return k
        let want = 0.1 * k as f32 * K;
        assert!(
            approx(o[0], want, 1e-5) && approx(o[1], want, 1e-5),
            "return {k}: {o:?}"
        );
    }
}

#[test]
fn monitor_send_taps_follow_the_send_level_and_dim_still_applies() {
    let mut h = master();
    h.set_param("monitor.source", 3.0); // Send 2
    h.set_param("send2.level", -6.0);
    h.set_param("monitor.dim", 1.0);
    mono_in(&mut h, "send_bus2_l", 0.5);
    mono_in(&mut h, "send_bus1_l", 0.9);
    let o = settled(&mut h, &["monitor_l", "main_l"]);
    assert!(
        approx(o[0], 0.5 * db_to_gain(-6.0) * db_to_gain(DIM_DB), 1e-6),
        "{o:?}"
    );
    assert!(
        o[1].abs() < 1e-6,
        "main is not touched by the source: {o:?}"
    );
}

#[test]
fn monitor_source_out_of_range_values_are_clamped() {
    let mut h = master();
    h.set_param("monitor.source", 99.0);
    mono_in(&mut h, "return8_l", 0.4);
    mono_in(&mut h, "return8_r", 0.4);
    let o = settled(&mut h, &MON);
    assert!(approx(o[0], 0.4 * K, 1e-5), "clamps to Return 8: {o:?}");
}

#[test]
fn a_return_tap_is_silent_once_the_return_is_unplugged_or_muted() {
    // Source Return 3: the tap carries the return; when the cable goes away or the return is
    // muted the tap must read silence, not the last block it saw.
    for how in ["unplug", "mute"] {
        let mut h = master();
        h.set_param("monitor.source", 12.0); // Return 3
        mono_in(&mut h, "return3_l", 0.5);
        mono_in(&mut h, "return3_r", 0.5);
        let on = h.render(20, &MON);
        assert!(
            on[0].last().unwrap().abs() > 0.1,
            "{how}: the return is heard"
        );
        match how {
            "unplug" => {
                h.set_connected("return3_l", false);
                h.set_connected("return3_r", false);
            }
            _ => h.set_param("return3.mute", 1.0),
        }
        let off = h.render(40, &MON);
        for side in &off {
            let tail = &side[side.len() - 2 * BATCH..];
            assert!(tail.iter().all(|x| *x == 0.0), "{how}: stale tap {tail:?}");
        }
    }
}

#[test]
fn switching_between_taps_crossfades_and_settles_exactly() {
    let mut h = master();
    mono_in(&mut h, "send_bus1_l", 0.8);
    mono_in(&mut h, "send_bus2_l", 0.2);
    h.set_param("monitor.source", 2.0);
    let o = settled(&mut h, &["monitor_l"]);
    assert_eq!(o[0], 0.8);
    h.set_param("monitor.source", 3.0);
    h.process();
    let first = h.output("monitor_l")[0];
    assert!(
        first > 0.2 && first < 0.8,
        "crossfading, not stepping: {first}"
    );
    let o = settled(&mut h, &["monitor_l"]);
    assert_eq!(o[0], 0.2, "settles bit-exactly on the new tap");
    h.set_param("monitor.source", 0.0);
    mono_in(&mut h, "bus_l", 0.5);
    let o = settled(&mut h, &["monitor_l"]);
    assert_eq!(o[0], 0.5);
}

#[test]
fn every_source_gives_the_same_bits_through_process_and_stage_by_stage() {
    use crate::test_support::StageRig;
    for src in 0..18 {
        let mut rig = StageRig::new(Box::new(Master::new()));
        let mut h = master();
        for (k, v) in [("monitor.source", src as f32), ("send4.level", -3.0)] {
            let p = DESCRIPTOR.param_by_key(k).unwrap();
            rig.device.set_param(p.id, v);
            h.set_param(k, v);
        }
        let mut rng = atomik_dsp::Rng::new(src as u64 + 1);
        for _ in 0..20 {
            for s in DESCRIPTOR.sockets.iter().filter(|s| s.direction.is_input()) {
                let x: [f32; BATCH] = std::array::from_fn(|_| rng.next_bipolar());
                *h.input_mut(s.key) = x;
                rig.signals[s.id.0 as usize] = x;
                rig.connected[s.id.0 as usize] = true;
            }
            h.process();
            for st in 0..3 {
                rig.stage(st);
            }
            for k in ["monitor_l", "monitor_r", "fx_send4_l", "main_l"] {
                assert_eq!(h.output(k), rig.get(k), "source {src} {k}");
            }
        }
    }
}

#[test]
fn the_master_tap_is_written_only_while_the_switch_is_on() {
    let play = |tap_live: bool| {
        let mut h = master();
        if tap_live {
            h.set_param("internal.tap_live", 1.0);
        }
        let mut out = Vec::new();
        for i in 0..30 {
            *h.input_mut("bus_l") = [0.2 + 0.01 * i as f32; BATCH];
            *h.input_mut("bus_r") = [0.1; BATCH];
            out.extend(h.render(1, &["main_l", "main_r"]).into_iter().flatten());
        }
        let tap = h.device().scope_tap().expect("a prepared master has a tap");
        let (mut l, mut r) = (vec![0.0f32; BATCH], vec![0.0f32; BATCH]);
        let got = tap.read_latest(&mut l, &mut r);
        (out, tap.head(), got, l)
    };
    let (off, off_head, ..) = play(false);
    let (on, on_head, got, l) = play(true);
    assert_eq!(off_head, 0);
    assert_eq!(on_head, 30 * BATCH as u64);
    assert_eq!(got.0, BATCH);
    assert!(l.iter().all(|x| *x > 0.2), "the bus signal reaches the tap");
    assert!(off.iter().zip(&on).all(|(a, b)| a.to_bits() == b.to_bits()));
}
