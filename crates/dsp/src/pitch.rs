//! Pitch conversion (12-TET, A4 = key 69 = 440 Hz).

/// Frequency in Hz of MIDI `key` (may be fractional) plus `cents_offset` cents.
#[inline]
pub fn note_to_hz(key: f32, cents_offset: f32) -> f32 {
    440.0 * ((key - 69.0 + cents_offset * 0.01) * (1.0 / 12.0)).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_to_hz_a4_is_440() {
        assert!((note_to_hz(69.0, 0.0) - 440.0).abs() < 1e-3);
        assert!((note_to_hz(81.0, 0.0) - 880.0).abs() < 1e-3);
        assert!((note_to_hz(60.0, 0.0) - 261.625_6).abs() < 1e-2);
        assert!((note_to_hz(69.0, 100.0) - note_to_hz(70.0, 0.0)).abs() < 1e-3);
        assert!((note_to_hz(69.0, -1200.0) - 220.0).abs() < 1e-3);
    }
}
