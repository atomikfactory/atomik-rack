//! Pure maths behind the effect panels' visuals: the compressor transfer curve against
//! the dsp gain computer, the EQ response against `atomik-dsp` biquads (within 0.01 dB), the
//! distortion curve against the device's `shape`, the reverb envelope and the LFO sketch.

use atomik_dsp::{BiquadCoeffs, BiquadShape, static_gain_reduction_knee_db};
use atomik_ui::fx::{
    self, EqBand, EqShape, compressor_gr_db, compressor_out_db, distortion_transfer,
    eq_band_response_db, eq_response_db, lfo_wave, reverb_sketch,
};

const SR: f64 = 48_000.0;

#[test]
fn transfer_curve_matches_the_dsp_gain_computer() {
    for &thr in &[-40.0f32, -18.0, -6.0] {
        for &ratio in &[1.0f32, 2.0, 4.0, 20.0] {
            for &knee in &[0.0f32, 6.0, 24.0] {
                let mut x = -60.0f32;
                while x <= 0.0 {
                    let want = static_gain_reduction_knee_db(x, thr, ratio, knee);
                    let got = compressor_gr_db(x, thr, ratio, knee, false);
                    assert!((want - got).abs() < 1e-5, "{x} {thr} {ratio} {knee}");
                    assert!(
                        (compressor_out_db(x, thr, ratio, knee, false) - (x - want)).abs() < 1e-5
                    );
                    x += 1.5;
                }
            }
        }
    }
    // The limit toggle is an infinite ratio.
    let want = static_gain_reduction_knee_db(-6.0, -18.0, f32::INFINITY, 6.0);
    assert!((compressor_gr_db(-6.0, -18.0, 4.0, 6.0, true) - want).abs() < 1e-5);
}

fn dsp_response(shape: BiquadShape, f0: f64, q: f64, g: f64, f: f64) -> f64 {
    BiquadCoeffs::design(shape, f0, q, g, SR).response_db(f, SR)
}

