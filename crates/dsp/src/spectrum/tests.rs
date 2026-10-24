use super::*;
use crate::test_util::{noise, sine};
use rustfft::{FftPlanner, num_complex::Complex};

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn db(p: f32) -> f32 {
    10.0 * p.log10()
}

fn settings(size: usize, window: WindowKind) -> Settings {
    Settings {
        size,
        window,
        channel: ChannelMode::Left,
        bands: false,
        averaging_ms: 0.0,
        hold_ms: 0.0,
        decay_db_per_s: 15.0,
        tilt_db_per_oct: 0.0,
    }
}

/// One analysis of `x` on the left channel (the right is silent).
fn analyse(x: &[f32], size: usize, window: WindowKind, mode: ChannelMode) -> Vec<f32> {
    let silent = vec![0.0; x.len()];
    Analyser::new(size, window)
        .analyse(x, &silent, mode)
        .to_vec()
}

#[test]
fn the_window_bandwidths_are_the_textbook_values() {
    assert!(near(
        Analyser::new(4096, WindowKind::Hann).enbw(),
        1.5,
        1e-3
    ));
    let bh = Analyser::new(4096, WindowKind::BlackmanHarris).enbw();
    assert!(near(bh, 2.004, 0.01), "{bh}");
}

#[test]
fn a_sine_on_a_bin_reads_its_own_power_for_both_windows() {
    // Independent check with a library FFT: amplitude 0.5 centred on bin 100 of 4096.
    let (size, k) = (4096usize, 100usize);
    let x: Vec<f32> = (0..size)
        .map(|i| (0.5 * (std::f64::consts::TAU * k as f64 * i as f64 / size as f64).sin()) as f32)
        .collect();
    for kind in [WindowKind::Hann, WindowKind::BlackmanHarris] {
        let p = analyse(&x, size, kind, ChannelMode::Left);
        assert!(near(p[k], 0.25, 0.002), "{kind:?}: {}", p[k]);
        // The same number from rustfft with the window written out here.
        let mut buf: Vec<Complex<f64>> = (0..size)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / size as f64;
                let w = match kind {
                    WindowKind::Hann => 0.5 - 0.5 * t.cos(),
                    WindowKind::BlackmanHarris => {
                        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                            - 0.01168 * (3.0 * t).cos()
                    }
                };
                Complex::new(f64::from(x[i]) * w, 0.0)
            })
            .collect();
        let sum_w: f64 = (0..size)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / size as f64;
                match kind {
                    WindowKind::Hann => 0.5 - 0.5 * t.cos(),
                    WindowKind::BlackmanHarris => {
                        0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                            - 0.01168 * (3.0 * t).cos()
                    }
                }
            })
            .sum();
        FftPlanner::new().plan_fft_forward(size).process(&mut buf);
        let reference = (2.0 * buf[k].norm() / sum_w).powi(2) as f32;
        assert!(
            near(p[k], reference, 1e-5),
            "{kind:?}: {} vs {reference}",
            p[k]
        );
        // Far from the tone the leakage is far below.
        assert!(db(p[k + 200] / p[k]) < -60.0, "{kind:?}");
    }
}

#[test]
fn the_peak_bin_of_a_sine_lands_within_a_bin_of_its_frequency() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        for size in [1024usize, 2048, 4096, 8192] {
            for kind in [WindowKind::Hann, WindowKind::BlackmanHarris] {
                let df = sr / size as f32;
                for f in [
                    60.0f32, 100.0, 440.0, 1000.0, 3333.0, 8000.0, 12_345.0, 19_000.0,
                ] {
                    let x = sine(f64::from(f), f64::from(sr), size, 0.5);
                    let silent = vec![0.0; size];
                    let mut spectrum = Spectrum::new();
                    spectrum.update(&x, &silent, 0, sr, &settings(size, kind), 256);
                    let err = (spectrum.peak_hz() - f).abs();
                    assert!(
                        err < df,
                        "sr {sr} size {size} {kind:?} {f} Hz: read {} (bin {df})",
                        spectrum.peak_hz()
                    );
                    // The interpolation is usually far better than a bin.
                    assert!(err < 0.5 * df, "{err} vs {df}");
                    // And the level is within the scalloping loss of the window.
                    // (Within a few bins of DC the parabola has no room.)
                    let level = spectrum.peak_db();
                    assert!(
                        f < 4.0 * df || near(level, -6.02, 1.5),
                        "sr {sr} size {size} {kind:?} {f} Hz: {level} dB"
                    );
                }
            }
        }
    }
}

