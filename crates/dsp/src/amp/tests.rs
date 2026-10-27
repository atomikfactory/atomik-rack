use super::*;
use crate::oversample::{OversampleFactor, Oversampler};
use crate::test_util::{db, noise, rms, sine, spectrum, tone_amplitude};

const SR: f32 = 48_000.0;

/// Frequency of bin `k` of an 8192-sample analysis at `SR` (a whole number of cycles).
fn bin_hz(k: u32) -> f64 {
    f64::from(k) * 48_000.0 / 8_192.0
}

/// The nonlinear part of the amp (preamp then power amp) at `factor` times `SR`, in blocks of 64.
fn drive_chain(
    input: &[f32],
    factor: OversampleFactor,
    gain: f32,
    master_db: f32,
    sag: f32,
) -> Vec<f32> {
    let hi = SR * factor.ratio() as f32;
    let mut pre = Preamp::new(hi);
    pre.set_gain(gain);
    let mut pow = PowerAmp::new(hi);
    pow.set_drive_db(master_db);
    pow.set_sag(sag);
    pre.reset();
    pow.reset();
    let mut os = Oversampler::new(64);
    os.set_factor(factor);
    let mut out = vec![0.0; input.len()];
    for (i, o) in input.chunks(64).zip(out.chunks_mut(64)) {
        let up = os.upsample(i);
        for v in up.iter_mut() {
            *v = pow.process(pre.process(*v));
        }
        os.downsample(o);
    }
    out
}

/// Power outside the harmonics `1..=4` of bin `bin` (a Hann-windowed spectrum of 8192 samples),
/// relative to the fundamental, in dB: what folded back from above the Nyquist frequency.
fn alias_db(y: &[f32], bin: usize) -> f64 {
    let m = spectrum(y);
    let near = |k: usize| (1..=4).any(|h| k.abs_diff(h * bin) <= 4);
    let fund: f64 = (bin - 4..=bin + 4).map(|k| m[k] * m[k]).sum();
    let alias: f64 = (8..m.len() - 1)
        .filter(|k| !near(*k))
        .map(|k| m[k] * m[k])
        .sum();
    10.0 * (alias / fund).max(1e-30).log10()
}

#[test]
fn block_biquad_glides_to_the_target_and_passes_through_when_identity() {
    let mut f = BlockBiquad::new();
    for i in 0..100 {
        let x = (i as f32 * 0.37).sin();
        assert_eq!(f.process(x).to_bits(), x.to_bits());
    }
    let c = crate::BiquadCoeffs::design(crate::BiquadShape::Lowpass, 1_000.0, 0.7, 0.0, 48_000.0);
    f.glide_to(c, 50);
    assert!(!f.is_settled());
    for _ in 0..50 {
        f.process(0.0);
    }
    assert!(f.is_settled());
    assert_eq!(f.target(), &c);
    // The same design again is not a new glide.
    f.glide_to(c, 50);
    assert!(f.is_settled());
}

#[test]
fn stage_gains_rise_with_the_knob_and_are_clamped() {
    let mut prev = stage_gains_db(0.0);
    assert!(prev.iter().all(|g| g.is_finite()));
    for i in 1..=20 {
        let g = stage_gains_db(i as f32 / 20.0);
        for s in 0..3 {
            assert!(g[s] >= prev[s], "stage {s} at {i}");
        }
        prev = g;
    }
    assert_eq!(stage_gains_db(7.0), stage_gains_db(1.0));
    assert_eq!(stage_gains_db(-3.0), stage_gains_db(0.0));
    assert_eq!(stage_gains_db(f32::NAN), stage_gains_db(0.0));
    // Very high gain is very high: the sum is above 60 dB.
    assert!(stage_gains_db(1.0).iter().sum::<f32>() > 60.0);
}

#[test]
fn the_preamp_is_nearly_linear_at_low_gain_and_low_level() {
    let x = sine(bin_hz(75), f64::from(SR), 16_384, 0.02);
    let y = drive_chain(&x, OversampleFactor::X4, 0.0, 0.0, 0.0);
    let y2 = drive_chain(
        &x.iter().map(|v| v * 2.0).collect::<Vec<_>>(),
        OversampleFactor::X4,
        0.0,
        0.0,
        0.0,
    );
    let a = tone_amplitude(&y[8_192..], bin_hz(75), f64::from(SR));
    let a2 = tone_amplitude(&y2[8_192..], bin_hz(75), f64::from(SR));
    // Doubling the input doubles the output within 0.2 dB.
    assert!((db(a2 / a) - 6.02).abs() < 0.2, "{}", db(a2 / a));
    // Harmonic distortion below -50 dB.
    let h3 = tone_amplitude(&y[8_192..], bin_hz(225), f64::from(SR));
    assert!(db(h3 / a) < -50.0, "{}", db(h3 / a));
}

