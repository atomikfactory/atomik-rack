//! Pure zone editing for the sample editor: note names, the numeric
//! fields, key and velocity range drags, waveform marker drags and the clamp that keeps every
//! emitted zone valid.
//!
//! Validity is the SDK's [`atomik_device_sdk::SampleMap::new`] rule, which the model's
//! `SetZones` applies too: [`sanitize`] clamps a zone into it (ranges, `start < end <=
//! frames`, the loop inside `start..end`, the crossfade within the loop and the pre-loop
//! audio). Every edit function ends in it, so the view can never emit a list the model rejects
//! (given an asset of `frames` frames that exists in the pool).

use atomik_device_sdk::{
    LoopMode, MAX_RR_LEN, MAX_ZONE_GROUPS, ZONE_CUTOFF_RANGE, ZONE_ENV_MS, ZONE_GAIN_RANGE,
    ZONE_TRANSPOSE_RANGE, ZONE_TUNE_RANGE, ZoneEnv, ZoneFilter,
};
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
    /// Group, 0 (none) to 31.
    Group,
    /// Velocity crossfade width.
    VelXfade,
    /// Round-robin cycle length (0: off).
    RrLen,
    /// Round-robin slot.
    RrPos,
    /// Own envelope: attack.
    EnvA,
    /// Own envelope: decay.
    EnvD,
    /// Own envelope: sustain.
    EnvS,
    /// Own envelope: release.
    EnvR,
    /// Own filter: mode.
    FltMode,
    /// Own filter: cutoff.
    FltCutoff,
    /// Own filter: resonance.
    FltRes,
    /// Own filter: key tracking.
    FltKey,
    /// Own filter: envelope amount.
    FltEnv,
}

/// The envelope a zone gets when its first envelope field is edited.
pub const DEFAULT_ENV: ZoneEnv = ZoneEnv {
    attack_ms: 1.0,
    decay_ms: 200.0,
    sustain: 1.0,
    release_ms: 150.0,
};

/// The filter a zone gets when its first filter field is edited.
pub const DEFAULT_FILTER: ZoneFilter = ZoneFilter {
    mode: 0,
    cutoff: 8_000.0,
    resonance: 0.2,
    key_track: 0.0,
    env_amount: 0.0,
};

impl Field {
    /// The fields of the synth page: group, velocity crossfade and round-robin, then the zone's
    /// own envelope and filter.
    pub const SYNTH: [Field; 13] = [
        Field::Group,
        Field::VelXfade,
        Field::RrLen,
        Field::RrPos,
        Field::EnvA,
        Field::EnvD,
        Field::EnvS,
        Field::EnvR,
        Field::FltMode,
        Field::FltCutoff,
        Field::FltRes,
        Field::FltKey,
        Field::FltEnv,
    ];

    /// Whether the field edits the zone's own envelope or filter (these follow the group: an
    /// edit reaches every zone of the same group, and a reset gives the block back to the
    /// device).
    pub fn is_block(self) -> bool {
        matches!(
            self,
            Field::EnvA
                | Field::EnvD
                | Field::EnvS
                | Field::EnvR
                | Field::FltMode
                | Field::FltCutoff
                | Field::FltRes
                | Field::FltKey
                | Field::FltEnv
        )
    }

