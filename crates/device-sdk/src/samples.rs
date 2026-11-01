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

/// Number of zone groups (group ids are `0..MAX_ZONE_GROUPS`; 0 is the default group).
pub const MAX_ZONE_GROUPS: usize = 32;
/// Longest round-robin cycle of a zone.
pub const MAX_RR_LEN: u8 = 16;
/// Longest velocity crossfade, in velocity steps.
pub const MAX_VEL_XFADE: u8 = 126;
/// Lowest and highest envelope time of a zone, in milliseconds.
pub const ZONE_ENV_MS: (f32, f32) = (0.5, 10_000.0);
/// Lowest and highest filter cutoff of a zone, in Hz.
pub const ZONE_CUTOFF_RANGE: (f32, f32) = (20.0, 20_000.0);
/// Number of filter modes of a zone filter (low-pass, band-pass, high-pass).
pub const ZONE_FILTER_MODES: u8 = 3;

/// An amplitude envelope that replaces the device's own for the voices of one zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneEnv {
    /// Attack in milliseconds, [`ZONE_ENV_MS`].
    pub attack_ms: f32,
    /// Decay in milliseconds, [`ZONE_ENV_MS`].
    pub decay_ms: f32,
    /// Sustain level, 0..=1.
    pub sustain: f32,
    /// Release in milliseconds, [`ZONE_ENV_MS`].
    pub release_ms: f32,
}

impl ZoneEnv {
    fn is_valid(&self) -> bool {
        let ms = |v: f32| (ZONE_ENV_MS.0..=ZONE_ENV_MS.1).contains(&v);
        ms(self.attack_ms)
            && ms(self.decay_ms)
            && ms(self.release_ms)
            && (0.0..=1.0).contains(&self.sustain)
    }
}

/// A filter that replaces the device's own for the voices of one zone (it runs even when the
/// device filter is off). Its envelope is the device's filter envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneFilter {
    /// 0 low-pass, 1 band-pass, 2 high-pass (12 dB state-variable).
    pub mode: u8,
    /// Cutoff in Hz, [`ZONE_CUTOFF_RANGE`].
    pub cutoff: f32,
    /// Resonance, 0..=1.
    pub resonance: f32,
    /// Key tracking, 0..=1 (1: the cutoff follows the key one octave per octave).
    pub key_track: f32,
    /// Filter envelope amount, -1..=1 (6 octaves at 1).
    pub env_amount: f32,
}

impl ZoneFilter {
    fn is_valid(&self) -> bool {
        self.mode < ZONE_FILTER_MODES
            && (ZONE_CUTOFF_RANGE.0..=ZONE_CUTOFF_RANGE.1).contains(&self.cutoff)
            && (0.0..=1.0).contains(&self.resonance)
            && (0.0..=1.0).contains(&self.key_track)
            && (-1.0..=1.0).contains(&self.env_amount)
    }
}

/// The grouping, layering and per-zone synthesis of a [`Zone`]: everything beyond key and
/// velocity range, tuning and playback that a sample map can say about a zone. The default
/// ([`ZoneExt::NONE`]) is "no group, hard velocity edges, no round-robin, the device's own
/// envelope and filter".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneExt {
    /// The group the zone belongs to, `0..`[`MAX_ZONE_GROUPS`]. Round-robin cycles count per
    /// group.
    pub group: u8,
    /// Velocity crossfade width, `0..=vel_hi - vel_lo` ([`MAX_VEL_XFADE`]). The zone fades in
    /// over `vel_lo..=vel_lo + vel_xfade` (unless `vel_lo` is 1) and out over
    /// `vel_hi - vel_xfade..=vel_hi` (unless `vel_hi` is 127), with an equal-power curve, so
    /// two neighbours whose ranges overlap by the same width sum to constant power.
    pub vel_xfade: u8,
    /// Round-robin cycle length. 0 and 1 mean no round-robin; otherwise at most
    /// [`MAX_RR_LEN`].
    pub rr_len: u8,
    /// This zone's slot in the cycle, `< rr_len` (0 without round-robin). The zone sounds when
    /// the group's note counter modulo `rr_len` equals it.
    pub rr_pos: u8,
    /// Amplitude envelope of the zone, if it has its own.
    pub env: Option<ZoneEnv>,
    /// Filter of the zone, if it has its own.
    pub filter: Option<ZoneFilter>,
}

impl ZoneExt {
    /// No group, no crossfade, no round-robin, no own envelope or filter.
    pub const NONE: ZoneExt = ZoneExt {
        group: 0,
        vel_xfade: 0,
        rr_len: 0,
        rr_pos: 0,
        env: None,
        filter: None,
    };

    /// Whether the zone takes part in a round-robin cycle.
    pub fn round_robin(&self) -> bool {
        self.rr_len > 1
    }
}

impl Default for ZoneExt {
    fn default() -> Self {
        Self::NONE
    }
}

