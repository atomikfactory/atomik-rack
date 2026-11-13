//! `atomik.channel` tests: contract keys measurements and HI-1.

use super::*;
use crate::test_support::{
    SR, harness, run_signal, sine, sine_gain_db, sweep_params_min_max, tone_amplitude,
};
use atomik_dsp::{BiquadCoeffs, BiquadShape, PassFilter, PassKind, fader_db};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::max_step;
use proptest::prelude::*;

const OUT: [&str; 2] = ["out_l", "out_r"];
const K: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn channel() -> DeviceHarness {
    harness(Box::new(Channel::new()))
}

fn dc(h: &mut DeviceHarness, key: &str, v: f32) {
    *h.input_mut(key) = [v; BATCH];
}

/// Last sample of each output after 20 batches (> 5 ms ramps).
fn settled(h: &mut DeviceHarness, outs: &[&str]) -> Vec<f32> {
    h.render(20, outs)
        .iter()
        .map(|o| *o.last().unwrap())
        .collect()
}

fn approx(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn db(x: f64) -> f64 {
    20.0 * x.log10()
}

#[test]
fn descriptor_keys_match_contract_exactly() {
    let mut params: Vec<String> = [
        "input.gain",
        "input.phase",
        "hpf.on",
        "hpf.freq",
        "lpf.on",
        "lpf.freq",
        "comp.on",
        "comp.threshold",
        "comp.ratio",
        "comp.release",
        "comp.peak",
        "comp.fast",
        "gate.on",
        "gate.threshold",
        "gate.range",
        "gate.hold",
        "gate.release",
        "gate.fast",
        "gate.expander",
        "key.on",
        "route.filters_to_sc",
        "eq.on",
        "eq.hf.gain",
        "eq.hf.freq",
        "eq.hf.bell",
        "eq.hmf.gain",
        "eq.hmf.freq",
        "eq.hmf.q",
        "eq.lmf.gain",
        "eq.lmf.freq",
        "eq.lmf.q",
        "eq.lf.gain",
        "eq.lf.freq",
        "eq.lf.bell",
        "route.dyn_post_eq",
        "route.insert_pre",
        "insert.bypass",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for k in 1..=8 {
        for p in ["on", "level", "pre"] {
            params.push(format!("send{k}.{p}"));
        }
    }
    for p in [
        "pan",
        "width",
        "fader",
        "mute",
        "solo",
        "latency.adjust",
        "internal.solo_silence",
    ] {
        params.push(p.into());
    }
    params.extend(["fader_cv_trim".into(), "pan_cv_trim".into()]);
    for k in 1..=8 {
        params.push(format!("send{k}.level_cv_trim"));
    }
    params.extend([
        "internal.vca_db".into(),
        "internal.vca_mute".into(),
        "internal.vca_solo".into(),
    ]);
    params.extend(
        [
            "on",
            "threshold",
            "ratio",
            "attack",
            "release",
            "makeup",
            "mix",
        ]
        .map(|p| format!("pcomp.{p}")),
    );
    params.push("internal.tap_live".into());
    params.extend(
        [
            "comp.lookahead",
            "pcomp.peak",
            "input.phase_r",
            "dyn.bypass",
            "eq.bypass",
            "pcomp.key",
            "pcomp.pre",
            "comp.auto_makeup",
            "eq.e_mode",
        ]
        .map(String::from),
    );
    let mut sockets: Vec<String> = [
        "in_l",
        "in_r",
        "key_in",
        "insert_send_l",
        "insert_send_r",
        "insert_return_l",
        "insert_return_r",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for k in 1..=8 {
        sockets.push(format!("send{k}_l"));
        sockets.push(format!("send{k}_r"));
    }
    for s in ["out_l", "out_r", "gr_cv_out"] {
        sockets.push(s.into());
    }
    sockets.extend(["fader_cv_in".into(), "pan_cv_in".into()]);
    for k in 1..=8 {
        sockets.push(format!("send{k}_level_cv_in"));
    }
    sockets.extend([
        "parallel_out_l".into(),
        "parallel_out_r".into(),
        "follow_in".into(),
    ]);
    let p: Vec<&str> = params.iter().map(String::as_str).collect();
    let s: Vec<&str> = sockets.iter().map(String::as_str).collect();
    crate::test_support::assert_keys(&DESCRIPTOR, &p, &s);
    assert_eq!(DESCRIPTOR.type_id, "atomik.channel");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Mixer);
}

#[test]
fn descriptor_socket_kinds_and_directions() {
    for s in DESCRIPTOR.sockets {
        let input = s.key.starts_with("in_")
            || s.key == "key_in"
            || s.key.starts_with("insert_return")
            || s.key == "follow_in"
            || s.key.ends_with("_cv_in");
        let want = if input {
            Direction::Input
        } else {
            Direction::Output
        };
        assert_eq!(s.direction, want, "{}", s.key);
        let kind = if s.key == "gr_cv_out" || s.key.ends_with("_cv_in") || s.key == "follow_in" {
            SocketKind::Cv
        } else {
            SocketKind::Audio
        };
        assert_eq!(s.kind, kind, "{}", s.key);
    }
    let f = DESCRIPTOR.param_by_key("fader").unwrap();
    assert_eq!((f.min, f.max, f.taper), (0.0, 1.0, Taper::Linear));
    assert!(approx(f.default, 0.75, 0.0));
    let l = DESCRIPTOR.param_by_key("send3.level").unwrap();
    assert_eq!((l.taper, l.unit), (Taper::Decibel, Unit::Db));
    let la = DESCRIPTOR.param_by_key("latency.adjust").unwrap();
    assert!(la.discrete && approx(la.max, MAX_LATENCY_ADJUST as f32, 0.0));
    assert!(approx(la.min, -(MAX_LATENCY_ADJUST as f32), 0.0) && approx(la.default, 0.0, 0.0));
    assert_eq!(MAX_LATENCY_ADJUST, 480_000);
}

#[test]
fn default_strip_is_unity_through_a_centred_constant_power_pan() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5); // mono: one side feeds both
    let o = settled(&mut h, &OUT);
    assert!(
        approx(o[0], 0.5 * K, 1e-5) && approx(o[1], 0.5 * K, 1e-5),
        "{o:?}"
    );
}

#[test]
fn ac_x1_pan_law_is_constant_power_minus_3_01_db_centre() {
    let mut h = channel();
    dc(&mut h, "in_l", 1.0);
    let o = settled(&mut h, &OUT);
    for v in &o {
        assert!((20.0 * v.log10() + 3.0103).abs() < 1e-3, "{o:?}");
    }
    h.set_param("pan", -1.0);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], 1.0, 1e-5) && o[1].abs() < 1e-5, "{o:?}");
    h.set_param("pan", 0.5);
    let o = settled(&mut h, &OUT);
    let th = 0.75 * std::f32::consts::FRAC_PI_2;
    assert!(approx(o[0], th.cos(), 1e-5) && approx(o[1], th.sin(), 1e-5));
    assert!(
        approx(o[0] * o[0] + o[1] * o[1], 1.0, 1e-4),
        "constant power"
    );
    // Stereo source: each side keeps its own signal through the same law.
    let mut h = channel();
    dc(&mut h, "in_l", 0.4);
    dc(&mut h, "in_r", -0.2);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], 0.4 * K, 1e-5) && approx(o[1], -0.2 * K, 1e-5));
}

#[test]
fn ac_x1_width_zero_is_mono_and_one_is_unchanged() {
    let mut h = channel();
    dc(&mut h, "in_l", 1.0);
    dc(&mut h, "in_r", 0.0);
    h.set_param("width", 0.0);
    let o = settled(&mut h, &OUT);
    assert!(
        approx(o[0], 0.5 * K, 1e-5) && approx(o[1], 0.5 * K, 1e-5),
        "{o:?}"
    );
    h.set_param("width", 1.0);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], K, 1e-5) && o[1].abs() < 1e-5, "{o:?}");
}

#[test]
fn ac_x1_phase_invert_and_input_gain() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.25);
    h.set_param("input.phase", 1.0);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], -0.25, 1e-6), "{o:?}");
    h.set_param("input.phase", 0.0);
    h.set_param("input.gain", 6.0);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], 0.25 * 10f32.powf(0.3), 1e-5), "{o:?}");
    h.set_param("input.gain", -24.0);
    let o = settled(&mut h, &OUT);
    assert!(approx(o[0], 0.25 * 10f32.powf(-1.2), 1e-6), "{o:?}");
}

#[test]
fn ac_x1_fader_taper_reference_points() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    for (pos, want_db) in [
        (0.75f32, 0.0f32),
        (1.0, 7.4963),
        (0.5, -10.5655),
        (0.25, -28.6273),
    ] {
        h.set_param("fader", pos);
        let o = settled(&mut h, &OUT);
        let got = 20.0 * (o[0] / 0.5).log10();
        assert!((got - want_db).abs() < 1e-3, "pos {pos}: {got} dB");
        assert!((fader_db(pos) - want_db).abs() < 1e-3);
    }
    h.set_param("fader", 0.0);
    let o = settled(&mut h, &OUT);
    assert_eq!(o[0].to_bits(), 0.0f32.to_bits(), "−∞ at the bottom");
}

#[test]
fn ac_x1_eq_bands_match_analytic_biquads_within_0_1_db() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("eq.on", 1.0);
    let set = |h: &mut DeviceHarness, k: &str, v: f32| h.set_param(k, v);
    set(&mut h, "eq.hf.gain", 6.0);
    set(&mut h, "eq.hf.freq", 8_000.0);
    set(&mut h, "eq.hmf.gain", 9.0);
    set(&mut h, "eq.hmf.freq", 2_500.0);
    set(&mut h, "eq.hmf.q", 1.4);
    set(&mut h, "eq.lmf.gain", -7.0);
    set(&mut h, "eq.lmf.freq", 400.0);
    set(&mut h, "eq.lmf.q", 0.8);
    set(&mut h, "eq.lf.gain", 5.0);
    set(&mut h, "eq.lf.freq", 80.0);
    set(&mut h, "eq.lf.bell", 1.0);
    let bands = [
        (BiquadShape::HighShelf, 8_000.0, f64::from(SHELF_Q), 6.0),
        (BiquadShape::Peaking, 2_500.0, 1.4, 9.0),
        (BiquadShape::Peaking, 400.0, 0.8, -7.0),
        (BiquadShape::Peaking, 80.0, f64::from(OUTER_BELL_Q), 5.0),
    ];
    let mut worst = 0.0f64;
    for i in 0..20 {
        let hz = (25.0 * 720f64.powf(i as f64 / 19.0)).round();
        let want: f64 = bands
            .iter()
            .map(|(s, f, q, g)| BiquadCoeffs::design(*s, *f, *q, *g, SR).response_db(hz, SR))
            .sum();
        let got = sine_gain_db(&mut h, &["in_l"], "out_l", hz, 0.1, 0.2);
        worst = worst.max((got - want).abs());
        assert!((got - want).abs() < 0.1, "{hz} Hz: {got:.3} vs {want:.3}");
    }
    eprintln!("EQ worst deviation {worst:.4} dB");
}

#[test]
fn ac_x1_hp_and_lp_slopes() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("hpf.on", 1.0);
    h.set_param("hpf.freq", 400.0);
    let corner = sine_gain_db(&mut h, &["in_l"], "out_l", 400.0, 0.5, 0.2);
    assert!((corner + 3.0103).abs() < 0.1, "HP corner {corner}");
    let a = sine_gain_db(&mut h, &["in_l"], "out_l", 50.0, 0.5, 0.3);
    let b = sine_gain_db(&mut h, &["in_l"], "out_l", 100.0, 0.5, 0.3);
    eprintln!("HP 50→100 Hz slope {:.3} dB/oct", b - a);
    assert!((b - a - 18.0).abs() < 0.3, "HP slope {}", b - a);
    let model = PassFilter::new(PassKind::Highpass, 400.0, SR as f32);
    assert!((a - f64::from(model.response_db(50.0))).abs() < 0.1);
    h.set_param("hpf.on", 0.0);
    h.set_param("lpf.on", 1.0);
    h.set_param("lpf.freq", 2_000.0);
    let corner = sine_gain_db(&mut h, &["in_l"], "out_l", 2_000.0, 0.5, 0.2);
    assert!((corner + 3.0103).abs() < 0.1, "LP corner {corner}");
    let a = sine_gain_db(&mut h, &["in_l"], "out_l", 8_000.0, 0.5, 0.2);
    let b = sine_gain_db(&mut h, &["in_l"], "out_l", 16_000.0, 0.5, 0.2);
    eprintln!("LP 8→16 kHz slope {:.3} dB/oct", a - b);
    assert!(a - b >= 17.5, "LP slope {}", a - b);
}