    /// Every field of the zone page, in panel order.
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
            Field::Group => "GROUP",
            Field::VelXfade => "VEL XFADE",
            Field::RrLen => "ROUND-ROBIN",
            Field::RrPos => "RR SLOT",
            Field::EnvA => "ATTACK",
            Field::EnvD => "DECAY",
            Field::EnvS => "SUSTAIN",
            Field::EnvR => "RELEASE",
            Field::FltMode => "FILTER",
            Field::FltCutoff => "CUTOFF",
            Field::FltRes => "RESONANCE",
            Field::FltKey => "KEY TRACK",
            Field::FltEnv => "ENV AMOUNT",
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
            Field::Group | Field::RrLen | Field::RrPos => 0.25,
            Field::VelXfade => 0.5,
            Field::EnvA | Field::EnvD | Field::EnvR => 0.03,
            Field::EnvS | Field::FltRes | Field::FltKey => 0.005,
            Field::FltMode => 0.1,
            Field::FltCutoff => 0.02,
            Field::FltEnv => 0.01,
            Field::Crossfade => f64::from(sample_rate.max(1)) / 1000.0,
        }
    }

    /// Units one wheel notch changes.
    pub fn wheel_step(self, sample_rate: u32) -> f64 {
        match self {
            Field::Tune => 1.0,
            Field::Gain => 0.5,
            Field::Pan => 0.05,
            Field::EnvA | Field::EnvD | Field::EnvR | Field::FltCutoff => 0.25,
            Field::EnvS | Field::FltRes => 0.02,
            Field::FltKey | Field::FltEnv => 0.05,
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
        Field::Group => f64::from(z.ext.group),
        Field::VelXfade => f64::from(z.ext.vel_xfade),
        Field::RrLen => f64::from(z.ext.rr_len),
        Field::RrPos => f64::from(z.ext.rr_pos),
        Field::EnvA => f64::from(env_of(z).attack_ms).log2(),
        Field::EnvD => f64::from(env_of(z).decay_ms).log2(),
        Field::EnvS => f64::from(env_of(z).sustain),
        Field::EnvR => f64::from(env_of(z).release_ms).log2(),
        Field::FltMode => f64::from(filter_of(z).mode),
        Field::FltCutoff => f64::from(filter_of(z).cutoff).log2(),
        Field::FltRes => f64::from(filter_of(z).resonance),
        Field::FltKey => f64::from(filter_of(z).key_track),
        Field::FltEnv => f64::from(filter_of(z).env_amount),
    }
}

/// The reset value of a field: that of [`ZoneDoc::new`]. For the own envelope and filter fields it
/// is NaN, which [`with_field`] reads as "give the block back to the device".
pub fn field_default(z: &ZoneDoc, field: Field, frames: u64) -> f64 {
    if field.is_block() {
        return f64::NAN;
    }
    field_value(&ZoneDoc::new(z.asset.clone(), frames), field)
}

/// The zone's own envelope, or the one an edit starts from.
fn env_of(z: &ZoneDoc) -> ZoneEnv {
    z.ext.env.unwrap_or(DEFAULT_ENV)
}

/// The zone's own filter, or the one an edit starts from.
fn filter_of(z: &ZoneDoc) -> ZoneFilter {
    z.ext.filter.unwrap_or(DEFAULT_FILTER)
}

fn env_mut(z: &mut ZoneDoc) -> &mut ZoneEnv {
    z.ext.env.get_or_insert(DEFAULT_ENV)
}

fn filter_mut(z: &mut ZoneDoc) -> &mut ZoneFilter {
    z.ext.filter.get_or_insert(DEFAULT_FILTER)
}

