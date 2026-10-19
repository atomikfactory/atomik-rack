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
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Result<Self> {
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
        Ok(Self {
            sample_rate,
            channels,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

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
