//! Parameter descriptors: ranges, tapers, normalization and dB helpers.

/// Identifies a parameter within one device. `ParamId(i)` is index `i` in the device's param table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId(pub u32);

/// Mapping between the normalized 0..=1 control range and plain values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taper {
    /// Plain = min + norm·(max − min).
    Linear,
    /// Plain = min·(max/min)^norm. Requires `min > 0`. Use for frequency and time.
    Exponential,
    /// Linear in dB between `min` and `max` dB; the minimum maps to gain 0 (−∞ dB).
    Decibel,
}

/// Display unit of a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Unitless.
    None,
    /// Hertz.
    Hz,
    /// Milliseconds.
    Ms,
    /// Decibels.
    Db,
    /// Percent (plain value 0..1 shown as 0..100 %).
    Percent,
    /// Semitones.
    Semitones,
    /// Cents.
    Cents,
    /// Octaves.
    Octaves,
    /// One of a list of named choices; plain value is the index.
    Choice(&'static [&'static str]),
    /// On/off; plain value 0 or 1.
    Toggle,
}

/// Static description of one parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDescriptor {
    /// Id == index in the device's param table.
    pub id: ParamId,
    /// Stable machine key, e.g. `"filter.cutoff"`. Used in patch files.
    pub key: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Minimum plain value.
    pub min: f32,
    /// Maximum plain value.
    pub max: f32,
    /// Default plain value.
    pub default: f32,
    /// Normalization curve.
    pub taper: Taper,
    /// Display unit.
    pub unit: Unit,
    /// True for integer-valued params (choices, toggles, counts): plain values are rounded.
    pub discrete: bool,
}

impl ParamDescriptor {
    /// Clamps to `min..=max` and rounds if discrete. NaN maps to `default`. Idempotent.
    pub fn clamp(&self, plain: f32) -> f32 {
        let v = if plain.is_nan() {
            self.default
        } else {
            plain.clamp(self.min, self.max)
        };
        if self.discrete {
            v.round().clamp(self.min, self.max)
        } else {
            v
        }
    }

    /// Maps a plain value to 0..=1 (clamping first).
    pub fn to_normalized(&self, plain: f32) -> f32 {
        let v = self.clamp(plain);
        if self.max <= self.min {
            return 0.0;
        }
        let n = match self.taper {
            Taper::Linear | Taper::Decibel => (v - self.min) / (self.max - self.min),
            Taper::Exponential => (v / self.min).ln() / (self.max / self.min).ln(),
        };
        n.clamp(0.0, 1.0)
    }

    /// Maps 0..=1 (clamped) to a plain value (clamped and rounded if discrete).
    pub fn from_normalized(&self, norm: f32) -> f32 {
        let n = if norm.is_nan() {
            0.0
        } else {
            norm.clamp(0.0, 1.0)
        };
        let v = match self.taper {
            Taper::Linear | Taper::Decibel => self.min + n * (self.max - self.min),
            Taper::Exponential => self.min * (self.max / self.min).powf(n),
        };
        self.clamp(v)
    }

    /// For [`Taper::Decibel`] params: the linear gain of a plain dB value (0 at `min`).
    pub fn db_param_gain(&self, plain_db: f32) -> f32 {
        let db = self.clamp(plain_db);
        if db <= self.min { 0.0 } else { db_to_gain(db) }
    }
}

/// Converts decibels to linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

/// Lowest VCA offset in dB: a VCA fader at this value is off (its members are silent), as the
/// bottom of a send level is.
pub const VCA_MIN_DB: f32 = -60.0;
/// Highest VCA offset in dB.
pub const VCA_MAX_DB: f32 = 12.0;

/// Key of the audio track device's recording input gain param (dB).
pub const REC_GAIN_PARAM: &str = "rec_gain";
/// Key of the audio track device's clip-safe switch param.
pub const REC_CLIP_SAFE_PARAM: &str = "rec_clip_safe";
/// Key of the audio track device's recording source param: 0 records the Audio I/O inputs
/// the track is cabled to, a larger value records the output of a console strip (see
/// `RecSource` in the model).
pub const REC_SOURCE_PARAM: &str = "rec_source";
/// Highest value of [`REC_SOURCE_PARAM`].
pub const REC_SOURCE_MAX: f32 = 2000.0;
/// Lowest recording input gain in dB: the bottom of the range is off.
pub const REC_GAIN_MIN_DB: f32 = -60.0;
/// Highest recording input gain in dB.
pub const REC_GAIN_MAX_DB: f32 = 24.0;

/// Converts linear gain to decibels (−∞ for 0).
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const CUTOFF: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "filter.cutoff",
        name: "Cutoff",
        min: 20.0,
        max: 20_000.0,
        default: 4_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    };
    const LEVEL: ParamDescriptor = ParamDescriptor {
        id: ParamId(1),
        key: "level",
        name: "Level",
        min: -60.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        discrete: false,
    };
    const WAVE: ParamDescriptor = ParamDescriptor {
        id: ParamId(2),
        key: "wave",
        name: "Wave",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["saw", "square", "triangle", "sine"]),
        discrete: true,
    };

    fn rel_close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-3)
    }

    proptest! {
        #[test]
        fn param_exponential_roundtrip_within_tolerance(x in 20.0f32..20_000.0) {
            let back = CUTOFF.from_normalized(CUTOFF.to_normalized(x));
            prop_assert!(rel_close(back, x, 1e-4), "{x} -> {back}");
        }

        #[test]
        fn param_linear_and_decibel_roundtrip(n in 0.0f32..=1.0) {
            for p in [LEVEL, CUTOFF] {
                let back = p.to_normalized(p.from_normalized(n));
                prop_assert!((back - n).abs() < 1e-5);
            }
        }

        #[test]
        fn param_clamp_is_idempotent(x in -1.0e6f32..1.0e6) {
            for p in [CUTOFF, LEVEL, WAVE] {
                let once = p.clamp(x);
                prop_assert!(once.to_bits() == p.clamp(once).to_bits());
                prop_assert!(once >= p.min && once <= p.max);
            }
        }
    }

    #[test]
    fn param_clamp_nan_is_default() {
        assert!(rel_close(CUTOFF.clamp(f32::NAN), 4_000.0, 1e-6));
    }

    #[test]
    fn param_decibel_min_is_silent() {
        assert_eq!(LEVEL.db_param_gain(LEVEL.min).to_bits(), 0.0f32.to_bits());
        assert!(rel_close(LEVEL.db_param_gain(0.0), 1.0, 1e-6));
        assert!(rel_close(LEVEL.from_normalized(0.0), -60.0, 1e-6));
    }

    #[test]
    fn param_discrete_rounds() {
        assert!(rel_close(WAVE.clamp(1.4), 1.0, 0.0));
        assert!(rel_close(WAVE.clamp(1.6), 2.0, 0.0));
        assert!(rel_close(WAVE.from_normalized(0.5), 2.0, 0.0)); // 1.5 rounds half away from zero
    }

    #[test]
    fn db_gain_conversions() {
        assert!(rel_close(db_to_gain(-6.0206), 0.5, 1e-4));
        assert!(rel_close(gain_to_db(2.0), 6.0206, 1e-4));
        assert!(gain_to_db(0.0).is_infinite() && gain_to_db(0.0) < 0.0);
    }
}