#[test]
fn eq_band_responses_match_the_dsp_biquads() {
    let freqs = [25.0, 80.0, 200.0, 1000.0, 3300.0, 8000.0, 15000.0];
    for &f0 in &[120.0, 500.0, 2500.0, 8000.0] {
        for &g in &[-12.0, 6.0, 18.0] {
            for &q in &[0.3, 0.707, 2.0] {
                for &f in &freqs {
                    let band = |shape| EqBand {
                        shape,
                        on: true,
                        freq: f0,
                        gain: g,
                        q,
                        slope: 0,
                    };
                    let cases = [
                        (EqShape::LowShelf, BiquadShape::LowShelf),
                        (EqShape::Peak, BiquadShape::Peaking),
                        (EqShape::HighShelf, BiquadShape::HighShelf),
                    ];
                    for (a, b) in cases {
                        let got = eq_band_response_db(&band(a), f, SR);
                        let want = dsp_response(b, f0, q, g, f);
                        assert!(
                            (got - want).abs() < 0.01,
                            "{a:?} {f0} {g} {q} {f}: {got} {want}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn eq_cut_slopes_are_butterworth_cascades() {
    let qs: [&[f64]; 3] = [
        &[std::f64::consts::FRAC_1_SQRT_2],
        &[0.541_196_1, 1.306_563],
        &[0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
    ];
    for (slope, qset) in qs.iter().enumerate() {
        for &f in &[30.0, 100.0, 300.0, 5000.0] {
            let band = EqBand {
                shape: EqShape::LowCut,
                on: true,
                freq: 100.0,
                gain: 0.0,
                q: std::f64::consts::FRAC_1_SQRT_2,
                slope,
            };
            let want: f64 = qset
                .iter()
                .map(|&q| dsp_response(BiquadShape::Highpass, 100.0, q, 0.0, f))
                .sum();
            let got = eq_band_response_db(&band, f, SR);
            assert!((got - want).abs() < 0.01, "{slope} {f}");
            let hi = EqBand {
                shape: EqShape::HighCut,
                freq: 4000.0,
                ..band
            };
            let want: f64 = qset
                .iter()
                .map(|&q| dsp_response(BiquadShape::Lowpass, 4000.0, q, 0.0, f))
                .sum();
            assert!((eq_band_response_db(&hi, f, SR) - want).abs() < 0.01);
        }
    }
    // The corner of a default Butterworth cut is -3.01 dB.
    let band = EqBand {
        shape: EqShape::LowCut,
        on: true,
        freq: 100.0,
        gain: 0.0,
        q: std::f64::consts::FRAC_1_SQRT_2,
        slope: 0,
    };
    assert!((eq_band_response_db(&band, 100.0, SR) + 3.01).abs() < 0.01);
}

#[test]
fn eq_total_response_sums_the_active_bands() {
    let mut bands = fx::default_eq_bands();
    assert!(
        eq_response_db(&bands, 1000.0, SR).abs() < 1e-9,
        "flat by default"
    );
    bands[2].gain = 12.0;
    let one = eq_band_response_db(&bands[2], 500.0, SR);
    assert!((eq_response_db(&bands, 500.0, SR) - one).abs() < 1e-9);
    bands[2].on = false;
    assert!(
        eq_response_db(&bands, 500.0, SR).abs() < 1e-9,
        "an off band is bypassed"
    );
}

#[test]
fn distortion_curves_follow_the_device() {
    use atomik_devices::distortion::{Curve, shape};
    use atomik_dsp::shaper::CharacterKind;
    let curves = [
        Curve::Soft,
        Curve::Hard,
        Curve::Tube,
        Curve::Fold,
        Curve::Crush,
        Curve::Character(CharacterKind::Tape),
        Curve::Character(CharacterKind::Fuzz),
        Curve::Character(CharacterKind::Modulate),
        Curve::Character(CharacterKind::Warp),
        Curve::Character(CharacterKind::Scream),
    ];
    for (i, c) in curves.iter().enumerate() {
        if i == 4 {
            continue; // the crush curve is checked below
        }
        for &(drive, bias) in &[(0.0f32, 0.0f32), (12.0, 0.0), (24.0, 0.3), (6.0, -0.5)] {
            let g = 10f32.powf(drive / 20.0);
            for k in -10..=10 {
                let x = k as f32 / 10.0;
                let want = shape(*c, x * g + bias) - shape(*c, bias);
                let got = distortion_transfer(i, drive, bias, 8.0, x);
                assert!((want - got).abs() < 1e-5, "{i} {drive} {bias} {x}");
            }
        }
    }
    // Bitcrush quantises to 2^(bits-1) levels.
    let y = distortion_transfer(4, 0.0, 0.0, 3.0, 0.3);
    assert!((y - 0.25).abs() < 1e-6, "{y}");
}

#[test]
fn reverb_tail_reaches_minus_60_db_at_the_decay_time() {
    let s = reverb_sketch(1, 40.0, 0.5, 2.0, 0.5, false);
    assert!((s.tail_db_at(s.tail_start + 2.0) + 60.0).abs() < 1e-3);
    assert!((s.tail_db_at(s.tail_start + 1.0) + 30.0).abs() < 1e-3);
    let frozen = reverb_sketch(1, 40.0, 0.5, 2.0, 0.5, true);
    assert!(frozen.tail_db_at(frozen.tail_start + 1.9) > -1.0);
    assert!(s.total > s.tail_start + 2.0);
    // Pre-delay shifts everything right.
    let late = reverb_sketch(1, 200.0, 0.5, 2.0, 0.5, false);
    assert!(late.tail_start > s.tail_start && late.taps[0].0 > s.taps[0].0);
}

#[test]
fn lfo_sketch_is_the_parabolic_wave() {
    assert!(lfo_wave(0.0).abs() < 1e-6);
    assert!((lfo_wave(0.25) + 1.0).abs() < 1e-6);
    assert!((lfo_wave(0.75) - 1.0).abs() < 1e-6);
    assert!((lfo_wave(1.25) + 1.0).abs() < 1e-6, "periodic");
}