fn comp_strip(peak: bool) -> DeviceHarness {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -20.0);
    h.set_param("comp.ratio", 4.0);
    h.set_param("comp.release", 200.0);
    h.set_param("comp.peak", if peak { 1.0 } else { 0.0 });
    h
}

#[test]
fn ac_x1_compressor_static_curve_within_0_2_db() {
    for peak in [false, true] {
        let mut h = comp_strip(peak);
        let mut worst = 0.0f64;
        for level in (-60..=0).step_by(6) {
            let level = f64::from(level);
            if (level + 20.0).abs() <= 3.0 {
                continue;
            }
            let got = sine_gain_db(
                &mut h,
                &["in_l"],
                "out_l",
                1_000.0,
                10f64.powf(level / 20.0),
                0.5,
            );
            let want = -f64::from(atomik_dsp::static_gain_reduction_db(
                level as f32,
                -20.0,
                4.0,
            ));
            worst = worst.max((got - want).abs());
            assert!(
                (got - want).abs() < 0.2,
                "peak={peak} {level} dB: {got:.3} vs {want:.3}"
            );
        }
        eprintln!("compressor (peak={peak}) worst static-curve error {worst:.4} dB");
    }
}

#[test]
fn ac_x1_gate_range_and_expander() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("gate.on", 1.0);
    h.set_param("gate.threshold", -40.0);
    h.set_param("gate.range", 30.0);
    let open = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.1, 0.3);
    assert!(open.abs() < 0.05, "open {open}");
    let closed = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.001, 1.0);
    assert!((closed + 30.0).abs() < 0.1, "closed {closed}");
    h.set_param("gate.expander", 1.0);
    let exp = sine_gain_db(
        &mut h,
        &["in_l"],
        "out_l",
        1_000.0,
        10f64.powf(-50.0 / 20.0),
        1.0,
    );
    assert!(
        (exp + 10.0).abs() < 0.3,
        "expander 10 dB under threshold: {exp}"
    );
}

#[test]
fn ac_x1_gate_hold() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("gate.on", 1.0);
    h.set_param("gate.threshold", -30.0);
    h.set_param("gate.range", 60.0);
    h.set_param("gate.hold", 50.0);
    h.set_param("gate.release", 10.0);
    run_signal(&mut h, &["in_l"], |_| 0.5, 75, &["out_l"]);
    let y = run_signal(&mut h, &["in_l"], |_| 0.001, 300, &["out_l"]);
    let g = |ms: f64| db(f64::from(y[0][(ms * 48.0) as usize]) / 0.001);
    assert!(g(60.0).abs() < 0.01, "held at 60 ms: {}", g(60.0));
    assert!(g(75.0) < -6.0, "releasing at 75 ms: {}", g(75.0));
    assert!((g(390.0) + 60.0).abs() < 0.2, "closed: {}", g(390.0));
}

#[test]
fn key_input_drives_dynamics_only_when_enabled_and_connected() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -30.0);
    h.set_param("comp.ratio", 20.0);
    h.set_param("comp.peak", 1.0);
    dc(&mut h, "key_in", 0.9);
    // key.on off: the quiet programme (−40 dBFS) is below the threshold.
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.01, 0.3);
    assert!(g.abs() < 0.05, "{g}");
    h.set_param("key.on", 1.0);
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.01, 0.3);
    let gr = atomik_dsp::static_gain_reduction_db(20.0 * 0.9f32.log10(), -30.0, 20.0);
    assert!((g + f64::from(gr)).abs() < 0.05, "{g} vs −{gr}");
    let cv = *h.output("gr_cv_out").last().unwrap();
    assert!(approx(cv, 1.0 - 10f32.powf(-gr / 20.0), 1e-3), "cv {cv}");
    // Key enabled but disconnected: the internal side-chain is used.
    h.set_connected("key_in", false);
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.01, 1.0);
    assert!(g.abs() < 0.05, "{g}");
    assert!(h.output("gr_cv_out").iter().all(|v| v.abs() < 1e-3));
}

#[test]
fn route_filters_to_sidechain_removes_them_from_the_audio_path() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("hpf.on", 1.0);
    h.set_param("hpf.freq", 1_000.0);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -30.0);
    // Normal routing: the 100 Hz tone is filtered away.
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 100.0, 0.5, 0.5);
    assert!(g < -55.0, "{g}");
    // Filters to side-chain: audio untouched, and the compressor no longer hears the tone.
    h.set_param("route.filters_to_sc", 1.0);
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 100.0, 0.5, 1.0);
    assert!(g.abs() < 0.1, "{g}");
    // Same routing with the HPF off: the compressor hears it again.
    h.set_param("hpf.on", 0.0);
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 100.0, 0.5, 1.0);
    assert!(g < -10.0, "{g}");
}

#[test]
fn route_dynamics_after_eq_changes_what_the_compressor_hears() {
    let setup = |post: bool| {
        let mut h = channel();
        h.set_param("pan", -1.0);
        h.set_param("eq.on", 1.0);
        h.set_param("eq.hmf.gain", 12.0);
        h.set_param("eq.hmf.freq", 1_000.0);
        h.set_param("comp.on", 1.0);
        h.set_param("comp.threshold", -20.0);
        h.set_param("comp.ratio", 20.0);
        h.set_param("route.dyn_post_eq", if post { 1.0 } else { 0.0 });
        sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.1, 0.5)
    };
    let pre = setup(false);
    let knee = f64::from(atomik_dsp::static_gain_reduction_db(-20.0, -20.0, 20.0));
    assert!((pre - (12.0 - knee)).abs() < 0.2, "dyn→EQ {pre}");
    let post = setup(true);
    // The compressor hears −8 dB: output −20 + 12 − GR(−8) ≈ −19.4 dB, a 0.6 dB gain.
    let want = 12.0 - f64::from(atomik_dsp::static_gain_reduction_db(-8.0, -20.0, 20.0));
    assert!((post - want).abs() < 0.2, "EQ→dyn {post} vs {want}");
}

#[test]
fn ac_x1_sends_pre_and_post_fader_taps() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    let s1 = ["send1_l", "send1_r", "send2_l"];
    let o = settled(&mut h, &s1);
    assert!(o.iter().all(|v| v.abs() < 1e-9), "sends default off: {o:?}");
    h.set_param("send1.on", 1.0);
    let o = settled(&mut h, &s1);
    assert!(
        approx(o[0], 0.5, 1e-5) && approx(o[1], 0.5, 1e-5),
        "post, unity: {o:?}"
    );
    assert!(o[2].abs() < 1e-9, "send 2 untouched");
    // Sends are not panned.
    h.set_param("pan", -1.0);
    let o = settled(&mut h, &s1);
    assert!(approx(o[1], 0.5, 1e-5), "{o:?}");
    // Post-fader follows the fader; pre-fader does not.
    h.set_param("fader", 0.0);
    let o = settled(&mut h, &s1);
    assert!(o[0].abs() < 1e-9, "post at fader −∞: {o:?}");
    h.set_param("send1.pre", 1.0);
    let o = settled(&mut h, &s1);
    assert!(approx(o[0], 0.5, 1e-5), "pre ignores the fader: {o:?}");
    h.set_param("fader", 1.0);
    h.set_param("send1.pre", 0.0);
    let o = settled(&mut h, &s1);
    assert!(
        approx(o[0], 0.5 * atomik_dsp::fader_gain(1.0), 1e-4),
        "{o:?}"
    );
    // Level taper is dB; the minimum is off.
    h.set_param("fader", 0.75);
    h.set_param("send1.level", -6.0);
    let o = settled(&mut h, &s1);
    assert!(approx(o[0], 0.5 * 10f32.powf(-0.3), 1e-5), "{o:?}");
    h.set_param("send1.level", -60.0);
    let o = settled(&mut h, &s1);
    assert!(o[0].abs() < 1e-9, "{o:?}");
    h.set_param("send1.level", 0.0);
    h.set_param("send1.on", 0.0);
    let o = settled(&mut h, &s1);
    assert!(o[0].abs() < 1e-9, "{o:?}");
    // Sends carry the width-processed signal.
    let mut h = channel();
    dc(&mut h, "in_l", 1.0);
    dc(&mut h, "in_r", 0.0);
    h.set_param("send8.on", 1.0);
    h.set_param("width", 0.0);
    let o = settled(&mut h, &["send8_l", "send8_r"]);
    assert!(approx(o[0], 0.5, 1e-5) && approx(o[1], 0.5, 1e-5), "{o:?}");
}

#[test]
fn ac_x1_mute_solo_silence_and_sends() {
    let all = ["out_l", "out_r", "send1_l", "send2_l"];
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    h.set_param("send1.on", 1.0); // post
    h.set_param("send2.on", 1.0);
    h.set_param("send2.pre", 1.0);
    let base = settled(&mut h, &all);
    assert!(base.iter().all(|v| v.abs() > 0.3), "{base:?}");
    h.set_param("mute", 1.0);
    let o = settled(&mut h, &all);
    assert!(
        o.iter().all(|v| v.abs() < 1e-9),
        "mute silences out and all sends: {o:?}"
    );
    // Mute is ignored on a soloed channel.
    h.set_param("solo", 1.0);
    let o = settled(&mut h, &all);
    assert_eq!(o, base);
    h.set_param("mute", 0.0);
    let o = settled(&mut h, &all);
    assert_eq!(o, base, "solo alone has no DSP effect");
    h.set_param("solo", 0.0);
    h.set_param("internal.solo_silence", 1.0);
    let o = settled(&mut h, &all);
    assert!(o.iter().all(|v| v.abs() < 1e-9), "solo silence: {o:?}");
    // The insert send and the GR CV are not muted.
    let o = settled(&mut h, &["insert_send_l"]);
    assert!(approx(o[0], 0.5, 1e-6));
}

/// Output of a default strip (input 0.5 on `in_l`, pan centre) after `setup`, settled.
fn out_with(setup: impl FnOnce(&mut DeviceHarness)) -> f32 {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    setup(&mut h);
    settled(&mut h, &["out_l"])[0]
}

#[test]
fn a_vca_offset_scales_the_fader_and_zero_changes_nothing() {
    let base = out_with(|_| {});
    let same = out_with(|h| h.set_param("internal.vca_db", 0.0));
    assert_eq!(same.to_bits(), base.to_bits(), "0 dB is bit-identical");
    let g = |db: f32| base * 10f32.powf(db / 20.0);
    assert!(approx(
        out_with(|h| h.set_param("internal.vca_db", -6.0)),
        g(-6.0),
        1e-5
    ));
    assert!(approx(
        out_with(|h| h.set_param("internal.vca_db", 6.0)),
        g(6.0),
        1e-5
    ));
    // It adds to the strip's own fader (here -inf + anything stays off, 0.5 is -9 dB).
    let half = out_with(|h| h.set_param("fader", 0.5));
    let both = out_with(|h| {
        h.set_param("fader", 0.5);
        h.set_param("internal.vca_db", -6.0);
    });
    assert!(
        approx(both, half * 10f32.powf(-6.0 / 20.0), 1e-5),
        "{both} {half}"
    );
    assert!(
        out_with(|h| h.set_param("internal.vca_db", -60.0)).abs() < 1e-9,
        "off"
    );
    // The fader CV rides on top of the VCA too.
    let cv = out_with(|h| {
        h.set_param("internal.vca_db", -6.0);
        h.set_param("fader_cv_trim", 1.0);
        dc(h, "fader_cv_in", 0.25);
    });
    assert!(approx(cv, base * 10f32.powf(0.0), 1e-3), "{cv}");
}

