//! Content hash of decoded audio.

use sha2::{Digest, Sha256};

use crate::data::AudioData;

/// Lowercase SHA-256 hex over a canonical encoding of the audio.
///
/// Encoding (all little-endian, no padding): `u32 sample_rate`, `u32 channel_count`,
/// `u64 frames`, then every sample as `f32::to_bits` bytes, channel by channel (planar, channel 0
/// first). Raw bits are hashed, so `-0.0` differs from `0.0` and NaN payloads count. The value
/// is identical on every platform.
pub fn content_hash(data: &AudioData) -> String {
    let mut h = ContentHasher::new(
        data.sample_rate,
        data.channels.len() as u32,
        data.frames() as u64,
    );
    for ch in &data.channels {
        h.update(ch);
    }
    h.finish()
}

/// [`content_hash`] fed piece by piece: the header, then the samples of channel 0 in order, then
/// those of channel 1, and so on, in pieces of any size. Gives the value [`content_hash`] gives
/// for the same audio.
pub(crate) struct ContentHasher {
    h: Sha256,
    buf: Vec<u8>,
}

impl ContentHasher {
    /// Starts the hash of audio of `frames` frames of `channels` channels at `sample_rate`.
    pub(crate) fn new(sample_rate: u32, channels: u32, frames: u64) -> Self {
        let mut h = Sha256::new();
        h.update(sample_rate.to_le_bytes());
        h.update(channels.to_le_bytes());
        h.update(frames.to_le_bytes());
        Self {
            h,
            buf: Vec::with_capacity(16 * 1024),
        }
    }

    /// Adds the next samples of the current channel (raw bits).
    pub(crate) fn update(&mut self, samples: &[f32]) {
        for chunk in samples.chunks(4096) {
            self.buf.clear();
            for s in chunk {
                self.buf.extend_from_slice(&s.to_bits().to_le_bytes());
            }
            self.h.update(&self.buf);
        }
    }

    /// The lowercase hex digest.
    pub(crate) fn finish(self) -> String {
        let digest = self.h.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AudioData {
        AudioData::new(44_100, vec![vec![0.0, 1.0, -1.0], vec![0.5, 0.25, -0.5]]).unwrap()
    }

    /// Known vector, computed independently with Python `hashlib` and `struct`:
    /// `u32 44100 | u32 2 | u64 3 | f32le samples channel by channel`.
    #[test]
    fn known_vector() {
        assert_eq!(
            content_hash(&sample()),
            "ec9e4aab8b45c3a0cb02e43ad8acc4b45cfee63cff371740e4539e70de3f5d4c"
        );
    }

    #[test]
    fn rate_channels_and_samples_all_change_the_hash() {
        let base = content_hash(&sample());
        let mut other_rate = sample();
        other_rate.sample_rate = 48_000;
        assert_ne!(base, content_hash(&other_rate));
        let mut other_sample = sample();
        other_sample.channels[1][2] = -0.25;
        assert_ne!(base, content_hash(&other_sample));
        // Same 6 samples laid out as 1 channel x 6 frames must differ.
        let flat = AudioData::new(44_100, vec![vec![0.0, 1.0, -1.0, 0.5, 0.25, -0.5]]).unwrap();
        assert_ne!(base, content_hash(&flat));
    }

    #[test]
    fn is_lowercase_hex_64() {
        let h = content_hash(&sample());
        assert_eq!(h.len(), 64);
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
}
