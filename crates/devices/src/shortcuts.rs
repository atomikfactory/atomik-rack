//! The switch the tests use to compare a bit-identical shortcut with the full computation.
//!
//! Some devices skip work whose result nothing reads (an LFO nobody uses, a silent operator, a
//! scanner with nothing to scan). Each shortcut must leave every output bit and every state as
//! the full computation does. A test runs the same scenario with [`with_full`], which turns the
//! shortcuts off on the calling thread, and compares the two. The switch exists in test builds
//! only: it is not a parameter, nothing is saved in a project, and a release build always takes
//! the shortcuts.

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static FULL: Cell<bool> = const { Cell::new(false) };
    static TAKEN: [Cell<u64>; 3] = const { [Cell::new(0), Cell::new(0), Cell::new(0)] };
}

/// The shortcuts that are counted.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Shortcut {
    /// An LFO that moves on without being evaluated.
    Lfo,
    /// The organ's scanner at rest.
    Scanner,
    /// An FM operator that is not computed.
    Operator,
}

/// Counts a shortcut taken (test builds only: a test checks that its scenario reaches the
/// shortcut, and that [`with_full`] never does).
#[inline]
pub(crate) fn took(which: Shortcut) {
    #[cfg(test)]
    TAKEN.with(|c| {
        let c = &c[which as usize];
        c.set(c.get() + 1);
    });
    #[cfg(not(test))]
    let _ = which;
}

/// The `which` shortcuts taken on this thread since the last call (test builds only).
#[cfg(test)]
pub(crate) fn taken(which: Shortcut) -> u64 {
    TAKEN.with(|c| c[which as usize].replace(0))
}

/// Whether the shortcuts may be taken (always, outside tests).
#[inline]
pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    {
        !FULL.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// Runs `f` with the shortcuts off on this thread.
#[cfg(test)]
pub(crate) fn with_full<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FULL.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(FULL.with(|c| c.replace(true)));
    f()
}

/// The scenario every instrument with a device LFO runs, shortcut on against shortcut off.
#[cfg(test)]
pub(crate) mod lfo_check {
    use super::{Shortcut, taken, with_full};
    use crate::test_support::{off, on};
    use atomik_testkit::harness::DeviceHarness;

    /// What differs between the instruments.
    pub struct Case<'a> {
        /// The notes input.
        pub notes: &'a str,
        /// The audio outputs whose every bit is compared.
        pub outs: &'a [&'a str],
        /// The LFO CV output, when the device has one.
        pub lfo_out: Option<&'a str>,
        /// A destination of the LFO and the amount it is raised to after idle batches.
        pub amount: (&'a str, f32),
        /// Parameters set before the scenario (a short release, a filter that is on).
        pub setup: &'a [(&'a str, f32)],
    }

    fn run(
        make: &dyn Fn() -> DeviceHarness,
        c: &Case<'_>,
        wave: f32,
        hz: f32,
        cabled: bool,
    ) -> Vec<u32> {
        let mut h = make();
        for (k, v) in c.setup {
            h.set_param(k, *v);
        }
        h.set_param("lfo.wave", wave);
        h.set_param("lfo.rate", hz);
        if let (Some(out), true) = (c.lfo_out, cabled) {
            h.set_connected(out, true);
        }
        let mut bits = Vec::new();
        let mut batch = |h: &mut DeviceHarness| {
            h.process();
            for out in c.outs.iter().copied().chain(c.lfo_out.filter(|_| cabled)) {
                bits.extend(h.output(out).iter().map(|s| s.to_bits()));
            }
        };
        // Idle batches, then a note that starts inside a batch.
        for _ in 0..20 {
            batch(&mut h);
        }
        h.push_note(c.notes, on(100, 60, 0.8));
        batch(&mut h);
        for _ in 0..12 {
            batch(&mut h);
        }
        // The release and the silence after it, then the destination amount raised from zero
        // while idle, and a note that starts at the top of a batch.
        h.push_note(c.notes, off(3, 60));
        for _ in 0..400 {
            batch(&mut h);
        }
        h.set_param(c.amount.0, c.amount.1);
        for _ in 0..7 {
            batch(&mut h);
        }
        h.push_note(c.notes, on(0, 64, 0.6));
        for _ in 0..8 {
            batch(&mut h);
        }
        bits
    }

    /// Runs the scenario for every shape and two rates, with and without a cable on the LFO
    /// output, and asserts that the two runs agree on every bit.
    pub fn assert_same(make: &dyn Fn() -> DeviceHarness, c: &Case<'_>) {
        let mut took = 0;
        for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
            for hz in [7.3, 3_000.0] {
                for cabled in [false, true] {
                    taken(Shortcut::Lfo);
                    let shortcut = run(make, c, wave, hz, cabled);
                    let used = taken(Shortcut::Lfo);
                    let full = with_full(|| run(make, c, wave, hz, cabled));
                    assert_eq!(taken(Shortcut::Lfo), 0, "the full run took a shortcut");
                    assert!(
                        shortcut == full,
                        "wave {wave} at {hz} Hz, cabled {cabled}: the outputs differ"
                    );
                    if cabled && c.lfo_out.is_some() {
                        assert_eq!(used, 0, "a cable on the LFO output keeps the LFO running");
                    } else {
                        took += used;
                    }
                }
            }
        }
        assert!(took > 0, "no scenario reached the shortcut");
    }
}
