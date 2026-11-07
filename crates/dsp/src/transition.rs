//! The impact of a transition, offline: a hit of the song through saturation and a long reverb.
//!
//! [`impact`] runs on a worker over audio that was rendered already (the first beat of the next
//! section and a stretch of silence after it for the tail), never on the audio thread, so it
//! allocates freely. It is plain sums around the existing [`Room`], so the same samples give the
//! same result on every platform within the float tolerance of the room.
//!
//! * **Saturation.** The hit is driven into a `tanh` curve ([`DRIVE`] times its peak) and scaled
//!   back to its own peak: it keeps its level and gains weight, the harmonics a hit needs to
//!   read as a punctuation.
//! * **Reverb.** The saturated hit goes through the large [`Room`] at the asked decay. The wet
//!   signal is set to [`WET_LEVEL`] of the dry peak and added to the dry hit, which stays at the
//!   very start (the impact lands on the downbeat).
//! * **Tail.** The result is as long as the source. It is faded out over the last
//!   [`FADE_END_MS`] so the print never ends on a step, and kept below the source's peak
//!   times [`OUTPUT_HEADROOM`].

use crate::{Room, RoomRange};

/// Seed of the room.
const SEED: u64 = 0x494d_5041_4354;
/// The shortest and longest decay, in seconds.
pub const DECAY_RANGE: (f32, f32) = (0.5, 12.0);
/// How hard the hit is driven into the saturator, in times its peak.
pub const DRIVE: f32 = 4.0;
/// The peak of the reverb, as a share of the saturated hit's peak.
pub const WET_LEVEL: f32 = 0.9;
/// The peak of the impact, as a share of the source's peak.
pub const OUTPUT_HEADROOM: f32 = 1.0;
/// The fade at the end of the print, in ms.
const FADE_END_MS: f32 = 30.0;

fn peak(channels: &[Vec<f32>]) -> f32 {
    channels
        .iter()
        .flatten()
        .fold(0.0_f32, |m, s| m.max(s.abs()))
}

/// The impact of `source` (one or two channels at `rate`, equal lengths): the source saturated,
/// with a reverb of `decay` seconds, in two channels of the source's length. Silent for a silent
/// source. See the module docs.
pub fn impact(source: &[Vec<f32>], rate: u32, decay: f32) -> Vec<Vec<f32>> {
    let n = source.first().map_or(0, Vec::len);
    let mut out = vec![vec![0.0_f32; n]; 2];
    let source_peak = peak(source);
    if n == 0 || rate == 0 || source_peak <= 0.0 {
        return out;
    }
    // Saturated, back to the level of the source.
    let norm = (DRIVE).tanh();
    let gain = DRIVE / source_peak;
    let saturate = |x: f32| source_peak * (gain * x).tanh() / norm;
    let right = source.get(1).unwrap_or(&source[0]);
    let dry: Vec<[f32; 2]> = (0..n)
        .map(|i| [saturate(source[0][i]), saturate(right[i])])
        .collect();
    let dry_peak = dry.iter().flatten().fold(0.0_f32, |m, s| m.max(s.abs()));
    let mut room = Room::new(rate as f32, SEED, RoomRange::Large);
    room.set_size(0.7);
    room.set_damping(0.45);
    room.set_rt60_s(decay.clamp(DECAY_RANGE.0, DECAY_RANGE.1));
    let mut wet = vec![vec![0.0_f32; n]; 2];
    for (i, d) in dry.iter().enumerate() {
        let (l, r) = room.process(d[0], d[1]);
        wet[0][i] = l;
        wet[1][i] = r;
    }
    let wet_peak = peak(&wet);
    let scale = if wet_peak > 0.0 {
        WET_LEVEL * dry_peak / wet_peak
    } else {
        0.0
    };
    for (i, d) in dry.iter().enumerate() {
        out[0][i] = d[0] + scale * wet[0][i];
        out[1][i] = d[1] + scale * wet[1][i];
    }
    let top = peak(&out);
    if top > source_peak * OUTPUT_HEADROOM && top > 0.0 {
        let down = source_peak * OUTPUT_HEADROOM / top;
        for s in out.iter_mut().flatten() {
            *s *= down;
        }
    }
    let fade = ((FADE_END_MS * 0.001 * rate as f32) as usize).clamp(1, n);
    for c in &mut out {
        for j in 0..fade {
            c[n - 1 - j] *= (j as f32 + 0.5) / fade as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A 60 ms burst of noise and a second of silence after it.
    fn hit() -> Vec<Vec<f32>> {
        let mut rng = crate::Rng::new(11);
        let mut l: Vec<f32> = (0..2_880)
            .map(|i| 0.5 * (rng.next_f32() * 2.0 - 1.0) * (-(i as f32) / 900.0).exp())
            .collect();
        l.resize(2_880 + RATE as usize, 0.0);
        vec![l.clone(), l]
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn the_impact_has_the_length_of_the_source_in_two_channels() {
        let out = impact(&hit(), RATE, 3.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 2_880 + RATE as usize);
        assert_eq!(out[1].len(), out[0].len());
        // A mono source is made stereo.
        let mono = vec![hit().remove(0)];
        assert_eq!(impact(&mono, RATE, 3.0).len(), 2);
    }

    #[test]
    fn the_impact_rings_on_after_the_hit_has_died() {
        let source = hit();
        let out = impact(&source, RATE, 3.0);
        // The source is silent from 2 880 on; the impact is not.
        let after = &out[0][4_000..RATE as usize / 2];
        assert!(rms(after) > 1.0e-3, "rms {}", rms(after));
        assert!(rms(&source[0][4_000..]) < 1.0e-9);
    }

    #[test]
    fn the_saturation_adds_weight_without_going_over_the_source_peak() {
        let source = hit();
        let out = impact(&source, RATE, 1.0);
        let (top, top_in) = (peak(&out), peak(&source));
        assert!(top <= top_in * OUTPUT_HEADROOM + 1.0e-6, "{top} > {top_in}");
        // Driven hard, the hit's body is louder than the source's.
        assert!(rms(&out[0][..2_880]) > rms(&source[0][..2_880]));
    }

    #[test]
    fn the_print_ends_on_silence() {
        let out = impact(&hit(), RATE, 6.0);
        let last = out[0].len() - 1;
        assert!(out[0][last].abs() < 1.0e-3);
    }

    #[test]
    fn a_silent_or_empty_source_gives_silence() {
        let silent = vec![vec![0.0_f32; 4_800]; 2];
        assert!(
            impact(&silent, RATE, 3.0)
                .iter()
                .flatten()
                .all(|s| *s == 0.0)
        );
        let empty: Vec<Vec<f32>> = vec![Vec::new()];
        assert!(impact(&empty, RATE, 3.0)[0].is_empty());
    }

    #[test]
    fn the_same_input_gives_the_same_impact() {
        let a = impact(&hit(), RATE, 3.0);
        let b = impact(&hit(), RATE, 3.0);
        assert!(a == b);
    }
}
