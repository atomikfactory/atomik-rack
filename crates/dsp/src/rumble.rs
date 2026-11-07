//! Rumble from a kick and the reverse reverb swell, offline.
//!
//! Both run on a worker over audio that was rendered already, never on the audio thread, so they
//! allocate freely. Everything is plain sums and comparisons around the existing filters and the
//! large [`Room`], so the same samples give the same result on every platform within the float
//! tolerance of those.
//!
//! # Rumble layer
//!
//! [`rumble_layer`] turns a rendered kick into a layer of the same length, one piece per hit. A
//! piece runs from the hit to the next hit (or the end), is made of the kick's own sub band, and
//! so starts in phase with the kick, then goes on as a sustained low tail until the next hit,
//! where it is faded out over a few milliseconds so it never runs into it (and faded in over
//! 1.5 ms: the zero-phase low-pass smooths the sub band a little ahead of the hit, so a piece
//! would start with a step).
//!
//! * **Sub band.** The hit is low-passed at [`RumbleParams::sub_hz`] (a fourth order Butterworth
//!   run forward and backward, so without a phase shift, 24 dB per octave): the rumble keeps the
//!   kick's lows and nothing above them, in phase with it.
//! * **Tail repeat** ([`RumbleMode::TailRepeat`]). The transient is left behind: the envelope of
//!   the sub band is read in 5 ms windows, and the tail starts where it has fallen to half of its
//!   peak. A chunk of the tail, a third of what is left between [`MIN_CHUNK_MS`] and
//!   [`MAX_CHUNK_MS`], cut at zero crossings, is repeated after the kick's own sound, each copy
//!   [`REPEAT_GAIN`] quieter than the one before and joined to it by an equal-power crossfade of a
//!   quarter of the chunk, until the next hit. A hit that has no tail worth repeating (the rest of
//!   its gap is shorter than two chunks) keeps its sub band as it is.
//! * **Reverb** ([`RumbleMode::Reverb`]). The sub band goes through the large [`Room`] at
//!   [`RumbleParams::decay`] seconds (dark: damping 0.8), alone for the gap of the hit, so one
//!   hit's tail never bleeds into the next. The reverb is low-passed again, set to
//!   [`REVERB_LEVEL`] of the sub band's peak and added to the sub band, which keeps the start in
//!   phase with the kick. Left and right are decorrelated by the room: the layer is wide.
//! * **Level.** The layer is scaled down, when it is louder than that, to the peak of the kick's
//!   sub band: a rumble never gets fatter than the kick under it.
//!
//! # Reverse swell
//!
//! [`reverse_swell`] is the reverse reverb of a source: the source is reversed before it enters
//! the reverb, the reverb tail that rings after the reversed source is kept and reversed again.
//! What remains is `seconds` of a swell that rises and ends at the loudest point, right where the
//! source starts. The swell is set to [`SWELL_LEVEL`] of the source's peak, so a quiet or very
//! short source still gives an audible one.

use crate::{Biquad, BiquadCoeffs, BiquadShape, Room, RoomRange};

/// Seed of the room.
const SEED: u64 = 0x5255_4d42_4c45;
/// The shortest and longest decay of the reverb mode, in seconds.
pub const DECAY_RANGE: (f32, f32) = (0.5, 12.0);
/// The range of the sub band's corner, in Hz.
pub const SUB_RANGE: (f32, f32) = (40.0, 400.0);
/// The shortest chunk of tail a repeat uses, in ms.
pub const MIN_CHUNK_MS: f32 = 30.0;
/// The longest chunk of tail a repeat uses, in ms.
pub const MAX_CHUNK_MS: f32 = 250.0;
/// How much quieter each repeat of the tail is than the one before.
pub const REPEAT_GAIN: f32 = 0.85;
/// The peak of the reverb tail, as a share of the sub band's peak.
pub const REVERB_LEVEL: f32 = 0.7;
/// The peak of a swell, as a share of the source's peak.
pub const SWELL_LEVEL: f32 = 0.7;
/// The window of the envelope that finds the tail, in ms.
const ENVELOPE_MS: f32 = 5.0;
/// The tail starts where the envelope has fallen to this share of its peak.
const TAIL_SHARE: f32 = 0.5;
/// How far a chunk edge moves to land on a zero crossing, in ms.
const SNAP_MS: f32 = 6.0;
/// The fade at the end of a piece, in ms.
const FADE_END_MS: f32 = 15.0;
/// The fade at the start of a piece, in ms: the sub band of a hit is smoothed ahead of the hit
/// by the zero-phase filter, so a piece would start with a step.
const FADE_IN_MS: f32 = 1.5;
/// The fade at the start of a swell, in ms.
const FADE_START_MS: f32 = 5.0;
/// The shortest piece that is built, in ms.
const MIN_PIECE_MS: f32 = 10.0;
/// The most repeats of a chunk in one piece.
const MAX_REPEATS: usize = 64;
/// The Q of the two sections of the sub band's fourth order Butterworth.
const BUTTERWORTH_Q: [f64; 2] = [0.541_196_1, 1.306_563];

