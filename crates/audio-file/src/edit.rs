//! Offline sample edits: pure functions from audio to new audio.
//!
//! The sample editor never changes an asset (assets are immutable and named by their content).
//! An edit reads the audio of an asset, applies one of these functions to a frame range and
//! stores the result as a new asset; the zones that played the old one are pointed at the new
//! one in a single undo step. Every function takes the range `[from, to)` in frames, leaves the
//! input alone and allocates freely: they run on a worker, never on the audio thread.
//!
//! * [`crop`] keeps only the range.
//! * [`normalise`] scales the whole sample so the range's peak reaches a level.
//! * [`reverse`] mirrors the range in time.
//! * [`fade_in`] / [`fade_out`] ramp the range's gain between silence and unity.

use crate::{AudioData, AudioFileError, Result};

/// The shape of a fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FadeShape {
    /// Gain moves in a straight line.
    #[default]
    Linear,
    /// Gain follows a quarter sine: constant power against a mirrored fade.
    EqualPower,
}

impl FadeShape {
    /// The gain at position `x` in `0..=1` of a fade that rises from 0 to 1.
    pub fn gain(self, x: f32) -> f32 {
        match self {
            Self::Linear => x,
            Self::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
        }
    }
}

/// Checks `[from, to)` against the audio and returns it as indices.
fn span(data: &AudioData, from: u64, to: u64) -> Result<(usize, usize)> {
    let frames = data.frames() as u64;
    if from >= to || to > frames {
        return Err(AudioFileError::InvalidData(format!(
            "the range {from}..{to} is not inside the {frames} frames of the sample"
        )));
    }
    Ok((from as usize, to as usize))
}

fn rebuild(data: &AudioData, channels: Vec<Vec<f32>>) -> Result<AudioData> {
    AudioData::new(data.sample_rate, channels)
}

/// The range `[from, to)` as a sample of its own.
pub fn crop(data: &AudioData, from: u64, to: u64) -> Result<AudioData> {
    let (a, b) = span(data, from, to)?;
    rebuild(
        data,
        data.channels.iter().map(|c| c[a..b].to_vec()).collect(),
    )
}

/// The largest absolute sample of `[from, to)` over every channel.
pub fn range_peak(data: &AudioData, from: u64, to: u64) -> Result<f32> {
    let (a, b) = span(data, from, to)?;
    Ok(data
        .channels
        .iter()
        .flat_map(|c| c[a..b].iter())
        .fold(0.0f32, |m, s| m.max(s.abs())))
}

/// The whole sample scaled by the gain that takes the peak of `[from, to)` to `target_db` dBFS.
/// A range of silence has no gain to find: it is an error rather than a sample of `inf`.
pub fn normalise(data: &AudioData, from: u64, to: u64, target_db: f32) -> Result<AudioData> {
    let peak = range_peak(data, from, to)?;
    if !(peak > 0.0 && peak.is_finite() && target_db.is_finite()) {
        return Err(AudioFileError::InvalidData(
            "the selection is silent: there is nothing to normalise".into(),
        ));
    }
    let gain = 10.0f32.powf(target_db / 20.0) / peak;
    rebuild(
        data,
        data.channels
            .iter()
            .map(|c| c.iter().map(|s| s * gain).collect())
            .collect(),
    )
}

/// The sample with `[from, to)` played backwards.
pub fn reverse(data: &AudioData, from: u64, to: u64) -> Result<AudioData> {
    let (a, b) = span(data, from, to)?;
    let mut channels = data.channels.clone();
    for c in &mut channels {
        c[a..b].reverse();
    }
    rebuild(data, channels)
}

/// The sample with the gain of `[from, to)` rising from silence to unity.
pub fn fade_in(data: &AudioData, from: u64, to: u64, shape: FadeShape) -> Result<AudioData> {
    fade(data, from, to, shape, false)
}

