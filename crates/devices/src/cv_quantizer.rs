//! `atomik.cv-quantizer`: snaps a pitch control voltage to the nearest note of a scale.
//!
//! # Pitch CV
//!
//! The pitch CV of the sequencers is `(key - 60) / 24`: 0 is middle C and +-1 is two octaves
//! either way, so one semitone is 1/24. The quantizer reads and writes that scale (an input
//! beyond +-1 still quantizes, up to +-8).
//!
//! ```text
//! x = 60 + pitch_cv_in * 24                 the input as a (fractional) key
//! q = the in-scale key nearest to x         ties go down
//! pitch_cv_out = (q + transpose - 60) / 24
//! ```
//!
//! * **Scales.** The 16 kinds of the piano roll's scale list (`atomik_core::theory::ScaleKind`:
//!   chromatic, major, the minors, the modes, pentatonics, blues, whole tone and the rest) on a
//!   `root` from C to B. A chromatic scale rounds to the nearest semitone.
//! * **Nearest** compares the distance to the highest scale key at or below `x` and the lowest one
//!   above it; the nearer one wins and the lower key wins an exact tie.
//! * **Transpose** (-24..+24 semitones) moves the quantized result, after the scale is applied.
//! * **`trig_out`** is a 1 ms pulse at every change of the quantized key. The first sample after
//!   the device starts only sets the starting key.
//! * **Custom scale.** `scale.custom` replaces the scale kind and root with the twelve
//!   `scale.note.N` toggles, one per pitch class from C (the project saves them with the device). A
//!   custom scale with no note on restricts nothing, like the chromatic scale.
//! * **Hysteresis** (0..1 semitone) makes the quantizer stay on its key until the input has moved
//!   that far past the middle between two scale keys, so a wobbling input does not chatter between
//!   them. At 0 it is the plain nearest key.
//! * No latency. A non-finite input counts as 0; allocation-free.

use crate::tool_support::{choice, count, cv_in, cv_out, finite, linear, pulse_samples, toggle};
use atomik_core::theory::{Scale, ScaleKind};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};

const P_SCALE: u32 = 0;
const P_ROOT: u32 = 1;
const P_TRANSPOSE: u32 = 2;
const P_CUSTOM: u32 = 3;
/// First of the twelve `scale.note.N` toggles (ids 4..=15, one per pitch class from C).
const P_NOTE0: u32 = 4;
const P_HYSTERESIS: u32 = 16;

/// The custom scale a device starts with: C major.
const C_MAJOR: u16 = 0b1010_1011_0101;

macro_rules! note_param {
    ($n:expr, $key:expr, $name:expr, $on:expr) => {
        toggle(P_NOTE0 + $n, $key, $name, $on)
    };
}

const PITCH_IN: SocketId = SocketId(0);
const PITCH_OUT: SocketId = SocketId(1);
const TRIG_OUT: SocketId = SocketId(2);

/// Keys per unit of pitch CV.
const KEYS_PER_UNIT: f32 = 24.0;
/// The key at CV 0.
const MIDDLE_C: i32 = 60;
/// The input limit.
const LIMIT: f32 = 8.0;

/// The names of [`ScaleKind::ALL`], in order (a test keeps them equal to the piano roll's).
static SCALES: [&str; 16] = [
    "Chromatic",
    "Major",
    "Natural Minor",
    "Harmonic Minor",
    "Melodic Minor",
    "Dorian",
    "Phrygian",
    "Lydian",
    "Mixolydian",
    "Locrian",
    "Major Pentatonic",
    "Minor Pentatonic",
    "Blues",
    "Whole Tone",
    "Phrygian Dominant",
    "Hungarian Minor",
];

static ROOTS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

static PARAMS: [ParamDescriptor; 17] = [
    choice(P_SCALE, "scale", "Scale", &SCALES, 1.0),
    choice(P_ROOT, "root", "Root", &ROOTS, 0.0),
    count(P_TRANSPOSE, "transpose", "Transpose", -24.0, 24.0, 0.0),
    toggle(P_CUSTOM, "scale.custom", "Custom scale", 0.0),
    note_param!(0, "scale.note.0", "Scale note C", 1.0),
    note_param!(1, "scale.note.1", "Scale note C#", 0.0),
    note_param!(2, "scale.note.2", "Scale note D", 1.0),
    note_param!(3, "scale.note.3", "Scale note D#", 0.0),
    note_param!(4, "scale.note.4", "Scale note E", 1.0),
    note_param!(5, "scale.note.5", "Scale note F", 1.0),
    note_param!(6, "scale.note.6", "Scale note F#", 0.0),
    note_param!(7, "scale.note.7", "Scale note G", 1.0),
    note_param!(8, "scale.note.8", "Scale note G#", 0.0),
    note_param!(9, "scale.note.9", "Scale note A", 1.0),
    note_param!(10, "scale.note.10", "Scale note A#", 0.0),
    note_param!(11, "scale.note.11", "Scale note B", 1.0),
    linear(
        P_HYSTERESIS,
        "hysteresis",
        "Hysteresis",
        0.0,
        1.0,
        0.0,
        Unit::Percent,
    ),
];