/// How the tail of a rumble is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RumbleMode {
    /// A trimmed chunk of the kick's tail, repeated with crossfades.
    TailRepeat,
    /// The kick through a long, dark reverb, low-passed to the sub band.
    Reverb,
}

/// The choices of [`rumble_layer`].
#[derive(Debug, Clone, PartialEq)]
pub struct RumbleParams {
    /// How the tail is made.
    pub mode: RumbleMode,
    /// The corner of the sub band, in Hz (within [`SUB_RANGE`]).
    pub sub_hz: f32,
    /// The decay of the reverb mode, in seconds (within [`DECAY_RANGE`]).
    pub decay: f32,
}

impl Default for RumbleParams {
    fn default() -> Self {
        Self {
            mode: RumbleMode::TailRepeat,
            sub_hz: 120.0,
            decay: 3.0,
        }
    }
}

fn frames_of(ms: f32, rate: u32) -> usize {
    ((ms * 0.001 * rate as f32).round() as usize).max(1)
}

fn peak(channels: &[Vec<f32>]) -> f32 {
    channels
        .iter()
        .flatten()
        .fold(0.0_f32, |m, s| m.max(s.abs()))
}

/// `x` through the fourth order Butterworth low-pass at `hz`, run forward and then backward: the
/// filter has no phase shift, so the sub band stays in phase with the kick it came from (and the
/// response is the square of the Butterworth's, 24 dB per octave and -6 dB at `hz`).
fn lowpass(x: &[f32], hz: f32, rate: u32) -> Vec<f32> {
    let sections: Vec<BiquadCoeffs> = BUTTERWORTH_Q
        .iter()
        .map(|&q| {
            BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(hz), q, 0.0, f64::from(rate))
        })
        .collect();
    let run = |input: &mut dyn Iterator<Item = f32>| -> Vec<f32> {
        let mut state = [Biquad::default(); 2];
        input
            .map(|s| {
                let mut v = f64::from(s);
                for (c, st) in sections.iter().zip(state.iter_mut()) {
                    v = st.process(c, v);
                }
                v as f32
            })
            .collect()
    };
    let mut forward = run(&mut x.iter().copied());
    forward.reverse();
    let mut back = run(&mut forward.into_iter());
    back.reverse();
    back
}

/// The index within `radius` of `at` where `x` crosses zero going up and is closest to `at`
/// (`at` itself when there is none).
fn zero_crossing_near(x: &[f32], at: usize, radius: usize) -> usize {
    let lo = at.saturating_sub(radius).max(1);
    let hi = (at + radius).min(x.len().saturating_sub(1));
    (lo..=hi)
        .filter(|&i| x[i - 1] <= 0.0 && x[i] > 0.0)
        .min_by_key(|&i| i.abs_diff(at))
        .unwrap_or(at.min(x.len()))
}