/// The sample with the gain of `[from, to)` falling from unity to silence.
pub fn fade_out(data: &AudioData, from: u64, to: u64, shape: FadeShape) -> Result<AudioData> {
    fade(data, from, to, shape, true)
}

fn fade(data: &AudioData, from: u64, to: u64, shape: FadeShape, out: bool) -> Result<AudioData> {
    let (a, b) = span(data, from, to)?;
    let n = (b - a) as f32;
    let mut channels = data.channels.clone();
    for c in &mut channels {
        for (i, s) in c[a..b].iter_mut().enumerate() {
            // The first frame of a fade-in is silent and the last of a fade-out is.
            let x = if n > 1.0 { i as f32 / (n - 1.0) } else { 1.0 };
            *s *= shape.gain(if out { 1.0 - x } else { x });
        }
    }
    rebuild(data, channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> AudioData {
        let l: Vec<f32> = (0..10).map(|i| i as f32 / 10.0).collect();
        let r: Vec<f32> = l.iter().map(|s| -s).collect();
        AudioData::new(48_000, vec![l, r]).unwrap()
    }

    #[test]
    fn crop_keeps_the_range_and_refuses_a_bad_one() {
        let d = ramp();
        let c = crop(&d, 2, 5).unwrap();
        assert_eq!(c.channels[0], d.channels[0][2..5]);
        assert_eq!(c.channels[1], d.channels[1][2..5]);
        assert_eq!(c.sample_rate, 48_000);
        assert!(crop(&d, 5, 5).is_err());
        assert!(crop(&d, 0, 11).is_err());
    }

    #[test]
    fn reverse_mirrors_only_the_range() {
        let d = ramp();
        let r = reverse(&d, 2, 6).unwrap();
        let mut expect = d.channels[0].clone();
        expect[2..6].reverse();
        assert_eq!(r.channels[0], expect);
        assert_eq!(reverse(&r, 2, 6).unwrap(), d, "twice is the identity");
    }

    #[test]
    fn normalise_scales_the_whole_sample_by_the_peak_of_the_range() {
        let d = ramp();
        // The peak of frames 0..5 is 0.4: -6.0206 dB is a gain of 0.5, so x1.25 for 0.5/0.4.
        let n = normalise(&d, 0, 5, -6.0206).unwrap();
        let gain = 0.5 / 0.4;
        for (c, o) in n.channels.iter().zip(&d.channels) {
            for (a, b) in c.iter().zip(o) {
                assert!((a - b * gain).abs() < 1e-4, "{a} vs {}", b * gain);
            }
        }
        let silent = AudioData::new(48_000, vec![vec![0.0; 4]]).unwrap();
        assert!(normalise(&silent, 0, 4, 0.0).is_err());
    }

    #[test]
    fn fades_ramp_the_range_and_leave_the_rest() {
        let d = AudioData::new(48_000, vec![vec![1.0; 8]]).unwrap();
        let f = fade_in(&d, 2, 6, FadeShape::Linear).unwrap();
        let expect = [1.0, 1.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0];
        for (a, b) in f.channels[0].iter().zip(expect) {
            assert!((a - b).abs() < 1e-6);
        }
        let o = fade_out(&d, 2, 6, FadeShape::Linear).unwrap();
        let expect = [1.0, 1.0, 1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0, 1.0, 1.0];
        for (a, b) in o.channels[0].iter().zip(expect) {
            assert!((a - b).abs() < 1e-6);
        }
        let p = fade_in(&d, 0, 8, FadeShape::EqualPower).unwrap();
        assert!(p.channels[0][0].abs() < 1e-6 && (p.channels[0][7] - 1.0).abs() < 1e-6);
        // Constant power against the mirrored fade.
        let q = fade_out(&d, 0, 8, FadeShape::EqualPower).unwrap();
        for i in 0..8 {
            let power = p.channels[0][i].powi(2) + q.channels[0][i].powi(2);
            assert!((power - 1.0).abs() < 1e-5);
        }
    }
}
