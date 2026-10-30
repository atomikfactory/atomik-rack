//! The engine's sticky fault state: which device panicked while rendering.
//!
//! A device that panics in `process` is caught where it runs (the audio thread or a worker),
//! its node is marked faulted (silent from then on) and the first such device is recorded
//! here with one atomic store. Nothing allocates or locks; the UI side polls
//! [`EngineHandle::fault`](crate::EngineHandle::fault).

use crate::graph::DeviceInstanceId;
use std::sync::atomic::{AtomicU64, Ordering};

/// The device that faulted first, as `id + 1` (0 = no fault).
#[derive(Debug, Default)]
pub(crate) struct FaultState {
    first: AtomicU64,
}

impl FaultState {
    /// Records `id` as faulted; the first device recorded stays.
    pub(crate) fn record(&self, id: DeviceInstanceId) {
        let _ = self.first.compare_exchange(
            0,
            u64::from(id.0) + 1,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    /// The first device that faulted, if any.
    pub(crate) fn get(&self) -> Option<DeviceInstanceId> {
        match self.first.load(Ordering::Acquire) {
            0 => None,
            n => u32::try_from(n - 1).ok().map(DeviceInstanceId),
        }
    }
}