/// One piece of the tail-repeat rumble: `lp` (the sub band of a hit) with its tail extended.
fn tail_repeat(lp: &[Vec<f32>], rate: u32) -> Vec<Vec<f32>> {
    let len = lp[0].len();
    // The envelope: the loudest sample of each window, over every channel.
    let window = frames_of(ENVELOPE_MS, rate);
    let env: Vec<f32> = (0..len.div_ceil(window))
        .map(|w| {
            let from = w * window;
            let to = (from + window).min(len);
            lp.iter()
                .flat_map(|c| &c[from..to])
                .fold(0.0_f32, |m, s| m.max(s.abs()))
        })
        .collect();
    let (top, top_at) =
        env.iter().enumerate().fold(
            (0.0_f32, 0),
            |(m, mi), (i, &e)| {
                if e > m { (e, i) } else { (m, mi) }
            },
        );
    let unchanged = || lp.to_vec();
    if top <= 0.0 {
        return unchanged();
    }
    let Some(tail_window) = (top_at + 1..env.len()).find(|&w| env[w] <= TAIL_SHARE * top) else {
        return unchanged();
    };
    let snap = frames_of(SNAP_MS, rate);
    let start = zero_crossing_near(&lp[0], tail_window * window, snap).min(len);
    let rest = len - start;
    let min_chunk = frames_of(MIN_CHUNK_MS, rate);
    if rest < 2 * min_chunk {
        return unchanged();
    }
    let wanted = (rest / 3).clamp(min_chunk, frames_of(MAX_CHUNK_MS, rate));
    let end = zero_crossing_near(&lp[0], start + wanted, snap).min(len);
    let chunk = end.saturating_sub(start);
    if chunk < min_chunk / 2 {
        return unchanged();
    }
    let fade = (chunk / 4).max(1);
    // Equal power: the weights of a crossfade are the square roots of a linear one.
    let rise = |j: usize| ((j as f32 + 0.5) / fade as f32).min(1.0).sqrt();
    let fall = |j: usize| (1.0 - (j as f32 + 0.5) / fade as f32).max(0.0).sqrt();
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; len]; lp.len()];
    // The kick's own sound up to the end of the first chunk, with the chunk's end faded out.
    for (o, c) in out.iter_mut().zip(lp) {
        for i in 0..end {
            let weight = if i + fade >= end {
                fall(i + fade - end)
            } else {
                1.0
            };
            o[i] = c[i] * weight;
        }
    }
    let mut cursor = end - fade;
    let mut gain = 1.0;
    for _ in 0..MAX_REPEATS {
        if cursor >= len {
            break;
        }
        gain *= REPEAT_GAIN;
        for j in 0..chunk {
            let at = cursor + j;
            if at >= len {
                break;
            }
            let mut weight = gain;
            if j < fade {
                weight *= rise(j);
            }
            if j + fade >= chunk {
                weight *= fall(j + fade - chunk);
            }
            for (o, c) in out.iter_mut().zip(lp) {
                o[at] += c[start + j] * weight;
            }
        }
        cursor += chunk - fade;
    }
    out
}

/// One piece of the reverb rumble: `lp` plus its reverb tail.
fn reverb_tail(lp: &[Vec<f32>], rate: u32, params: &RumbleParams) -> Vec<Vec<f32>> {
    let len = lp[0].len();
    let mut room = Room::new(rate as f32, SEED, RoomRange::Large);
    room.set_size(0.6);
    room.set_damping(0.8);
    room.set_rt60_s(params.decay.clamp(DECAY_RANGE.0, DECAY_RANGE.1));
    let right = lp.get(1).unwrap_or(&lp[0]);
    let mut wet = vec![vec![0.0_f32; len]; 2];
    for i in 0..len {
        let (l, r) = room.process(lp[0][i], right[i]);
        wet[0][i] = l;
        wet[1][i] = r;
    }
    for w in &mut wet {
        *w = lowpass(w, params.sub_hz, rate);
    }
    let (dry_peak, wet_peak) = (peak(lp), peak(&wet));
    let scale = if wet_peak > 0.0 {
        REVERB_LEVEL * dry_peak / wet_peak
    } else {
        0.0
    };
    lp.iter()
        .enumerate()
        .map(|(c, dry)| {
            dry.iter()
                .zip(&wet[c.min(1)])
                .map(|(d, w)| d + scale * w)
                .collect()
        })
        .collect()
}