static SOCKETS: [SocketDescriptor; 3] = [
    cv_in(0, "pitch_cv_in", "Pitch in"),
    cv_out(1, "pitch_cv_out", "Pitch out"),
    cv_out(2, "trig_out", "Change trig"),
];

/// Static description of CV Quantizer.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.cv-quantizer",
    name: "CV Quantizer",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`CvQuantizer`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(CvQuantizer::new()),
    }
}

/// The key of `mask` (a 12-bit pitch-class set) nearest to the fractional key `x`, ties down.
/// A mask with no bits set returns the rounded key.
pub fn nearest_key(mask: u16, x: f32) -> i32 {
    let has = |k: i32| mask & (1 << k.rem_euclid(12)) != 0;
    let f = x.floor() as i32;
    let Some(lo) = (0..12).map(|d| f - d).find(|k| has(*k)) else {
        return x.round() as i32;
    };
    let hi = (1..=12).map(|d| f + d).find(|k| has(*k)).unwrap_or(lo);
    if x - lo as f32 <= hi as f32 - x {
        lo
    } else {
        hi
    }
}

/// Like [`nearest_key`], but the key `current` stays while `x` has not moved more than `hyst`
/// semitones past the middle towards the new key. `current` must be in the scale to be kept.
pub fn nearest_key_sticky(mask: u16, x: f32, current: i32, hyst: f32) -> i32 {
    let q = nearest_key(mask, x);
    if hyst <= 0.0 || q == current || mask != 0 && mask & (1 << current.rem_euclid(12)) == 0 {
        return q;
    }
    // Moving `d` past the middle makes the current key `2 d` farther than the new one.
    if (x - current as f32).abs() - (x - q as f32).abs() <= 2.0 * hyst {
        current
    } else {
        q
    }
}

/// The pitch quantizer.
#[derive(Debug, Clone, Copy)]
pub struct CvQuantizer {
    scale: u8,
    root: u8,
    transpose: i32,
    mask: u16,
    custom: bool,
    custom_mask: u16,
    hysteresis: f32,
    pulse_len: u32,
    pulse_left: u32,
    key: Option<i32>,
    last_in: f32,
    last_key: i32,
}

impl CvQuantizer {
    fn new() -> Self {
        let mut q = Self {
            scale: 1,
            root: 0,
            transpose: 0,
            mask: 0,
            custom: false,
            custom_mask: C_MAJOR,
            hysteresis: 0.0,
            pulse_len: pulse_samples(48_000.0),
            pulse_left: 0,
            key: None,
            last_in: f32::NAN,
            last_key: MIDDLE_C,
        };
        q.derive();
        q
    }

    fn derive(&mut self) {
        let kind = ScaleKind::ALL[usize::from(self.scale)];
        self.mask = if self.custom {
            self.custom_mask
        } else {
            Scale::new(self.root, kind).pitch_mask()
        };
        self.last_in = f32::NAN;
    }
}

