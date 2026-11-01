//! Hits of a drum step that start after the step does: the second hit of a flam and the repeats
//! of a ratchet.
//!
//! The generator plays a step's first hit when the step starts and parks the rest here, each
//! with the tick it is due at and the tick it ends. The table has a fixed size (no allocation on
//! the audio thread); a hit that does not fit is dropped, never an earlier one. Nothing sounds
//! from here: a hit becomes a note only when it is due, and the note then has its scheduled end,
//! so every On has its Off whatever happens to the table (a jump or an all-off just clears it).

use atomik_device_sdk::{MAX_FLAM, MAX_RATCHET};

/// Slots of the table: a full step of sixteen lanes at the largest ratchet.
pub(super) const CAP: usize = 16 * MAX_RATCHET as usize * 2;

/// One parked hit.
#[derive(Debug, Clone, Copy)]
pub(super) struct Hit {
    /// When the hit starts.
    pub tick: f64,
    /// When its note ends.
    pub end: f64,
    /// The key.
    pub key: u8,
    /// The velocity, 1..=127.
    pub velocity: u8,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    hit: Hit,
    seq: u32,
    used: bool,
}

/// Fixed table of parked hits.
#[derive(Debug, Clone)]
pub(super) struct PendingHits {
    slots: [Slot; CAP],
    seq: u32,
}

impl PendingHits {
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                hit: Hit {
                    tick: 0.0,
                    end: 0.0,
                    key: 0,
                    velocity: 0,
                },
                seq: 0,
                used: false,
            }; CAP],
            seq: 0,
        }
    }

    /// Forgets every parked hit.
    pub fn clear(&mut self) {
        for s in &mut self.slots {
            s.used = false;
        }
    }

    /// Parks `hit`; dropped when the table is full.
    pub fn push(&mut self, hit: Hit) {
        if let Some(slot) = self.slots.iter_mut().find(|s| !s.used) {
            self.seq = self.seq.wrapping_add(1);
            *slot = Slot {
                hit,
                seq: self.seq,
                used: true,
            };
        }
    }

    /// The slot and start tick of the earliest parked hit, ties by insertion order.
    pub fn earliest(&self) -> Option<(usize, f64)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.used)
            .min_by(|(_, a), (_, b)| a.hit.tick.total_cmp(&b.hit.tick).then(a.seq.cmp(&b.seq)))
            .map(|(i, s)| (i, s.hit.tick))
    }

    /// Frees slot `i` and returns its hit.
    pub fn take(&mut self, i: usize) -> Hit {
        self.slots[i].used = false;
        self.slots[i].hit
    }

    /// How many hits are parked.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.used).count()
    }
}

/// The soft velocity of the grace hit of a flam: half, at least 1.
pub(super) fn grace_velocity(v: u8) -> u8 {
    (v / 2).max(1)
}

/// Start ticks of the `n` hits of a ratchet over a step that starts at `tick` and lasts `len`
/// ticks: evenly spaced, integer ticks, the first at the step start.
pub(super) fn ratchet_tick(tick: f64, len: f64, j: u8, n: u8) -> f64 {
    tick + (len * f64::from(j) / f64::from(n)).floor()
}

/// The tick the full hit of a flam of `flam` percent starts at, in `(tick, next]`.
pub(super) fn flam_tick(tick: f64, next: f64, flam: u8) -> f64 {
    let len = next - tick;
    let late = (len * f64::from(flam.min(MAX_FLAM)) / 100.0)
        .floor()
        .max(1.0);
    (tick + late).min(next)
}

/// The end of a note that starts at `from` and may last until `until`: `gate` (0..=1) of the
/// span, at least one tick, legato at 1.
pub(super) fn span_end(from: f64, until: f64, gate: f64) -> f64 {
    if gate >= 1.0 {
        return until;
    }
    let len = ((until - from) * gate).floor().max(1.0);
    (from + len).min(until)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(tick: f64) -> Hit {
        Hit {
            tick,
            end: tick + 1.0,
            key: 36,
            velocity: 100,
        }
    }

    #[test]
    fn the_table_is_bounded_and_drops_the_newest_hit() {
        let mut t = PendingHits::new();
        for i in 0..CAP + 10 {
            t.push(hit(i as f64));
        }
        assert_eq!(t.len(), CAP);
        let (i, tick) = t.earliest().unwrap();
        assert_eq!(tick, 0.0, "the first hits stayed");
        assert_eq!(t.take(i).tick, 0.0);
        t.clear();
        assert_eq!(t.len(), 0);
        assert!(t.earliest().is_none());
    }

    #[test]
    fn hits_come_out_in_tick_order_then_insertion_order() {
        let mut t = PendingHits::new();
        for tick in [30.0, 10.0, 10.0, 20.0] {
            t.push(Hit {
                key: tick as u8,
                ..hit(tick)
            });
        }
        t.push(Hit {
            key: 99,
            ..hit(10.0)
        });
        let mut keys = Vec::new();
        while let Some((i, _)) = t.earliest() {
            keys.push(t.take(i).key);
        }
        assert_eq!(keys, [10, 10, 99, 20, 30]);
    }

    #[test]
    fn tick_arithmetic_stays_inside_the_step() {
        assert_eq!(ratchet_tick(100.0, 3_840.0, 0, 4), 100.0);
        assert_eq!(ratchet_tick(100.0, 3_840.0, 3, 4), 100.0 + 2_880.0);
        assert_eq!(ratchet_tick(0.0, 1_000.0, 2, 3), 666.0);
        assert_eq!(flam_tick(0.0, 3_840.0, 50), 1_920.0);
        assert_eq!(
            flam_tick(0.0, 3_840.0, 200),
            1_920.0,
            "clamped to the largest flam"
        );
        assert_eq!(flam_tick(0.0, 10.0, 1), 1.0, "at least one tick later");
        assert_eq!(flam_tick(0.0, 1.0, 50), 1.0, "never past the step");
        assert_eq!(span_end(0.0, 100.0, 0.25), 25.0);
        assert_eq!(span_end(0.0, 100.0, 1.0), 100.0);
        assert_eq!(span_end(0.0, 100.0, 0.0), 1.0);
        assert_eq!(grace_velocity(100), 50);
        assert_eq!(grace_velocity(1), 1);
    }
}
