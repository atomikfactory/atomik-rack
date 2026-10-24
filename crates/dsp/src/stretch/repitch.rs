//! Repitch: the source read at the speed the map asks for with Hermite interpolation, so speed
//! and pitch change together. It is also the resampler that turns a stretched signal into a
//! pitch shift.

use super::map::TimeMap;
use crate::read_hermite;

/// `out[t] = source[map.src_at(t)]` for `t` in `0..out_frames`; frames whose source position
/// lies outside the source are silent.
pub(crate) fn repitch(
    source: &[&[f32]],
    map: &TimeMap,
    out_frames: usize,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let mut out: Vec<Vec<f32>> = source.iter().map(|_| vec![0.0; out_frames]).collect();
    let len = source.first().map_or(0, |c| c.len()) as f64;
    for t in 0..out_frames {
        if t.is_multiple_of(4096) && cancel() {
            return None;
        }
        let pos = map.src_at(t as f64);
        if !(pos >= 0.0 && pos < len) {
            continue;
        }
        let whole = pos.floor();
        let frac = pos - whole;
        for (o, ch) in out.iter_mut().zip(source) {
            o[t] = read_hermite(ch, whole as u64, frac);
        }
    }
    Some(out)
}

/// `out[t] = signal[t * ratio]`: the signal played `ratio` times as fast (pitch times `ratio`),
/// `out_frames` long.
pub(crate) fn resample_by(signal: &[Vec<f32>], ratio: f64, out_frames: usize) -> Vec<Vec<f32>> {
    signal
        .iter()
        .map(|ch| {
            let len = ch.len() as f64;
            (0..out_frames)
                .map(|t| {
                    let pos = t as f64 * ratio;
                    if !(pos >= 0.0 && pos < len) {
                        return 0.0;
                    }
                    let whole = pos.floor();
                    read_hermite(ch, whole as u64, pos - whole)
                })
                .collect()
        })
        .collect()
}