/// Equal-power gain (0..=1) of a zone with `vel_lo..=vel_hi` and crossfade width `xfade` at
/// 7-bit velocity `vel7`: 1 in the middle, a quarter sine rising over the first `xfade` steps
/// above `vel_lo` and a quarter cosine falling over the last `xfade` below `vel_hi`.
/// A `vel_lo` of 1 and a `vel_hi` of 127 are hard edges. Outside the range the gain is 0.
pub fn velocity_fade(vel_lo: u8, vel_hi: u8, xfade: u8, vel7: u8) -> f32 {
    if vel7 < vel_lo || vel7 > vel_hi {
        return 0.0;
    }
    if xfade == 0 {
        return 1.0;
    }
    let x = f32::from(xfade);
    let mut g = 1.0f32;
    if vel_lo > 1 {
        let t = (f32::from(vel7) - f32::from(vel_lo)) / x;
        if t < 1.0 {
            g *= (t * std::f32::consts::FRAC_PI_2).sin();
        }
    }
    if vel_hi < 127 {
        let t = (f32::from(vel_hi) - f32::from(vel7)) / x;
        if t < 1.0 {
            g *= (t * std::f32::consts::FRAC_PI_2).sin();
        }
    }
    g
}

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
    /// Group, velocity crossfade, round-robin and per-zone envelope and filter.
    pub ext: ZoneExt,
}

impl Zone {
    /// Whether the zone sounds for `key` at 7-bit velocity `vel7`.
    pub fn contains(&self, key: u8, vel7: u8) -> bool {
        (self.key_lo..=self.key_hi).contains(&key) && (self.vel_lo..=self.vel_hi).contains(&vel7)
    }

    /// The equal-power velocity crossfade gain at `vel7` ([`velocity_fade`]); 1 without a
    /// crossfade.
    pub fn velocity_gain(&self, vel7: u8) -> f32 {
        velocity_fade(self.vel_lo, self.vel_hi, self.ext.vel_xfade, vel7)
    }

    /// Whether the zone's round-robin slot is the one `counter` selects (always true without
    /// round-robin).
    pub fn rr_turn(&self, counter: u32) -> bool {
        !self.ext.round_robin()
            || counter % u32::from(self.ext.rr_len) == u32::from(self.ext.rr_pos)
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
    let e = &z.ext;
    range("group", usize::from(e.group) < MAX_ZONE_GROUPS)?;
    range(
        "vel_xfade",
        e.vel_xfade <= MAX_VEL_XFADE && e.vel_xfade <= z.vel_hi - z.vel_lo,
    )?;
    range("rr_len", e.rr_len <= MAX_RR_LEN)?;
    range("rr_pos", e.rr_pos < e.rr_len.max(1))?;
    range("env", e.env.is_none_or(|v| v.is_valid()))?;
    range("filter", e.filter.is_none_or(|v| v.is_valid()))?;
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
            ext: ZoneExt::NONE,
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

    #[test]
    fn neighbours_that_overlap_by_the_crossfade_sum_to_constant_power() {
        // A covers 1..=70 and B 60..=127, each with a 10-step crossfade.
        for v in 1..=127u8 {
            let a = velocity_fade(1, 70, 10, v);
            let b = velocity_fade(60, 127, 10, v);
            if (60..=70).contains(&v) {
                assert!((a * a + b * b - 1.0).abs() < 1e-5, "vel {v}: {a} {b}");
            } else {
                assert!(
                    (a + b - 1.0).abs() < 1e-6 && a * b == 0.0,
                    "vel {v}: {a} {b}"
                );
            }
        }
        // The ends of the keyboard are hard edges; outside the range nothing sounds.
        assert_eq!(velocity_fade(1, 70, 10, 1), 1.0);
        assert_eq!(velocity_fade(60, 127, 10, 127), 1.0);
        assert_eq!(velocity_fade(60, 127, 10, 59), 0.0);
        assert_eq!(velocity_fade(60, 127, 0, 60), 1.0);
    }

    #[test]
    fn round_robin_turns_follow_the_counter() {
        let mut z = zone();
        assert!((0..8).all(|c| z.rr_turn(c)));
        z.ext.rr_len = 3;
        z.ext.rr_pos = 1;
        let turns: Vec<u32> = (0..9).filter(|c| z.rr_turn(*c)).collect();
        assert_eq!(turns, [1, 4, 7]);
    }

    #[test]
    fn zone_ext_fields_are_validated() {
        let cases: [(&str, fn(&mut Zone)); 8] = [
            ("group", |z| z.ext.group = MAX_ZONE_GROUPS as u8),
            ("vel_xfade", |z| z.ext.vel_xfade = 127),
            ("rr_len", |z| z.ext.rr_len = MAX_RR_LEN + 1),
            ("rr_pos", |z| z.ext.rr_pos = 1),
            ("env", |z| {
                z.ext.env = Some(ZoneEnv {
                    attack_ms: 1.0,
                    decay_ms: 1.0,
                    sustain: 2.0,
                    release_ms: 1.0,
                });
            }),
            ("filter", |z| {
                z.ext.filter = Some(ZoneFilter {
                    mode: 0,
                    cutoff: f32::NAN,
                    resonance: 0.0,
                    key_track: 0.0,
                    env_amount: 0.0,
                });
            }),
            ("vel_xfade", |z| {
                z.vel_lo = 10;
                z.vel_hi = 20;
                z.ext.vel_xfade = 11;
            }),
            ("rr_pos", |z| {
                z.ext.rr_len = 2;
                z.ext.rr_pos = 2;
            }),
        ];
        for (field, edit) in cases {
            let mut z = zone();
            edit(&mut z);
            assert_eq!(err(z), SampleMapError::OutOfRange { zone: 0, field });
        }
        let mut ok = zone();
        ok.ext.rr_len = 4;
        ok.ext.rr_pos = 3;
        ok.ext.vel_xfade = 126;
        assert!(SampleMap::new(vec![buffer(1000)], vec![ok]).is_ok());
    }
}