#[test]
fn a_vca_offset_ramps_without_a_step() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    settled(&mut h, &["out_l"]);
    h.set_param("internal.vca_db", -12.0);
    let o = h.render(20, &["out_l"]);
    let ramp: Vec<f32> = o[0].clone();
    assert!(max_step(&ramp) < 0.05, "smoothed: {}", max_step(&ramp));
}

#[test]
fn vca_mute_and_solo_follow_the_strips_own_rules() {
    let base = out_with(|_| {});
    let muted = out_with(|h| h.set_param("internal.vca_mute", 1.0));
    assert!(muted.abs() < 1e-9, "a VCA mute silences the strip");
    // A VCA solo counts as the strip's own: it beats the VCA mute and the strip's mute.
    let soloed = out_with(|h| {
        h.set_param("internal.vca_mute", 1.0);
        h.set_param("mute", 1.0);
        h.set_param("internal.vca_solo", 1.0);
    });
    assert_eq!(soloed.to_bits(), base.to_bits());
    // The strip's own mute still works under a VCA that is neither muted nor soloed.
    let own = out_with(|h| h.set_param("mute", 1.0));
    assert!(own.abs() < 1e-9);
}

#[test]
fn insert_send_return_routing() {
    let ins = ["insert_send_l", "insert_send_r", "out_l", "out_r"];
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("input.gain", 6.0);
    dc(&mut h, "in_l", 0.25);
    let g6 = 10f32.powf(0.3);
    let o = settled(&mut h, &ins);
    assert!(
        approx(o[0], 0.25 * g6, 1e-5) && approx(o[1], 0.25 * g6, 1e-5),
        "{o:?}"
    );
    assert!(
        approx(o[2], 0.25 * g6, 1e-5),
        "no return: pass-through {o:?}"
    );
    // A connected return replaces the signal (one side feeds both).
    dc(&mut h, "insert_return_l", 0.1);
    let o = settled(&mut h, &ins);
    assert!(approx(o[0], 0.25 * g6, 1e-5), "send unchanged");
    assert!(approx(o[2], 0.1, 1e-6), "{o:?}");
    h.set_param("pan", 0.0);
    let o = settled(&mut h, &ins);
    assert!(approx(o[3], 0.1 * K, 1e-6), "mono return feeds R: {o:?}");
    dc(&mut h, "insert_return_r", -0.1);
    let o = settled(&mut h, &ins);
    assert!(approx(o[3], -0.1 * K, 1e-6), "stereo return {o:?}");
    // Bypass forces pass-through.
    h.set_param("insert.bypass", 1.0);
    let o = settled(&mut h, &ins);
    assert!(approx(o[2], 0.25 * g6 * K, 1e-5), "{o:?}");
    // Insert point: after EQ by default, before dynamics/EQ with route.insert_pre.
    let mut h = channel();
    dc(&mut h, "in_l", 0.25);
    h.set_param("eq.on", 1.0);
    h.set_param("eq.lf.gain", 6.0); // shelf: +6 dB at DC
    let o = settled(&mut h, &ins);
    assert!(
        approx(o[0], 0.25 * 10f32.powf(0.3), 1e-4),
        "post-EQ tap {o:?}"
    );
    h.set_param("route.insert_pre", 1.0);
    let o = settled(&mut h, &ins);
    assert!(approx(o[0], 0.25, 1e-4), "pre-EQ tap {o:?}");
    dc(&mut h, "insert_return_l", 0.1);
    let o = settled(&mut h, &ins);
    assert!(
        approx(o[2], 0.1 * 10f32.powf(0.3) * K, 1e-4),
        "return goes through EQ {o:?}"
    );
}

/// Position of the impulse fed at sample 0 on `out_l`.
fn impulse_position(h: &mut DeviceHarness) -> usize {
    h.device_mut().reset();
    let y = run_signal(
        h,
        &["in_l"],
        |n| if n == 0 { 1.0 } else { 0.0 },
        80,
        &["out_l"],
    );
    y[0].iter().position(|v| v.abs() > 0.5).unwrap()
}

#[test]
fn latency_adjust_positive_delays_without_reporting() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    assert_eq!(h.device().latency_frames(), 0);
    assert_eq!(impulse_position(&mut h), 0);
    for d in [100i32, 1, 4_800] {
        h.set_param("latency.adjust", d as f32);
        assert_eq!(h.device().latency_frames(), 0, "+{d}: not reported");
        assert_eq!(impulse_position(&mut h), d as usize, "+{d}: really delayed");
    }
}

#[test]
fn latency_adjust_reaches_ten_seconds_and_clamps_there() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    for d in [4_801u32, 20_000, MAX_LATENCY_ADJUST] {
        h.set_param("latency.adjust", d as f32);
        h.device_mut().reset();
        let batches = d as usize / BATCH + 4;
        let y = run_signal(
            &mut h,
            &["in_l"],
            |n| if n == 0 { 1.0 } else { 0.0 },
            batches,
            &["out_l"],
        );
        assert_eq!(h.device().latency_frames(), 0, "+{d}: not reported");
        assert_eq!(
            y[0].iter().position(|v| v.abs() > 0.5),
            Some(d as usize),
            "+{d}: really delayed"
        );
    }
    h.set_param("latency.adjust", 1e9);
    h.device_mut().reset();
    let y = run_signal(
        &mut h,
        &["in_l"],
        |n| if n == 0 { 1.0 } else { 0.0 },
        MAX_LATENCY_ADJUST as usize / BATCH + 4,
        &["out_l"],
    );
    assert_eq!(
        y[0].iter().position(|v| v.abs() > 0.5),
        Some(MAX_LATENCY_ADJUST as usize),
        "clamped"
    );
}

#[test]
fn latency_adjust_negative_reports_phantom_latency_without_delaying() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    for d in [-100i32, -1, -4_800, -480_000] {
        h.set_param("latency.adjust", d as f32);
        assert_eq!(
            h.device().latency_frames(),
            d.unsigned_abs(),
            "{d}: reported"
        );
        assert_eq!(impulse_position(&mut h), 0, "{d}: not delayed");
    }
    h.set_param("latency.adjust", -1e9);
    assert_eq!(h.device().latency_frames(), MAX_LATENCY_ADJUST);
    h.set_param("latency.adjust", 0.0);
    assert_eq!(h.device().latency_frames(), 0);
    assert_eq!(impulse_position(&mut h), 0);
}

#[test]
fn gain_changes_are_smoothed() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    settled(&mut h, &OUT);
    for (k, v) in [
        ("fader", 0.0),
        ("fader", 1.0),
        ("mute", 1.0),
        ("mute", 0.0),
        ("input.phase", 1.0),
        ("pan", 1.0),
        ("eq.on", 1.0),
        ("eq.lf.gain", 18.0),
        ("eq.on", 0.0),
    ] {
        h.set_param(k, v);
        let y = h.render(20, &OUT);
        // No jump: every change is spread over at least 200 samples (5 ms ramps are 240).
        for o in &y {
            let range = o.iter().fold(0.0f32, |m, v| m.max((v - o[0]).abs()));
            let step = max_step(o);
            assert!(
                step <= range / 200.0 + 1e-4,
                "{k} = {v}: step {step}, range {range}"
            );
        }
    }
}

