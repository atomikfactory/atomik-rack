//! Descriptor builders and small helpers shared by the utility devices (gain tool, stereo tool,
//! gate, dynamics, sidechain tool, envelope filter, the two spiders): `const` constructors so a
//! param or socket table reads as one line per entry, and the stereo-balance gains.

use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{Direction, SocketDescriptor, SocketId, SocketKind};

/// A param row.
#[allow(clippy::too_many_arguments)]
pub(crate) const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    taper: Taper,
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete: false,
    }
}

/// A linear continuous param row.
pub(crate) const fn linear(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: Unit,
) -> ParamDescriptor {
    param(id, key, name, min, max, default, Taper::Linear, unit)
}

/// An exponential continuous param row (`min` > 0).
pub(crate) const fn expo(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: Unit,
) -> ParamDescriptor {
    param(id, key, name, min, max, default, Taper::Exponential, unit)
}

/// An on/off param row.
pub(crate) const fn toggle(
    id: u32,
    key: &'static str,
    name: &'static str,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 1.0,
        default,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    }
}

/// A choice param row (`default` is an index into `choices`).
pub(crate) const fn choice(
    id: u32,
    key: &'static str,
    name: &'static str,
    choices: &'static [&'static str],
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: (choices.len() - 1) as f32,
        default,
        taper: Taper::Linear,
        unit: Unit::Choice(choices),
        discrete: true,
    }
}

/// A whole-number param row `min..=max` (shown as a plain number).
pub(crate) const fn count(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    }
}

/// A `*_cv_trim` row: -1..+1, default +1.
pub(crate) const fn trim(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    linear(id, key, name, -1.0, 1.0, 1.0, Unit::Percent)
}

/// A socket row.
pub(crate) const fn sock(
    id: u16,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind,
        direction,
    }
}

/// Audio input row.
pub(crate) const fn audio_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Audio, Direction::Input)
}

/// Audio output row.
pub(crate) const fn audio_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Audio, Direction::Output)
}

/// CV input row.
pub(crate) const fn cv_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Cv, Direction::Input)
}

/// CV output row.
pub(crate) const fn cv_out(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    sock(id, key, name, SocketKind::Cv, Direction::Output)
}

/// A gate counts as high above this level...
pub(crate) const GATE_HIGH: f32 = 0.5;
/// ...and as low again below this one.
pub(crate) const GATE_LOW: f32 = 0.3;

/// A gate input with hysteresis: high above [`GATE_HIGH`], low below [`GATE_LOW`], unchanged in
/// between (a non-finite level counts as low).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GateIn {
    high: bool,
}

impl GateIn {
    /// Feeds one sample and returns whether the gate is high now.
    #[inline]
    pub(crate) fn update(&mut self, x: f32) -> bool {
        if x >= GATE_HIGH {
            self.high = true;
        } else if x < GATE_LOW || x.is_nan() {
            self.high = false;
        }
        self.high
    }

    /// Feeds one sample and returns whether the gate went from low to high with it.
    #[inline]
    pub(crate) fn rose(&mut self, x: f32) -> bool {
        let was = self.high;
        self.update(x) && !was
    }

    /// Back to low.
    pub(crate) fn reset(&mut self) {
        self.high = false;
    }
}

/// Length of a 1 ms trigger pulse in samples (at least one).
pub(crate) fn pulse_samples(sample_rate: f32) -> u32 {
    ((sample_rate * 0.001).round() as u32).max(1)
}

/// The note-value names of the tempo-synced choices, in the order of
/// [`crate::stereo_delay::SYNC_BEATS`].
pub(crate) static DIVISIONS: [&str; 12] = [
    "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2", "1/1",
];

/// The one-pole coefficient `1 - exp(-1 / (ms * sample_rate / 1000))` of a time constant, from the
/// deterministic `2^x` (the same bits on every platform). A time of one sample or less gives 1
/// (instant). Devices with long times keep their state in `f64` so a small step still counts.
pub(crate) fn time_coef(ms: f32, sample_rate: f32) -> f64 {
    let samples = f64::from(ms) * 0.001 * f64::from(sample_rate);
    if samples <= 1.0 {
        return 1.0;
    }
    1.0 - atomik_dsp::exp2_det_f64(-std::f64::consts::LOG2_E / samples)
}

/// `x` with a non-finite value replaced by 0.
#[inline]
pub(crate) fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Left and right gains of a balance control `-1..=1`: unity on both at the centre, the side away
/// from the balance falls linearly to silence at the end.
#[inline]
pub(crate) fn balance_gains(balance: f32) -> (f32, f32) {
    let b = balance.clamp(-1.0, 1.0);
    ((1.0 - b).min(1.0), (1.0 + b).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_is_unity_at_the_centre_and_silences_the_far_side() {
        assert_eq!(balance_gains(0.0), (1.0, 1.0));
        assert_eq!(balance_gains(1.0), (0.0, 1.0));
        assert_eq!(balance_gains(-1.0), (1.0, 0.0));
        assert_eq!(balance_gains(0.5), (0.5, 1.0));
        assert_eq!(balance_gains(9.0), (0.0, 1.0));
    }

    #[test]
    fn gate_in_has_hysteresis() {
        let mut g = GateIn::default();
        assert!(!g.update(0.4));
        assert!(g.update(0.6));
        assert!(g.update(0.4));
        assert!(!g.update(0.2));
        assert!(!g.update(0.4));
        assert!(g.update(1.0));
        assert!(!g.update(f32::NAN));
        assert!(g.rose(0.9));
        assert!(!g.rose(0.9));
        assert!(!g.rose(0.0));
        assert!(g.rose(0.5));
    }

    #[test]
    fn time_coefficients_are_instant_at_zero_and_slower_for_longer_times() {
        assert_eq!(time_coef(0.0, 48_000.0), 1.0);
        let fast = time_coef(1.0, 48_000.0);
        let slow = time_coef(100.0, 48_000.0);
        assert!(fast > slow && slow > 0.0 && fast < 1.0);
        // One time constant reaches 1 - 1/e.
        let n = (0.1 * 48_000.0) as i32;
        let v = 1.0 - (1.0 - slow).powi(n);
        assert!((v - (1.0 - (-1.0f64).exp())).abs() < 1e-3);
    }

    #[test]
    fn finite_zeroes_nan_and_infinity() {
        assert_eq!(finite(f32::NAN), 0.0);
        assert_eq!(finite(f32::NEG_INFINITY), 0.0);
        assert_eq!(finite(0.5), 0.5);
    }
}