impl Device for CvQuantizer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.pulse_len = pulse_samples(info.sample_rate.hz() as f32);
        self.reset();
    }

    fn reset(&mut self) {
        self.pulse_left = 0;
        self.key = None;
        self.last_in = f32::NAN;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_SCALE => {
                self.scale = v.clamp(0.0, (SCALES.len() - 1) as f32).round() as u8;
                self.derive();
            }
            P_ROOT => {
                self.root = v.clamp(0.0, 11.0).round() as u8;
                self.derive();
            }
            P_TRANSPOSE => self.transpose = v.clamp(-24.0, 24.0).round() as i32,
            P_CUSTOM => {
                self.custom = v >= 0.5;
                self.derive();
            }
            n @ P_NOTE0..=15 => {
                let bit = 1u16 << (n - P_NOTE0);
                if v >= 0.5 {
                    self.custom_mask |= bit;
                } else {
                    self.custom_mask &= !bit;
                }
                self.derive();
            }
            P_HYSTERESIS => {
                self.hysteresis = v.clamp(0.0, 1.0);
                self.last_in = f32::NAN;
            }
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let cv = *ctx.input(PITCH_IN);
        let mut pitch = [0.0f32; BATCH];
        let mut trig = [0.0f32; BATCH];
        for n in 0..BATCH {
            let x = finite(cv[n]).clamp(-LIMIT, LIMIT);
            let q = if x.to_bits() == self.last_in.to_bits() {
                self.last_key
            } else {
                let at = MIDDLE_C as f32 + x * KEYS_PER_UNIT;
                let q = match self.key {
                    Some(cur) => nearest_key_sticky(self.mask, at, cur, self.hysteresis),
                    None => nearest_key(self.mask, at),
                };
                self.last_in = x;
                self.last_key = q;
                q
            };
            if let Some(prev) = self.key
                && prev != q
            {
                self.pulse_left = self.pulse_len;
            }
            self.key = Some(q);
            if self.pulse_left > 0 {
                trig[n] = 1.0;
                self.pulse_left -= 1;
            }
            pitch[n] = (q + self.transpose - MIDDLE_C) as f32 / KEYS_PER_UNIT;
        }
        *ctx.output(PITCH_OUT) = pitch;
        *ctx.output(TRIG_OUT) = trig;
    }
}