#[test]
fn a_hot_preamp_saturates_and_stays_bounded() {
    let hot = sine(220.0, f64::from(SR), 16_384, 4.0);
    let y = drive_chain(&hot, OversampleFactor::X4, 1.0, 12.0, 1.0);
    assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1.6), "bounded");
    // Compression: the output of a 10x louder input is not 10x louder.
    let soft = sine(220.0, f64::from(SR), 16_384, 0.4);
    let y2 = drive_chain(&soft, OversampleFactor::X4, 1.0, 12.0, 1.0);
    let r = rms(&y[8_192..]) / rms(&y2[8_192..]);
    assert!(r < 1.6, "10x the input gives {r}x the output");
}

#[test]
fn gain_adds_harmonics() {
    let x = sine(bin_hz(56), f64::from(SR), 16_384, 0.1);
    let thd = |gain: f32| {
        let y = drive_chain(&x, OversampleFactor::X4, gain, 0.0, 0.0);
        let y = &y[8_192..];
        let f = tone_amplitude(y, bin_hz(56), f64::from(SR));
        let h: f64 = (2..=6)
            .map(|k| tone_amplitude(y, bin_hz(56 * k), f64::from(SR)).powi(2))
            .sum();
        h.sqrt() / f
    };
    assert!(thd(0.2) < thd(0.5) && thd(0.5) < thd(0.9), "monotonic");
    assert!(thd(0.9) > 0.3, "very distorted at high gain: {}", thd(0.9));
    assert!(thd(0.0) < 0.05, "clean at 0: {}", thd(0.0));
}

#[test]
fn the_preamp_makes_even_harmonics_and_the_power_amp_does_not() {
    let x = sine(bin_hz(43), f64::from(SR), 16_384, 0.3);
    let y = drive_chain(&x, OversampleFactor::X4, 0.6, -6.0, 0.0);
    let (h2, h3) = (
        tone_amplitude(&y[8_192..], bin_hz(86), f64::from(SR)),
        tone_amplitude(&y[8_192..], bin_hz(129), f64::from(SR)),
    );
    assert!(
        h2 > 0.02 * h3 && h2 > 0.003,
        "even harmonic present: {h2} {h3}"
    );
    // The symmetric power amp alone: a pure odd curve, the second harmonic stays at the
    // numerical floor.
    let mut p = PowerAmp::new(SR);
    p.set_drive_db(12.0);
    p.reset();
    let z: Vec<f32> = x.iter().map(|v| p.process(*v)).collect();
    let (z1, z2, z3) = (
        tone_amplitude(&z[8_192..], bin_hz(43), f64::from(SR)),
        tone_amplitude(&z[8_192..], bin_hz(86), f64::from(SR)),
        tone_amplitude(&z[8_192..], bin_hz(129), f64::from(SR)),
    );
    assert!(db(z2 / z1) < -80.0, "{}", db(z2 / z1));
    assert!(db(z3 / z1) > -30.0, "odd harmonics: {}", db(z3 / z1));
}

#[test]
fn oversampling_keeps_the_aliasing_of_a_high_note_low() {
    // 5859.375 Hz is bin 1000 of 8192 at 48 kHz: its harmonics fall on bins, so what is left
    // outside the first four is what folded back.
    let x = sine(5_859.375, f64::from(SR), 8_192 + 2_048, 0.5);
    let measure = |factor| {
        let y = drive_chain(&x, factor, 1.0, 0.0, 0.0);
        alias_db(&y[2_048..], 1_000)
    };
    let x4 = measure(OversampleFactor::X4);
    let x1 = measure(OversampleFactor::X1);
    println!("alias 1x {x1:.1} dB, 4x {x4:.1} dB");
    assert!(x4 < -45.0, "4x: {x4}");
    assert!(x4 < x1 - 10.0, "4x {x4} against 1x {x1}");
}

