//! Cable geometry and compatibility.
//!
//! A cable is a cubic Bézier whose two inner control points hang below the chord by a sag
//! proportional to the horizontal distance ([`SAG_PER_DX`], at least [`MIN_SAG`]).

use atomik_device_sdk::{Direction, SocketKind};
use atomik_model::{Cable, SocketRef};
use vello::kurbo::{CubicBez, ParamCurveNearest, Point, Vec2};

/// Sag added per logical unit of horizontal distance.
pub const SAG_PER_DX: f64 = 0.3;
/// Minimum sag (short or vertical cables still hang a little).
pub const MIN_SAG: f64 = 22.0;
/// Maximum sag.
pub const MAX_SAG: f64 = 260.0;
/// Hover tolerance around a cable, in rack units.
pub const HIT_TOLERANCE: f64 = 6.0;

/// The sagging curve between two jack centres.
pub fn cable_curve(a: Point, b: Point) -> CubicBez {
    let dx = (b.x - a.x).abs();
    let sag = (MIN_SAG + SAG_PER_DX * dx).min(MAX_SAG);
    let down = Vec2::new(0.0, sag);
    CubicBez::new(
        a,
        a.lerp(b, 1.0 / 3.0) + down,
        a.lerp(b, 2.0 / 3.0) + down,
        b,
    )
}

/// True when `p` is within `tolerance` of the curve.
pub fn hits_curve(curve: &CubicBez, p: Point, tolerance: f64) -> bool {
    curve.nearest(p, 0.05).distance_sq <= tolerance * tolerance
}

/// A socket's identity and electrical type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketEnd {
    /// Which socket.
    pub at: SocketRef,
    /// Signal kind.
    pub kind: SocketKind,
    /// Direction seen from its device.
    pub direction: Direction,
}

/// Whether a cable dragged from `anchor` may be dropped on `target`: one output and one input
/// (a plain or summing input), same kind, different devices. Cycles are checked by the model
/// when the command is applied.
pub fn compatible(anchor: &SocketEnd, target: &SocketEnd) -> bool {
    anchor.direction.is_input() != target.direction.is_input()
        && anchor.kind == target.kind
        && anchor.at.device != target.at.device
}

/// The cable connecting two compatible ends, oriented output → input.
pub fn orient(a: &SocketEnd, b: &SocketEnd) -> Cable {
    if a.direction == Direction::Output {
        Cable {
            from: a.at.clone(),
            to: b.at.clone(),
        }
    } else {
        Cable {
            from: b.at.clone(),
            to: a.at.clone(),
        }
    }
}

/// What a cable is for, which decides its colour: the signal kind, and for audio the console
/// routing it belongs to (an insert loop, a send or return, a sidechain key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableRole {
    /// Plain audio.
    Audio,
    /// A control signal.
    Cv,
    /// Notes.
    Notes,
    /// A channel's insert loop (`insert_send`, `insert_return`).
    Insert,
    /// A send or return of the master section (`fx_send1_l`, `return1_l`) or a channel send.
    Send,
    /// A sidechain key input (`sc_in_l`).
    Sidechain,
}

/// The role of `cable`, whose signal is of `kind`.
pub fn role_of(cable: &Cable, kind: SocketKind) -> CableRole {
    match kind {
        SocketKind::Cv => CableRole::Cv,
        SocketKind::Notes => CableRole::Notes,
        SocketKind::Audio => {
            let on = |f: fn(&str) -> bool| f(&cable.from.socket) || f(&cable.to.socket);
            if on(|s| s.starts_with("insert_send") || s.starts_with("insert_return")) {
                CableRole::Insert
            } else if on(|s| s.starts_with("fx_send") || s.starts_with("return")) {
                CableRole::Send
            } else if on(|s| s.starts_with("sc_in")) {
                CableRole::Sidechain
            } else {
                CableRole::Audio
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::DeviceKey;

    fn end(dev: &str, s: &str, kind: SocketKind, direction: Direction) -> SocketEnd {
        SocketEnd {
            at: SocketRef {
                device: DeviceKey(dev.into()),
                socket: s.into(),
            },
            kind,
            direction,
        }
    }

    #[test]
    fn compatibility_rules() {
        let out = end("a", "out", SocketKind::Audio, Direction::Output);
        assert!(compatible(
            &out,
            &end("b", "in", SocketKind::Audio, Direction::Input)
        ));
        assert!(!compatible(
            &out,
            &end("b", "in", SocketKind::Cv, Direction::Input)
        ));
        assert!(!compatible(
            &out,
            &end("b", "o", SocketKind::Audio, Direction::Output)
        ));
        assert!(!compatible(
            &out,
            &end("a", "in", SocketKind::Audio, Direction::Input)
        ));
    }

    fn cable(from: &str, to: &str) -> Cable {
        let at = |s: &str| {
            let (d, k) = s.split_once(':').unwrap();
            SocketRef {
                device: DeviceKey(d.into()),
                socket: k.into(),
            }
        };
        Cable {
            from: at(from),
            to: at(to),
        }
    }

    #[test]
    fn roles_follow_the_kind_and_the_console_socket_names() {
        let audio = SocketKind::Audio;
        let role = |a: &str, b: &str, kind| role_of(&cable(a, b), kind);
        assert_eq!(role("s:out_l", "ch:in_l", audio), CableRole::Audio);
        assert_eq!(
            role("ch:insert_send_l", "fx:in_l", audio),
            CableRole::Insert
        );
        assert_eq!(
            role("fx:out_l", "ch:insert_return_l", audio),
            CableRole::Insert
        );
        assert_eq!(role("m:fx_send1_l", "fx:in_l", audio), CableRole::Send);
        assert_eq!(role("fx:out_l", "m:return1_l", audio), CableRole::Send);
        assert_eq!(role("s:out_l", "c:sc_in_l", audio), CableRole::Sidechain);
        assert_eq!(role("a:cv_out", "b:cv_in", SocketKind::Cv), CableRole::Cv);
        assert_eq!(
            role("a:notes_out", "b:notes_in", SocketKind::Notes),
            CableRole::Notes
        );
    }

    #[test]
    fn orient_puts_the_output_first() {
        let o = end("a", "out", SocketKind::Cv, Direction::Output);
        let i = end("b", "in", SocketKind::Cv, Direction::Input);
        assert_eq!(orient(&i, &o), orient(&o, &i));
        assert_eq!(orient(&i, &o).from.socket, "out");
    }

    #[test]
    fn curve_hit_testing() {
        let c = cable_curve(Point::new(0.0, 0.0), Point::new(300.0, 0.0));
        let mid = vello::kurbo::ParamCurve::eval(&c, 0.5);
        assert!(mid.y > 50.0);
        assert!(hits_curve(&c, mid + Vec2::new(0.0, 4.0), HIT_TOLERANCE));
        assert!(!hits_curve(&c, mid + Vec2::new(0.0, 20.0), HIT_TOLERANCE));
    }

    #[test]
    fn summing_inputs_accept_outputs_but_not_inputs() {
        let out = end("a", "out", SocketKind::Audio, Direction::Output);
        let bus = end("m", "bus_l", SocketKind::Audio, Direction::SummingInput);
        let inp = end("b", "in", SocketKind::Audio, Direction::Input);
        assert!(compatible(&out, &bus));
        assert!(compatible(&bus, &out));
        assert!(!compatible(&bus, &inp));
        assert!(!compatible(&inp, &bus));
        let c = orient(&bus, &out);
        assert_eq!(c.from, out.at);
        assert_eq!(c.to, bus.at);
    }
}
