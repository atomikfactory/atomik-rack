//! Sample rate, tempo and musical/sample time conversions.

use crate::PPQ;

/// Error for out-of-range time values.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum TimeError {
    /// Sample rate outside 8 kHz..=384 kHz.
    #[error("sample rate {0} Hz is outside 8000..=384000")]
    SampleRateOutOfRange(f64),
    /// Tempo outside 20..=999 BPM.
    #[error("tempo {0} BPM is outside 20..=999")]
    TempoOutOfRange(f64),
}

/// Audio sample rate in Hz. Invariant: 8 000 ≤ rate ≤ 384 000.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct SampleRate(f64);

impl SampleRate {
    /// Creates a sample rate, rejecting values outside 8 kHz..=384 kHz (and NaN).
    pub fn new(hz: f64) -> Result<Self, TimeError> {
        if (8_000.0..=384_000.0).contains(&hz) {
            Ok(Self(hz))
        } else {
            Err(TimeError::SampleRateOutOfRange(hz))
        }
    }

    /// The rate in Hz.
    pub fn hz(self) -> f64 {
        self.0
    }
}

/// Tempo in beats (quarter notes) per minute. Invariant: 20 ≤ bpm ≤ 999.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Tempo(f64);

impl Tempo {
    /// Creates a tempo, rejecting values outside 20..=999 BPM (and NaN).
    pub fn new(bpm: f64) -> Result<Self, TimeError> {
        if (20.0..=999.0).contains(&bpm) {
            Ok(Self(bpm))
        } else {
            Err(TimeError::TempoOutOfRange(bpm))
        }
    }

    /// The tempo in BPM.
    pub fn bpm(self) -> f64 {
        self.0
    }

    /// Number of sample frames per tick at `sr`.
    pub fn samples_per_tick(self, sr: SampleRate) -> f64 {
        sr.hz() * 60.0 / (self.0 * PPQ as f64)
    }
}

/// Musical position in ticks ([`PPQ`] ticks per quarter note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ticks(pub i64);

/// Transport information handed to devices with every batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeInfo {
    /// Absolute frame index of the first sample of the batch.
    pub frame: u64,
    /// Current sample rate.
    pub sample_rate: SampleRate,
    /// Tempo at the first frame of the batch (inside a ramp, the instantaneous tempo there).
    pub tempo: Tempo,
    /// Whether the transport is playing.
    pub playing: bool,
    /// Transport position in ticks ([`PPQ`] per quarter) at the first frame of the batch
    /// (fractional). While stopped it stays at the playhead; without a song it advances with
    /// the engine at the engine tempo.
    pub tick: f64,
    /// Ticks per sample frame in this batch (constant inside a batch; the tempo map's tempo at its first frame).
    pub ticks_per_frame: f64,
    /// Set when the transport jumps back to the loop start inside this batch.
    pub loop_wrap: Option<LoopWrap>,
    /// The enabled loop region as `(start, end)` in ticks, `None` when the
    /// loop is off or there is no song.
    pub loop_ticks: Option<(f64, f64)>,
    /// Time signature `(numerator, denominator)` in effect at `tick`; 4/4 without a song.
    pub time_signature: (u8, u8),
    /// The master tune in cents around A = 440 Hz for this batch (the project value, glided by
    /// the engine when it changes). 0 outside a project. Pitched devices shift by it.
    pub tune_cents: f32,
}

/// A loop wrap inside one batch (see [`TimeInfo::loop_wrap`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopWrap {
    /// Frame offset inside the batch at which the transport is already back at `to_tick`.
    pub offset: u16,
    /// The tick the transport jumps to.
    pub to_tick: f64,
}

impl TimeInfo {
    /// A `TimeInfo` for a constant tempo whose tick is derived from the frame (tick 0 at
    /// frame 0) with no loop wrap. Used by tests, fakes and song-less hosts.
    pub fn at(frame: u64, sample_rate: SampleRate, tempo: Tempo, playing: bool) -> Self {
        let ticks_per_frame = tempo.bpm() * PPQ as f64 / (60.0 * sample_rate.hz());
        Self {
            frame,
            sample_rate,
            tempo,
            playing,
            tick: frame as f64 * ticks_per_frame,
            ticks_per_frame,
            loop_wrap: None,
            loop_ticks: None,
            time_signature: (4, 4),
            tune_cents: 0.0,
        }
    }
}