#[test]
fn tone_knobs_move_their_bands_and_load_each_other() {
    let mut t = ToneStack::new(192_000.0);
    t.set(0.5, 0.5, 0.5);
    let flat = [100.0, 480.0, 4_000.0].map(|f| t.response_db(f));
    t.set(1.0, 0.5, 0.5);
    assert!(
        t.response_db(100.0) > flat[0] + 6.0,
        "bass up raises 100 Hz"
    );
    assert!(
        (t.response_db(4_000.0) - flat[2]).abs() < 1.5,
        "treble stays"
    );
    t.set(0.5, 0.5, 1.0);
    assert!(
        t.response_db(4_000.0) > flat[2] + 6.0,
        "treble up raises 4 kHz"
    );
    t.set(0.5, 1.0, 0.5);
    assert!(
        t.response_db(480.0) > flat[1] + 3.5,
        "mid up fills the scoop"
    );
    t.set(0.5, 0.0, 0.5);
    assert!(t.response_db(480.0) < flat[1] - 2.0, "mid down deepens it");
    // The middle knob loads the shelves: with the bass at the maximum the lows are lower when
    // the mid is up.
    t.set(1.0, 0.0, 0.5);
    let bass_mid_low = t.response_db(100.0);
    t.set(1.0, 1.0, 0.5);
    let bass_mid_high = t.response_db(100.0);
    assert!(
        bass_mid_high < bass_mid_low - 0.5,
        "{bass_mid_low} {bass_mid_high}"
    );
    // The scoop is deeper with the shelves up.
    t.set(0.5, 0.5, 0.5);
    let d0 = t.response_db(480.0) - t.response_db(100.0);
    t.set(1.0, 0.5, 1.0);
    let d1 = t.response_db(480.0) - t.response_db(100.0);
    assert!(d1 < d0, "{d0} {d1}");
    // A stack at the middle loses level at 1 kHz.
    t.set(0.5, 0.5, 0.5);
    assert!(t.response_db(1_000.0) < 0.0);
}

#[test]
fn the_tone_stack_filters_the_signal_as_its_response_says() {
    let mut t = ToneStack::new(SR);
    t.set(0.0, 0.5, 1.0);
    t.reset();
    let want = t.response_db(4_000.0) - t.response_db(100.0);
    let hi = sine(4_000.0, f64::from(SR), 8_192, 0.2);
    let lo = sine(100.0, f64::from(SR), 8_192, 0.2);
    let run = |x: &[f32]| {
        let mut t = t.clone();
        t.reset();
        let y: Vec<f32> = x.iter().map(|v| t.process(*v)).collect();
        rms(&y[4_096..])
    };
    let got = db(f64::from(run(&hi) / run(&lo)));
    assert!((got - f64::from(want)).abs() < 0.5, "{got} {want}");
}

#[test]
fn the_input_stage_tightens_and_boosts() {
    let mut s = InputStage::new(SR);
    let low = sine(60.0, f64::from(SR), 16_384, 0.3);
    let mid = sine(780.0, f64::from(SR), 16_384, 0.3);
    let level = |s: &mut InputStage, x: &[f32]| {
        s.reset();
        let y: Vec<f32> = x.iter().map(|v| s.process(*v)).collect();
        rms(&y[8_192..])
    };
    let open_low = level(&mut s, &low);
    let open_mid = level(&mut s, &mid);
    s.set_tight_hz(300.0);
    s.reset();
    assert!(level(&mut s, &low) < open_low * 0.2, "60 Hz is cut");
    assert!(
        (level(&mut s, &mid) / open_mid - 1.0).abs() < 0.2,
        "780 Hz is not"
    );
    s.set_tight_hz(20.0);
    s.set_boost_db(12.0);
    s.reset();
    let boosted = level(&mut s, &mid) / open_mid;
    assert!((db(f64::from(boosted)) - 12.0).abs() < 1.0, "{boosted}");
}

#[test]
fn sag_squashes_a_loud_passage_and_recovers() {
    let burst = sine(200.0, f64::from(SR), 24_000, 0.9);
    let level = |sag| {
        let mut p = PowerAmp::new(SR);
        p.set_drive_db(6.0);
        p.set_sag(sag);
        p.reset();
        let y: Vec<f32> = burst.iter().map(|v| p.process(*v)).collect();
        // The start (before the envelope has built up) and the end of the burst.
        (rms(&y[0..480]), rms(&y[23_000..24_000]))
    };
    let (s0, s1) = level(0.0);
    let (g0, g1) = level(1.0);
    assert!((s0 - s1).abs() < 0.02, "no sag, no change: {s0} {s1}");
    assert!(g1 < g0 * 0.9, "the sagged level falls: {g0} {g1}");
    assert!(g1 < s1 * 0.9, "and ends lower than without sag");
    // A static signal fed after silence recovers: the envelope decays.
    let mut p = PowerAmp::new(SR);
    p.set_sag(1.0);
    for v in &burst {
        p.process(*v);
    }
    let high = p.envelope();
    for _ in 0..48_000 {
        p.process(0.0);
    }
    assert!(p.envelope() < high * 0.05, "{high} {}", p.envelope());
}

