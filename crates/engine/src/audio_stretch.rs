//! The real-time stretchers of the interim warps that keep their pitch (see `audio.rs`, module
//! docs, *Interim warps*).
//!
//! Each audio track whose interim regions need one owns a [`StretchBank`]: up to
//! [`MAX_STRETCHERS`] [`RtStretcher`]s, built off the audio thread when the song is compiled and
//! swapped in with it. `compile_audio` gives every such region a slot (its rank among them,
//! modulo the cap), so two regions that play at once (a crossfade mixes at most two) never
//! share one, and a later region simply takes over the slot of an earlier one that has ended
//! (the stretcher restarts). A region whose slot another region already used in the same
//! segment, or a track without a bank, falls back to the plain Hermite read and is counted.
//! Nothing here allocates on the audio thread.

use atomik_dsp::stretch::{RT_MAX_SAMPLE_RATE, RtStretcher};

/// Most real-time stretchers one audio track holds.
pub(crate) const MAX_STRETCHERS: usize = 4;

struct Slot {
    st: RtStretcher,
    /// The region (index in its lane) the stretcher plays.
    owner: Option<u32>,
    /// The segment it last played in.
    used: u64,
}

/// The stretchers of one audio track.
#[derive(Default)]
pub(crate) struct StretchBank {
    slots: Vec<Slot>,
    /// Counts the segments rendered (a slot used in the current one is busy).
    epoch: u64,
    /// Sample rate of the segments.
    sample_rate: u32,
    /// Region segments that fell back to the Hermite read.
    fallbacks: u64,
}

impl Clone for StretchBank {
    /// A fresh bank of the same size (allocates: never on the audio thread).
    fn clone(&self) -> Self {
        Self::with_slots(self.slots.len())
    }
}

impl StretchBank {
    /// A bank of `n` stretchers (at most [`MAX_STRETCHERS`]); allocates.
    pub(crate) fn with_slots(n: usize) -> Self {
        Self {
            slots: (0..n.min(MAX_STRETCHERS))
                .map(|_| Slot {
                    st: RtStretcher::new(RT_MAX_SAMPLE_RATE),
                    owner: None,
                    used: 0,
                })
                .collect(),
            ..Self::default()
        }
    }

    /// Starts a segment at `sample_rate`.
    pub(crate) fn begin(&mut self, sample_rate: u32) {
        self.epoch += 1;
        self.sample_rate = sample_rate;
    }

    /// Sample rate of the current segment.
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The stretcher of region `region` in slot `slot`, restarted when the slot changes hands;
    /// `None` (counted) when the bank has no slot for it or another region already played in it
    /// during this segment.
    pub(crate) fn take(&mut self, region: u32, slot: u16) -> Option<&mut RtStretcher> {
        let len = self.slots.len();
        if len == 0 {
            self.fallbacks += 1;
            return None;
        }
        let epoch = self.epoch;
        let s = &mut self.slots[usize::from(slot) % len];
        if s.owner != Some(region) {
            if s.owner.is_some() && s.used == epoch {
                self.fallbacks += 1;
                return None;
            }
            s.owner = Some(region);
            s.st.reset(0);
        }
        s.used = epoch;
        Some(&mut s.st)
    }

    /// Region segments that fell back to the Hermite read so far.
    pub(crate) fn fallbacks(&self) -> u64 {
        self.fallbacks
    }

    /// Number of stretchers.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_slot_falls_back_and_a_free_one_changes_hands() {
        let mut bank = StretchBank::with_slots(9);
        assert_eq!(bank.len(), MAX_STRETCHERS);
        bank.begin(48_000);
        assert!(bank.take(0, 0).is_some());
        assert!(bank.take(0, 0).is_some(), "the owner again");
        assert!(bank.take(4, 4).is_none(), "slot 0 is busy in this segment");
        assert_eq!(bank.fallbacks(), 1);
        bank.begin(48_000);
        assert!(bank.take(4, 4).is_some(), "free in the next segment");
        let mut empty = StretchBank::default();
        empty.begin(48_000);
        assert!(empty.take(0, 0).is_none());
        assert_eq!(empty.fallbacks(), 1);
        assert_eq!(bank.clone().len(), MAX_STRETCHERS);
    }
}
