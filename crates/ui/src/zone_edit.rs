//! Pure zone editing for the sample editor: note names, the numeric
//! fields, key and velocity range drags, waveform marker drags and the clamp that keeps every
//! emitted zone valid.
//!
//! Validity is the SDK's [`atomik_device_sdk::SampleMap::new`] rule, which the model's
//! `SetZones` applies too: [`sanitize`] clamps a zone into it (ranges, `start < end <=
//! frames`, the loop inside `start..end`, the crossfade within the loop and the pre-loop
//! audio). Every edit function ends in it, so the view can never emit a list the model rejects
//! (given an asset of `frames` frames that exists in the pool).

use atomik_device_sdk::{LoopMode, ZONE_GAIN_RANGE, ZONE_TRANSPOSE_RANGE, ZONE_TUNE_RANGE};
use atomik_model::ZoneDoc;

/// Note name of a MIDI key with **C4 = 60**: `0` is `C-1`, `61` is `C#4`, `127` is `G9`.
pub fn note_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key % 12)],
        i32::from(key / 12) - 1
    )
}

/// A numeric field of the zone panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    /// Root key.
    Root,
    /// Transpose in semitones.
    Transpose,
    /// Fine tune in cents.
    Tune,
    /// Gain in dB.
    Gain,
    /// Pan -1..=1.
    Pan,
    /// Lowest key.
    KeyLo,
    /// Highest key.
    KeyHi,
    /// Lowest velocity.
    VelLo,
    /// Highest velocity.
    VelHi,
    /// Loop crossfade, in frames (shown in milliseconds).
    Crossfade,
}

impl Field {
    /// Every field, in panel order.
    pub const ALL: [Field; 10] = [
        Field::Root,
        Field::Transpose,
        Field::Tune,
        Field::Gain,
        Field::Pan,
        Field::KeyLo,
        Field::KeyHi,
        Field::VelLo,
        Field::VelHi,
        Field::Crossfade,
    ];

    /// Panel label.
    pub fn label(self) -> &'static str {
        match self {
            Field::Root => "ROOT",
            Field::Transpose => "TRANSPOSE",
            Field::Tune => "TUNE",
            Field::Gain => "GAIN",
            Field::Pan => "PAN",
            Field::KeyLo => "KEY LO",
            Field::KeyHi => "KEY HI",
            Field::VelLo => "VEL LO",
            Field::VelHi => "VEL HI",
            Field::Crossfade => "XFADE",
        }
    }

    /// Units a vertical drag changes per logical pixel (`sample_rate` converts the crossfade:
    /// 1 ms per pixel).
    pub fn per_pixel(self, sample_rate: u32) -> f64 {
        match self {
            Field::Root | Field::Transpose | Field::KeyLo | Field::KeyHi => 0.25,
            Field::VelLo | Field::VelHi => 0.5,
            Field::Tune => 0.5,
            Field::Gain => 0.1,
            Field::Pan => 0.01,
            Field::Crossfade => f64::from(sample_rate.max(1)) / 1000.0,
        }
    }

    /// Units one wheel notch changes.
    pub fn wheel_step(self, sample_rate: u32) -> f64 {
        match self {
            Field::Tune => 1.0,
            Field::Gain => 0.5,
            Field::Pan => 0.05,
            Field::Crossfade => f64::from(sample_rate.max(1)) / 1000.0,
            _ => 1.0,
        }
    }
}

/// The current value of a field (the crossfade in frames).
pub fn field_value(z: &ZoneDoc, field: Field) -> f64 {
    match field {
        Field::Root => f64::from(z.root_key),
        Field::Transpose => f64::from(z.transpose),
        Field::Tune => f64::from(z.tune_cents),
        Field::Gain => f64::from(z.gain_db),
        Field::Pan => f64::from(z.pan),
        Field::KeyLo => f64::from(z.key_lo),
        Field::KeyHi => f64::from(z.key_hi),
        Field::VelLo => f64::from(z.vel_lo),
        Field::VelHi => f64::from(z.vel_hi),
        Field::Crossfade => z.crossfade as f64,
    }
}

/// The reset value of a field: that of [`ZoneDoc::new`].
pub fn field_default(z: &ZoneDoc, field: Field, frames: u64) -> f64 {
    field_value(&ZoneDoc::new(z.asset.clone(), frames), field)
}

/// The text of a field: note names for keys (`C4`), `+2 st`, `-12.0 c`, `-3.0 dB`, `L20`,
/// `0.35 R`, velocities and the crossfade in milliseconds.
pub fn format_field(z: &ZoneDoc, field: Field, sample_rate: u32) -> String {
    match field {
        Field::Root => note_name(z.root_key),
        Field::KeyLo => note_name(z.key_lo),
        Field::KeyHi => note_name(z.key_hi),
        Field::Transpose => format!("{:+} st", z.transpose),
        Field::Tune => format!("{:+.0} c", z.tune_cents),
        Field::Gain => format!("{:+.1} dB", z.gain_db),
        Field::Pan => {
            if z.pan.abs() < 0.005 {
                "C".to_string()
            } else if z.pan < 0.0 {
                format!("L{:.0}", -z.pan * 100.0)
            } else {
                format!("R{:.0}", z.pan * 100.0)
            }
        }
        Field::VelLo => z.vel_lo.to_string(),
        Field::VelHi => z.vel_hi.to_string(),
        Field::Crossfade => format!(
            "{:.1} ms",
            z.crossfade as f64 * 1000.0 / f64::from(sample_rate.max(1))
        ),
    }
}

fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() {
        0.0f32.clamp(lo, hi)
    } else {
        v.clamp(lo, hi)
    }
}

/// Clamps `z` into validity for an asset of `frames` frames (see the module documentation).
/// Values already valid are left untouched.
pub fn sanitize(z: &mut ZoneDoc, frames: u64) {
    z.key_hi = z.key_hi.min(127);
    z.key_lo = z.key_lo.min(z.key_hi);
    z.vel_hi = z.vel_hi.clamp(1, 127);
    z.vel_lo = z.vel_lo.clamp(1, z.vel_hi);
    z.root_key = z.root_key.min(127);
    z.transpose = z
        .transpose
        .clamp(ZONE_TRANSPOSE_RANGE.0, ZONE_TRANSPOSE_RANGE.1);
    z.tune_cents = clamp_f32(z.tune_cents, ZONE_TUNE_RANGE.0, ZONE_TUNE_RANGE.1);
    z.gain_db = clamp_f32(z.gain_db, ZONE_GAIN_RANGE.0, ZONE_GAIN_RANGE.1);
    z.pan = clamp_f32(z.pan, -1.0, 1.0);
    if frames == 0 {
        return;
    }
    z.end = z.end.clamp(1, frames);
    z.start = z.start.min(z.end - 1);
    z.loop_end = z.loop_end.clamp(z.start + 1, z.end);
    z.loop_start = z.loop_start.clamp(z.start, z.loop_end - 1);
    z.crossfade = z
        .crossfade
        .min(z.loop_end - z.loop_start)
        .min(z.loop_start - z.start);
}

/// `z` with `field` set to `value` (rounded for whole-number fields, clamped into validity:
/// the lower key or velocity never passes the upper one, and the other way round).
pub fn with_field(z: &ZoneDoc, field: Field, value: f64, frames: u64) -> ZoneDoc {
    let mut z = z.clone();
    let v = if value.is_nan() { 0.0 } else { value };
    let int = |lo: f64, hi: f64| v.round().clamp(lo, hi);
    match field {
        Field::Root => z.root_key = int(0.0, 127.0) as u8,
        Field::Transpose => {
            z.transpose = int(
                f64::from(ZONE_TRANSPOSE_RANGE.0),
                f64::from(ZONE_TRANSPOSE_RANGE.1),
            ) as i8;
        }
        Field::Tune => z.tune_cents = v as f32,
        Field::Gain => z.gain_db = v as f32,
        Field::Pan => z.pan = v as f32,
        Field::KeyLo => z.key_lo = int(0.0, f64::from(z.key_hi)) as u8,
        Field::KeyHi => z.key_hi = int(f64::from(z.key_lo), 127.0) as u8,
        Field::VelLo => z.vel_lo = int(1.0, f64::from(z.vel_hi)) as u8,
        Field::VelHi => z.vel_hi = int(f64::from(z.vel_lo), 127.0) as u8,
        Field::Crossfade => z.crossfade = v.round().max(0.0) as u64,
    }
    sanitize(&mut z, frames);
    z
}

/// Which edges of a zone rectangle a drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    /// The key range's lower end.
    pub left: bool,
    /// The key range's upper end.
    pub right: bool,
    /// The velocity range's upper end (top of the rectangle).
    pub top: bool,
    /// The velocity range's lower end (bottom of the rectangle).
    pub bottom: bool,
}

impl Edges {
    /// True when no edge is set (a body drag).
    pub fn is_none(self) -> bool {
        !(self.left || self.right || self.top || self.bottom)
    }
}

/// `orig` with the edges in `edges` moved by `dkey` keys and `dvel` velocity steps, or, with no
/// edge, the key range moved by `dkey` keeping its width (stopping at the ends of the
/// keyboard). Edges never cross.
pub fn drag_edges(orig: &ZoneDoc, edges: Edges, dkey: i32, dvel: i32, frames: u64) -> ZoneDoc {
    let mut z = orig.clone();
    if edges.is_none() {
        let d = dkey.clamp(-i32::from(orig.key_lo), 127 - i32::from(orig.key_hi));
        z.key_lo = (i32::from(orig.key_lo) + d) as u8;
        z.key_hi = (i32::from(orig.key_hi) + d) as u8;
    } else {
        if edges.left {
            z.key_lo = (i32::from(orig.key_lo) + dkey).clamp(0, i32::from(z.key_hi)) as u8;
        }
        if edges.right {
            z.key_hi = (i32::from(orig.key_hi) + dkey).clamp(i32::from(z.key_lo), 127) as u8;
        }
        if edges.top {
            z.vel_hi = (i32::from(orig.vel_hi) + dvel).clamp(i32::from(z.vel_lo), 127) as u8;
        }
        if edges.bottom {
            z.vel_lo = (i32::from(orig.vel_lo) + dvel).clamp(1, i32::from(z.vel_hi)) as u8;
        }
    }
    sanitize(&mut z, frames);
    z
}

