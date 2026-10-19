//! `IParameterChanges` and `IParamValueQueue` with fixed capacity: everything is
//! allocated when the plugin is activated, so filling the lists in `process` never allocates.
//! The same classes serve as the input list (the host fills it) and the output list (the plugin
//! fills it through `addParameterData`).

use std::cell::Cell;
use vst3::Steinberg::Vst::{IParamValueQueue, IParamValueQueueTrait};
use vst3::Steinberg::Vst::{IParameterChanges, IParameterChangesTrait};
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

/// Points one queue holds per block (more are refused and counted by the caller).
pub(crate) const QUEUE_POINTS: usize = 16;

/// One parameter's value changes inside a block.
pub(crate) struct ParamQueue {
    id: Cell<u32>,
    len: Cell<usize>,
    offsets: [Cell<i32>; QUEUE_POINTS],
    values: [Cell<f64>; QUEUE_POINTS],
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl ParamQueue {
    fn new() -> Self {
        Self {
            id: Cell::new(0),
            len: Cell::new(0),
            offsets: [const { Cell::new(0) }; QUEUE_POINTS],
            values: [const { Cell::new(0.0) }; QUEUE_POINTS],
        }
    }

    fn push(&self, offset: i32, value: f64) -> Option<usize> {
        let n = self.len.get();
        if n >= QUEUE_POINTS {
            return None;
        }
        self.offsets[n].set(offset);
        self.values[n].set(value);
        self.len.set(n + 1);
        Some(n)
    }

    /// The last point `(offset, value)`.
    fn last(&self) -> Option<(i32, f64)> {
        let n = self.len.get();
        (n > 0).then(|| (self.offsets[n - 1].get(), self.values[n - 1].get()))
    }
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> u32 {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> i32 {
        self.len.get() as i32
    }

    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut f64) -> tresult {
        let Ok(i) = usize::try_from(index) else {
            return kResultFalse;
        };
        if i >= self.len.get() || offset.is_null() || value.is_null() {
            return kResultFalse;
        }
        // SAFETY: non-null out pointers from the caller.
        unsafe {
            *offset = self.offsets[i].get();
            *value = self.values[i].get();
        }
        kResultTrue
    }

    unsafe fn addPoint(&self, offset: i32, value: f64, index: *mut i32) -> tresult {
        match self.push(offset, value) {
            Some(n) => {
                if !index.is_null() {
                    // SAFETY: a non-null out pointer from the caller.
                    unsafe { *index = n as i32 };
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
}

/// A list of [`ParamQueue`]s, one per parameter id, found through a small open-addressing table
/// so adding a point is O(1) even for a plugin with thousands of params.
pub(crate) struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    used: Cell<usize>,
    table: Vec<Cell<u16>>,
    mask: usize,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

fn hash(id: u32) -> usize {
    (id.wrapping_mul(0x9E37_79B1) >> 7) as usize
}

impl ParamChanges {
    /// A list able to hold `capacity` different params.
    pub(crate) fn new(capacity: usize) -> ComWrapper<Self> {
        let capacity = capacity.clamp(1, usize::from(u16::MAX) - 1);
        let size = (capacity * 2).next_power_of_two();
        ComWrapper::new(Self {
            queues: (0..capacity)
                .map(|_| ComWrapper::new(ParamQueue::new()))
                .collect(),
            used: Cell::new(0),
            table: (0..size).map(|_| Cell::new(0)).collect(),
            mask: size - 1,
        })
    }

    /// Empties the list for the next block without allocating.
    pub(crate) fn clear(&self) {
        for q in 0..self.used.get() {
            let mut slot = hash(self.queues[q].id.get()) & self.mask;
            // The entry is in this cluster; neighbours already cleared are skipped, not stops.
            while usize::from(self.table[slot].get()) != q + 1 {
                slot = (slot + 1) & self.mask;
            }
            self.table[slot].set(0);
            self.queues[q].len.set(0);
        }
        self.used.set(0);
    }

    /// The queue of `id`, created when new; `None` when the list is full.
    fn queue_of(&self, id: u32) -> Option<usize> {
        let mut slot = hash(id) & self.mask;
        loop {
            match usize::from(self.table[slot].get()) {
                0 => break,
                n if self.queues[n - 1].id.get() == id => return Some(n - 1),
                _ => slot = (slot + 1) & self.mask,
            }
        }
        let n = self.used.get();
        if n >= self.queues.len() {
            return None;
        }
        self.queues[n].id.set(id);
        self.queues[n].len.set(0);
        self.table[slot].set((n + 1) as u16);
        self.used.set(n + 1);
        Some(n)
    }

    /// Adds a point of param `id`; false when the list or the param's queue is full.
    pub(crate) fn add_point(&self, id: u32, offset: i32, value: f64) -> bool {
        self.queue_of(id)
            .is_some_and(|q| self.queues[q].push(offset, value).is_some())
    }

    /// Number of params with points.
    pub(crate) fn used(&self) -> usize {
        self.used.get()
    }

    /// The id and last point of queue `i`.
    pub(crate) fn last_of(&self, i: usize) -> Option<(u32, i32, f64)> {
        if i >= self.used.get() {
            return None;
        }
        self.queues[i]
            .last()
            .map(|(o, v)| (self.queues[i].id.get(), o, v))
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.used.get() as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        match usize::try_from(index) {
            Ok(i) if i < self.used.get() => self.queues[i]
                .as_com_ref::<IParamValueQueue>()
                .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            _ => std::ptr::null_mut(),
        }
    }

    unsafe fn addParameterData(&self, id: *const u32, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: a non-null id pointer from the caller.
        let id = unsafe { *id };
        match self.queue_of(id) {
            Some(q) => {
                if !index.is_null() {
                    // SAFETY: a non-null out pointer from the caller.
                    unsafe { *index = q as i32 };
                }
                self.queues[q]
                    .as_com_ref::<IParamValueQueue>()
                    .map_or(std::ptr::null_mut(), |r| r.as_ptr())
            }
            None => std::ptr::null_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_kept_per_param_in_order_and_cleared_for_the_next_block() {
        let c = ParamChanges::new(8);
        assert!(c.add_point(7, 0, 0.25));
        assert!(c.add_point(9, 3, 0.5));
        assert!(c.add_point(7, 10, 0.75));
        assert_eq!(c.used(), 2);
        assert_eq!(c.last_of(0), Some((7, 10, 0.75)));
        assert_eq!(c.last_of(1), Some((9, 3, 0.5)));
        let r = c.as_com_ref::<IParameterChanges>().unwrap();
        // SAFETY: calling the list's methods with valid arguments.
        unsafe {
            assert_eq!(r.getParameterCount(), 2);
            let q = vst3::ComRef::from_raw(r.getParameterData(0)).unwrap();
            assert_eq!(q.getParameterId(), 7);
            assert_eq!(q.getPointCount(), 2);
            let (mut off, mut v) = (0, 0.0);
            assert_eq!(q.getPoint(1, &mut off, &mut v), kResultTrue);
            assert_eq!((off, v), (10, 0.75));
            assert_eq!(q.getPoint(2, &mut off, &mut v), kResultFalse);
            assert!(r.getParameterData(2).is_null());
        }
        c.clear();
        assert_eq!(c.used(), 0);
        assert!(c.add_point(9, 1, 0.1));
        assert_eq!(c.last_of(0), Some((9, 1, 0.1)));
    }

    #[test]
    fn a_full_list_or_queue_refuses_instead_of_growing() {
        let c = ParamChanges::new(2);
        assert!(c.add_point(1, 0, 0.0));
        assert!(c.add_point(2, 0, 0.0));
        assert!(!c.add_point(3, 0, 0.0), "no third queue");
        for k in 0..QUEUE_POINTS - 1 {
            assert!(c.add_point(1, k as i32, 0.0));
        }
        assert!(!c.add_point(1, 99, 0.0), "no more points in one queue");
    }

    #[test]
    fn many_ids_with_colliding_hashes_are_all_found_and_cleared() {
        let c = ParamChanges::new(512);
        for round in 0..3 {
            for id in 0..512u32 {
                assert!(
                    c.add_point(id * 8192, 0, f64::from(id)),
                    "round {round} id {id}"
                );
            }
            assert_eq!(c.used(), 512);
            for id in (0..512u32).rev() {
                assert!(c.add_point(id * 8192, 1, 1.0));
            }
            assert_eq!(c.used(), 512, "no duplicates");
            c.clear();
            assert_eq!(c.used(), 0);
        }
    }

    #[test]
    fn the_plugin_can_add_output_params_through_the_interface() {
        let c = ParamChanges::new(4);
        let r = c.as_com_ref::<IParameterChanges>().unwrap();
        let id = 42u32;
        let mut index = -1;
        // SAFETY: valid pointers for the calls.
        unsafe {
            let q = vst3::ComRef::from_raw(r.addParameterData(&id, &mut index)).unwrap();
            assert_eq!(index, 0);
            assert_eq!(q.addPoint(5, 0.5, &mut index), kResultOk);
            assert_eq!(index, 0);
            let again = r.addParameterData(&id, &mut index);
            assert_eq!(again, q.as_ptr(), "the same queue for the same id");
            assert!(r.addParameterData(std::ptr::null(), &mut index).is_null());
        }
        assert_eq!(c.last_of(0), Some((42, 5, 0.5)));
    }
}