/// The rumble layer of `kick` (rendered audio, one `Vec` per channel, equal lengths): one piece
/// per hit, `hits` being the frames where the kick sounds. The result has the length and the
/// channels of `kick`; it is silent where no piece plays (before the first hit, in a piece that is
/// too short). See the module docs.
pub fn rumble_layer(
    kick: &[Vec<f32>],
    rate: u32,
    hits: &[usize],
    params: &RumbleParams,
) -> Vec<Vec<f32>> {
    let len = kick.first().map_or(0, Vec::len);
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; len]; kick.len()];
    if len == 0 || rate == 0 {
        return out;
    }
    let sub_hz = params.sub_hz.clamp(SUB_RANGE.0, SUB_RANGE.1);
    let mut starts: Vec<usize> = hits.iter().copied().filter(|&h| h < len).collect();
    starts.sort_unstable();
    starts.dedup();
    let min_piece = frames_of(MIN_PIECE_MS, rate);
    let mut kick_peak = 0.0_f32;
    for (i, &from) in starts.iter().enumerate() {
        let to = starts.get(i + 1).copied().unwrap_or(len);
        if to - from < min_piece {
            continue;
        }
        let lp: Vec<Vec<f32>> = kick
            .iter()
            .map(|c| lowpass(&c[from..to], sub_hz, rate))
            .collect();
        kick_peak = kick_peak.max(peak(&lp));
        let mut piece = match params.mode {
            RumbleMode::TailRepeat => tail_repeat(&lp, rate),
            RumbleMode::Reverb => reverb_tail(&lp, rate, params),
        };
        // In without a step, out before the next hit.
        let fade = frames_of(FADE_END_MS, rate).min((to - from) / 4).max(1);
        let fade_in = frames_of(FADE_IN_MS, rate).min((to - from) / 4).max(1);
        for c in &mut piece {
            let n = c.len();
            for j in 0..fade {
                c[n - 1 - j] *= (j as f32 + 0.5) / fade as f32;
            }
            for (j, s) in c.iter_mut().take(fade_in).enumerate() {
                *s *= (j as f32 + 0.5) / fade_in as f32;
            }
        }
        for (o, c) in out.iter_mut().zip(&piece) {
            o[from..to].copy_from_slice(c);
        }
    }
    // Never fatter than the kick.
    let top = peak(&out);
    if top > kick_peak && top > 0.0 {
        let scale = kick_peak / top;
        for s in out.iter_mut().flatten() {
            *s *= scale;
        }
    }
    out
}