/// Gives every other zone of `list[index]`'s group (when it has one) the envelope and filter of
/// `list[index]`, after an edit of `field` (a no-op for fields that are not [`Field::is_block`]).
/// Group settings are the settings every zone of the group shares.
pub fn share_group(list: &mut [ZoneDoc], index: usize, field: Field) {
    let Some(src) = list.get(index) else {
        return;
    };
    if !field.is_block() || src.ext.group == 0 {
        return;
    }
    let (group, env, filter) = (src.ext.group, src.ext.env, src.ext.filter);
    for (i, z) in list.iter_mut().enumerate() {
        if i != index && z.ext.group == group {
            z.ext.env = env;
            z.ext.filter = filter;
        }
    }
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
        Field::Group => match z.ext.group {
            0 => "none".to_string(),
            g => format!("G{g}"),
        },
        Field::VelXfade => match z.ext.vel_xfade {
            0 => "off".to_string(),
            n => format!("{n}"),
        },
        Field::RrLen => match z.ext.rr_len {
            0 | 1 => "off".to_string(),
            n => format!("{n}"),
        },
        Field::RrPos => {
            if z.ext.rr_len > 1 {
                format!("{} of {}", z.ext.rr_pos + 1, z.ext.rr_len)
            } else {
                "-".to_string()
            }
        }
        Field::EnvA | Field::EnvD | Field::EnvS | Field::EnvR if z.ext.env.is_none() => {
            "device".to_string()
        }
        Field::EnvA => format!("{:.1} ms", env_of(z).attack_ms),
        Field::EnvD => format!("{:.0} ms", env_of(z).decay_ms),
        Field::EnvS => format!("{:.0}%", env_of(z).sustain * 100.0),
        Field::EnvR => format!("{:.0} ms", env_of(z).release_ms),
        Field::FltMode | Field::FltCutoff | Field::FltRes | Field::FltKey | Field::FltEnv
            if z.ext.filter.is_none() =>
        {
            "device".to_string()
        }
        Field::FltMode => {
            ["LP12", "BP12", "HP12"][usize::from(filter_of(z).mode.min(2))].to_string()
        }
        Field::FltCutoff => {
            let hz = filter_of(z).cutoff;
            if hz >= 1000.0 {
                format!("{:.2} kHz", hz / 1000.0)
            } else {
                format!("{hz:.0} Hz")
            }
        }
        Field::FltRes => format!("{:.0}%", filter_of(z).resonance * 100.0),
        Field::FltKey => format!("{:.0}%", filter_of(z).key_track * 100.0),
        Field::FltEnv => format!("{:+.0}%", filter_of(z).env_amount * 100.0),
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
    let e = &mut z.ext;
    e.group = e.group.min((MAX_ZONE_GROUPS - 1) as u8);
    e.vel_xfade = e.vel_xfade.min(z.vel_hi - z.vel_lo);
    e.rr_len = e.rr_len.min(MAX_RR_LEN);
    e.rr_pos = e.rr_pos.min(e.rr_len.max(1) - 1);
    if let Some(v) = &mut e.env {
        let ms = |x: f32| clamp_f32(x, ZONE_ENV_MS.0, ZONE_ENV_MS.1);
        (v.attack_ms, v.decay_ms, v.release_ms) =
            (ms(v.attack_ms), ms(v.decay_ms), ms(v.release_ms));
        v.sustain = clamp_f32(v.sustain, 0.0, 1.0);
    }
    if let Some(v) = &mut e.filter {
        v.mode = v.mode.min(2);
        v.cutoff = clamp_f32(v.cutoff, ZONE_CUTOFF_RANGE.0, ZONE_CUTOFF_RANGE.1);
        v.resonance = clamp_f32(v.resonance, 0.0, 1.0);
        v.key_track = clamp_f32(v.key_track, 0.0, 1.0);
        v.env_amount = clamp_f32(v.env_amount, -1.0, 1.0);
    }
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
    if value.is_nan() && field.is_block() {
        // A reset: the own envelope or filter goes back to the device settings.
        if matches!(field, Field::EnvA | Field::EnvD | Field::EnvS | Field::EnvR) {
            z.ext.env = None;
        } else {
            z.ext.filter = None;
        }
        return z;
    }
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
        Field::Group => z.ext.group = int(0.0, (MAX_ZONE_GROUPS - 1) as f64) as u8,
        Field::VelXfade => z.ext.vel_xfade = int(0.0, f64::from(z.vel_hi - z.vel_lo)) as u8,
        Field::RrLen => z.ext.rr_len = int(0.0, f64::from(MAX_RR_LEN)) as u8,
        Field::RrPos => z.ext.rr_pos = int(0.0, f64::from(z.ext.rr_len.max(1) - 1)) as u8,
        Field::EnvA => env_mut(&mut z).attack_ms = v.exp2() as f32,
        Field::EnvD => env_mut(&mut z).decay_ms = v.exp2() as f32,
        Field::EnvS => env_mut(&mut z).sustain = v as f32,
        Field::EnvR => env_mut(&mut z).release_ms = v.exp2() as f32,
        Field::FltMode => filter_mut(&mut z).mode = int(0.0, 2.0) as u8,
        Field::FltCutoff => filter_mut(&mut z).cutoff = v.exp2() as f32,
        Field::FltRes => filter_mut(&mut z).resonance = v as f32,
        Field::FltKey => filter_mut(&mut z).key_track = v as f32,
        Field::FltEnv => filter_mut(&mut z).env_amount = v as f32,
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

    #[test]
    fn the_synth_fields_start_from_the_defaults_and_reset_to_the_device() {
        let z = ZoneDoc::new("a", 1000);
        assert_eq!(format_field(&z, Field::EnvA, 48_000), "device");
        assert_eq!(format_field(&z, Field::FltCutoff, 48_000), "device");
        // The first edit of an envelope field creates the envelope from the defaults.
        let a = with_field(&z, Field::EnvA, 50.0f64.log2(), 1000);
        let env = a.ext.env.unwrap();
        assert!((env.attack_ms - 50.0).abs() < 0.01);
        assert_eq!(
            (env.decay_ms, env.sustain, env.release_ms),
            (200.0, 1.0, 150.0)
        );
        assert_eq!(format_field(&a, Field::EnvA, 48_000), "50.0 ms");
        // A reset takes the whole block back, not just the field.
        let reset = field_default(&a, Field::EnvD, 1000);
        assert!(reset.is_nan());
        assert_eq!(with_field(&a, Field::EnvD, reset, 1000).ext.env, None);
        let f = with_field(&z, Field::FltCutoff, 1500.0f64.log2(), 1000);
        assert_eq!(f.ext.filter.unwrap().mode, 0);
        assert_eq!(format_field(&f, Field::FltCutoff, 48_000), "1.50 kHz");
        assert_eq!(
            with_field(&f, Field::FltMode, f64::NAN, 1000).ext.filter,
            None
        );
        // The other fields reset to their plain defaults.
        assert_eq!(field_default(&a, Field::Group, 1000), 0.0);
    }

    #[test]
    fn sanitize_keeps_the_synth_fields_inside_the_sdk_rules() {
        let mut z = ZoneDoc::new("a", 1000);
        (z.vel_lo, z.vel_hi) = (10, 20);
        z.ext.group = 200;
        z.ext.vel_xfade = 99;
        z.ext.rr_len = 99;
        z.ext.rr_pos = 99;
        z.ext.env = Some(ZoneEnv {
            attack_ms: f32::NAN,
            decay_ms: 1e9,
            sustain: 5.0,
            release_ms: -1.0,
        });
        z.ext.filter = Some(ZoneFilter {
            mode: 9,
            cutoff: 1.0,
            resonance: 2.0,
            key_track: -1.0,
            env_amount: 9.0,
        });
        sanitize(&mut z, 1000);
        assert_eq!(
            (z.ext.group, z.ext.vel_xfade, z.ext.rr_len, z.ext.rr_pos),
            (31, 10, 16, 15)
        );
        let map = atomik_device_sdk::SampleMap::new(
            vec![std::sync::Arc::new(atomik_device_sdk::SampleBuffer {
                data: std::sync::Arc::new([Vec::new()]),
                frames: 1000,
                channels: 1,
            })],
            vec![z.to_zone(0)],
        );
        assert!(map.is_ok(), "{map:?}");
        // Narrowing the velocity range pulls the crossfade in with it.
        let narrowed = drag_edges(
            &z,
            Edges {
                top: true,
                ..Edges::default()
            },
            -8,
            -8,
            1000,
        );
        assert_eq!(narrowed.ext.vel_xfade, narrowed.vel_hi - narrowed.vel_lo);
    }

    #[test]
    fn envelope_and_filter_edits_follow_the_group() {
        let mut list = vec![
            ZoneDoc::new("a", 10),
            ZoneDoc::new("b", 10),
            ZoneDoc::new("c", 10),
        ];
        list[0].ext.group = 2;
        list[1].ext.group = 2;
        list[0] = with_field(&list[0], Field::EnvR, 1000.0f64.log2(), 10);
        share_group(&mut list, 0, Field::EnvR);
        assert_eq!(list[1].ext.env, list[0].ext.env);
        assert_eq!(list[2].ext.env, None, "another group is left alone");
        // A zone without a group, and fields that are not blocks, share nothing.
        list[2].ext.filter = list[0].ext.filter;
        list[2] = with_field(&list[2], Field::FltRes, 0.9, 10);
        share_group(&mut list, 2, Field::FltRes);
        assert_eq!(list[0].ext.filter, None);
        list[0].ext.rr_len = 4;
        share_group(&mut list, 0, Field::RrLen);
        assert_eq!(list[1].ext.rr_len, 0);
    }
}
