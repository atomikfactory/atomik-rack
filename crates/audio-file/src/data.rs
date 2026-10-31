//! The decoded audio container.

use crate::error::{AudioFileError, Result};

/// Planar audio at one sample rate. Every channel has the same length.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioData {
    /// Sample rate in Hz (non-zero).
    pub sample_rate: u32,
    /// One `Vec` per channel, all of equal length.
    pub channels: Vec<Vec<f32>>,
}

impl AudioData {
    /// Validating constructor: at least one channel, a non-zero rate, equal channel lengths.
    /// Zero frames are allowed here (a recording that was stopped at once); decoding rejects them.
    ///
    /// A sample that is NaN or infinite becomes silence (`0.0`): such a value reaches the mixer
    /// and the meters as an undefined result. [`Self::new_counted`] also says how many there
    /// were; samples that are already finite are never touched.
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Result<Self> {
        Self::new_counted(sample_rate, channels).map(|(data, _)| data)
    }

    /// [`Self::new`] that also returns the number of non-finite samples it replaced with `0.0`.
    pub fn new_counted(sample_rate: u32, mut channels: Vec<Vec<f32>>) -> Result<(Self, u64)> {
        if sample_rate == 0 {
            return Err(AudioFileError::InvalidData("sample rate is zero".into()));
        }
        let Some(first) = channels.first() else {
            return Err(AudioFileError::InvalidData("no channels".into()));
        };
        if u16::try_from(channels.len()).is_err() {
            return Err(AudioFileError::InvalidData("too many channels".into()));
        }
        if channels.iter().any(|c| c.len() != first.len()) {
            return Err(AudioFileError::InvalidData("channel lengths differ".into()));
        }
        let replaced = sanitize(&mut channels);
        Ok((
            Self {
                sample_rate,
                channels,
            },
            replaced,
        ))
    }

    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Checks the invariants of a value built through the public fields.
    pub fn validate(&self) -> Result<()> {
        Self::new(self.sample_rate, self.channels.clone()).map(|_| ())
    }
}

/// Replaces every NaN and infinity with `0.0`; returns how many there were.
pub(crate) fn sanitize(channels: &mut [Vec<f32>]) -> u64 {
    let mut replaced = 0;
    for sample in channels.iter_mut().flatten() {
        if !sample.is_finite() {
            *sample = 0.0;
            replaced += 1;
        }
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_samples_become_silence_and_are_counted() {
        let channels = vec![
            vec![0.5, f32::NAN, -0.0],
            vec![f32::INFINITY, f32::NEG_INFINITY, 1.0],
        ];
        let (d, replaced) = AudioData::new_counted(48_000, channels).unwrap();
        assert_eq!(replaced, 3);
        assert_eq!(d.channels[0][0].to_bits(), 0.5f32.to_bits());
        assert_eq!(d.channels[0][1].to_bits(), 0);
        assert_eq!(
            d.channels[0][2].to_bits(),
            (-0.0f32).to_bits(),
            "finite is untouched"
        );
        assert_eq!(d.channels[1], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn constructor_accepts_equal_channels() {
        let d = AudioData::new(44_100, vec![vec![0.0; 5], vec![1.0; 5]]).unwrap();
        assert_eq!(
            (d.frames(), d.channel_count(), d.sample_rate),
            (5, 2, 44_100)
        );
    }

    #[test]
    fn constructor_rejects_bad_input() {
        assert!(matches!(
            AudioData::new(0, vec![vec![0.0]]),
            Err(AudioFileError::InvalidData(_))
        ));
        assert!(matches!(
            AudioData::new(48_000, vec![]),
            Err(AudioFileError::InvalidData(_))
        ));
        assert!(matches!(
            AudioData::new(48_000, vec![vec![0.0; 3], vec![0.0; 4]]),
            Err(AudioFileError::InvalidData(_))
        ));
    }

    #[test]
    fn zero_frames_are_allowed_and_counted() {
        let d = AudioData::new(48_000, vec![vec![]]).unwrap();
        assert_eq!(d.frames(), 0);
    }

    #[test]
    fn validate_catches_public_field_mutation() {
        let mut d = AudioData::new(48_000, vec![vec![0.0; 3], vec![0.0; 3]]).unwrap();
        d.channels[1].push(0.0);
        assert!(d.validate().is_err());
    }
}
