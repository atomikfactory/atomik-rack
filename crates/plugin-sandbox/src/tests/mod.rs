//! Unit tests of the private types over a fake child (and, for supervision, a real process).

mod lost;
mod no_alloc;
mod notifications;
mod processor;
mod rpc;
mod supervision;

use crate::fake::{Fake, FakeOptions, start};
use crate::instance::RemoteInstance;
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{ActivateConfig, BackendError, LOST, ManagerEnd, PluginMessage};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

/// A started instance with everything its tests look at.
pub(super) struct Rig {
    pub instance: RemoteInstance,
    pub fake: Fake,
    pub manager: ManagerEnd,
    pub live: Arc<AtomicUsize>,
}

pub(super) fn rig(options: FakeOptions) -> Rig {
    let started = start(options);
    Rig {
        instance: started
            .instance
            .map_err(|e| e.to_string())
            .expect("the fake greets"),
        fake: started.fake,
        manager: started.manager,
        live: started.live,
    }
}

/// Waits (polling) until `f` is true.
pub(super) fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out waiting for {what}");
}

/// The next message of the plugin, waiting for the reader thread.
pub(super) fn next_message(m: &ManagerEnd) -> PluginMessage {
    let mut got = None;
    eventually("a plugin message", || {
        got = m.try_recv();
        got.is_some()
    });
    got.expect("a message")
}

pub(super) fn cfg(sample_rate: f64, mode: RunMode) -> ActivateConfig {
    ActivateConfig {
        sample_rate,
        frames: 64,
        mode,
    }
}

pub(super) fn lost_error() -> BackendError {
    BackendError::Failed(LOST.to_string())
}