/// The reverse reverb swell of `source` (one or two channels at `rate`): `seconds` of audio, in
/// two channels, that rises to its loudest at the very end, where the source starts. See the
/// module docs. Silent for a silent source.
pub fn reverse_swell(source: &[Vec<f32>], rate: u32, seconds: f32) -> Vec<Vec<f32>> {
    let n = source.first().map_or(0, Vec::len);
    let tail = frames_of(seconds.max(0.0) * 1000.0, rate);
    let mut swell = vec![vec![0.0_f32; tail]; 2];
    if n == 0 || rate == 0 {
        return swell;
    }
    let mut room = Room::new(rate as f32, SEED, RoomRange::Large);
    room.set_size(0.5);
    room.set_damping(0.35);
    room.set_rt60_s(seconds.clamp(DECAY_RANGE.0, DECAY_RANGE.1));
    let right = source.get(1).unwrap_or(&source[0]);
    // The reversed source goes in; what rings after it is the tail, kept back to front.
    for i in 0..n + tail {
        let (l, r) = if i < n {
            (source[0][n - 1 - i], right[n - 1 - i])
        } else {
            (0.0, 0.0)
        };
        let (wl, wr) = room.process(l, r);
        if i >= n {
            let at = tail - 1 - (i - n);
            swell[0][at] = wl;
            swell[1][at] = wr;
        }
    }
    let swell_peak = peak(&swell);
    let source_peak = peak(source);
    if swell_peak <= 0.0 || source_peak <= 0.0 {
        return vec![vec![0.0; tail]; 2];
    }
    let scale = SWELL_LEVEL * source_peak / swell_peak;
    let fade = frames_of(FADE_START_MS, rate).min(tail / 4).max(1);
    for c in &mut swell {
        for (j, s) in c.iter_mut().enumerate() {
            let weight = if j < fade {
                (j as f32 + 0.5) / fade as f32
            } else {
                1.0
            };
            *s *= scale * weight;
        }
    }
    swell
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A kick: a 55 Hz sine that falls from 90 Hz, decaying over about 90 ms, with a click.
    fn kick(frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (-t / 0.09).exp();
                let phase = 2.0
                    * std::f32::consts::PI
                    * (55.0 * t + 35.0 * 0.02 * (1.0 - (-t / 0.02).exp()));
                let click = if i < 40 {
                    0.3 * (1.0 - i as f32 / 40.0)
                } else {
                    0.0
                };
                0.9 * env * phase.sin() + click
            })
            .collect()
    }

    /// Two hits `gap` frames apart in a render of `hits` of them, in stereo.
    fn render(gap: usize, hits: usize) -> (Vec<Vec<f32>>, Vec<usize>) {
        let one = kick(gap);
        let mut l = Vec::new();
        for _ in 0..hits {
            l.extend_from_slice(&one);
        }
        let starts = (0..hits).map(|h| h * gap).collect();
        (vec![l.clone(), l], starts)
    }

    fn correlation(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let (ea, eb) = (
            a.iter().map(|x| x * x).sum::<f32>(),
            b.iter().map(|x| x * x).sum::<f32>(),
        );
        dot / (ea * eb).sqrt()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn params(mode: RumbleMode) -> RumbleParams {
        RumbleParams {
            mode,
            ..RumbleParams::default()
        }
    }

    #[test]
    fn both_modes_start_in_phase_with_the_kick() {
        let (kick, hits) = render(RATE as usize / 2, 3);
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            let layer = rumble_layer(&kick, RATE, &hits, &params(mode));
            for &h in &hits {
                let w = 960; // 20 ms
                let c = correlation(&kick[0][h..h + w], &layer[0][h..h + w]);
                assert!(c > 0.95, "{mode:?} hit at {h}: correlation {c}");
            }
        }
    }

    #[test]
    fn the_tail_repeat_keeps_ringing_after_the_kick_has_died() {
        let (kick, hits) = render(RATE as usize, 2);
        let layer = rumble_layer(&kick, RATE, &hits, &params(RumbleMode::TailRepeat));
        let (from, to) = (24_000, 40_000); // 0.5 s to 0.83 s into the hit
        assert!(rms(&kick[0][from..to]) < 1.0e-3);
        assert!(
            rms(&layer[0][from..to]) > 5.0e-3,
            "rms {}",
            rms(&layer[0][from..to])
        );
    }

    #[test]
    fn the_reverb_rumble_rings_on_and_is_wide() {
        let (kick, hits) = render(RATE as usize, 2);
        let layer = rumble_layer(&kick, RATE, &hits, &params(RumbleMode::Reverb));
        let (from, to) = (24_000, 40_000);
        assert!(rms(&layer[0][from..to]) > 5.0e-3);
        // The two sides of a mono kick differ: the room decorrelates them.
        let difference: Vec<f32> = layer[0][from..to]
            .iter()
            .zip(&layer[1][from..to])
            .map(|(a, b)| a - b)
            .collect();
        assert!(rms(&difference) > 1.0e-3, "{}", rms(&difference));
    }

    #[test]
    fn the_layer_only_holds_the_sub_band() {
        // A kick with a loud 4 kHz tone: the layer has none of it.
        let (mut kick, hits) = render(RATE as usize / 2, 2);
        for ch in &mut kick {
            for (i, s) in ch.iter_mut().enumerate() {
                *s += 0.5 * (2.0 * std::f32::consts::PI * 4000.0 * i as f32 / RATE as f32).sin();
            }
        }
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            let layer = rumble_layer(&kick, RATE, &hits, &params(mode));
            let high: Vec<f32> = layer[0].windows(2).map(|w| w[1] - w[0]).collect();
            assert!(rms(&high) < 0.05, "{mode:?}: {}", rms(&high));
        }
    }

    #[test]
    fn a_piece_is_cut_off_before_the_next_hit() {
        let (kick, hits) = render(RATE as usize / 2, 3);
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            let layer = rumble_layer(&kick, RATE, &hits, &params(mode));
            for &h in &hits[1..] {
                assert!(layer[0][h - 1].abs() < 0.02, "{mode:?} at {h}");
            }
        }
    }

    #[test]
    fn the_layer_is_never_louder_than_the_kick() {
        let (kick, hits) = render(RATE as usize / 2, 3);
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            let layer = rumble_layer(&kick, RATE, &hits, &params(mode));
            assert!(peak(&layer) <= peak(&kick), "{mode:?}");
            assert!(peak(&layer) > 0.1);
        }
    }

    #[test]
    fn nothing_to_build_gives_silence_of_the_same_shape() {
        let (kick, _) = render(RATE as usize / 2, 2);
        let layer = rumble_layer(&kick, RATE, &[], &RumbleParams::default());
        assert_eq!(layer.len(), 2);
        assert_eq!(layer[0].len(), kick[0].len());
        assert!(peak(&layer) == 0.0);
        let silent = vec![vec![0.0; 24_000]; 2];
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            assert!(peak(&rumble_layer(&silent, RATE, &[0], &params(mode))) == 0.0);
        }
        assert!(rumble_layer(&[], RATE, &[0], &RumbleParams::default()).is_empty());
    }

    #[test]
    fn a_hit_too_close_to_the_next_is_skipped_and_a_mono_kick_stays_mono() {
        let one = kick(24_000);
        let hits = [0, 100, 12_000]; // 100 frames is 2 ms: too short for a piece
        let layer = rumble_layer(&[one], RATE, &hits, &params(RumbleMode::Reverb));
        assert_eq!(layer.len(), 1);
        assert!(peak(&layer) > 0.0);
    }

    #[test]
    fn the_same_input_gives_the_same_layer() {
        let (kick, hits) = render(RATE as usize / 2, 2);
        for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
            let a = rumble_layer(&kick, RATE, &hits, &params(mode));
            let b = rumble_layer(&kick, RATE, &hits, &params(mode));
            assert!(a == b, "{mode:?}");
        }
    }

    /// 100 ms of a deterministic noise burst.
    fn burst() -> Vec<Vec<f32>> {
        let mut rng = crate::Rng::new(7);
        let l: Vec<f32> = (0..4_800)
            .map(|_| 0.6 * (rng.next_f32() * 2.0 - 1.0))
            .collect();
        vec![l.clone(), l]
    }

    #[test]
    fn the_swell_has_the_asked_length_and_rises_to_its_end() {
        for seconds in [0.5_f32, 2.0] {
            let swell = reverse_swell(&burst(), RATE, seconds);
            let n = (seconds * RATE as f32).round() as usize;
            assert_eq!(swell.len(), 2);
            assert_eq!(swell[0].len(), n);
            let tenth = n / 10;
            let (early, late) = (rms(&swell[0][..tenth]), rms(&swell[0][n - tenth..]));
            assert!(late > 4.0 * early, "{seconds}: {early} {late}");
            // The loudest sample is in the last tenth.
            let loudest = swell[0]
                .iter()
                .enumerate()
                .fold((0.0_f32, 0), |(m, mi), (i, s)| {
                    if s.abs() > m { (s.abs(), i) } else { (m, mi) }
                });
            assert!(
                loudest.1 >= n - n / 4,
                "{seconds}: loudest at {}",
                loudest.1
            );
        }
    }

    #[test]
    fn the_swell_is_set_to_a_share_of_the_source_peak() {
        let source = burst();
        let swell = reverse_swell(&source, RATE, 1.0);
        let ratio = peak(&swell) / peak(&source);
        assert!((ratio - SWELL_LEVEL).abs() < 0.01, "{ratio}");
        let quiet: Vec<Vec<f32>> = source
            .iter()
            .map(|c| c.iter().map(|s| s * 0.01).collect())
            .collect();
        let ratio = peak(&reverse_swell(&quiet, RATE, 1.0)) / peak(&quiet);
        assert!((ratio - SWELL_LEVEL).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn the_swell_of_a_mono_source_is_stereo_and_a_silent_one_is_silent() {
        let mono = vec![burst().remove(0)];
        let swell = reverse_swell(&mono, RATE, 1.0);
        assert_eq!(swell.len(), 2);
        assert!(peak(&swell) > 0.0);
        let silent = reverse_swell(&[vec![0.0; 2_000]], RATE, 1.0);
        assert!(peak(&silent) == 0.0 && silent[0].len() == RATE as usize);
        assert_eq!(reverse_swell(&[], RATE, 1.0)[0].len(), RATE as usize);
    }

    #[test]
    fn the_swell_follows_the_decay_of_the_reverb() {
        // The swell is a reverb tail played backwards: half a second before its end it is about
        // as far down as the tail is half a second after its start (30 dB at a decay of one
        // second), not silent and not as loud as the end.
        let swell = reverse_swell(&burst(), RATE, 1.0);
        let last_100ms = rms(&swell[0][RATE as usize - 4_800..]);
        let middle = rms(&swell[0][RATE as usize / 2 - 2_400..RATE as usize / 2 + 2_400]);
        assert!(middle > 0.005 * last_100ms, "{middle} {last_100ms}");
        assert!(middle < 0.2 * last_100ms, "{middle} {last_100ms}");
    }
}
