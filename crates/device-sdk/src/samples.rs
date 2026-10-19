//! Sample maps for sample-playing devices.
//!
//! A [`SampleMap`] is built off the RT thread, validated once, and handed to a device through
//! [`crate::Device::swap_samples`]. It is never mutated afterwards; a change is a new map that
//! shares the unchanged [`SampleBuffer`]s by `Arc`.

use std::sync::Arc;

/// Planar, immutable sample data at the session rate.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleBuffer {
    /// One `Vec` per channel, all `frames` long (shared with the engine's song assets).
    pub data: Arc<[Vec<f32>]>,
    /// Frames per channel.
    pub frames: u64,
    /// Channel count (>= 1).
    pub channels: u16,
}

impl SampleBuffer {
    /// A buffer from planar data. `frames` and `channels` are taken from `data`.
    ///
    /// # Errors
    /// [`SampleMapError::EmptyBuffer`] without channels, [`SampleMapError::RaggedBuffer`] when
    /// the channels differ in length.
    pub fn new(data: Arc<[Vec<f32>]>) -> Result<Self, SampleMapError> {
        let first = data.first().ok_or(SampleMapError::EmptyBuffer)?.len();
        if data.iter().any(|c| c.len() != first) {
            return Err(SampleMapError::RaggedBuffer);
        }
        Ok(Self {
            frames: first as u64,
            channels: u16::try_from(data.len()).unwrap_or(u16::MAX),
            data,
        })
    }
}

/// How a zone loops while its note is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    /// Play from `start` to `end` once.
    #[default]
    Off,
    /// Wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Play `loop_start..loop_end` forwards, then backwards, and so on.
    PingPong,
}

/// What a note-off does to a zone's voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerMode {
    /// Note-off starts the release.
    #[default]
    Gate,
    /// Note-off is ignored; the voice plays to `end` (or until stolen or all-notes-off).
    OneShot,
}

/// Lowest and highest transpose, in semitones.
pub const ZONE_TRANSPOSE_RANGE: (i8, i8) = (-48, 48);
/// Lowest and highest fine tune, in cents.
pub const ZONE_TUNE_RANGE: (f32, f32) = (-100.0, 100.0);
/// Lowest and highest zone gain, in dB.
pub const ZONE_GAIN_RANGE: (f32, f32) = (-60.0, 12.0);

/// One zone of a [`SampleMap`]. All fields are checked by [`SampleMap::new`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// Index into [`SampleMap::buffers`].
    pub buffer: u32,
    /// Lowest key, 0..=127.
    pub key_lo: u8,
    /// Highest key, `key_lo..=127`.
    pub key_hi: u8,
    /// Lowest velocity, 1..=127. A note velocity `v` in (0, 1] maps to
    /// [`velocity_to_7bit`]`(v)`.
    pub vel_lo: u8,
    /// Highest velocity, `vel_lo..=127`.
    pub vel_hi: u8,
    /// The key at which the sample plays at its recorded pitch, 0..=127.
    pub root_key: u8,
    /// Transpose in semitones, [`ZONE_TRANSPOSE_RANGE`].
    pub transpose: i8,
    /// Fine tune in cents, [`ZONE_TUNE_RANGE`].
    pub tune_cents: f32,
    /// Gain in dB, [`ZONE_GAIN_RANGE`].
    pub gain_db: f32,
    /// Pan, −1 (left) ..= 1 (right).
    pub pan: f32,
    /// First frame played.
    pub start: u64,
    /// End frame (exclusive): `start < end <= buffer frames`.
    pub end: u64,
    /// Loop mode.
    pub loop_mode: LoopMode,
    /// Loop start frame. When looping: `start <= loop_start < loop_end <= end`.
    pub loop_start: u64,
    /// Loop end frame (exclusive).
    pub loop_end: u64,
    /// Crossfade length in frames. When looping: `crossfade <= loop_end - loop_start` and
    /// `crossfade <= loop_start - start` (the fade reads the audio before the loop start).
    /// Ignored when the loop is off.
    pub crossfade: u64,
    /// Note-off behaviour.
    pub trigger: TriggerMode,
    /// Plays the zone backwards: from `end - 1` down to `start`, ending (with a short fade)
    /// at `start`. A reversed zone never loops: `loop_mode`, `loop_start`, `loop_end` and
    /// `crossfade` are ignored and are not validated. The loop rules of [`SampleMap::new`] apply
    /// only when `loop_mode != Off && !reverse`; `start`/`end` are always validated.
    pub reverse: bool,
}

impl Zone {
    /// Whether the zone sounds for `key` at 7-bit velocity `vel7`.
    pub fn contains(&self, key: u8, vel7: u8) -> bool {
        (self.key_lo..=self.key_hi).contains(&key) && (self.vel_lo..=self.vel_hi).contains(&vel7)
    }
}

