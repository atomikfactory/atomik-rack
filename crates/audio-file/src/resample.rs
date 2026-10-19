//! High-quality sample-rate conversion (rubato sinc).
//!
//! Settings (fixed, so results never depend on the caller): asynchronous windowed-sinc resampler
//! in `f64`, `sinc_len` 256, Blackman-Harris-2 window, automatic cut-off, oversampling 256,
//! cubic interpolation, chunks of 1024 input frames. The whole clip is converted in one call.
//!
//! Latency compensation. Measured and derived from rubato's indexing: with `sinc_len` 256 an
//! input event at time `x` appears at output frame `x * r + 128 * r - 1`, where
//! `r = target / source`. `Resampler::process_all` already drops `output_delay()` =
//! `floor(128 * r)` leading frames, which leaves a fractional error of up to one output frame.
//! To make the alignment exact this module prepends `P` zero frames so that
//! `(128 + P) * r` is an integer (`P` is the smallest value >= 2 / r with
//! `(128 + P) * a = 0 mod b`, for `r = a / b` in lowest terms), then drops the remaining integer
//! number of leading frames. Output frame `n` then corresponds to input time `n / r` exactly,
//! up to the resampler's filter accuracy. Pathological rate pairs that would need more than
//! 65 536 padding frames fall back to rounding the delay (error at most half an output frame).
//! The clip is also followed by zeros so that the tail is flushed, and the result is cut to
//! `round(frames * target / source)` frames.

use rubato::audioadapter_buffers::owned::SequentialOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::data::AudioData;
use crate::error::{AudioFileError, Result};

/// Input frames per resampler chunk.
const CHUNK: usize = 1024;
/// Half the sinc length: `sinc_len / 2`.
const HALF: u128 = 128;
/// Largest leading pad before falling back to a rounded delay.
const MAX_PAD: u128 = 65_536;
/// Zero frames appended so that the filter tail is flushed.
const TAIL: usize = 256;

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Returns `(pad, drop)`: leading zero frames to prepend and output frames to drop after
/// rubato's own trimming.
fn alignment(src: u128, dst: u128, rubato_trim: usize) -> (usize, usize) {
    let g = gcd(src, dst);
    let (a, b) = (dst / g, src / g); // r = a / b
    // Smallest pad with P * r >= 2 (keeps the extra drop non-negative) and (128 + P) * r integer.
    let min_pad = (2 * b).div_ceil(a);
    let mut pad = (b - HALF % b) % b;
    while pad < min_pad {
        pad += b;
    }
    if pad > MAX_PAD {
        pad = min_pad;
    }
    let num = (HALF + pad) * a;
    // round(num / b) - 1; exact when b divides num.
    let delay = ((2 * num + b) / (2 * b)) as usize - 1;
    (pad as usize, delay - rubato_trim)
}

/// Converts `data` to `target_rate`. A no-op copy when the rates are equal. The output has
/// `round(frames * target / source)` frames and is time-aligned with the input (no delay).
pub fn resample(data: &AudioData, target_rate: u32) -> Result<AudioData> {
    data.validate()?;
    if target_rate == 0 {
        return Err(AudioFileError::InvalidData(
            "target sample rate is zero".into(),
        ));
    }
    if data.sample_rate == target_rate {
        return Ok(data.clone());
    }
    let frames = data.frames();
    let nch = data.channel_count();
    let (src, dst) = (u128::from(data.sample_rate), u128::from(target_rate));
    let out_len = ((frames as u128 * dst * 2 + src) / (2 * src)) as usize;
    if frames == 0 {
        return AudioData::new(target_rate, vec![Vec::new(); nch]);
    }

    let params = SincInterpolationParameters::new(256, WindowFunction::BlackmanHarris2)
        .oversampling_factor(256)
        .interpolation(SincInterpolationType::Cubic);
    let ratio = f64::from(target_rate) / f64::from(data.sample_rate);
    let mut resampler = Async::<f64>::new_sinc(ratio, 1.0, &params, CHUNK, nch, FixedAsync::Input)
        .map_err(|e| AudioFileError::InvalidData(format!("resampler setup: {e}")))?;

    let (pad, extra_drop) = alignment(src, dst, resampler.output_delay());
    let padded = pad + frames + TAIL;
    let mut planar = Vec::with_capacity(nch * padded);
    for ch in &data.channels {
        planar.resize(planar.len() + pad, 0.0);
        planar.extend(ch.iter().map(|s| f64::from(*s)));
        planar.resize(planar.len() + TAIL, 0.0);
    }
    let input = SequentialOwned::new_from(planar, nch, padded)
        .map_err(|e| AudioFileError::InvalidData(format!("resampler input: {e}")))?;
    let output = resampler
        .process_all(&input, padded, None)
        .map_err(|e| AudioFileError::Decode(format!("resampling failed: {e}")))?;
    let interleaved = output.take_data();

    let mut channels = vec![Vec::with_capacity(out_len); nch];
    for (c, out) in channels.iter_mut().enumerate() {
        out.extend(
            interleaved
                .iter()
                .skip(c)
                .step_by(nch)
                .skip(extra_drop)
                .take(out_len)
                .map(|s| *s as f32),
        );
        out.resize(out_len, 0.0);
    }
    AudioData::new(target_rate, channels)
}