#[test]
fn presence_and_depth_shape_the_ends() {
    let level = |p: f32, d: f32, f: f64| {
        let mut a = PowerAmp::new(SR);
        a.set_drive_db(-24.0);
        a.set_voice(p, d);
        a.reset();
        let x = sine(f, f64::from(SR), 16_384, 0.05);
        let y: Vec<f32> = x.iter().map(|v| a.process(*v)).collect();
        rms(&y[8_192..])
    };
    assert!(
        db(f64::from(
            level(1.0, 0.0, 6_000.0) / level(0.0, 0.0, 6_000.0)
        )) > 8.0
    );
    assert!(db(f64::from(level(0.0, 1.0, 45.0) / level(0.0, 0.0, 45.0))) > 6.0);
    // And leave the other end alone.
    assert!(db(f64::from(level(1.0, 0.0, 70.0) / level(0.0, 0.0, 70.0))).abs() < 1.5);
}

#[test]
fn the_cabinets_roll_off_the_top_and_differ() {
    let mut c = Cabinet::new(SR);
    let mut at = |k: CabType, mic: f32, f: f32| {
        c.set_type(k);
        c.set_mic(mic);
        c.response_db(f)
    };
    for k in [CabType::Open1x12, CabType::Combo2x12, CabType::Closed4x12] {
        assert!(
            at(k, 0.3, 12_000.0) < at(k, 0.3, 2_500.0) - 20.0,
            "{k:?} top"
        );
        assert!(at(k, 0.3, 40.0) < at(k, 0.3, 120.0) - 10.0, "{k:?} bottom");
    }
    // The closed 4x12 has more low end and less top than the open 1x12.
    assert!(
        at(CabType::Closed4x12, 0.3, 105.0) - at(CabType::Closed4x12, 0.3, 1_000.0)
            > at(CabType::Open1x12, 0.3, 105.0) - at(CabType::Open1x12, 0.3, 1_000.0) + 2.0
    );
    assert!(at(CabType::Closed4x12, 0.3, 6_500.0) < at(CabType::Open1x12, 0.3, 6_500.0));
    // The mic: the centre is brighter than the edge.
    assert!(at(CabType::Closed4x12, 0.0, 4_000.0) > at(CabType::Closed4x12, 1.0, 4_000.0) + 5.0);
    // The loudness of the three is alike (pink-ish: the average level over the guitar range).
    let mut avg = |k| {
        (0..20)
            .map(|i| at(k, 0.3, 100.0 * 1.2f32.powi(i)))
            .sum::<f32>()
            / 20.0
    };
    let l: Vec<f32> = [CabType::Open1x12, CabType::Combo2x12, CabType::Closed4x12]
        .iter()
        .map(|k| avg(*k))
        .collect();
    let spread =
        l.iter().fold(f32::MIN, |a, b| a.max(*b)) - l.iter().fold(f32::MAX, |a, b| a.min(*b));
    assert!(spread < 3.0, "{l:?}");
}

#[test]
fn the_cabinet_off_is_a_wire_and_a_cabinet_filters_noise() {
    let x = noise(7, 16_384, 0.5);
    let mut c = Cabinet::new(SR);
    c.set_type(CabType::Off);
    c.reset();
    for v in &x {
        assert_eq!(c.process(*v).to_bits(), v.to_bits());
    }
    c.set_type(CabType::Closed4x12);
    c.reset();
    let y: Vec<f32> = x.iter().map(|v| c.process(*v)).collect();
    // The top is gone: compare the content above 10 kHz.
    let hi = |s: &[f32]| {
        let m = spectrum(s);
        let k0 = (10_000.0 / 48_000.0 * s.len() as f64) as usize;
        m[k0..].iter().map(|v| v * v).sum::<f64>()
    };
    assert!(hi(&y) < 0.001 * hi(&x));
}

#[test]
fn switching_the_cabinet_glides_without_a_click() {
    let x = sine(1_000.0, f64::from(SR), 9_600, 0.3);
    let mut c = Cabinet::new(SR);
    c.set_type(CabType::Open1x12);
    c.reset();
    let mut y = Vec::new();
    for (i, v) in x.iter().enumerate() {
        if i == 4_800 {
            c.set_type(CabType::Closed4x12);
        }
        y.push(c.process(*v));
    }
    let step = y
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < 0.2, "largest step {step}");
}

#[test]
fn every_block_is_deterministic_and_denormal_free() {
    let x = noise(3, 20_000, 1.0);
    let run = || drive_chain(&x, OversampleFactor::X4, 0.8, 3.0, 0.5);
    let (a, b) = (run(), run());
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
    // Silence after a loud passage decays to exact zeros (no denormals linger).
    let mut x = noise(3, 4_000, 1.0);
    x.extend(std::iter::repeat_n(0.0, 96_000));
    let y = drive_chain(&x, OversampleFactor::X4, 1.0, 0.0, 1.0);
    assert!(y[90_000..].iter().all(|v| v.abs() < 1e-12 || v.is_normal()));
    assert!(y[90_000..].iter().all(|v| v.abs() < 1e-6));
}
