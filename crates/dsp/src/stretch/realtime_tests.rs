use super::realtime::RtStretcher;
use super::{TimeMap, pitch_ratio_for_cents};
use crate::test_util::{dominant_hz, max_step, noise, sine};

const SR: u32 = 48_000;
const BLOCK: usize = 64;

/// Plays output frames `from..from + frames` through `st` in blocks of [`BLOCK`].
fn play(
    st: &mut RtStretcher,
    source: &[Vec<f32>; 2],
    map: &[(f64, f64)],
    ratio: f64,
    from: u64,
    frames: usize,
) -> [Vec<f32>; 2] {
    let mut out = [vec![0.0f32; frames], vec![0.0f32; frames]];
    let mut k = 0;
    while k < frames {
        let n = BLOCK.min(frames - k);
        let [l, r] = &mut out;
        st.process(
            [&source[0], &source[1]],
            map,
            ratio,
            SR,
            from + k as u64,
            [&mut l[k..k + n], &mut r[k..k + n]],
        );
        k += n;
    }
    out
}

fn fresh() -> RtStretcher {
    RtStretcher::new(SR)
}

fn stereo(x: Vec<f32>) -> [Vec<f32>; 2] {
    let r = x.iter().map(|v| v * 0.5).collect();
    [x, r]
}

fn near(got: f64, want: f64, tol: f64) -> bool {
    (got - want).abs() <= tol * want.abs()
}

#[test]
fn at_a_ratio_of_one_without_transposition_the_source_passes_through_bit_exact() {
    let src = stereo(noise(7, 40_000, 0.5));
    let map = [(0.0, 0.0), (100_000.0, 100_000.0)];
    let out = play(&mut fresh(), &src, &map, 1.0, 0, 30_000);
    for c in 0..2 {
        for t in 0..30_000 {
            assert_eq!(out[c][t].to_bits(), src[c][t].to_bits(), "ch {c} frame {t}");
        }
    }
    // An offset map started mid-clip: output frame t plays source frame t + 500.
    let shifted = [(0.0, 500.0), (100_000.0, 100_500.0)];
    let out = play(&mut fresh(), &src, &shifted, 1.0, 7_000, 9_000);
    for t in 0..9_000 {
        assert_eq!(
            out[0][t].to_bits(),
            src[0][7_500 + t].to_bits(),
            "frame {t}"
        );
    }
}

#[test]
fn a_stretched_sine_keeps_its_pitch_and_the_length_follows_the_map() {
    let len = SR as usize * 2;
    let src = stereo(sine(440.0, f64::from(SR), len, 0.5));
    for stretch in [1.5, 0.75] {
        let out_len = (len as f64 * stretch) as usize;
        let map = TimeMap::linear(1.0 / stretch);
        let knots = [(0.0, 0.0), (1.0, 1.0 / stretch)];
        let out = play(&mut fresh(), &src, &knots, 1.0, 0, out_len + 4_000);
        let mid = &out[0][out_len / 4..out_len * 3 / 4];
        let hz = dominant_hz(mid, f64::from(SR));
        assert!(near(hz, 440.0, 0.01), "stretch {stretch}: {hz} Hz");
        let hz_r = dominant_hz(&out[1][out_len / 4..out_len * 3 / 4], f64::from(SR));
        assert!(near(hz_r, 440.0, 0.01), "right channel: {hz_r} Hz");
        // The source ends where the map says (within one hop of 15 ms): sound before, silence
        // after.
        let end = map.out_at(len as f64) as usize;
        let hop = 720;
        let before = &out[0][end - 2 * hop..end - hop];
        assert!(before.iter().any(|v| v.abs() > 0.3), "sound before the end");
        assert!(
            out[0][end + hop..].iter().all(|v| *v == 0.0),
            "silence after the end ({stretch})"
        );
        // No clicks: a 440 Hz sine at 0.5 never steps by more than about 0.03 a frame.
        assert!(max_step(mid) < 0.05, "smooth ({stretch})");
    }
}

#[test]
fn a_transposition_changes_the_pitch_and_not_the_timing() {
    let len = SR as usize * 2;
    let src = stereo(sine(440.0, f64::from(SR), len, 0.5));
    let map = [(0.0, 0.0), (1.0, 1.0)];
    for (cents, want) in [(700, 659.255), (-1200, 220.0), (500, 587.33)] {
        let ratio = pitch_ratio_for_cents(cents);
        let out = play(&mut fresh(), &src, &map, ratio, 0, len + 4_000);
        let hz = dominant_hz(&out[0][len / 4..len * 3 / 4], f64::from(SR));
        assert!(near(hz, want, 0.01), "{cents} cents: {hz} Hz, want {want}");
        // The length stays the map's.
        assert!(
            out[0][len - 2_000..len - 1_000]
                .iter()
                .any(|v| v.abs() > 0.3)
        );
        assert!(out[0][len + 1_500..].iter().all(|v| *v == 0.0), "{cents}");
    }
}