#[test]
fn the_channel_modes_pick_the_right_signal() {
    let size = 4096;
    // A tone exactly on bin 80, so that no scalloping loss blurs the comparisons.
    let bin = 80usize;
    let l = sine(
        f64::from(bin as f32 * 48_000.0 / size as f32),
        48_000.0,
        size,
        0.5,
    );
    let r: Vec<f32> = l.iter().map(|v| -v).collect();
    let at =
        |l: &[f32], r: &[f32], mode| Analyser::new(size, WindowKind::Hann).analyse(l, r, mode)[bin];
    // Left only: the left channel is 0.5, the right silent.
    let silent = vec![0.0; size];
    assert!(at(&l, &silent, ChannelMode::Left) > 0.2);
    assert!(at(&l, &silent, ChannelMode::Right) < 1e-9);
    // Mid and side of a left-only signal are each half of it (-6 dB in amplitude).
    let (mid, side) = (
        at(&l, &silent, ChannelMode::Mid),
        at(&l, &silent, ChannelMode::Side),
    );
    assert!(near(db(mid), db(0.0625), 0.3) && near(db(side), db(0.0625), 0.3));
    // Sum: the mean of the two powers.
    assert!(near(
        at(&l, &silent, ChannelMode::Sum),
        0.5 * at(&l, &silent, ChannelMode::Left),
        1e-6
    ));
    // Opposite polarity: the mid cancels, the side is the full amplitude.
    assert!(at(&l, &r, ChannelMode::Mid) < 1e-9, "cancels");
    assert!(near(at(&l, &r, ChannelMode::Side), 0.25, 0.01));
    // Identical channels: the opposite.
    assert!(at(&l, &l, ChannelMode::Side) < 1e-9);
    assert!(near(at(&l, &l, ChannelMode::Mid), 0.25, 0.01));
}

#[test]
fn fewer_frames_than_the_size_are_padded_and_garbage_is_ignored() {
    let x = sine(1000.0, 48_000.0, 1000, 0.5);
    let p = analyse(&x, 4096, WindowKind::Hann, ChannelMode::Left);
    assert_eq!(p.len(), 2049);
    assert!(p.iter().all(|v| v.is_finite() && *v >= 0.0));
    let mut bad = sine(1000.0, 48_000.0, 4096, 0.5);
    bad[10] = f32::NAN;
    bad[20] = f32::INFINITY;
    let p = analyse(&bad, 4096, WindowKind::Hann, ChannelMode::Left);
    assert!(p.iter().all(|v| v.is_finite()));
    // More frames than the size: the newest are used.
    let long = sine(1000.0, 48_000.0, 20_000, 0.5);
    let p = analyse(&long, 1024, WindowKind::Hann, ChannelMode::Left);
    assert_eq!(p.len(), 513);
}

#[test]
fn silence_reads_the_floor() {
    let z = vec![0.0; 4096];
    let mut s = Spectrum::new();
    s.update(&z, &z, 0, 48_000.0, &Settings::default(), 128);
    assert_eq!(s.trace().len(), 128);
    assert!(s.trace().iter().all(|v| near(*v, FLOOR_DB, 0.0)));
    assert!(near(s.peak_hz(), 0.0, 0.0) && near(s.peak_db(), FLOOR_DB, 0.0));
}

#[test]
fn the_trace_peaks_at_the_column_of_the_tone_on_a_log_axis() {
    let size = 4096;
    let s = Settings {
        size,
        ..settings(size, WindowKind::Hann)
    };
    for f in [50.0f32, 440.0, 1000.0, 7000.0, 15_000.0] {
        let x = sine(f64::from(f), 48_000.0, size, 0.5);
        let silent = vec![0.0; size];
        let mut spectrum = Spectrum::new();
        spectrum.update(&x, &silent, 0, 48_000.0, &s, 400);
        let best = spectrum
            .trace()
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let (f0, f1) = (
            column_edge_hz(best as f64, 400),
            column_edge_hz(best as f64 + 1.0, 400),
        );
        // The tone is in the loudest column, or within a bin of it: below a bin's width the
        // curve between bins peaks at the centre of the loudest bin.
        let bin = 48_000.0 / size as f64;
        assert!(
            (f64::from(f) - (f0 * f1).sqrt()).abs() <= bin.max(0.03 * f64::from(f)),
            "{f} Hz: column {best} spans {f0:.1} to {f1:.1} Hz"
        );
        // The loudest column carries the tone at close to its level at any frequency.
        assert!(spectrum.trace()[best] > -8.0, "{}", spectrum.trace()[best]);
    }
    assert!(near(column_edge_hz(0.0, 400) as f32, 20.0, 1e-3));
    assert!(near(column_edge_hz(400.0, 400) as f32, 20_000.0, 0.05));
}

