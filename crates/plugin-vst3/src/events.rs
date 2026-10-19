//! `IEventList` with fixed capacity: allocated when the plugin is activated; the host
//! fills the input list and reads the output list inside `process` without allocating.

use std::cell::Cell;
use vst3::Steinberg::Vst::{Event, IEventList, IEventListTrait};
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

/// A fixed-capacity list of VST3 events.
pub(crate) struct EventListObj {
    events: Vec<Cell<Event>>,
    len: Cell<usize>,
}

impl Class for EventListObj {
    type Interfaces = (IEventList,);
}

/// An all-zero event (valid: every member is a plain integer, float or pointer).
pub(crate) fn zero_event() -> Event {
    // SAFETY: `Event` is plain data; the all-zero bit pattern is a valid value of every member
    // of its union.
    unsafe { std::mem::zeroed() }
}

impl EventListObj {
    /// A list able to hold `capacity` events.
    pub(crate) fn new(capacity: usize) -> ComWrapper<Self> {
        ComWrapper::new(Self {
            events: (0..capacity).map(|_| Cell::new(zero_event())).collect(),
            len: Cell::new(0),
        })
    }

    /// Empties the list.
    pub(crate) fn clear(&self) {
        self.len.set(0);
    }

    /// Appends an event; false (and nothing added) when the list is full.
    pub(crate) fn push(&self, e: Event) -> bool {
        let n = self.len.get();
        if n >= self.events.len() {
            return false;
        }
        self.events[n].set(e);
        self.len.set(n + 1);
        true
    }

    /// Number of events.
    pub(crate) fn len(&self) -> usize {
        self.len.get()
    }

    /// Event `i`.
    pub(crate) fn get(&self, i: usize) -> Option<Event> {
        (i < self.len.get()).then(|| self.events[i].get())
    }
}

impl IEventListTrait for EventListObj {
    unsafe fn getEventCount(&self) -> i32 {
        self.len.get() as i32
    }

    unsafe fn getEvent(&self, index: i32, e: *mut Event) -> tresult {
        match usize::try_from(index).ok().and_then(|i| self.get(i)) {
            Some(event) if !e.is_null() => {
                // SAFETY: a non-null out pointer from the caller.
                unsafe { *e = event };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null event pointer from the caller.
        if self.push(unsafe { *e }) {
            kResultOk
        } else {
            kOutOfMemory
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_kept_in_order_and_refused_when_full() {
        let l = EventListObj::new(2);
        let mut a = zero_event();
        a.sampleOffset = 3;
        let mut b = zero_event();
        b.sampleOffset = 9;
        assert!(l.push(a));
        assert!(l.push(b));
        assert!(!l.push(a), "full");
        assert_eq!(l.len(), 2);
        let r = l.as_com_ref::<IEventList>().unwrap();
        let mut out = zero_event();
        // SAFETY: valid out pointer.
        unsafe {
            assert_eq!(r.getEventCount(), 2);
            assert_eq!(r.getEvent(1, &mut out), kResultOk);
            assert_eq!(out.sampleOffset, 9);
            assert_eq!(r.getEvent(2, &mut out), kResultFalse);
            assert_eq!(r.getEvent(-1, &mut out), kResultFalse);
            assert_eq!(r.addEvent(&mut a), kOutOfMemory);
        }
        l.clear();
        assert_eq!(l.len(), 0);
        // SAFETY: valid pointer.
        assert_eq!(unsafe { r.addEvent(&mut a) }, kResultOk);
    }
}
