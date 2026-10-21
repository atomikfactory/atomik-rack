//! Widget value semantics. Every value goes through the [`ParamDescriptor`]:
//! drags and steps work in the normalized domain and results are clamped plain values.
//!
//! - Knob: vertical drag, [`KNOB_PIXELS_PER_RANGE`] logical pixels (screen space, independent
//!   of zoom) sweep the full range; Shift divides by [`FINE_DIVISOR`]; double-click resets to
//!   the default; one wheel notch moves [`WHEEL_STEP`] of the range (one step for discrete
//!   params).
//! - Fader: like a knob with [`FADER_PIXELS_PER_RANGE`].
//! - Choice: click cycles forward (wrapping), right-click or wheel-down goes back.
//! - Toggle: click flips.

use atomik_core::{ParamDescriptor, Unit};
use atomik_model::RackDevice;

/// Logical pixels of vertical drag for the full range of a knob.
pub const KNOB_PIXELS_PER_RANGE: f64 = 200.0;
/// Logical pixels of vertical drag for the full range of a fader.
pub const FADER_PIXELS_PER_RANGE: f64 = 150.0;
/// Fine-adjust (Shift) divisor.
pub const FINE_DIVISOR: f64 = 10.0;
/// Normalized change per wheel notch for continuous params.
pub const WHEEL_STEP: f64 = 0.02;

/// The current plain value of a param on a device (descriptor default when unset; clamped).
pub fn current_value(device: &RackDevice, p: &ParamDescriptor) -> f32 {
    let v = device.params.get(p.key).copied().unwrap_or(p.default);
    p.clamp(v)
}

/// A relative vertical drag in the normalized domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueDrag {
    norm: f64,
    pixels_per_range: f64,
}

impl ValueDrag {
    /// Starts a drag from `plain`.
    pub fn new(p: &ParamDescriptor, plain: f32, pixels_per_range: f64) -> Self {
        Self {
            norm: f64::from(p.to_normalized(plain)),
            pixels_per_range: pixels_per_range.max(1.0),
        }
    }

    /// Applies `dy_up` logical pixels of upward movement (negative = down); returns the new
    /// clamped plain value.
    pub fn update(&mut self, p: &ParamDescriptor, dy_up: f64, fine: bool) -> f32 {
        let scale = if fine { FINE_DIVISOR } else { 1.0 };
        self.norm = (self.norm + dy_up / self.pixels_per_range / scale).clamp(0.0, 1.0);
        p.from_normalized(self.norm as f32)
    }
}

/// The value after `notches` wheel notches (positive = increase).
pub fn wheel_value(p: &ParamDescriptor, plain: f32, notches: f64, fine: bool) -> f32 {
    if p.discrete {
        // One step per notch; fine has no meaning for whole steps.
        return p.clamp(p.clamp(plain) + notches.round() as f32);
    }
    let scale = if fine { FINE_DIVISOR } else { 1.0 };
    let n = f64::from(p.to_normalized(plain)) + notches * WHEEL_STEP / scale;
    p.from_normalized(n.clamp(0.0, 1.0) as f32)
}

/// Next (or previous) choice, wrapping.
pub fn cycle_choice(p: &ParamDescriptor, plain: f32, forward: bool) -> f32 {
    let v = p.clamp(plain);
    let (lo, hi) = (p.min.round(), p.max.round());
    let next = if forward { v + 1.0 } else { v - 1.0 };
    if next > hi {
        lo
    } else if next < lo {
        hi
    } else {
        p.clamp(next)
    }
}

/// The flipped toggle value.
pub fn toggled(p: &ParamDescriptor, plain: f32) -> f32 {
    let mid = (p.min + p.max) / 2.0;
    if p.clamp(plain) > mid { p.min } else { p.max }
}

/// Human-readable value with unit.
pub fn format_value(p: &ParamDescriptor, plain: f32) -> String {
    let v = p.clamp(plain);
    match p.unit {
        Unit::Choice(names) => names
            .get(v.max(0.0) as usize)
            .map_or_else(|| format!("{v:.0}"), |s| (*s).to_string()),
        Unit::Toggle => if v >= 0.5 { "On" } else { "Off" }.to_string(),
        Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        Unit::Hz if v >= 100.0 => format!("{v:.0} Hz"),
        Unit::Hz => format!("{v:.1} Hz"),
        Unit::Ms if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
        Unit::Ms if v >= 100.0 => format!("{v:.0} ms"),
        Unit::Ms => format!("{v:.1} ms"),
        Unit::Db if matches!(p.taper, atomik_core::Taper::Decibel) && v <= p.min => {
            "-inf dB".to_string()
        }
        Unit::Db => format!("{v:.1} dB"),
        Unit::Percent => format!("{:.0} %", v * 100.0),
        Unit::Semitones => format!("{} st", number(v, p.discrete)),
        Unit::Cents => format!("{} ct", number(v, p.discrete)),
        Unit::Octaves => format!("{} oct", number(v, p.discrete)),
        Unit::None => number(v, p.discrete),
    }
}