#[test]
fn averaging_follows_the_time_constant_independently_of_the_update_rate() {
    let size = 2048;
    let tone = sine(1000.0, 48_000.0, size, 0.5);
    let silent = vec![0.0; size];
    let s = Settings {
        averaging_ms: 100.0,
        ..settings(size, WindowKind::Hann)
    };
    let run = |step: u64, steps: u64| {
        let mut sp = Spectrum::new();
        sp.update(&silent, &silent, 0, 48_000.0, &s, 200);
        // The tone starts now; `steps` updates of `step` frames each.
        for _ in 0..steps {
            sp.update(&tone, &silent, step, 48_000.0, &s, 200);
        }
        sp.peak_db()
    };
    // 100 ms in one update, or in ten of 10 ms: the same one-pole average to a small tolerance.
    let once = run(4800, 1);
    let tenfold = run(480, 10);
    assert!(near(once, tenfold, 0.6), "{once} vs {tenfold}");
    // After one time constant the power is at 63% of its final value: -2 dB from -6.02.
    assert!(near(once, -6.02 + 10.0 * 0.632f32.log10(), 0.5), "{once}");
    // After many time constants it is there.
    assert!(near(run(4800, 10), -6.02, 0.2));
    // No averaging: at once.
    let mut sp = Spectrum::new();
    let none = settings(size, WindowKind::Hann);
    sp.update(&silent, &silent, 0, 48_000.0, &none, 200);
    sp.update(&tone, &silent, 1, 48_000.0, &none, 200);
    assert!(near(sp.peak_db(), -6.02, 0.5));
}

#[test]
fn the_peak_holds_then_falls_at_the_decay_rate_down_to_the_trace() {
    let size = 2048;
    let tone = sine(1000.0, 48_000.0, size, 0.5);
    let silent = vec![0.0; size];
    let s = Settings {
        hold_ms: 500.0,
        decay_db_per_s: 20.0,
        ..settings(size, WindowKind::Hann)
    };
    let mut sp = Spectrum::new();
    sp.update(&tone, &silent, 0, 48_000.0, &s, 200);
    let col = sp
        .trace()
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    let top = sp.peaks()[col];
    assert!(near(top, sp.trace()[col], 0.0));
    // 10 ms updates of silence. The trace falls at once; the peak holds for 500 ms.
    let step = 480;
    for _ in 0..40 {
        sp.update(&silent, &silent, step, 48_000.0, &s, 200);
    }
    assert!(sp.trace()[col] < -150.0);
    assert!(
        near(sp.peaks()[col], top, 1e-3),
        "held: {}",
        sp.peaks()[col]
    );
    // 1 s more (1.4 s in all): the fall began at 500 ms, so 0.9 s at 20 dB per second.
    for _ in 0..100 {
        sp.update(&silent, &silent, step, 48_000.0, &s, 200);
    }
    let fallen = top - sp.peaks()[col];
    assert!(near(fallen, 20.0 * (1.4 - 0.5), 1.0), "fell {fallen} dB");
    // And in the end it meets the trace.
    for _ in 0..2000 {
        sp.update(&silent, &silent, step, 48_000.0, &s, 200);
    }
    assert!(near(sp.peaks()[col], sp.trace()[col], 0.0));
    // A louder moment raises it at once; a hold of 0 turns the peaks into the trace.
    let off = Settings { hold_ms: 0.0, ..s };
    sp.update(&tone, &silent, step, 48_000.0, &off, 200);
    sp.update(&silent, &silent, step, 48_000.0, &off, 200);
    assert_eq!(sp.peaks(), sp.trace());
}

