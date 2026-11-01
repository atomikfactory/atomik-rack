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
    fn finite_zeroes_nan_and_infinity() {
        assert_eq!(finite(f32::NAN), 0.0);
        assert_eq!(finite(f32::NEG_INFINITY), 0.0);
        assert_eq!(finite(0.5), 0.5);
    }
}
