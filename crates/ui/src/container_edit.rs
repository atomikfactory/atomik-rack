//! Pure editing of a container's note splits and macro targets for the container editor:
//! the drag of a split's edges and body, the split list
//! after an add, a delete, a retarget or a transpose step, and the macro range fields. Every
//! function ends in the model's rules (`key_lo <= key_hi`, velocity 1 to 127, transpose -48 to
//! 48, at most [`MAX_SPLITS`] splits, one split per target), so the editor can never emit a
//! `SetSplits` the model rejects.

use atomik_core::ParamDescriptor;
use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_model::{ContainerDoc, DeviceKey, MAX_SPLITS, MacroTarget, NoteSplit};

/// Largest transpose, in semitones.
pub const TRANSPOSE_MAX: i8 = 48;

/// The part of a split bar a press picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The left edge: `key_lo`.
    Left,
    /// The right edge: `key_hi`.
    Right,
    /// The top edge: `vel_hi`.
    Top,
    /// The bottom edge: `vel_lo`.
    Bottom,
    /// The body: the whole zone moves.
    Body,
}

/// `split` after dragging `edge` by `dk` keys and `dv` velocity steps (up is positive), from
/// the state at the press. A range never inverts: an edge stops at the opposite one.
pub fn drag_split(base: &NoteSplit, edge: Edge, dk: i32, dv: i32) -> NoteSplit {
    let mut s = base.clone();
    let key = |v: i32| v.clamp(0, 127) as u8;
    let vel = |v: i32| v.clamp(1, 127) as u8;
    match edge {
        Edge::Left => {
            s.key_lo = key(i32::from(base.key_lo) + dk).min(base.key_hi);
        }
        Edge::Right => {
            s.key_hi = key(i32::from(base.key_hi) + dk).max(base.key_lo);
        }
        Edge::Top => {
            s.vel_hi = vel(i32::from(base.vel_hi) + dv).max(base.vel_lo);
        }
        Edge::Bottom => {
            s.vel_lo = vel(i32::from(base.vel_lo) + dv).min(base.vel_hi);
        }
        Edge::Body => {
            // Both ends move by the same amount, as far as the range allows.
            let dk = dk.clamp(-i32::from(base.key_lo), 127 - i32::from(base.key_hi));
            let dv = dv.clamp(1 - i32::from(base.vel_lo), 127 - i32::from(base.vel_hi));
            s.key_lo = key(i32::from(base.key_lo) + dk);
            s.key_hi = key(i32::from(base.key_hi) + dk);
            s.vel_lo = vel(i32::from(base.vel_lo) + dv);
            s.vel_hi = vel(i32::from(base.vel_hi) + dv);
        }
    }
    s
}

/// `list` with split `i` replaced by `split`.
pub fn with_split(list: &[NoteSplit], i: usize, split: NoteSplit) -> Vec<NoteSplit> {
    let mut v = list.to_vec();
    if let Some(slot) = v.get_mut(i) {
        *slot = split;
    }
    v
}

/// `list` without split `i`.
pub fn without_split(list: &[NoteSplit], i: usize) -> Vec<NoteSplit> {
    let mut v = list.to_vec();
    if i < v.len() {
        v.remove(i);
    }
    v
}

/// A split's transpose moved by `by` semitones, clamped to -48..=48.
pub fn transposed(s: &NoteSplit, by: i32) -> NoteSplit {
    let t =
        (i32::from(s.transpose) + by).clamp(-i32::from(TRANSPOSE_MAX), i32::from(TRANSPOSE_MAX));
    NoteSplit {
        transpose: t as i8,
        ..s.clone()
    }
}

/// The inner devices a split can target: they have a Notes input, no split yet and no inner
/// cable on that input. In document order.
pub fn eligible_targets(inner: &ContainerDoc, registry: &Registry) -> Vec<DeviceKey> {
    inner
        .devices
        .iter()
        .filter(|d| {
            let Some(notes) = registry.get(&d.type_id).and_then(|f| {
                f.descriptor
                    .sockets
                    .iter()
                    .find(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
            }) else {
                return false;
            };
            !inner.splits.iter().any(|s| s.target == d.key)
                && !inner
                    .cables
                    .iter()
                    .any(|c| c.to.device == d.key && c.to.socket == notes.key)
        })
        .map(|d| d.key.clone())
        .collect()
}

/// The target a split would move to when its selector is clicked: the next device in document
/// order after its current one that is free, wrapping; `None` when there is no other.
pub fn next_target(
    inner: &ContainerDoc,
    registry: &Registry,
    current: &DeviceKey,
) -> Option<DeviceKey> {
    let free = eligible_targets(inner, registry);
    if free.is_empty() {
        return None;
    }
    let order: Vec<&DeviceKey> = inner.devices.iter().map(|d| &d.key).collect();
    let at = order.iter().position(|k| *k == current).unwrap_or(0);
    (1..=order.len())
        .map(|step| order[(at + step) % order.len()])
        .find(|k| free.contains(k))
        .cloned()
}

/// The split list after ADD: a full-range split on the next free device, if there is room.
pub fn added(inner: &ContainerDoc, registry: &Registry) -> Option<Vec<NoteSplit>> {
    if inner.splits.len() >= MAX_SPLITS {
        return None;
    }
    let target = eligible_targets(inner, registry).into_iter().next()?;
    let mut v = inner.splits.clone();
    v.push(NoteSplit::full(target));
    Some(v)
}

/// Which end of a macro target's range a field edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// The value at macro 0.
    Min,
    /// The value at macro 1.
    Max,
}