/// Converts ticks to (fractional) sample frames at a constant tempo.
pub fn ticks_to_frames(ticks: Ticks, tempo: Tempo, sr: SampleRate) -> f64 {
    ticks.0 as f64 * tempo.samples_per_tick(sr)
}

/// Converts quarter-note beats to (fractional) sample frames at a constant tempo.
pub fn beats_to_frames(beats: f64, tempo: Tempo, sr: SampleRate) -> f64 {
    beats * sr.hz() * 60.0 / tempo.bpm()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn samplerate_new_out_of_range_errors() {
        assert!(SampleRate::new(7_999.0).is_err());
        assert!(SampleRate::new(384_001.0).is_err());
        assert!(SampleRate::new(f64::NAN).is_err());
        assert!(close(SampleRate::new(48_000.0).unwrap().hz(), 48_000.0));
    }

    #[test]
    fn tempo_new_out_of_range_errors() {
        assert!(Tempo::new(19.9).is_err());
        assert!(Tempo::new(999.1).is_err());
        assert!(Tempo::new(f64::NAN).is_err());
        assert!(close(Tempo::new(120.0).unwrap().bpm(), 120.0));
    }

    #[test]
    fn time_info_at_derives_the_tick_from_the_frame() {
        let sr = SampleRate::new(48_000.0).unwrap();
        let t = TimeInfo::at(48_000, sr, Tempo::new(120.0).unwrap(), true);
        assert_eq!((t.frame, t.sample_rate, t.playing), (48_000, sr, true));
        assert_eq!(t.loop_wrap, None);
        // 120 BPM = 2 beats per second: one second is two quarters.
        assert!(close(t.tick, 2.0 * PPQ as f64), "{}", t.tick);
        assert!(close(t.ticks_per_frame, 0.64), "{}", t.ticks_per_frame);
        let z = TimeInfo::at(0, sr, Tempo::new(97.0).unwrap(), false);
        assert_eq!((z.tick, z.playing), (0.0, false));
        assert!(close(
            z.ticks_per_frame * Tempo::new(97.0).unwrap().samples_per_tick(sr),
            1.0
        ));
    }

    #[test]
    fn time_info_at_has_no_loop_region_and_is_four_four() {
        let sr = SampleRate::new(48_000.0).unwrap();
        let t = TimeInfo::at(0, sr, Tempo::new(120.0).unwrap(), true);
        assert_eq!(t.loop_ticks, None);
        assert_eq!(t.time_signature, (4, 4));
        let with_loop = TimeInfo {
            loop_ticks: Some((0.0, 4.0 * PPQ as f64)),
            time_signature: (7, 8),
            ..t
        };
        assert_eq!(with_loop.loop_ticks, Some((0.0, 61_440.0)));
        assert_eq!(with_loop.time_signature, (7, 8));
    }

    #[test]
    fn loop_wrap_is_a_plain_copyable_value() {
        let w = LoopWrap {
            offset: 12,
            to_tick: 480.0,
        };
        let v = w;
        assert_eq!(v, w);
    }

    #[test]
    fn beats_to_frames_120bpm_48k_is_24000() {
        let f = beats_to_frames(
            1.0,
            Tempo::new(120.0).unwrap(),
            SampleRate::new(48_000.0).unwrap(),
        );
        assert!(close(f, 24_000.0), "{f}");
    }

    #[test]
    fn ticks_to_frames_one_quarter_equals_one_beat() {
        let (t, sr) = (
            Tempo::new(97.0).unwrap(),
            SampleRate::new(44_100.0).unwrap(),
        );
        assert!(close(
            ticks_to_frames(Ticks(PPQ), t, sr),
            beats_to_frames(1.0, t, sr)
        ));
        assert!(close(
            t.samples_per_tick(sr) * PPQ as f64,
            beats_to_frames(1.0, t, sr)
        ));
    }
}