#[test]
fn a_changing_slope_keeps_the_pitch_and_places_an_onset_where_the_map_says() {
    // A quiet 440 Hz sine that turns loud at source frame 60 000 (the marked onset).
    let len = SR as usize * 2;
    let onset = 60_000usize;
    let mut x = sine(440.0, f64::from(SR), len, 0.1);
    for v in &mut x[onset..] {
        *v *= 8.0;
    }
    let src = stereo(x);
    // Output frames 0..48 000 play source 0..36 000 (slow), then twice as fast.
    let knots = [(0.0, 0.0), (48_000.0, 36_000.0), (96_000.0, 132_000.0)];
    let map = TimeMap::new(&knots).unwrap();
    let out = play(&mut fresh(), &src, &knots, 1.0, 0, 70_000);
    let slow = dominant_hz(&out[0][8_000..40_000], f64::from(SR));
    let fast = dominant_hz(&out[0][62_000..70_000], f64::from(SR));
    assert!(near(slow, 440.0, 0.01), "slow part {slow} Hz");
    assert!(near(fast, 440.0, 0.01), "fast part {fast} Hz");
    let want = map.out_at(onset as f64);
    let got = out[0].iter().position(|v| v.abs() > 0.45).unwrap() as f64;
    let ms = (got - want).abs() * 1000.0 / f64::from(SR);
    assert!(ms <= 10.0, "onset at {got}, the map says {want} ({ms} ms)");
}

#[test]
fn a_reset_mid_stream_starts_like_a_fresh_stretcher() {
    let src = stereo(noise(3, 120_000, 0.4));
    let map = [(0.0, 0.0), (1.0, 0.8)];
    let ratio = pitch_ratio_for_cents(300);
    let mut used = fresh();
    let _ = play(&mut used, &src, &map, ratio, 0, 20_000);
    // A locate: the next block starts elsewhere (the stretcher notices by itself)...
    let jumped = play(&mut used, &src, &map, ratio, 70_000, 4_000);
    let clean = play(&mut fresh(), &src, &map, ratio, 70_000, 4_000);
    for t in 0..4_000 {
        assert!((jumped[0][t] - clean[0][t]).abs() < 1.0e-6, "frame {t}");
    }
    // ...and an explicit reset at the same place gives the same start.
    let mut again = fresh();
    let _ = play(&mut again, &src, &map, ratio, 0, 10_000);
    again.reset(70_000);
    let reset = play(&mut again, &src, &map, ratio, 70_000, 4_000);
    for t in 0..4_000 {
        assert!((reset[1][t] - clean[1][t]).abs() < 1.0e-6, "frame {t}");
    }
    // The first frames after the reset are the source's level, not a burst of old state.
    let peak = reset[0][..480].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak <= 0.6, "no click after the reset ({peak})");
}

#[test]
fn both_channels_use_the_same_offset() {
    // The right channel is the left one scaled: a linked stretcher keeps that exactly.
    let src = stereo(noise(11, 60_000, 0.5));
    let map = [(0.0, 0.0), (1.0, 1.0 / 1.3)];
    let out = play(
        &mut fresh(),
        &src,
        &map,
        pitch_ratio_for_cents(-200),
        0,
        40_000,
    );
    for (t, (l, r)) in out[0].iter().zip(&out[1]).enumerate() {
        assert!((r - l * 0.5).abs() < 1.0e-6, "frame {t}");
    }
}

#[test]
fn a_new_sample_rate_reconfigures_without_a_rebuild() {
    let src = stereo(sine(440.0, 96_000.0, 192_000, 0.5));
    let mut st = fresh();
    let map = [(0.0, 0.0), (1.0, 0.8)];
    let mut out = [vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
    let mut k = 0;
    while k < 96_000 {
        let [l, r] = &mut out;
        st.process(
            [&src[0], &src[1]],
            &map,
            1.0,
            96_000,
            k as u64,
            [&mut l[k..k + BLOCK], &mut r[k..k + BLOCK]],
        );
        k += BLOCK;
    }
    let hz = dominant_hz(&out[0][24_000..72_000], 96_000.0);
    assert!(near(hz, 440.0, 0.01), "{hz} Hz at 96 kHz");
}