/// `t` with one end moved by `pixels` of drag (up is positive): 200 px cover the whole range
/// in the param's normalized domain, `fine` makes it ten times slower. The value stays in the
/// descriptor's range.
pub fn dragged_bound(
    t: &MacroTarget,
    p: &ParamDescriptor,
    bound: Bound,
    start: f32,
    pixels: f64,
    fine: bool,
) -> MacroTarget {
    let step = pixels / 200.0 * if fine { 0.1 } else { 1.0 };
    let n = f64::from(p.to_normalized(start)) + step;
    let v = p.from_normalized(n.clamp(0.0, 1.0) as f32);
    let v = v.clamp(p.min, p.max);
    let mut out = t.clone();
    match bound {
        Bound::Min => out.min = v,
        Bound::Max => out.max = v,
    }
    out
}

/// `t` with one end reset to the descriptor's range end.
pub fn reset_bound(t: &MacroTarget, p: &ParamDescriptor, bound: Bound) -> MacroTarget {
    let mut out = t.clone();
    match bound {
        Bound::Min => out.min = p.min,
        Bound::Max => out.max = p.max,
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> NoteSplit {
        NoteSplit {
            target: DeviceKey("a".into()),
            key_lo: 40,
            key_hi: 60,
            vel_lo: 20,
            vel_hi: 100,
            transpose: 0,
        }
    }

    #[test]
    fn edges_move_one_end_and_never_cross() {
        let b = s();
        assert_eq!(drag_split(&b, Edge::Left, -10, 0).key_lo, 30);
        assert_eq!(
            drag_split(&b, Edge::Left, 99, 0).key_lo,
            60,
            "stops at key_hi"
        );
        assert_eq!(drag_split(&b, Edge::Left, -99, 0).key_lo, 0);
        assert_eq!(drag_split(&b, Edge::Right, 99, 0).key_hi, 127);
        assert_eq!(
            drag_split(&b, Edge::Right, -99, 0).key_hi,
            40,
            "stops at key_lo"
        );
        assert_eq!(drag_split(&b, Edge::Top, 0, 99).vel_hi, 127);
        assert_eq!(drag_split(&b, Edge::Top, 0, -99).vel_hi, 20);
        assert_eq!(drag_split(&b, Edge::Bottom, 0, -99).vel_lo, 1);
        assert_eq!(drag_split(&b, Edge::Bottom, 0, 99).vel_lo, 100);
        // The other fields stay.
        let l = drag_split(&b, Edge::Left, -5, 7);
        assert_eq!((l.key_hi, l.vel_lo, l.vel_hi), (60, 20, 100));
    }

    #[test]
    fn the_body_moves_both_ends_and_keeps_the_width() {
        let b = s();
        let m = drag_split(&b, Edge::Body, 10, -5);
        assert_eq!((m.key_lo, m.key_hi, m.vel_lo, m.vel_hi), (50, 70, 15, 95));
        let m = drag_split(&b, Edge::Body, 500, 500);
        assert_eq!((m.key_lo, m.key_hi), (107, 127));
        assert_eq!((m.vel_lo, m.vel_hi), (47, 127));
        let m = drag_split(&b, Edge::Body, -500, -500);
        assert_eq!((m.key_lo, m.key_hi), (0, 20));
        assert_eq!((m.vel_lo, m.vel_hi), (1, 81));
    }

    #[test]
    fn transpose_steps_are_clamped() {
        assert_eq!(transposed(&s(), 12).transpose, 12);
        assert_eq!(transposed(&s(), 99).transpose, 48);
        assert_eq!(transposed(&s(), -99).transpose, -48);
    }

    #[test]
    fn list_edits() {
        let l = vec![s(), NoteSplit::full(DeviceKey("b".into()))];
        assert_eq!(without_split(&l, 0), vec![l[1].clone()]);
        assert_eq!(without_split(&l, 5), l);
        let r = with_split(&l, 1, s());
        assert_eq!(r[1], s());
        assert_eq!(with_split(&l, 9, s()), l);
    }

    #[test]
    fn bound_drags_follow_the_taper_and_stay_in_range() {
        let reg = {
            let mut r = Registry::new();
            atomik_devices::register_all(&mut r).unwrap();
            r
        };
        let p = reg
            .get("atomik.poly-synth")
            .unwrap()
            .descriptor
            .param_by_key("filter.cutoff")
            .copied()
            .unwrap();
        let t = MacroTarget {
            device: DeviceKey("synth".into()),
            param: "filter.cutoff".into(),
            min: p.min,
            max: p.max,
        };
        let up = dragged_bound(&t, &p, Bound::Min, t.min, 40.0, false);
        assert!(up.min > t.min && up.max.to_bits() == t.max.to_bits());
        let way = dragged_bound(&t, &p, Bound::Max, t.max, 9999.0, false);
        assert_eq!(way.max, p.max);
        let down = dragged_bound(&t, &p, Bound::Max, t.max, -9999.0, false);
        assert_eq!(down.max, p.min);
        let fine = dragged_bound(&t, &p, Bound::Min, t.min, 40.0, true);
        assert!(fine.min < up.min && fine.min > t.min);
        assert_eq!(reset_bound(&down, &p, Bound::Max).max, p.max);
    }
}
