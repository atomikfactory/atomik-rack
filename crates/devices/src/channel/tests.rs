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