/// A draggable marker of the waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// First frame played.
    Start,
    /// End frame (exclusive).
    End,
    /// Loop start.
    LoopStart,
    /// Loop end (exclusive).
    LoopEnd,
    /// Left edge of the crossfade (it ends at the loop start): its position is
    /// `loop_start - crossfade`.
    Crossfade,
}

/// The frame a marker sits at.
pub fn marker_frame(z: &ZoneDoc, marker: Marker) -> u64 {
    match marker {
        Marker::Start => z.start,
        Marker::End => z.end,
        Marker::LoopStart => z.loop_start,
        Marker::LoopEnd => z.loop_end,
        Marker::Crossfade => z.loop_start.saturating_sub(z.crossfade),
    }
}

/// `z` with `marker` moved to `frame` (clamped): `start < end <= frames`, the loop inside
/// `start..end` with `loop_start < loop_end`, and the crossfade within the loop length and the
/// audio before the loop start. Moving the start or end pushes the loop along when it would
/// fall outside.
pub fn with_marker(z: &ZoneDoc, marker: Marker, frame: i64, frames: u64) -> ZoneDoc {
    let mut z = z.clone();
    if frames == 0 {
        return z;
    }
    // Loop fields of a non-looping zone are not validated by the model: tidy them first so
    // the clamps below always have a non-empty range.
    sanitize(&mut z, frames);
    let f = u64::try_from(frame.max(0)).unwrap_or(0).min(frames);
    match marker {
        Marker::Start => z.start = f.min(z.end.saturating_sub(1)),
        Marker::End => z.end = f.clamp(z.start + 1, frames),
        Marker::LoopStart => z.loop_start = f.clamp(z.start, z.loop_end.saturating_sub(1)),
        Marker::LoopEnd => z.loop_end = f.clamp(z.loop_start + 1, z.end),
        Marker::Crossfade => z.crossfade = z.loop_start.saturating_sub(f),
    }
    sanitize(&mut z, frames);
    z
}

/// `z` with the loop mode set.
pub fn with_loop_mode(z: &ZoneDoc, mode: LoopMode, frames: u64) -> ZoneDoc {
    let mut z = z.clone();
    z.loop_mode = mode;
    sanitize(&mut z, frames);
    z
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_device_sdk::TriggerMode;

    #[test]
    fn note_names_use_c4_equals_60() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(127), "G9");
        assert_eq!(note_name(48), "C3");
    }

    #[test]
    fn sanitize_fixes_everything() {
        let mut z = ZoneDoc::new("a", 1000);
        z.key_lo = 100;
        z.key_hi = 20;
        z.vel_lo = 0;
        z.vel_hi = 200;
        z.tune_cents = f32::NAN;
        z.gain_db = 99.0;
        z.start = 900;
        z.end = 5000;
        z.loop_start = 10;
        z.loop_end = 20;
        z.crossfade = 500;
        z.trigger = TriggerMode::OneShot;
        sanitize(&mut z, 1000);
        assert!(z.key_lo <= z.key_hi && z.vel_lo >= 1 && z.vel_hi <= 127);
        assert_eq!((z.tune_cents, z.gain_db), (0.0, 12.0));
        assert!(z.start < z.end && z.end <= 1000);
        assert!(z.start <= z.loop_start && z.loop_start < z.loop_end && z.loop_end <= z.end);
        assert!(z.crossfade <= z.loop_start - z.start);
    }

    #[test]
    fn crossfade_marker_is_the_loop_start_minus_the_crossfade() {
        let mut z = ZoneDoc::new("a", 1000);
        z.loop_mode = LoopMode::Forward;
        z.loop_start = 400;
        z.loop_end = 800;
        z.crossfade = 100;
        assert_eq!(marker_frame(&z, Marker::Crossfade), 300);
        let z2 = with_marker(&z, Marker::Crossfade, 250, 1000);
        assert_eq!(z2.crossfade, 150);
        let z3 = with_marker(&z, Marker::Crossfade, -50, 1000);
        assert_eq!(z3.crossfade, 400, "limited by the loop length 400");
        let z4 = with_marker(&z, Marker::Crossfade, 900, 1000);
        assert_eq!(z4.crossfade, 0);
    }

    #[test]
    fn body_drag_keeps_the_width_and_stops_at_the_ends() {
        let mut z = ZoneDoc::new("a", 10);
        z.key_lo = 10;
        z.key_hi = 20;
        let m = drag_edges(&z, Edges::default(), 500, 0, 10);
        assert_eq!((m.key_lo, m.key_hi), (117, 127));
        let m = drag_edges(&z, Edges::default(), -500, 0, 10);
        assert_eq!((m.key_lo, m.key_hi), (0, 10));
    }
}