/// Maps a note velocity in (0, 1] to 1..=127 (`round(v * 127)`, clamped; NaN gives 1).
pub fn velocity_to_7bit(velocity: f32) -> u8 {
    let v = (velocity * 127.0).round();
    if v.is_nan() || v < 1.0 {
        1
    } else if v > 127.0 {
        127
    } else {
        v as u8
    }
}

/// A zone or buffer that breaks the [`SampleMap`] invariants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SampleMapError {
    /// A buffer has no channels.
    #[error("sample buffer has no channels")]
    EmptyBuffer,
    /// A buffer's channels differ in length.
    #[error("sample buffer channels differ in length")]
    RaggedBuffer,
    /// A zone names a buffer that does not exist.
    #[error("zone {zone} uses buffer {buffer}, but the map has {buffers}")]
    NoSuchBuffer {
        /// Zone index.
        zone: usize,
        /// The buffer index it names.
        buffer: u32,
        /// Number of buffers.
        buffers: usize,
    },
    /// A key, velocity, tune, gain or pan field is out of range or not finite.
    #[error("zone {zone}: `{field}` is out of range")]
    OutOfRange {
        /// Zone index.
        zone: usize,
        /// Field name.
        field: &'static str,
    },
    /// The start/end or loop frames break the ordering rules.
    #[error("zone {zone}: `{field}` breaks the frame ordering")]
    BadFrames {
        /// Zone index.
        zone: usize,
        /// Field name.
        field: &'static str,
    },
}

/// Buffers and zones for one device. Immutable once built.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleMap {
    buffers: Vec<Arc<SampleBuffer>>,
    zones: Vec<Zone>,
}

/// A shared sample map, as it travels to a device.
pub type SampleMapRef = Arc<SampleMap>;

impl SampleMap {
    /// Validates every zone against its buffer.
    ///
    /// # Errors
    /// The first broken invariant, see [`Zone`] and [`SampleMapError`].
    pub fn new(buffers: Vec<Arc<SampleBuffer>>, zones: Vec<Zone>) -> Result<Self, SampleMapError> {
        for (i, z) in zones.iter().enumerate() {
            validate_zone(i, z, &buffers)?;
        }
        Ok(Self { buffers, zones })
    }

    /// The buffers.
    pub fn buffers(&self) -> &[Arc<SampleBuffer>] {
        &self.buffers
    }

    /// The zones, in the order they were given.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// The buffer of `zone` (valid by construction).
    pub fn buffer_of(&self, zone: &Zone) -> &Arc<SampleBuffer> {
        &self.buffers[zone.buffer as usize]
    }
}