#[test]
fn set_param_every_param_min_max_does_not_panic() {
    let mut h = channel();
    for k in ["in_l", "in_r", "key_in", "insert_return_l"] {
        dc(&mut h, k, 0.3);
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn process_is_allocation_free() {
    let mut h = channel();
    for (i, k) in [
        "in_l",
        "in_r",
        "key_in",
        "insert_return_l",
        "insert_return_r",
    ]
    .iter()
    .enumerate()
    {
        dc(&mut h, k, 0.1 * i as f32);
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
fn tone_helper_is_exact_on_whole_cycles() {
    let x: Vec<f32> = (0..48_000).map(sine(440.0, 0.3)).collect();
    assert!((tone_amplitude(&x, 440.0, 0) - 0.3).abs() < 1e-6);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn any_params_and_signals_give_finite_bounded_output(
        norms in prop::collection::vec(0.0f32..=1.0, PARAM_COUNT),
        seed in any::<u64>(),
        connect_return in any::<bool>(),
    ) {
        let mut h = channel();
        for (p, n) in DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        let mut rng = atomik_dsp::Rng::new(seed);
        let outs = crate::test_support::output_keys(&DESCRIPTOR);
        for _ in 0..30 {
            let mut keys = vec!["in_l", "in_r", "key_in"];
            if connect_return {
                keys.push("insert_return_l");
                keys.push("insert_return_r");
            }
            for k in keys {
                let b = h.input_mut(k);
                for s in b.iter_mut() {
                    *s = rng.next_f32() * 2.0 - 1.0;
                }
            }
            h.process();
            for k in &outs {
                for v in h.output(k) {
                    prop_assert!(v.is_finite() && v.abs() < 1e5, "{k}: {v}");
                }
            }
            let cv = h.output("gr_cv_out");
            prop_assert!(cv.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}

#[test]
fn stage_table_matches_plan_section_9() {
    let mut stage1: Vec<String> = vec!["insert_return_l".into(), "insert_return_r".into()];
    for k in 1..=8 {
        stage1.push(format!("send{k}_l"));
        stage1.push(format!("send{k}_r"));
    }
    for s in ["out_l", "out_r", "gr_cv_out"] {
        stage1.push(s.into());
    }
    stage1.extend(["fader_cv_in".into(), "pan_cv_in".into()]);
    for k in 1..=8 {
        stage1.push(format!("send{k}_level_cv_in"));
    }
    stage1.extend([
        "parallel_out_l".into(),
        "parallel_out_r".into(),
        "follow_in".into(),
    ]);
    crate::test_support::assert_stages(
        &DESCRIPTOR,
        &[
            vec!["in_l", "in_r", "key_in", "insert_send_l", "insert_send_r"],
            stage1.iter().map(String::as_str).collect(),
        ],
    );
}

#[test]
fn stages_run_separately_through_an_external_insert() {
    use crate::test_support::StageRig;
    let mut rig = StageRig::new(Box::new(Channel::new()));
    rig.device.set_param(ParamId(P_PAN as u32), -1.0);
    rig.device.set_param(ParamId(P_INPUT_GAIN as u32), 6.0);
    rig.device.set_param(ParamId(P_SEND_FIRST as u32), 1.0); // send1.on
    rig.device.reset(); // land every ramp on its target
    rig.set_input("in_l", 0.25);
    let g6 = 10f32.powf(0.3);
    for _ in 0..10 {
        // Sentinels: stage 0 must not touch stage-1 outputs.
        let out = DESCRIPTOR.socket_by_key("out_l").unwrap().id.0 as usize;
        rig.signals[out] = [7.0; BATCH];
        rig.stage(0);
        assert!(
            rig.get("out_l").iter().all(|v| approx(*v, 7.0, 0.0)),
            "stage 0 wrote out_l"
        );
        let send = rig.get("insert_send_l")[0];
        assert!(approx(send, 0.25 * g6, 1e-5), "insert send {send}");
        // The "effect" halves the insert send into the return, between the stages.
        let half = rig.get("insert_send_l").map(|v| v * 0.5);
        let ret = DESCRIPTOR.socket_by_key("insert_return_l").unwrap().id.0 as usize;
        rig.signals[ret] = half;
        rig.connected[ret] = true;
        rig.stage(1);
    }
    assert!(approx(rig.get("out_l")[BATCH - 1], 0.125 * g6, 1e-5));
    assert!(approx(rig.get("send1_l")[BATCH - 1], 0.125 * g6, 1e-5));
    // With route.insert_pre the EQ runs in stage 1, after the return.
    rig.device.set_param(ParamId(P_INSERT_PRE as u32), 1.0);
    rig.device.set_param(ParamId(P_EQ_ON as u32), 1.0);
    rig.device.set_param(ParamId(P_EQ_FIRST as u32 + 9), 6.0); // eq.lf.gain (shelf, DC +6 dB)
    rig.device.reset();
    for _ in 0..40 {
        rig.stage(0);
        let ret = DESCRIPTOR.socket_by_key("insert_return_l").unwrap().id.0 as usize;
        rig.signals[ret] = rig.get("insert_send_l").map(|v| v * 0.5);
        rig.stage(1);
    }
    assert!(
        approx(rig.get("insert_send_l")[0], 0.25 * g6, 1e-4),
        "pre-EQ tap"
    );
    assert!(
        approx(rig.get("out_l")[BATCH - 1], 0.125 * g6 * g6, 1e-3),
        "{}",
        rig.get("out_l")[0]
    );
}

#[test]
fn process_equals_stage_by_stage() {
    use crate::test_support::StageRig;
    let mut rig = StageRig::new(Box::new(Channel::new()));
    let mut h = channel();
    for (k, v) in [
        ("comp.on", 1.0),
        ("eq.on", 1.0),
        ("eq.hmf.gain", 5.0),
        ("send2.on", 1.0),
        ("latency.adjust", 70.0),
    ] {
        let p = DESCRIPTOR.param_by_key(k).unwrap();
        rig.device.set_param(p.id, v);
        h.set_param(k, v);
    }
    let mut rng = atomik_dsp::Rng::new(5);
    for _ in 0..30 {
        let x: [f32; BATCH] = std::array::from_fn(|_| rng.next_bipolar());
        *h.input_mut("in_l") = x;
        let i = DESCRIPTOR.socket_by_key("in_l").unwrap().id.0 as usize;
        rig.signals[i] = x;
        rig.connected[i] = true;
        h.process();
        rig.stage(0);
        rig.stage(1);
        for k in ["insert_send_l", "out_l", "out_r", "send2_l", "gr_cv_out"] {
            assert_eq!(h.output(k), rig.get(k), "{k}");
        }
    }
}

#[test]
fn params_set_before_the_first_batch_apply_without_ramp() {
    use crate::test_support::assert_first_batch_settled;
    let params = [
        ("fader", 0.5),
        ("pan", -0.4),
        ("width", 0.3),
        ("input.gain", 5.0),
        ("input.phase", 1.0),
        ("eq.on", 1.0),
        ("eq.lf.gain", 12.0),
        ("eq.lf.freq", 300.0),
        ("eq.hmf.gain", -9.0),
        ("hpf.on", 1.0),
        ("hpf.freq", 200.0),
        ("send1.on", 1.0),
        ("send1.level", -6.0),
        ("send2.on", 1.0),
        ("send2.pre", 1.0),
    ];
    let outs = ["out_l", "out_r", "send1_l", "send2_r", "insert_send_l"];
    let input = sine(440.0, 0.25);
    // Fresh device: params set after prepare, first batch.
    let mut a = channel();
    for (k, v) in params {
        a.set_param(k, v);
    }
    let first = run_signal(&mut a, &["in_l"], &input, 1, &outs);
    // Reference: the same params fully settled, then reset (filter state cleared, params kept).
    let mut b = channel();
    for (k, v) in params {
        b.set_param(k, v);
    }
    b.render(40, &outs);
    b.device_mut().reset();
    let reference = run_signal(&mut b, &["in_l"], &input, 1, &outs);
    for ((k, x), y) in outs.iter().zip(&first).zip(&reference) {
        assert_eq!(x, y, "{k}: first batch differs from settled params");
    }
    // Without filters in the path the first batch equals the steady state outright.
    let mut h = channel();
    dc(&mut h, "in_l", 0.25);
    for (k, v) in [
        ("fader", 0.5),
        ("pan", -0.4),
        ("width", 0.3),
        ("input.gain", 5.0),
    ] {
        h.set_param(k, v);
    }
    h.set_param("send1.on", 1.0);
    h.set_param("send1.level", -6.0);
    assert_first_batch_settled(&mut h, &["out_l", "out_r", "send1_l"], 40);
    // Muted before the first batch: silent from sample 0.
    let mut h = channel();
    dc(&mut h, "in_l", 1.0);
    h.set_param("mute", 1.0);
    h.set_param("send1.on", 1.0);
    h.process();
    for k in ["out_l", "out_r", "send1_l"] {
        assert!(
            h.output(k).iter().all(|v| v.abs() < 1e-9),
            "{k} audible at start"
        );
    }
    // Same after reset(): the next batch snaps.
    h.set_param("mute", 0.0);
    h.device_mut().reset();
    h.set_param("fader", 0.0);
    h.process();
    assert!(
        h.output("out_l").iter().all(|v| v.abs() < 1e-9),
        "after reset"
    );
}

#[test]
fn param_changes_after_the_first_batch_still_ramp() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    h.process();
    assert!(approx(h.output("out_l")[0], 0.5, 1e-6));
    h.set_param("fader", 0.0);
    h.process();
    let o = h.output("out_l");
    assert!(
        o[0] > 0.49 && o[BATCH - 1] > 0.3,
        "ramping, not jumping: {} {}",
        o[0],
        o[BATCH - 1]
    );
    h.set_param("eq.on", 1.0);
    h.set_param("eq.lf.gain", 12.0);
    h.set_param("fader", 0.75);
    h.render(10, &["out_l"]);
    h.set_param("mute", 1.0);
    h.process();
    assert!(h.output("out_l")[0].abs() > 0.1, "mute ramps");
}

#[test]
fn connected_insert_return_is_used_from_the_first_sample() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    dc(&mut h, "insert_return_l", 0.1);
    h.process();
    assert!(
        h.output("out_l").iter().all(|v| approx(*v, 0.1, 1e-7)),
        "dry leak: {:?}",
        &h.output("out_l")[..4]
    );
    // Same after reset(), and through the staged path.
    h.device_mut().reset();
    h.process();
    assert!(
        h.output("out_l").iter().all(|v| approx(*v, 0.1, 1e-7)),
        "after reset"
    );
    let mut rig = crate::test_support::StageRig::new(Box::new(Channel::new()));
    rig.device.set_param(ParamId(P_PAN as u32), -1.0);
    rig.set_input("in_l", 0.5);
    rig.set_input("insert_return_l", 0.1);
    rig.stage(0);
    rig.stage(1);
    assert!(
        rig.get("out_l").iter().all(|v| approx(*v, 0.1, 1e-7)),
        "staged"
    );
    // A later connection still crossfades.
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    h.process();
    dc(&mut h, "insert_return_l", 0.1);
    h.process();
    assert!(
        h.output("out_l")[0] > 0.45,
        "crossfades after the first batch"
    );
}

/// Settled `out_l`/`out_r` for a DC input of 1.0 on `in_l` with the given setup.
fn cv_out(setup: impl Fn(&mut DeviceHarness)) -> Vec<f32> {
    let mut h = channel();
    dc(&mut h, "in_l", 1.0);
    setup(&mut h);
    settled(&mut h, &OUT)
}

#[test]
fn fader_cv_adds_trim_times_24_db_and_clamps() {
    let base = K;
    let g = |db: f32| base * 10f32.powf(db / 20.0);
    let with = |trim: f32, cv: f32| {
        cv_out(|h| {
            h.set_param("fader_cv_trim", trim);
            dc(h, "fader_cv_in", cv);
        })
    };
    assert!(
        approx(with(1.0, 0.25)[0], g(6.0), 1e-3),
        "{:?}",
        with(1.0, 0.25)
    );
    assert!(approx(with(1.0, -0.25)[0], g(-6.0), 1e-3));
    assert!(approx(with(0.5, 0.5)[0], g(6.0), 1e-3), "trim scales");
    assert!(approx(with(-1.0, 0.25)[0], g(-6.0), 1e-3), "trim inverts");
    // Clamped beyond +-1.
    assert!(approx(with(1.0, 5.0)[0], g(24.0), 1e-2));
    assert!(approx(with(1.0, -5.0)[0], g(-24.0), 1e-3));
    // Amount 0 cancels; the CV rides on the knob, and a closed fader stays closed.
    assert!(approx(with(0.0, 1.0)[0], base, 1e-5));
    let low = cv_out(|h| {
        h.set_param("fader", 0.0);
        dc(h, "fader_cv_in", 1.0);
    });
    assert!(low[0].abs() < 1e-9);
}

#[test]
fn pan_cv_adds_to_the_pan_position_and_clamps() {
    let knob = |pan: f32| cv_out(move |h| h.set_param("pan", pan));
    let with = |pan: f32, trim: f32, cv: f32| {
        cv_out(move |h| {
            h.set_param("pan", pan);
            h.set_param("pan_cv_trim", trim);
            dc(h, "pan_cv_in", cv);
        })
    };
    for (a, b) in with(0.0, 1.0, 0.5).iter().zip(knob(0.5)) {
        assert!(approx(*a, b, 1e-4));
    }
    for (a, b) in with(-0.25, 0.5, 1.0).iter().zip(knob(0.25)) {
        assert!(approx(*a, b, 1e-4), "trim scales and adds to the knob");
    }
    // Beyond +-1 clamps to hard right / left.
    let r = with(0.0, 1.0, 5.0);
    assert!(r[0].abs() < 1e-3 && approx(r[1], 1.0, 1e-3), "{r:?}");
    let l = with(0.0, 1.0, -5.0);
    assert!(approx(l[0], 1.0, 1e-3) && l[1].abs() < 1e-3, "{l:?}");
    // Amount 0 cancels.
    for (a, b) in with(0.3, 0.0, 1.0).iter().zip(knob(0.3)) {
        assert!(approx(*a, b, 1e-6));
    }
}

#[test]
fn send_level_cv_adds_trim_times_66_db_and_clamps_per_send() {
    for k in [1usize, 5, 8] {
        let run = |level: f32, trim: f32, cv: f32, on: bool| {
            let mut h = channel();
            dc(&mut h, "in_l", 1.0);
            h.set_param(&format!("send{k}.on"), if on { 1.0 } else { 0.0 });
            h.set_param(&format!("send{k}.level"), level);
            h.set_param(&format!("send{k}.level_cv_trim"), trim);
            dc(&mut h, &format!("send{k}_level_cv_in"), cv);
            settled(&mut h, &[&format!("send{k}_l")])[0]
        };
        let lin = |db: f32| 10f32.powf(db / 20.0);
        // -20 dB + 0.25 * 66 = -3.5 dB.
        assert!(
            approx(run(-20.0, 1.0, 0.25, true), lin(-3.5), 1e-3),
            "send {k}"
        );
        // Half trim: 0.5 * 0.5 * 66 = +16.5 dB.
        assert!(approx(run(-20.0, 0.5, 0.5, true), lin(-3.5), 1e-3));
        // Clamped: cv 5 behaves as cv 1, and the level clamps at +6 dB / off at -60.
        assert!(approx(run(-20.0, 1.0, 5.0, true), lin(6.0), 1e-3));
        assert!(run(-20.0, 1.0, -5.0, true).abs() < 1e-9);
        // Amount 0 cancels; an off send stays off.
        assert!(approx(run(-20.0, 0.0, 1.0, true), lin(-20.0), 1e-4));
        assert!(run(-20.0, 1.0, 1.0, false).abs() < 1e-9);
    }
}

#[test]
fn cv_inputs_are_allocation_free_and_finite() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.3);
    h.set_param("send2.on", 1.0);
    for i in 0..300usize {
        let v = ((i % 9) as f32 - 4.0) * 0.7;
        dc(&mut h, "fader_cv_in", v);
        dc(&mut h, "pan_cv_in", -v);
        dc(&mut h, "send2_level_cv_in", v);
        h.process();
        for o in ["out_l", "out_r", "send2_l"] {
            assert!(h.output(o).iter().all(|x| x.is_finite()));
        }
    }
    // Unplugging restores the knob values (send, fader, pan).
    for k in ["fader_cv_in", "pan_cv_in", "send2_level_cv_in"] {
        h.set_connected(k, false);
    }
    let o = settled(&mut h, &["out_l", "out_r", "send2_l"]);
    assert!(
        approx(o[0], 0.3 * K, 1e-5) && approx(o[1], 0.3 * K, 1e-5),
        "{o:?}"
    );
    assert!(approx(o[2], 0.3, 1e-5), "{o:?}");
}

// ------------------------------------------------------------------ input trim and polarity

/// A program with loud and quiet passages: a 220 Hz sine whose level steps every 0.1 s.
fn program(n: usize) -> f32 {
    let level = if (n / 4_800).is_multiple_of(2) {
        0.9
    } else {
        0.05
    };
    sine(220.0, level)(n)
}

/// `out_l` of a strip with the left pan and `setup` applied before the first batch.
fn program_out(batches: usize, setup: impl FnOnce(&mut DeviceHarness)) -> Vec<f32> {
    let mut h = channel();
    h.set_param("pan", -1.0);
    setup(&mut h);
    run_signal(&mut h, &["in_l", "in_r"], program, batches, &["out_l"]).remove(0)
}

#[test]
fn polarity_inverts_the_strip_and_nulls_against_the_inverted_copy() {
    let normal = program_out(200, |_| {});
    let flipped = program_out(200, |h| h.set_param("input.phase", 1.0));
    assert!(normal.iter().any(|s| s.abs() > 0.5), "it sounds");
    for (a, b) in normal.iter().zip(&flipped) {
        assert_eq!(
            (a + 0.0).to_bits(),
            (-b + 0.0).to_bits(),
            "an exact sign flip"
        );
        assert!((a + b).abs() == 0.0, "the sum of the pair is silence");
    }
    // Flipped back it is the normal strip again, after the ramp.
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("input.phase", 1.0);
    run_signal(&mut h, &["in_l", "in_r"], program, 20, &["out_l"]);
    h.set_param("input.phase", 0.0);
    let tail = run_signal(&mut h, &["in_l", "in_r"], program, 40, &["out_l"]).remove(0);
    assert!(tail.iter().any(|s| s.abs() > 0.5));
}

#[test]
fn polarity_flips_through_the_ramp_without_a_click() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    dc(&mut h, "in_l", 0.5);
    settled(&mut h, &["out_l"]);
    h.set_param("input.phase", 1.0);
    let ramp = h.render(8, &["out_l"]).remove(0);
    assert!(
        max_step(&ramp) < 0.5 * 2.0 / 200.0,
        "a flip crosses zero over about 5 ms, not in one sample: {}",
        max_step(&ramp)
    );
    assert!(approx(*ramp.last().unwrap(), -0.5, 1e-6));
}

#[test]
fn input_trim_is_the_db_gain_over_the_whole_range() {
    for db in [-24.0f32, -12.0, -1.5, 0.0, 3.0, 12.0, 24.0] {
        let got = program_out(8, |h| h.set_param("input.gain", db));
        let plain = program_out(8, |_| {});
        let want = 10f32.powf(db / 20.0);
        for (g, p) in got.iter().zip(&plain).skip(20) {
            assert!(
                approx(*g, *p * want, 1e-5 * want.max(1.0)),
                "{db} dB: {g} vs {}",
                p * want
            );
        }
    }
    // Beyond the range it is clamped by the descriptor, not by the DSP: +24 is the top.
    assert!(
        approx(PARAMS[P_INPUT_GAIN].max, 24.0, 0.0) && approx(PARAMS[P_INPUT_GAIN].min, -24.0, 0.0)
    );
}

// ------------------------------------------------------------------ the parallel compressor

fn pcomp_params() -> [(&'static str, f32); 5] {
    [
        ("pcomp.threshold", -30.0),
        ("pcomp.ratio", 8.0),
        ("pcomp.attack", 10.0),
        ("pcomp.release", 150.0),
        ("pcomp.makeup", 6.0),
    ]
}

#[test]
fn the_parallel_compressor_is_off_by_default_and_costs_the_signal_nothing() {
    let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap().default;
    assert_eq!(d("pcomp.on"), 0.0);
    assert!(approx(d("pcomp.mix"), 0.5, 0.0));
    let base = program_out(200, |_| {});
    // Off with the mix up, on with the mix at 0: both are the untouched strip, bit for bit.
    let off = program_out(200, |h| h.set_param("pcomp.mix", 1.0));
    let zero = program_out(200, |h| {
        h.set_param("pcomp.on", 1.0);
        h.set_param("pcomp.mix", 0.0);
    });
    let set_all = program_out(200, |h| {
        for (k, v) in pcomp_params() {
            h.set_param(k, v);
        }
    });
    for other in [&off, &zero, &set_all] {
        assert!(
            base.iter()
                .zip(other.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
}

#[test]
fn the_parallel_compressor_at_mix_one_is_the_standalone_compressor_with_its_makeup() {
    let got = program_out(200, |h| {
        h.set_param("pcomp.on", 1.0);
        h.set_param("pcomp.mix", 1.0);
        for (k, v) in pcomp_params() {
            h.set_param(k, v);
        }
    });
    let mut c = Compressor::new(SR as f32);
    c.set_threshold_db(-30.0);
    c.set_ratio(8.0);
    c.set_attack_ms(10.0);
    c.set_release_ms(150.0);
    let makeup = db_to_gain(6.0);
    let mut worst = 0.0f32;
    let mut squashed = false;
    for (n, y) in got.iter().enumerate() {
        let x = program(n);
        let g = c.process(x, x) * makeup;
        // Pan hard left, unity fader and full width leave the left channel as it is.
        worst = worst.max((y - x * g).abs());
        squashed |= g < 0.5;
    }
    assert!(squashed, "the loud passages are really compressed");
    assert!(
        worst < 1e-5,
        "a standalone compressor render differs by {worst}"
    );
    // The compressed strip is clearly not the dry one.
    let dry = program_out(200, |_| {});
    assert!(got.iter().zip(&dry).any(|(a, b)| (a - b).abs() > 0.05));
}

#[test]
fn the_parallel_blend_is_linear_between_the_dry_and_the_compressed_signal() {
    let set = |mix: f32| {
        program_out(200, move |h| {
            h.set_param("pcomp.on", 1.0);
            h.set_param("pcomp.mix", mix);
            for (k, v) in pcomp_params() {
                h.set_param(k, v);
            }
        })
    };
    let (dry, half, wet) = (program_out(200, |_| {}), set(0.5), set(1.0));
    for n in 0..dry.len() {
        let want = 0.5 * dry[n] + 0.5 * wet[n];
        assert!(approx(half[n], want, 1e-5), "{n}: {} vs {want}", half[n]);
    }
}

#[test]
fn the_parallel_compressor_adds_no_latency_and_keeps_the_dry_path_aligned() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("pcomp.on", 1.0);
    h.set_param("pcomp.mix", 1.0);
    for (k, v) in pcomp_params() {
        h.set_param(k, v);
    }
    assert_eq!(h.device().latency_frames(), 0, "nothing is reported");
    // A unit impulse stays at its frame (the compressor has no lookahead) at any mix.
    for mix in [0.0f32, 0.3, 1.0] {
        h.set_param("pcomp.mix", mix);
        assert_eq!(impulse_position(&mut h), 0, "mix {mix}");
    }
    // With a latency adjust the pair moves together: still one impulse, not two.
    h.set_param("latency.adjust", 100.0);
    h.set_param("pcomp.mix", 0.5);
    h.device_mut().reset();
    let y = run_signal(
        &mut h,
        &["in_l"],
        |n| if n == 0 { 1.0 } else { 0.0 },
        8,
        &["out_l"],
    );
    let hits: Vec<usize> = y[0]
        .iter()
        .enumerate()
        .filter(|(_, v)| v.abs() > 1e-3)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, [100]);
}

#[test]
fn switching_the_parallel_compressor_on_and_off_ramps_without_a_click() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    for (k, v) in pcomp_params() {
        h.set_param(k, v);
    }
    h.set_param("pcomp.mix", 1.0);
    dc(&mut h, "in_l", 0.5);
    dc(&mut h, "in_r", 0.5);
    settled(&mut h, &["out_l"]);
    h.set_param("pcomp.on", 1.0);
    let up = h.render(6, &["out_l"]).remove(0);
    assert!(max_step(&up) < 0.05, "on: {}", max_step(&up));
    h.set_param("pcomp.on", 0.0);
    let down = h.render(60, &["out_l"]).remove(0);
    assert!(max_step(&down) < 0.05, "off: {}", max_step(&down));
    assert!(
        approx(*down.last().unwrap(), 0.5, 1e-6),
        "back to the dry level"
    );
    // Switched off and settled the section does not run: a fresh strip agrees bit for bit.
    let plain = {
        let mut p = channel();
        p.set_param("pan", -1.0);
        dc(&mut p, "in_l", 0.5);
        dc(&mut p, "in_r", 0.5);
        settled(&mut p, &["out_l"])[0]
    };
    assert_eq!(down.last().unwrap().to_bits(), plain.to_bits());
}

#[test]
fn the_parallel_compressor_taps_the_signal_after_the_insert_return() {
    // The return replaces the signal, so the compressor listens to the return: a loud return
    // is squashed even though the input is quiet.
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("pcomp.on", 1.0);
    h.set_param("pcomp.mix", 1.0);
    h.set_param("pcomp.makeup", 0.0);
    h.set_param("pcomp.threshold", -40.0);
    h.set_param("pcomp.ratio", 20.0);
    h.set_param("pcomp.attack", 0.1);
    dc(&mut h, "in_l", 0.001);
    dc(&mut h, "insert_return_l", 0.8);
    let o = settled(&mut h, &["out_l"]);
    assert!(
        o[0] < 0.4 && o[0] > 0.0,
        "the return {} is compressed",
        o[0]
    );
}

#[test]
fn follow_in_scales_the_post_fader_sends_only() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    dc(&mut h, "follow_in", 0.5);
    h.set_param("send1.on", 1.0); // post
    h.set_param("send2.on", 1.0);
    h.set_param("send2.pre", 1.0);
    let o = settled(&mut h, &["out_l", "send1_l", "send2_l"]);
    assert!(approx(o[1], 0.25, 1e-5), "the post send follows: {o:?}");
    assert!(approx(o[2], 0.5, 1e-5), "the pre send does not: {o:?}");
    let main = settled(&mut channel_with_input(), &["out_l"]);
    assert!(
        approx(o[0], main[0], 0.0),
        "the main output does not: {o:?}"
    );
    // A follow of 0 (a muted bus) silences the post send and nothing else.
    dc(&mut h, "follow_in", 0.0);
    let o = settled(&mut h, &["send1_l", "send2_l"]);
    assert!(o[0].abs() < 1e-9 && approx(o[1], 0.5, 1e-5), "{o:?}");
}

fn channel_with_input() -> DeviceHarness {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    h
}

#[test]
fn an_uncabled_follow_in_changes_nothing_bit_for_bit() {
    let run = |cable: bool| {
        let mut h = channel_with_input();
        if cable {
            // Cabled at unity: the same value, bit for bit, as the plain strip.
            dc(&mut h, "follow_in", 1.0);
        }
        h.set_param("send1.on", 1.0);
        h.set_param("send1.level", -6.0);
        h.set_param("send3.on", 1.0);
        h.set_param("send3.pre", 1.0);
        h.render(30, &["out_l", "out_r", "send1_l", "send3_l"])
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect::<Vec<u32>>()
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn parallel_out_is_the_widened_signal_before_the_fader_and_the_pan() {
    let mut h = channel_with_input();
    h.set_connected("parallel_out_l", true);
    h.set_connected("parallel_out_r", true);
    h.set_param("fader", 0.2);
    h.set_param("pan", -1.0);
    let o = settled(&mut h, &["parallel_out_l", "parallel_out_r", "out_r"]);
    assert!(approx(o[0], 0.5, 1e-6) && approx(o[1], 0.5, 1e-6), "{o:?}");
    assert!(o[2].abs() < 1e-6, "the main output is panned away: {o:?}");
    // Width acts on it; the mute silences it.
    h.set_param("width", 0.0);
    dc(&mut h, "in_r", 0.0);
    let o = settled(&mut h, &["parallel_out_l", "parallel_out_r"]);
    assert!(
        approx(o[0], 0.25, 1e-6) && approx(o[1], 0.25, 1e-6),
        "{o:?}"
    );
    h.set_param("mute", 1.0);
    let o = settled(&mut h, &["parallel_out_l"]);
    assert!(o[0].abs() < 1e-9, "{o:?}");
}

#[test]
fn nothing_is_written_to_the_parallel_out_while_it_is_not_cabled() {
    let mut h = channel_with_input();
    let o = settled(&mut h, &["parallel_out_l", "parallel_out_r"]);
    assert!(o.iter().all(|v| *v == 0.0), "{o:?}");
}

fn tap_of(h: &DeviceHarness) -> Arc<ScopeTap> {
    h.device()
        .scope_tap()
        .expect("a prepared channel has a tap")
}

#[test]
fn the_tap_is_silent_until_the_switch_and_never_changes_the_output() {
    let play = |tap_live: bool| {
        let mut h = channel();
        h.set_param("eq.on", 1.0);
        h.set_param("eq.hmf.gain", 9.0);
        if tap_live {
            h.set_param("internal.tap_live", 1.0);
        }
        let mut out = Vec::new();
        for i in 0..40 {
            *h.input_mut("in_l") = [0.5 * (i as f32 * 0.1).sin(); BATCH];
            *h.input_mut("in_r") = [0.25; BATCH];
            out.extend(h.render(1, &OUT).into_iter().flatten());
        }
        (out, tap_of(&h).head())
    };
    let (off, off_head) = play(false);
    let (on, on_head) = play(true);
    assert_eq!(off_head, 0, "nothing is written while the switch is off");
    assert_eq!(on_head, 40 * BATCH as u64);
    let same = off.iter().zip(&on).all(|(a, b)| a.to_bits() == b.to_bits());
    assert!(same, "the tap does not touch the signal");
}

#[test]
fn the_tap_carries_the_signal_after_the_eq_and_before_the_fader() {
    let mut h = channel();
    h.set_param("internal.tap_live", 1.0);
    h.set_param("fader", 0.0);
    h.set_param("eq.on", 1.0);
    h.set_param("eq.lf.gain", 12.0);
    h.set_param("eq.lf.freq", 100.0);
    let tone = sine(60.0, 0.1);
    let n = 300;
    for i in 0..n {
        let x: Vec<f32> = (0..BATCH).map(|k| tone(i * BATCH + k)).collect();
        h.input_mut("in_l").copy_from_slice(&x);
        h.input_mut("in_r").copy_from_slice(&x);
        h.render(1, &OUT);
    }
    let (mut l, mut r) = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
    let (got, head) = tap_of(&h).read_latest(&mut l, &mut r);
    assert_eq!((got, head), (4096, (n * BATCH) as u64));
    let amp = tone_amplitude(&l, 60.0, n * BATCH - 4096);
    // The low shelf lifts 60 Hz by close to +12 dB, and the fader at 0 does not show.
    assert!(
        db(amp / 0.1) > 8.0 && db(amp / 0.1) < 13.0,
        "{}",
        db(amp / 0.1)
    );
    assert_eq!(l, r);
}

/// Plays 40 batches of a tone through a strip with the fader and pan moved off unity; the live
/// flag and the pre-fader switch are chosen by the caller. Returns the output and the tap.
fn play_with_flags(
    mut h: DeviceHarness,
    live: bool,
    pre: bool,
) -> (Vec<f32>, Vec<f32>, Vec<f32>, u64) {
    h.set_param("fader", 0.5);
    h.set_param("pan", -0.4);
    if pre {
        h.set_param("internal.tap_live", 1.0);
    }
    tap_of(&h).set_live(live);
    let tone = sine(440.0, 0.3);
    let (key_l, key_r) = (
        h.device().descriptor().sockets[0].key,
        h.device().descriptor().sockets[1].key,
    );
    let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
    for i in 0..40 {
        let x: Vec<f32> = (0..BATCH).map(|k| tone(i * BATCH + k)).collect();
        h.input_mut(key_l).copy_from_slice(&x);
        h.input_mut(key_r).copy_from_slice(&x);
        let o = h.render(1, &OUT);
        out_l.extend_from_slice(&o[0]);
        out_r.extend_from_slice(&o[1]);
    }
    let head = tap_of(&h).head();
    let (mut l, mut r) = (vec![0.0; 40 * BATCH], vec![0.0; 40 * BATCH]);
    tap_of(&h).read_latest(&mut l, &mut r);
    let _ = r;
    (out_l, out_r, l, head)
}

#[test]
fn the_output_tap_is_off_until_the_live_flag_and_never_changes_the_output() {
    let (off_l, off_r, _, off_head) = play_with_flags(channel(), false, false);
    let (on_l, on_r, _, on_head) = play_with_flags(channel(), true, false);
    assert_eq!(off_head, 0, "no data while the flag is off");
    assert_eq!(on_head, 40 * BATCH as u64);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&off_l),
        bits(&on_l),
        "the tap does not touch the signal"
    );
    assert_eq!(bits(&off_r), bits(&on_r));
}

#[test]
fn the_output_tap_carries_the_signal_after_the_fader_and_the_pan() {
    let (out_l, _, tap_l, head) = play_with_flags(channel(), true, false);
    assert_eq!(head, 40 * BATCH as u64);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&tap_l),
        bits(&out_l),
        "what the strip hands on, frame for frame"
    );
    // The pre-fader switch wins while both are on: that is the signal before the fader.
    let (_, _, pre_l, _) = play_with_flags(channel(), true, true);
    let peak = |v: &[f32]| {
        v[v.len() - 1024..]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    assert!(
        peak(&pre_l) > 1.5 * peak(&tap_l),
        "{} {}",
        peak(&pre_l),
        peak(&tap_l)
    );
}

#[test]
fn a_bus_has_the_same_output_tap() {
    let (out_l, _, tap_l, head) =
        play_with_flags(harness(Box::new(crate::bus::new())), true, false);
    assert_eq!(head, 40 * BATCH as u64);
    assert_eq!(
        tap_l.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        out_l.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn the_strip_tap_shows_the_same_spectrum_as_a_spectrum_device_after_the_strip() {
    use atomik_dsp::spectrum::{ChannelMode, Settings, Spectrum as Display, WindowKind};
    let mut strip = channel();
    strip.set_param("fader", 0.6);
    tap_of(&strip).set_live(true);
    let mut meter = harness(Box::new(crate::spectrum::Spectrum::new()));
    let tone = |n: usize| 0.2 * (n as f32 * 0.31).sin() + 0.1 * (n as f32 * 1.7).sin();
    for i in 0..200 {
        let x: Vec<f32> = (0..BATCH).map(|k| tone(i * BATCH + k)).collect();
        strip.input_mut("in_l").copy_from_slice(&x);
        strip.input_mut("in_r").copy_from_slice(&x);
        let o = strip.render(1, &OUT);
        meter.input_mut("in_l").copy_from_slice(&o[0]);
        meter.input_mut("in_r").copy_from_slice(&o[1]);
        meter.process();
    }
    let analyse = |tap: Arc<ScopeTap>| {
        let (mut l, mut r) = (vec![0.0; 4096], vec![0.0; 4096]);
        assert_eq!(tap.read_latest(&mut l, &mut r).0, 4096);
        let mut display = Display::new();
        let s = Settings {
            size: 4096,
            window: WindowKind::Hann,
            channel: ChannelMode::Sum,
            averaging_ms: 0.0,
            ..Settings::default()
        };
        display.update(&l, &r, 0, 48_000.0, &s, 200);
        (
            display.peak_hz(),
            display.peak_db(),
            display.trace().to_vec(),
        )
    };
    let a = analyse(tap_of(&strip));
    let b = analyse(meter.device().scope_tap().unwrap());
    assert_eq!(a.0.to_bits(), b.0.to_bits());
    assert!((a.1 - b.1).abs() < 1e-3, "{} {}", a.1, b.1);
    assert!(a.2.iter().zip(&b.2).all(|(x, y)| (x - y).abs() < 1e-3));
}

#[test]
fn a_strip_that_was_never_prepared_has_no_tap() {
    assert!(Channel::new().scope_tap().is_none());
}

#[test]
fn the_drawn_strip_curve_matches_a_measured_sweep_within_a_tenth_of_a_db() {
    use atomik_dsp::strip_eq::{StripBand, StripCurve};
    let mut h = channel();
    let settings: [(&str, f32); 14] = [
        ("hpf.on", 1.0),
        ("hpf.freq", 120.0),
        ("lpf.on", 1.0),
        ("lpf.freq", 9_000.0),
        ("eq.on", 1.0),
        ("eq.hf.gain", 5.0),
        ("eq.hf.freq", 6_000.0),
        ("eq.hmf.gain", -7.0),
        ("eq.hmf.freq", 2_000.0),
        ("eq.hmf.q", 2.0),
        ("eq.lmf.gain", 6.0),
        ("eq.lmf.freq", 400.0),
        ("eq.lf.gain", 4.0),
        ("eq.lf.bell", 1.0),
    ];
    for (k, v) in settings {
        h.set_param(k, v);
    }
    h.set_param("eq.lf.freq", 90.0);
    let curve = StripCurve {
        sample_rate: SR,
        eq_on: true,
        e_mode: false,
        hpf: Some(120.0),
        lpf: Some(9_000.0),
        hf: StripBand {
            gain_db: 5.0,
            freq: 6_000.0,
            q: 0.0,
            bell: false,
        },
        hmf: StripBand {
            gain_db: -7.0,
            freq: 2_000.0,
            q: 2.0,
            bell: false,
        },
        lmf: StripBand {
            gain_db: 6.0,
            freq: 400.0,
            q: 1.0,
            bell: false,
        },
        lf: StripBand {
            gain_db: 4.0,
            freq: 90.0,
            q: 0.0,
            bell: true,
        },
    };
    // The reference: the same strip with the filters and the EQ off, for the fixed pan law.
    let mut flat = channel();
    let mut worst = 0.0f64;
    for hz in [
        60.0, 100.0, 150.0, 250.0, 400.0, 800.0, 1_500.0, 2_000.0, 3_000.0, 6_000.0, 8_000.0,
        12_000.0,
    ] {
        let got = sine_gain_db(&mut h, &["in_l", "in_r"], "out_l", hz, 0.1, 0.3);
        let base = sine_gain_db(&mut flat, &["in_l", "in_r"], "out_l", hz, 0.1, 0.3);
        let err = (got - base - curve.response_db(hz)).abs();
        worst = worst.max(err);
    }
    assert!(worst < 0.1, "worst error {worst} dB");
}

// ---- strip upgrades follow-ups: look-ahead, per-side polarity, section bypass, detector

/// A strip with a hard peak compressor with the fastest attack, left pan.
fn lookahead_strip(ms: f32) -> DeviceHarness {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.peak", 1.0);
    h.set_param("comp.fast", 1.0);
    h.set_param("comp.threshold", -30.0);
    h.set_param("comp.ratio", 20.0);
    h.set_param("comp.release", 200.0);
    h.set_param("comp.lookahead", ms);
    h
}

/// A quiet tone that jumps to a loud one at `ONSET`.
const ONSET: usize = 2_000;
fn burst(n: usize) -> f32 {
    sine(1_000.0, if n < ONSET { 0.01 } else { 0.9 })(n)
}

#[test]
fn the_lookahead_is_reported_and_delays_the_strip_by_exactly_that_much() {
    let mut h = channel();
    h.set_param("pan", -1.0);
    assert_eq!(h.device().latency_frames(), 0, "off by default");
    for ms in [1.0f32, 5.0, 10.0] {
        let frames = (ms * 0.001 * 48_000.0).round() as u32;
        // With the compressor off, on and bypassed the strip keeps the same alignment.
        for (comp, bypass) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)] {
            h.set_param("comp.on", comp);
            h.set_param("dyn.bypass", bypass);
            h.set_param("comp.lookahead", ms);
            assert_eq!(h.device().latency_frames(), frames, "{ms} ms reported");
            assert_eq!(
                impulse_position(&mut h),
                frames as usize,
                "{ms} ms (comp {comp}, bypass {bypass}) really delayed"
            );
        }
    }
    h.set_param("comp.lookahead", 0.0);
    assert_eq!(h.device().latency_frames(), 0);
    h.set_param("latency.adjust", -100.0);
    h.set_param("comp.lookahead", 2.0);
    assert_eq!(
        h.device().latency_frames(),
        100 + 96,
        "adds to the phantom latency"
    );
}

#[test]
fn a_lookahead_of_zero_changes_nothing_bit_for_bit() {
    let plain = {
        let mut h = lookahead_strip(0.0);
        run_signal(&mut h, &["in_l", "in_r"], burst, 80, &["out_l"]).remove(0)
    };
    let mut off = channel();
    off.set_param("pan", -1.0);
    off.set_param("comp.on", 1.0);
    off.set_param("comp.peak", 1.0);
    off.set_param("comp.fast", 1.0);
    off.set_param("comp.threshold", -30.0);
    off.set_param("comp.ratio", 20.0);
    off.set_param("comp.release", 200.0);
    let base = run_signal(&mut off, &["in_l", "in_r"], burst, 80, &["out_l"]).remove(0);
    assert_eq!(plain, base);
}

#[test]
fn the_lookahead_has_the_compressor_down_before_the_loud_part_arrives() {
    let peak_after_onset = |ms: f32| {
        let mut h = lookahead_strip(ms);
        let delay = h.device().latency_frames() as usize;
        let y = run_signal(&mut h, &["in_l", "in_r"], burst, 80, &["out_l"]).remove(0);
        let from = ONSET + delay;
        y[from..from + 40]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let none = peak_after_onset(0.0);
    let ahead = peak_after_onset(5.0);
    assert!(
        none > 0.5,
        "without it the first cycles get through: {none}"
    );
    assert!(
        ahead < 0.5 * none,
        "with it the onset is already reduced: {ahead} vs {none}"
    );
}

#[test]
fn a_reset_clears_the_lookahead_line() {
    let mut h = lookahead_strip(10.0);
    run_signal(&mut h, &["in_l", "in_r"], burst, 40, &["out_l"]);
    h.device_mut().reset();
    let y = run_signal(&mut h, &["in_l", "in_r"], |_| 0.0, 8, &["out_l"]).remove(0);
    assert!(y.iter().all(|v| *v == 0.0), "no history survives a reset");
}

#[test]
fn each_side_has_its_own_polarity() {
    let sides = |phase: f32, phase_r: f32| {
        let mut h = channel();
        h.set_param("input.phase", phase);
        h.set_param("input.phase_r", phase_r);
        dc(&mut h, "in_l", 0.5);
        dc(&mut h, "in_r", 0.5);
        let o = settled(&mut h, &["out_l", "out_r"]);
        (o[0] > 0.0, o[1] > 0.0)
    };
    assert_eq!(sides(0.0, 0.0), (true, true));
    assert_eq!(sides(1.0, 0.0), (false, false), "the old switch flips both");
    assert_eq!(sides(0.0, 1.0), (true, false), "the right side alone");
    assert_eq!(sides(1.0, 1.0), (false, true), "the left side alone");
}

#[test]
fn the_right_polarity_ramps_without_a_click_and_leaves_the_left_alone() {
    let mut h = channel();
    dc(&mut h, "in_l", 0.5);
    dc(&mut h, "in_r", 0.5);
    let before = settled(&mut h, &["out_l", "out_r"]);
    h.set_param("input.phase_r", 1.0);
    let o = h.render(8, &["out_l", "out_r"]);
    assert!(
        o[0].iter().all(|v| v.to_bits() == before[0].to_bits()),
        "left is untouched"
    );
    assert!(
        max_step(&o[1]) < 2.0 * before[1] / 100.0,
        "a flip crosses zero over about 5 ms: {}",
        max_step(&o[1])
    );
    assert!(approx(*o[1].last().unwrap(), -before[1], 1e-6));
}

#[test]
fn the_parallel_compressor_detector_can_be_peak_or_rms() {
    let out = |peak: Option<f32>| {
        program_out(100, |h| {
            h.set_param("pcomp.on", 1.0);
            h.set_param("pcomp.mix", 1.0);
            for (k, v) in pcomp_params() {
                h.set_param(k, v);
            }
            if let Some(p) = peak {
                h.set_param("pcomp.peak", p);
            }
        })
    };
    let (rms, peak) = (out(Some(0.0)), out(Some(1.0)));
    assert_ne!(rms, peak, "the detector changes the gain");
    assert!(peak.iter().all(|v| v.is_finite()));
    assert_eq!(rms, out(None), "RMS is the default, as before the switch");
}

#[test]
fn bypassing_the_dynamics_takes_the_section_out_without_clicking() {
    let level = |bypass: f32| {
        let mut h = channel();
        h.set_param("pan", -1.0);
        h.set_param("comp.on", 1.0);
        h.set_param("comp.threshold", -40.0);
        h.set_param("comp.ratio", 20.0);
        h.set_param("dyn.bypass", bypass);
        run_signal(
            &mut h,
            &["in_l", "in_r"],
            sine(1_000.0, 0.5),
            120,
            &["out_l"],
        )
        .remove(0)
    };
    let (on, bypassed) = (level(0.0), level(1.0));
    let tail = |v: &[f32]| {
        v[v.len() - 480..]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    assert!(tail(&on) < 0.2, "the compressor works: {}", tail(&on));
    assert!(approx(tail(&bypassed), 0.5, 0.01), "bypassed it is unity");
    // The gate goes the same way.
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("gate.on", 1.0);
    h.set_param("gate.threshold", -10.0);
    h.set_param("gate.range", 80.0);
    dc(&mut h, "in_l", 0.1);
    h.render(200, &["out_l"]);
    let closed = settled(&mut h, &["out_l"])[0];
    assert!(closed < 0.01, "the gate is closed: {closed}");
    h.set_param("dyn.bypass", 1.0);
    let o = settled(&mut h, &["out_l"])[0];
    assert!(
        approx(o, 0.1, 1e-3),
        "the bypassed gate lets it through: {o}"
    );
}

#[test]
fn bypassing_the_eq_takes_it_out_and_the_settings_come_back() {
    let level = |on: f32, bypass: f32| {
        let mut h = channel();
        h.set_param("pan", -1.0);
        h.set_param("eq.on", on);
        h.set_param("eq.lf.gain", 12.0);
        h.set_param("eq.lf.freq", 100.0);
        h.set_param("eq.bypass", bypass);
        dc(&mut h, "in_l", 0.1);
        settled(&mut h, &["out_l"])[0]
    };
    assert!(level(1.0, 0.0) > 0.2, "the low shelf lifts DC");
    assert!(approx(level(1.0, 1.0), 0.1, 1e-4), "bypassed it is flat");
    assert!(approx(level(0.0, 0.0), 0.1, 1e-4));
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("eq.on", 1.0);
    h.set_param("eq.lf.gain", 12.0);
    dc(&mut h, "in_l", 0.1);
    let eq = settled(&mut h, &["out_l"])[0];
    h.set_param("eq.bypass", 1.0);
    settled(&mut h, &["out_l"]);
    h.set_param("eq.bypass", 0.0);
    assert!(approx(settled(&mut h, &["out_l"])[0], eq, 1e-4));
}

// ------------------------------------------------------------------ key and tap of the parallel compressor

/// `out_l` of a strip with the parallel compressor fully wet, a constant `key_in` and `setup`.
fn pcomp_keyed(key: Option<f32>, setup: impl FnOnce(&mut DeviceHarness)) -> Vec<f32> {
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("pcomp.on", 1.0);
    h.set_param("pcomp.mix", 1.0);
    for (k, v) in pcomp_params() {
        h.set_param(k, v);
    }
    if let Some(v) = key {
        dc(&mut h, "key_in", v);
    }
    setup(&mut h);
    run_signal(&mut h, &["in_l", "in_r"], program, 200, &["out_l"]).remove(0)
}

#[test]
fn the_parallel_compressor_listens_to_the_key_only_when_enabled_and_connected() {
    let own = pcomp_keyed(None, |_| {});
    // The key switch alone, or a connected key alone, change nothing.
    let switch_only = pcomp_keyed(None, |h| h.set_param("pcomp.key", 1.0));
    let key_only = pcomp_keyed(Some(0.9), |_| {});
    for other in [&switch_only, &key_only] {
        assert!(
            own.iter()
                .zip(other.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
    // A loud key squashes the quiet passages too, which the signal itself does not.
    let keyed = pcomp_keyed(Some(0.9), |h| h.set_param("pcomp.key", 1.0));
    let quiet = |v: &[f32]| {
        v[4_800 + 2_400..2 * 4_800]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    assert!(
        quiet(&keyed) < quiet(&own) * 0.7,
        "{} vs {}",
        quiet(&keyed),
        quiet(&own)
    );
    // The dynamics section is not keyed by it.
    let mut h = channel();
    h.set_param("pan", -1.0);
    h.set_param("pcomp.key", 1.0);
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", -30.0);
    dc(&mut h, "key_in", 0.9);
    let g = sine_gain_db(&mut h, &["in_l"], "out_l", 1_000.0, 0.01, 0.3);
    assert!(g.abs() < 0.05, "{g}");
}

#[test]
fn the_parallel_key_matches_a_standalone_compressor_fed_by_the_key() {
    let got = pcomp_keyed(Some(0.5), |h| h.set_param("pcomp.key", 1.0));
    let mut c = Compressor::new(SR as f32);
    c.set_threshold_db(-30.0);
    c.set_ratio(8.0);
    c.set_attack_ms(10.0);
    c.set_release_ms(150.0);
    let makeup = db_to_gain(6.0);
    for (n, y) in got.iter().enumerate() {
        let x = program(n);
        let g = c.process(0.5, 0.5) * makeup;
        assert!(approx(*y, x * g, 1e-5), "{n}: {y} vs {}", x * g);
    }
}

#[test]
fn the_pre_insert_tap_compresses_the_signal_the_insert_chain_receives() {
    // No insert cabled: the tap and the return are the same signal, so the tap is invisible.
    let post = pcomp_keyed(None, |_| {});
    let pre = pcomp_keyed(None, |h| h.set_param("pcomp.pre", 1.0));
    assert!(
        post.iter()
            .zip(&pre)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    // A return that differs from the send (a constant) shows which signal is compressed: with
    // the tap before the insert the wet path is the program, with the default tap it is the
    // return.
    let wet = |pre: bool| {
        let mut h = channel();
        h.set_param("pan", -1.0);
        h.set_param("pcomp.on", 1.0);
        h.set_param("pcomp.mix", 1.0);
        for (k, v) in pcomp_params() {
            h.set_param(k, v);
        }
        h.set_param("pcomp.pre", if pre { 1.0 } else { 0.0 });
        dc(&mut h, "insert_return_l", 0.02);
        dc(&mut h, "insert_return_r", 0.02);
        run_signal(&mut h, &["in_l", "in_r"], program, 200, &["out_l"]).remove(0)
    };
    let (after, before) = (wet(false), wet(true));
    let mut c = Compressor::new(SR as f32);
    c.set_threshold_db(-30.0);
    c.set_ratio(8.0);
    c.set_attack_ms(10.0);
    c.set_release_ms(150.0);
    let makeup = db_to_gain(6.0);
    for (n, y) in before.iter().enumerate() {
        let x = program(n);
        let g = c.process(x, x) * makeup;
        assert!(approx(*y, x * g, 1e-5), "{n}: {y} vs {}", x * g);
    }
    // After the inserts the wet path is the (quiet, constant) return, not the program.
    assert!(after.iter().skip(40).all(|v| v.abs() < 0.1));
    assert!(before.iter().skip(40).any(|v| v.abs() > 0.3));
}

// ---- console editing follow-ups: auto make-up and the E mode of the EQ

/// A tone at -6 dB through a compressor with the auto make-up as asked: its gain in dB and the
/// gain-reduction CV of the last batch.
fn made_up(auto: bool, threshold: f32, ratio: f32) -> (f64, Vec<f32>) {
    let mut h = channel();
    h.set_param("comp.on", 1.0);
    h.set_param("comp.threshold", threshold);
    h.set_param("comp.ratio", ratio);
    h.set_param("comp.auto_makeup", if auto { 1.0 } else { 0.0 });
    let gain = sine_gain_db(&mut h, &["in_l", "in_r"], "out_l", 1_000.0, 0.5, 0.6);
    (gain, h.output("gr_cv_out").to_vec())
}

#[test]
fn auto_makeup_adds_the_reduction_a_full_scale_signal_gets_and_leaves_the_meter_alone() {
    let (plain, gr_plain) = made_up(false, -20.0, 4.0);
    let (auto, gr_auto) = made_up(true, -20.0, 4.0);
    // 20 dB below full scale at 4:1 is 15 dB of reduction for a full-scale signal.
    assert!((auto - plain - 15.0).abs() < 0.2, "{auto} vs {plain}");
    assert_eq!(gr_plain, gr_auto, "the make-up is not gain reduction");
    assert!(
        gr_auto.iter().any(|g| *g > 0.1),
        "the meter reads the reduction"
    );
    // A ratio of 1 reduces nothing, so there is nothing to make up.
    let (flat, _) = made_up(false, -20.0, 1.0);
    let (flat_auto, _) = made_up(true, -20.0, 1.0);
    assert!((flat_auto - flat).abs() < 1e-3, "{flat_auto} vs {flat}");
    // A threshold at full scale reduces nothing either.
    let (top, _) = made_up(false, 0.0, 4.0);
    let (top_auto, _) = made_up(true, 0.0, 4.0);
    assert!((top_auto - top).abs() < 1e-3);
}

#[test]
fn auto_makeup_is_capped_follows_the_knobs_and_waits_for_the_compressor() {
    let mut c = Channel::new();
    let target = |c: &Channel| c.comp_makeup.target();
    assert_eq!(target(&c), 1.0, "off by default");
    c.set_param(ParamId(P_COMP_AUTO_MAKEUP as u32), 1.0);
    // The default threshold (-20 dB) and ratio (4:1).
    assert!((target(&c) - db_to_gain(15.0)).abs() < 1e-5);
    c.set_param(ParamId(P_COMP_THRESHOLD as u32), -60.0);
    c.set_param(ParamId(P_COMP_RATIO as u32), 20.0);
    assert!(
        (target(&c) - db_to_gain(24.0)).abs() < 1e-5,
        "capped at 24 dB"
    );
    c.set_param(ParamId(P_COMP_THRESHOLD as u32), -10.0);
    c.set_param(ParamId(P_COMP_RATIO as u32), 2.0);
    assert!(
        (target(&c) - db_to_gain(5.0)).abs() < 1e-5,
        "follows the knobs"
    );
    c.set_param(ParamId(P_COMP_AUTO_MAKEUP as u32), 0.0);
    assert_eq!(target(&c), 1.0);
    // With the compressor off the make-up changes nothing, whatever the switch says.
    let quiet = |auto: f32| {
        let mut h = channel();
        h.set_param("comp.auto_makeup", auto);
        run_signal(&mut h, &["in_l", "in_r"], sine(440.0, 0.4), 40, &["out_l"])
    };
    assert_eq!(quiet(0.0), quiet(1.0));
}

#[test]
fn a_strip_with_the_new_switches_off_is_bit_identical() {
    let run = |set: &[(&str, f32)]| {
        let mut h = channel();
        for (k, v) in [
            ("comp.on", 1.0),
            ("comp.threshold", -24.0),
            ("eq.on", 1.0),
            ("eq.hmf.gain", 7.0),
            ("eq.hmf.q", 2.0),
            ("eq.lmf.gain", -5.0),
        ] {
            h.set_param(k, v);
        }
        for (k, v) in set {
            h.set_param(k, *v);
        }
        run_signal(
            &mut h,
            &["in_l", "in_r"],
            sine(700.0, 0.6),
            60,
            &["out_l", "out_r"],
        )
    };
    let base = run(&[]);
    assert_eq!(base, run(&[("comp.auto_makeup", 0.0), ("eq.e_mode", 0.0)]));
    assert_ne!(base, run(&[("comp.auto_makeup", 1.0)]));
    assert_ne!(base, run(&[("eq.e_mode", 1.0)]));
}

#[test]
fn e_mode_makes_the_bells_follow_their_gain_and_matches_the_drawn_curve() {
    use atomik_dsp::strip_eq::{StripBand, StripCurve};
    let band = |gain_db: f64, freq: f64, q: f64, bell: bool| StripBand {
        gain_db,
        freq,
        q,
        bell,
    };
    let settings: [(&str, f32); 10] = [
        ("eq.on", 1.0),
        ("eq.hf.bell", 1.0),
        ("eq.hf.gain", 6.0),
        ("eq.hf.freq", 8_000.0),
        ("eq.hmf.gain", 12.0),
        ("eq.hmf.freq", 2_500.0),
        ("eq.hmf.q", 1.0),
        ("eq.lmf.gain", -9.0),
        ("eq.lmf.freq", 300.0),
        ("eq.lmf.q", 2.0),
    ];
    let curve = |e_mode: bool| StripCurve {
        sample_rate: SR,
        eq_on: true,
        e_mode,
        hpf: None,
        lpf: None,
        hf: band(6.0, 8_000.0, 0.0, true),
        hmf: band(12.0, 2_500.0, 1.0, false),
        lmf: band(-9.0, 300.0, 2.0, false),
        lf: band(0.0, 100.0, 0.0, false),
    };
    let freqs = [300.0, 1_000.0, 2_000.0, 2_500.0, 4_000.0, 8_000.0, 12_000.0];
    let measure = |e_mode: bool| {
        let mut h = channel();
        for (k, v) in settings {
            h.set_param(k, v);
        }
        h.set_param("eq.e_mode", if e_mode { 1.0 } else { 0.0 });
        let mut flat = channel();
        freqs.map(|hz| {
            sine_gain_db(&mut h, &["in_l", "in_r"], "out_l", hz, 0.1, 0.3)
                - sine_gain_db(&mut flat, &["in_l", "in_r"], "out_l", hz, 0.1, 0.3)
        })
    };
    let (off, on) = (measure(false), measure(true));
    for (e, got) in [(false, off), (true, on)] {
        for (i, hz) in freqs.into_iter().enumerate() {
            let want = curve(e).response_db(hz);
            assert!(
                (got[i] - want).abs() < 0.1,
                "e {e} {hz} Hz: {} vs {want}",
                got[i]
            );
        }
    }
    // Off the centre the focused bell lets less through.
    assert!(on[4] < off[4] - 0.5, "{} vs {}", on[4], off[4]);
    // Alone, a bell has its full gain at its centre either way.
    let alone = |e: f32| {
        let mut h = channel();
        for (k, v) in [
            ("eq.on", 1.0),
            ("eq.hmf.gain", 12.0),
            ("eq.hmf.freq", 2_500.0),
        ] {
            h.set_param(k, v);
        }
        h.set_param("eq.e_mode", e);
        let mut flat = channel();
        sine_gain_db(&mut h, &["in_l", "in_r"], "out_l", 2_500.0, 0.1, 0.3)
            - sine_gain_db(&mut flat, &["in_l", "in_r"], "out_l", 2_500.0, 0.1, 0.3)
    };
    assert!((alone(0.0) - 12.0).abs() < 0.1 && (alone(1.0) - 12.0).abs() < 0.1);
    // Flat bands are not changed: a strip with no gain sounds the same in E mode.
    let flat = |e: f32| {
        let mut h = channel();
        h.set_param("eq.on", 1.0);
        h.set_param("eq.e_mode", e);
        run_signal(&mut h, &["in_l", "in_r"], sine(900.0, 0.5), 40, &["out_l"])
    };
    assert_eq!(flat(0.0), flat(1.0));
}

/// Plays 60 batches of two tones through a strip with an EQ boost and the fader at `fader`,
/// with the tap live. `how` chooses the point: the Spectrum EQ switch, the tap's pre flag, or
/// the default output point. Returns the strip's output and the tap's frames.
fn play_point(fader: f32, how: &str) -> (Vec<f32>, Vec<f32>) {
    let mut h = channel();
    h.set_param("fader", fader);
    h.set_param("pan", 0.3);
    h.set_param("eq.on", 1.0);
    h.set_param("eq.hmf.gain", 6.0);
    let tap = tap_of(&h);
    tap.set_live(true);
    match how {
        "switch" => h.set_param("internal.tap_live", 1.0),
        "flag" => tap.set_pre(true),
        _ => {}
    }
    let (mut out, mut frames) = (Vec::new(), vec![0.0f32; 60 * BATCH]);
    for i in 0..60 {
        let x: Vec<f32> = (0..BATCH)
            .map(|k| {
                let n = (i * BATCH + k) as f32;
                0.2 * (n * 0.31).sin() + 0.1 * (n * 1.7).sin()
            })
            .collect();
        h.input_mut("in_l").copy_from_slice(&x);
        h.input_mut("in_r").copy_from_slice(&x);
        out.extend_from_slice(&h.render(1, &OUT)[0]);
    }
    let mut r = vec![0.0f32; 60 * BATCH];
    tap.read_latest(&mut frames, &mut r);
    (out, frames)
}

#[test]
fn the_pre_flag_taps_the_signal_before_the_fader_exactly_like_the_switch() {
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let (out_switch, by_switch) = play_point(0.5, "switch");
    let (out_flag, by_flag) = play_point(0.5, "flag");
    assert_eq!(bits(&by_flag), bits(&by_switch), "the same frames");
    assert_eq!(bits(&out_flag), bits(&out_switch), "and the same output");
    // The fader does not show before it: another fader gives the same tap, another output.
    let (out_loud, loud_tap) = play_point(1.0, "flag");
    assert_eq!(bits(&loud_tap), bits(&by_flag));
    assert_ne!(bits(&out_loud), bits(&out_flag));
    // Post is the default and shows the fader.
    let (out_post, post_tap) = play_point(0.5, "post");
    assert_eq!(bits(&post_tap), bits(&out_post));
    assert_ne!(bits(&post_tap), bits(&by_flag));
    assert_eq!(
        bits(&out_post),
        bits(&out_flag),
        "the point never alters the output"
    );
}

#[test]
fn the_pre_flag_alone_writes_nothing_while_the_tap_is_not_live() {
    let mut h = channel();
    let tap = tap_of(&h);
    tap.set_pre(true);
    for _ in 0..8 {
        *h.input_mut("in_l") = [0.4; BATCH];
        h.render(1, &OUT);
    }
    assert_eq!(tap.head(), 0, "a tap nobody shows stays silent");
}

#[test]
fn a_large_ring_on_a_strip_keeps_the_bins_of_a_65536_window_and_the_output_bit_exact() {
    use atomik_dsp::spectrum::{ChannelMode, Settings, Spectrum as Display, WindowKind};
    let run = |large: bool| {
        let mut h = channel();
        let tap = tap_of(&h);
        tap.set_live(true);
        if large {
            tap.reserve_large();
        }
        let mut out = Vec::new();
        let tone = |n: usize| 0.2 * (n as f32 * 0.31).sin() + 0.1 * (n as f32 * 1.7).sin();
        for i in 0..1200 {
            let x: Vec<f32> = (0..BATCH).map(|k| tone(i * BATCH + k)).collect();
            h.input_mut("in_l").copy_from_slice(&x);
            h.input_mut("in_r").copy_from_slice(&x);
            out.extend_from_slice(&h.render(1, &OUT)[0]);
        }
        (out, tap)
    };
    let (small_out, _) = run(false);
    let (big_out, big) = run(true);
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&small_out),
        bits(&big_out),
        "a bounce never sees the ring"
    );
    let (mut l, mut r) = (vec![0.0; 65_536], vec![0.0; 65_536]);
    assert_eq!(big.read_latest(&mut l, &mut r).0, 65_536);
    // The bins of the large window equal a window of the same frames from the device ring.
    let bins = |l: &[f32], r: &[f32]| {
        let mut d = Display::new();
        let s = Settings {
            size: 65_536,
            window: WindowKind::Hann,
            channel: ChannelMode::Sum,
            averaging_ms: 0.0,
            ..Settings::default()
        };
        d.update(l, r, 0, 48_000.0, &s, 200);
        d.trace().to_vec()
    };
    let n = 65_536;
    let from_big = bins(&l[..n], &r[..n]);
    // The newest 16384 frames of the large window are the frames a small tap holds.
    let mut small = (vec![0.0; 16_384], vec![0.0; 16_384]);
    let (_, small_tap) = run(false);
    small_tap.read_latest(&mut small.0, &mut small.1);
    assert_eq!(bits(&l[n - 16_384..n]), bits(&small.0));
    assert!(
        from_big.iter().any(|v| *v > -100.0),
        "a spectrum was measured"
    );
}