#[test]
fn the_bands_hold_the_energy_of_a_tone_and_of_noise() {
    let sr = 48_000.0;
    let size = 8192;
    let s = Settings {
        bands: true,
        ..settings(size, WindowKind::Hann)
    };
    let silent = vec![0.0; size];
    // 1 kHz at amplitude 0.5 in its own band: -6 dB, the neighbours far below.
    let tone = sine(1000.0, f64::from(sr), size, 0.5);
    let mut sp = Spectrum::new();
    sp.update(&tone, &silent, 0, sr, &s, 400);
    assert_eq!(
        sp.trace().len(),
        BAND_COUNT,
        "bands ignore the column count"
    );
    let centres = band_centres();
    let band = centres
        .iter()
        .position(|c| (c - 1000.0).abs() < 1.0)
        .unwrap();
    assert!(near(sp.trace()[band], -6.02, 0.6), "{}", sp.trace()[band]);
    for far in [band - 3, band + 3] {
        assert!(sp.trace()[far] < -45.0, "band {far}: {}", sp.trace()[far]);
    }
    // The same tone off the centre of the band still reads its power (a 1/3 octave is wide).
    let off = sine(1080.0, f64::from(sr), size, 0.5);
    sp.update(&off, &silent, 0, sr, &s, 400);
    assert!(near(sp.trace()[band], -6.02, 0.8), "{}", sp.trace()[band]);
    // White noise: the bands add up to the power of the noise inside 17.8 Hz to 22.4 kHz, in
    // the units where a sine of amplitude A reads A^2 (twice its power): +3 dB over the RMS.
    let amp = 0.5f32;
    let x = noise(7, size, amp);
    sp.update(&x, &silent, 0, sr, &s, 400);
    let total: f32 = sp.trace().iter().map(|d| 10f32.powf(d / 10.0)).sum();
    let sigma2 = 2.0 * amp * amp / 3.0;
    let covered = (22_387.0 - 17.78) / (sr / 2.0);
    assert!(
        near(db(total), db(sigma2 * covered), 1.0),
        "{} dB vs {} dB",
        db(total),
        db(sigma2 * covered)
    );
    // The nominal labels match the exact centres to 3 %.
    for (exact, label) in centres.iter().zip(BAND_LABELS) {
        assert!((exact / label - 1.0).abs() < 0.03, "{exact} vs {label}");
    }
    assert!(near(centres[0], 19.95, 0.05) && near(centres[BAND_COUNT - 1], 19_953.0, 5.0));
}

#[test]
fn the_tilt_raises_the_top_by_its_slope_per_octave() {
    let size = 4096;
    let x = noise(9, size, 0.5);
    let silent = vec![0.0; size];
    let flat = settings(size, WindowKind::Hann);
    let tilted = Settings {
        tilt_db_per_oct: 4.5,
        ..flat
    };
    let (mut a, mut b) = (Spectrum::new(), Spectrum::new());
    a.update(&x, &silent, 0, 48_000.0, &flat, 200);
    b.update(&x, &silent, 0, 48_000.0, &tilted, 200);
    for c in [20usize, 100, 150, 190] {
        let hz = (column_edge_hz(c as f64, 200) * column_edge_hz(c as f64 + 1.0, 200)).sqrt();
        let want = 4.5 * (hz / 1000.0).log2() as f32;
        assert!(
            near(b.trace()[c] - a.trace()[c], want, 0.01),
            "column {c} at {hz:.0} Hz: {} vs {want}",
            b.trace()[c] - a.trace()[c]
        );
    }
    // The readout is not tilted.
    assert!(near(a.peak_db(), b.peak_db(), 0.0));
}

#[test]
fn a_change_of_settings_starts_over_and_the_result_is_deterministic() {
    let size = 2048;
    let x = noise(11, size, 0.5);
    let silent = vec![0.0; size];
    let s = Settings {
        averaging_ms: 500.0,
        ..settings(size, WindowKind::Hann)
    };
    let (mut a, mut b) = (Spectrum::new(), Spectrum::new());
    for sp in [&mut a, &mut b] {
        sp.update(&x, &silent, 0, 48_000.0, &s, 100);
        sp.update(&x, &silent, 480, 48_000.0, &s, 100);
    }
    let bits = |sp: &Spectrum| sp.trace().iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
    // A new window forgets the average (a second update with `averaging` would blend).
    let quiet = sine(1000.0, 48_000.0, size, 0.01);
    a.update(&quiet, &silent, 480, 48_000.0, &s, 100);
    let blended = a.peak_db();
    let other = Settings {
        window: WindowKind::BlackmanHarris,
        ..s
    };
    a.update(&quiet, &silent, 480, 48_000.0, &other, 100);
    assert!(
        a.peak_db() < blended - 10.0,
        "restarted from the quiet tone"
    );
    assert_eq!(a.trace().len(), 100);
    // A different number of columns rebuilds the trace.
    a.update(&quiet, &silent, 480, 48_000.0, &other, 60);
    assert_eq!(a.trace().len(), 60);
    assert_eq!(a.peaks().len(), 60);
    a.reset();
    assert!(a.trace().is_empty());
}