fn number(v: f32, discrete: bool) -> String {
    if discrete || v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::{ParamId, Taper};
    use atomik_model::DeviceKey;
    use std::collections::BTreeMap;

    const LIN: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "a.lin",
        name: "Lin",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    };
    const EXP: ParamDescriptor = ParamDescriptor {
        key: "a.exp",
        min: 20.0,
        max: 20_000.0,
        default: 1000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        ..LIN
    };
    const DISC: ParamDescriptor = ParamDescriptor {
        key: "a.oct",
        min: -2.0,
        max: 2.0,
        default: 0.0,
        unit: Unit::Octaves,
        discrete: true,
        ..LIN
    };
    const CHOICE: ParamDescriptor = ParamDescriptor {
        key: "a.wave",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: Unit::Choice(&["saw", "sq", "tri"]),
        discrete: true,
        ..LIN
    };
    const TOGGLE: ParamDescriptor = ParamDescriptor {
        key: "a.on",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: Unit::Toggle,
        discrete: true,
        ..LIN
    };
    const DB: ParamDescriptor = ParamDescriptor {
        key: "ch1.level",
        min: -60.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        ..LIN
    };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn knob_full_range_is_200_pixels() {
        let mut d = ValueDrag::new(&LIN, 0.0, KNOB_PIXELS_PER_RANGE);
        assert!(close(d.update(&LIN, 100.0, false), 0.5));
        assert!(close(d.update(&LIN, 100.0, false), 1.0));
    }

    #[test]
    fn knob_drag_is_in_normalized_domain_for_exponential() {
        let mut d = ValueDrag::new(&EXP, 20.0, KNOB_PIXELS_PER_RANGE);
        let mid = d.update(&EXP, 100.0, false);
        assert!((mid - 632.455).abs() < 0.1, "{mid}");
    }

    #[test]
    fn fine_modifier_divides_by_ten() {
        let mut d = ValueDrag::new(&LIN, 0.0, KNOB_PIXELS_PER_RANGE);
        assert!(close(d.update(&LIN, 100.0, true), 0.05));
        // Mixed: coarse then fine accumulate.
        assert!(close(d.update(&LIN, 20.0, false), 0.15));
    }

    #[test]
    fn drag_clamps_and_reverses_immediately_at_the_ends() {
        let mut d = ValueDrag::new(&LIN, 0.5, KNOB_PIXELS_PER_RANGE);
        assert!(close(d.update(&LIN, 1000.0, false), 1.0));
        assert!(close(d.update(&LIN, -20.0, false), 0.9));
        assert!(close(d.update(&LIN, -5000.0, false), 0.0));
    }

    #[test]
    fn discrete_params_round() {
        let mut d = ValueDrag::new(&DISC, 0.0, KNOB_PIXELS_PER_RANGE);
        // 4 steps over 200 px: 30 px = 0.6 step -> rounds to 1.
        assert!(close(d.update(&DISC, 30.0, false), 1.0));
    }

    #[test]
    fn wheel_steps_continuous_and_discrete_and_clamps() {
        assert!(close(wheel_value(&LIN, 0.5, 1.0, false), 0.52));
        assert!(close(wheel_value(&LIN, 0.5, -1.0, true), 0.498));
        assert!(close(wheel_value(&LIN, 0.99, 5.0, false), 1.0));
        assert!(close(wheel_value(&DISC, 0.0, 1.0, false), 1.0));
        assert!(close(wheel_value(&DISC, 2.0, 1.0, false), 2.0));
        assert!(close(wheel_value(&DISC, 0.0, -2.0, false), -2.0));
    }

    #[test]
    fn choice_cycles_and_wraps_both_ways() {
        assert!(close(cycle_choice(&CHOICE, 0.0, true), 1.0));
        assert!(close(cycle_choice(&CHOICE, 2.0, true), 0.0));
        assert!(close(cycle_choice(&CHOICE, 0.0, false), 2.0));
    }

    #[test]
    fn toggle_flips() {
        assert!(close(toggled(&TOGGLE, 0.0), 1.0));
        assert!(close(toggled(&TOGGLE, 1.0), 0.0));
    }

    #[test]
    fn current_value_defaults_and_clamps() {
        let mut dev = RackDevice {
            key: DeviceKey("d".into()),
            type_id: "t".into(),
            name: "D".into(),
            params: BTreeMap::new(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            inner: None,
            plugin: None,
            column: 0,
        };
        assert!(close(current_value(&dev, &LIN), 0.25));
        dev.params.insert("a.lin".into(), 7.0);
        assert!(close(current_value(&dev, &LIN), 1.0));
    }

    #[test]
    fn values_format_with_units() {
        assert_eq!(format_value(&EXP, 1500.0), "1.50 kHz");
        assert_eq!(format_value(&EXP, 440.0), "440 Hz");
        assert_eq!(format_value(&LIN, 0.3), "30 %");
        assert_eq!(format_value(&DB, -60.0), "-inf dB");
        assert_eq!(format_value(&DB, -6.0), "-6.0 dB");
        assert_eq!(format_value(&CHOICE, 1.0), "sq");
        assert_eq!(format_value(&TOGGLE, 1.0), "On");
        assert_eq!(format_value(&DISC, -1.0), "-1 oct");
    }
}