#[cfg(test)]
#[allow(
    clippy::needless_range_loop,
    clippy::float_cmp,
    clippy::type_complexity
)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;

    fn cv_of(key: f32) -> f32 {
        (key - 60.0) / 24.0
    }

    fn quantize(h: &mut DeviceHarness, key: f32) -> f32 {
        *h.input_mut("pitch_cv_in") = [cv_of(key); BATCH];
        h.process();
        h.output("pitch_cv_out")[BATCH - 1] * 24.0 + 60.0
    }

    #[test]
    fn descriptor_lists_the_keys_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "scale",
                "root",
                "transpose",
                "scale.custom",
                "scale.note.0",
                "scale.note.1",
                "scale.note.2",
                "scale.note.3",
                "scale.note.4",
                "scale.note.5",
                "scale.note.6",
                "scale.note.7",
                "scale.note.8",
                "scale.note.9",
                "scale.note.10",
                "scale.note.11",
                "hysteresis",
            ],
            &["pitch_cv_in", "pitch_cv_out", "trig_out"],
        );
    }

    #[test]
    fn the_scale_names_are_those_of_the_piano_roll() {
        assert_eq!(SCALES.len(), ScaleKind::ALL.len());
        for (name, kind) in SCALES.iter().zip(ScaleKind::ALL) {
            assert_eq!(*name, kind.label());
        }
    }

    #[test]
    fn keys_in_the_scale_pass_and_others_snap_to_the_nearest_scale_key() {
        let mut h = harness(Box::new(CvQuantizer::new())); // C major
        for key in [60.0, 62.0, 64.0, 65.0, 67.0, 69.0, 71.0, 72.0] {
            assert!((quantize(&mut h, key) - key).abs() < 1e-3, "{key}");
        }
        // Just either side of the middle between C and D.
        assert!((quantize(&mut h, 60.95) - 60.0).abs() < 1e-3);
        assert!((quantize(&mut h, 61.05) - 62.0).abs() < 1e-3);
        assert!((quantize(&mut h, 62.9) - 62.0).abs() < 1e-3);
        assert!((quantize(&mut h, 63.1) - 64.0).abs() < 1e-3);
        // Octaves above and below.
        assert!((quantize(&mut h, 49.0) - 48.0).abs() < 1e-3);
        assert!((quantize(&mut h, 80.4) - 81.0).abs() < 1e-3);
    }

    #[test]
    fn an_exact_middle_goes_down() {
        let c_major = Scale::new(0, ScaleKind::Major).pitch_mask();
        assert_eq!(nearest_key(c_major, 61.0), 60);
        assert_eq!(nearest_key(c_major, 63.0), 62);
        assert_eq!(nearest_key(c_major, 66.0), 65);
        assert_eq!(nearest_key(c_major, 66.01), 67);
        assert_eq!(nearest_key(c_major, -1.0), -1);
        assert_eq!(nearest_key(0, 5.4), 5);
    }

    #[test]
    fn every_scale_and_root_agrees_with_the_piano_roll_for_whole_keys() {
        for kind in ScaleKind::ALL {
            for root in 0..12u8 {
                let scale = Scale::new(root, kind);
                for key in 24..=96u8 {
                    let q = nearest_key(scale.pitch_mask(), f32::from(key));
                    assert_eq!(q, i32::from(scale.nearest(key)), "{kind:?} {root} {key}");
                }
            }
        }
    }

    #[test]
    fn root_and_scale_params_choose_the_notes() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        h.set_param("scale", 11.0); // minor pentatonic
        h.set_param("root", 9.0); // A C D E G
        for (input, want) in [(61.0, 60.0), (63.0, 62.0), (65.0, 64.0), (66.0, 67.0)] {
            let got = quantize(&mut h, input);
            assert!((got - want).abs() < 1e-3, "{input} gave {got}");
        }
        h.set_param("scale", 0.0); // chromatic: the nearest semitone
        assert!((quantize(&mut h, 61.0) - 61.0).abs() < 1e-3);
        assert!((quantize(&mut h, 61.4) - 61.0).abs() < 1e-3);
        assert!((quantize(&mut h, 61.6) - 62.0).abs() < 1e-3);
    }

    #[test]
    fn transpose_moves_the_quantized_result() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        h.set_param("transpose", 7.0);
        assert!((quantize(&mut h, 61.0) - 67.0).abs() < 1e-3);
        h.set_param("transpose", -24.0);
        assert!((quantize(&mut h, 64.0) - 40.0).abs() < 1e-3);
    }

    #[test]
    fn trig_pulses_for_one_millisecond_at_each_change_only() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        // The first batch only sets the key.
        *h.input_mut("pitch_cv_in") = [cv_of(60.0); BATCH];
        h.process();
        assert!(h.output("trig_out").iter().all(|v| *v == 0.0));
        // A move inside the same key (60.0 -> 60.3) is not a change.
        *h.input_mut("pitch_cv_in") = [cv_of(60.3); BATCH];
        h.process();
        assert!(h.output("trig_out").iter().all(|v| *v == 0.0));
        // A new key pulses for 48 samples at 48 kHz.
        let mut x = [cv_of(60.3); BATCH];
        x[10..].fill(cv_of(64.0));
        *h.input_mut("pitch_cv_in") = x;
        h.process();
        let t = h.output("trig_out");
        assert_eq!(t[9], 0.0);
        assert_eq!(t[10], 1.0);
        assert_eq!(t.iter().filter(|v| **v == 1.0).count(), 48);
        assert_eq!(t[10 + 47], 1.0);
        assert_eq!(t[10 + 48], 0.0);
        *h.input_mut("pitch_cv_in") = [cv_of(64.0); BATCH];
        for _ in 0..2 {
            h.process();
        }
        assert!(h.output("trig_out").iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_pitch_ramp_gives_a_staircase_of_scale_keys() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        let mask = Scale::new(0, ScaleKind::Major).pitch_mask();
        for b in 0..64 {
            let mut x = [0.0f32; BATCH];
            for (i, v) in x.iter_mut().enumerate() {
                *v = -1.0 + 2.0 * (b * BATCH + i) as f32 / (64 * BATCH) as f32;
            }
            *h.input_mut("pitch_cv_in") = x;
            h.process();
            for v in h.output("pitch_cv_out") {
                let key = (v * 24.0 + 60.0).round() as i32;
                assert!((v * 24.0 + 60.0 - key as f32).abs() < 1e-3);
                assert!(
                    mask & (1 << key.rem_euclid(12)) != 0,
                    "{key} is not in C major"
                );
            }
        }
    }

    #[test]
    fn a_custom_scale_uses_the_twelve_toggles() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        h.set_param("scale.custom", 1.0); // starts as C major
        assert!((quantize(&mut h, 61.0) - 60.0).abs() < 1e-3);
        assert!((quantize(&mut h, 66.1) - 67.0).abs() < 1e-3);
        // C, E and G sharp only: an augmented triad.
        for n in 0..12 {
            h.set_param(
                &format!("scale.note.{n}"),
                if [0, 4, 8].contains(&n) { 1.0 } else { 0.0 },
            );
        }
        for (input, want) in [
            (61.0, 60.0),
            (62.5, 64.0),
            (66.0, 64.0),
            (66.1, 68.0),
            (70.0, 68.0),
        ] {
            let got = quantize(&mut h, input);
            assert!((got - want).abs() < 1e-3, "{input} gave {got}");
        }
        // Switching it off goes back to the scale and root.
        h.set_param("scale.custom", 0.0);
        assert!((quantize(&mut h, 61.0) - 60.0).abs() < 1e-3);
        assert!((quantize(&mut h, 63.1) - 64.0).abs() < 1e-3);
    }

    #[test]
    fn a_custom_scale_with_no_note_restricts_nothing() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        h.set_param("scale.custom", 1.0);
        for n in 0..12 {
            h.set_param(&format!("scale.note.{n}"), 0.0);
        }
        assert!((quantize(&mut h, 61.0) - 61.0).abs() < 1e-3);
        assert!((quantize(&mut h, 61.55) - 62.0).abs() < 1e-3);
    }

    #[test]
    fn the_custom_scale_matches_the_matching_preset_scale() {
        let mut preset = harness(Box::new(CvQuantizer::new()));
        let mut custom = harness(Box::new(CvQuantizer::new()));
        custom.set_param("scale.custom", 1.0);
        let minor = Scale::new(0, ScaleKind::ALL[2]).pitch_mask();
        for n in 0..12 {
            custom.set_param(&format!("scale.note.{n}"), f32::from(minor >> n & 1));
        }
        preset.set_param("scale", 2.0);
        for i in 0..200 {
            let key = 40.0 + i as f32 * 0.31;
            assert_eq!(
                quantize(&mut preset, key),
                quantize(&mut custom, key),
                "{key}"
            );
        }
    }

    #[test]
    fn hysteresis_keeps_the_key_until_the_input_is_well_past_the_middle() {
        let mut h = harness(Box::new(CvQuantizer::new())); // C major: C 60, D 62
        h.set_param("hysteresis", 0.5);
        assert!((quantize(&mut h, 60.0) - 60.0).abs() < 1e-3);
        // The middle is 61; moving up it stays on C until 61.5.
        for key in [61.1, 61.4, 61.45] {
            assert!((quantize(&mut h, key) - 60.0).abs() < 1e-3, "{key}");
        }
        assert!((quantize(&mut h, 61.55) - 62.0).abs() < 1e-3);
        // Coming back down it stays on D until 60.5.
        for key in [61.9, 61.0, 60.55] {
            assert!((quantize(&mut h, key) - 62.0).abs() < 1e-3, "{key}");
        }
        assert!((quantize(&mut h, 60.45) - 60.0).abs() < 1e-3);
        // A jump of a whole step is not held back.
        assert!((quantize(&mut h, 64.0) - 64.0).abs() < 1e-3);
    }

    #[test]
    fn hysteresis_stops_a_wobble_from_chattering() {
        let count = |hyst: f32| {
            let mut h = harness(Box::new(CvQuantizer::new()));
            h.set_param("hysteresis", hyst);
            let mut changes = 0;
            let mut last = quantize(&mut h, 60.0);
            for i in 0..100 {
                let key = 61.0 + if i % 2 == 0 { 0.2 } else { -0.2 };
                let q = quantize(&mut h, key);
                if (q - last).abs() > 1e-3 {
                    changes += 1;
                    last = q;
                }
            }
            changes
        };
        assert!(count(0.0) >= 50);
        assert_eq!(count(0.4), 0);
    }

    #[test]
    fn hysteresis_zero_is_the_plain_nearest_key_and_a_dropped_key_is_not_kept() {
        let c_major = Scale::new(0, ScaleKind::Major).pitch_mask();
        for i in 0..100 {
            let x = 50.0 + i as f32 * 0.37;
            assert_eq!(
                nearest_key_sticky(c_major, x, 61, 0.0),
                nearest_key(c_major, x)
            );
        }
        // 61 is not in C major, so it is never held.
        assert_eq!(nearest_key_sticky(c_major, 61.2, 61, 1.0), 62);
    }

    #[test]
    fn non_finite_input_counts_as_zero_and_sweeps_stay_finite() {
        let mut h = harness(Box::new(CvQuantizer::new()));
        *h.input_mut("pitch_cv_in") = [f32::NAN; BATCH];
        h.process();
        assert!(h.output("pitch_cv_out").iter().all(|v| *v == 0.0));
        *h.input_mut("pitch_cv_in") = [100.0; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
        assert!(h.output("pitch_cv_out").iter().all(|v| v.abs() <= 10.0));
    }
}