fn validate_zone(i: usize, z: &Zone, buffers: &[Arc<SampleBuffer>]) -> Result<(), SampleMapError> {
    let range = |field: &'static str, ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(SampleMapError::OutOfRange { zone: i, field })
        }
    };
    let frames = |field: &'static str, ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(SampleMapError::BadFrames { zone: i, field })
        }
    };
    let buf = buffers
        .get(z.buffer as usize)
        .ok_or(SampleMapError::NoSuchBuffer {
            zone: i,
            buffer: z.buffer,
            buffers: buffers.len(),
        })?;
    range("key_hi", z.key_hi <= 127 && z.key_lo <= z.key_hi)?;
    range(
        "vel_lo",
        (1..=127).contains(&z.vel_lo) && z.vel_hi <= 127 && z.vel_lo <= z.vel_hi,
    )?;
    range("root_key", z.root_key <= 127)?;
    range(
        "transpose",
        (ZONE_TRANSPOSE_RANGE.0..=ZONE_TRANSPOSE_RANGE.1).contains(&z.transpose),
    )?;
    range(
        "tune_cents",
        (ZONE_TUNE_RANGE.0..=ZONE_TUNE_RANGE.1).contains(&z.tune_cents),
    )?;
    range(
        "gain_db",
        (ZONE_GAIN_RANGE.0..=ZONE_GAIN_RANGE.1).contains(&z.gain_db),
    )?;
    range("pan", (-1.0..=1.0).contains(&z.pan))?;
    frames("end", z.start < z.end && z.end <= buf.frames)?;
    if z.loop_mode != LoopMode::Off && !z.reverse {
        frames(
            "loop_start",
            z.start <= z.loop_start && z.loop_start < z.loop_end && z.loop_end <= z.end,
        )?;
        frames(
            "crossfade",
            z.crossfade <= z.loop_end - z.loop_start && z.crossfade <= z.loop_start - z.start,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(frames: usize) -> Arc<SampleBuffer> {
        let data: Arc<[Vec<f32>]> = vec![vec![0.0; frames], vec![0.0; frames]].into();
        Arc::new(SampleBuffer::new(data).unwrap())
    }

    fn zone() -> Zone {
        Zone {
            buffer: 0,
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            root_key: 60,
            transpose: 0,
            tune_cents: 0.0,
            gain_db: 0.0,
            pan: 0.0,
            start: 0,
            end: 1000,
            loop_mode: LoopMode::Forward,
            loop_start: 100,
            loop_end: 900,
            crossfade: 100,
            trigger: TriggerMode::Gate,
            reverse: false,
        }
    }

    type Case = (&'static str, fn(&mut Zone));

    fn err(z: Zone) -> SampleMapError {
        SampleMap::new(vec![buffer(1000)], vec![z]).unwrap_err()
    }

    #[test]
    fn buffer_takes_shape_from_data_and_rejects_bad_data() {
        let b = buffer(10);
        assert_eq!((b.frames, b.channels), (10, 2));
        let empty: Arc<[Vec<f32>]> = Vec::new().into();
        assert_eq!(SampleBuffer::new(empty), Err(SampleMapError::EmptyBuffer));
        let ragged: Arc<[Vec<f32>]> = vec![vec![0.0; 2], vec![0.0; 3]].into();
        assert_eq!(SampleBuffer::new(ragged), Err(SampleMapError::RaggedBuffer));
    }

    #[test]
    fn a_valid_map_keeps_its_zones_in_order() {
        let mut b = zone();
        b.loop_mode = LoopMode::Off;
        let m = SampleMap::new(vec![buffer(1000)], vec![zone(), b]).unwrap();
        assert_eq!(m.zones(), &[zone(), b]);
        assert_eq!(m.buffer_of(&m.zones()[0]).frames, 1000);
    }

    #[test]
    fn zones_out_of_range_are_rejected() {
        let mut z = zone();
        z.buffer = 1;
        assert!(matches!(err(z), SampleMapError::NoSuchBuffer { .. }));
        let cases: [Case; 9] = [
            ("key_hi", |z| z.key_lo = 61),
            ("key_hi", |z| z.key_hi = 128),
            ("vel_lo", |z| z.vel_lo = 0),
            ("vel_lo", |z| z.vel_hi = 128),
            ("root_key", |z| z.root_key = 200),
            ("transpose", |z| z.transpose = 49),
            ("tune_cents", |z| z.tune_cents = f32::NAN),
            ("gain_db", |z| z.gain_db = 13.0),
            ("pan", |z| z.pan = -1.5),
        ];
        for (field, edit) in cases {
            let mut z = zone();
            z.key_hi = if field == "key_hi" { 60 } else { z.key_hi };
            edit(&mut z);
            assert_eq!(err(z), SampleMapError::OutOfRange { zone: 0, field });
        }
    }

    #[test]
    fn frames_must_be_ordered_inside_the_buffer() {
        let cases: [Case; 6] = [
            ("end", |z| z.end = 1001),
            ("end", |z| z.start = 1000),
            ("loop_start", |z| z.loop_start = 900),
            ("loop_start", |z| z.loop_end = 1001),
            ("crossfade", |z| z.crossfade = 101),
            ("crossfade", |z| {
                z.loop_start = 850;
                z.crossfade = 60;
            }),
        ];
        for (field, edit) in cases {
            let mut z = zone();
            edit(&mut z);
            assert_eq!(err(z), SampleMapError::BadFrames { zone: 0, field });
        }
    }

    #[test]
    fn loop_fields_are_ignored_when_the_loop_is_off() {
        let mut z = zone();
        z.loop_mode = LoopMode::Off;
        z.loop_start = 5000;
        z.crossfade = 99_999;
        assert!(SampleMap::new(vec![buffer(1000)], vec![z]).is_ok());
    }

    #[test]
    fn reversed_zones_ignore_and_do_not_validate_their_loop() {
        let mut z = zone();
        z.loop_start = 5000;
        z.loop_end = 10;
        z.crossfade = 99_999;
        assert_eq!(
            err(z),
            SampleMapError::BadFrames {
                zone: 0,
                field: "loop_start"
            }
        );
        z.reverse = true;
        assert!(SampleMap::new(vec![buffer(1000)], vec![z]).is_ok());
        // start/end are still checked.
        z.end = 1001;
        assert_eq!(
            err(z),
            SampleMapError::BadFrames {
                zone: 0,
                field: "end"
            }
        );
    }

    #[test]
    fn velocity_maps_to_seven_bits() {
        assert_eq!(velocity_to_7bit(1.0), 127);
        assert_eq!(velocity_to_7bit(0.5), 64);
        assert_eq!(velocity_to_7bit(0.001), 1);
        assert_eq!(velocity_to_7bit(f32::NAN), 1);
        assert_eq!(velocity_to_7bit(3.0), 127);
        let mut z = zone();
        z.key_lo = 60;
        z.key_hi = 64;
        z.vel_lo = 64;
        assert!(z.contains(60, 64) && z.contains(64, 127));
        assert!(!z.contains(59, 100) && !z.contains(65, 100) && !z.contains(62, 63));
    }
}
