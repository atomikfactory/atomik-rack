//! Edge: a powered-off, fully bypassed device must still deliver note
//! Off and AllOff to the plugin so a held note cannot hang, and a sleeping plugin that asks to
//! be processed (CLAP `request_process`) is processed.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_plugin_host::{
    PluginEventKind as K, PluginIo, PluginProcessor, ProcessStatus, SleepRule,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use support::*;

fn on(key: u8) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::On { key, velocity: 1.0 },
    }
}

fn off(key: u8) -> NoteEvent {
    NoteEvent {
        offset: 3,
        kind: NoteKind::Off { key },
    }
}

fn all_off() -> NoteEvent {
    NoteEvent {
        offset: 1,
        kind: NoteKind::AllOff,
    }
}

/// A powered-off synth rig with `held` keys sounding, settled (the fade is done).
fn stopped_synth(
    held: &[u8],
) -> (
    atomik_testkit::harness::DeviceHarness,
    Rest,
    Arc<ProbeShared>,
) {
    let (probe, shared) = Probe::new();
    let (mut h, r) = harness(rig("pw.off", &synth_layout(), Box::new(probe)));
    for key in held {
        h.push_note("notes_in", on(*key));
    }
    h.process();
    r.manager.slot().set_powered(false);
    for _ in 0..8 {
        h.process();
    }
    (h, r, shared)
}

fn events_since(shared: &ProbeShared, call: usize) -> Vec<K> {
    let log = shared.log.lock().unwrap();
    (call..log.calls)
        .flat_map(|c| log.events_of_call(c).iter().map(|e| e.kind))
        .collect()
}

#[test]
fn a_note_off_reaches_a_powered_off_plugin() {
    let (mut h, _r, shared) = stopped_synth(&[60]);
    let calls = shared.calls();
    h.process();
    assert_eq!(shared.calls(), calls, "stopped: no call without events");
    h.push_note("notes_in", off(60));
    h.process();
    assert_eq!(shared.calls(), calls + 1, "the Off needed one call");
    let seen = events_since(&shared, calls);
    assert!(
        matches!(seen.as_slice(), [K::NoteOff { key: 60, .. }]),
        "{seen:?}"
    );
    assert!(
        h.output("out_l").iter().all(|s| *s == 0.0),
        "an instrument stays silent"
    );
    // And stopped again afterwards.
    for _ in 0..4 {
        h.process();
    }
    assert_eq!(shared.calls(), calls + 1);
}

#[test]
fn all_off_reaches_a_powered_off_plugin_as_an_off_per_open_key_and_the_cc() {
    let (mut h, _r, shared) = stopped_synth(&[60, 64]);
    let calls = shared.calls();
    h.push_note("notes_in", all_off());
    h.process();
    assert_eq!(shared.calls(), calls + 1);
    let seen = events_since(&shared, calls);
    assert!(
        seen.iter().any(|k| matches!(k, K::NoteOff { key: 60, .. }))
            && seen.iter().any(|k| matches!(k, K::NoteOff { key: 64, .. }))
            && seen.iter().any(|k| matches!(
                k,
                K::Cc {
                    controller: 123,
                    ..
                }
            )),
        "{seen:?}"
    );
}

#[test]
fn a_powered_off_plugin_still_ignores_new_notes_and_keeps_its_params_pending() {
    let (mut h, r, shared) = stopped_synth(&[]);
    let calls = shared.calls();
    h.push_note("notes_in", on(72));
    h.set_param("p2", 0.25);
    h.process();
    assert_eq!(shared.calls(), calls, "no call for a new note or a param");
    // An Off of a key that was never open is not worth a call either.
    h.push_note("notes_in", off(40));
    h.process();
    assert_eq!(shared.calls(), calls);
    // Power on: the param arrives, the held-back note does not.
    r.manager.slot().set_powered(true);
    h.process();
    let seen = events_since(&shared, calls);
    assert!(
        seen.iter()
            .any(|k| matches!(k, K::Param { id: 2, value } if (*value - 0.25).abs() < 1e-6)),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|k| matches!(k, K::NoteOn { .. })),
        "{seen:?}"
    );
}

#[test]
fn a_note_off_while_powered_off_does_not_allocate() {
    let (mut h, _r, _shared) = stopped_synth(&[60]);
    h.push_note("notes_in", off(60));
    atomik_testkit::no_alloc::assert_no_alloc(|| h.process());
}

// ----- wake on request --------------------------------------------------------------------

struct Sleeper {
    calls: Arc<AtomicUsize>,
    wake: Arc<AtomicBool>,
}

impl PluginProcessor for Sleeper {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {}
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.calls.fetch_add(1, Ordering::Relaxed);
        for ch in io.main_out.iter_mut() {
            *ch = [0.0; BATCH];
        }
        ProcessStatus::Sleep
    }
    fn sleep_rule(&self) -> SleepRule {
        SleepRule::PluginDecides
    }
    fn wake_requested(&self) -> bool {
        self.wake.swap(false, Ordering::Relaxed)
    }
}

#[test]
fn a_sleeping_plugin_that_asks_for_processing_is_processed() {
    let calls = Arc::new(AtomicUsize::new(0));
    let wake = Arc::new(AtomicBool::new(false));
    let sleeper = Sleeper {
        calls: Arc::clone(&calls),
        wake: Arc::clone(&wake),
    };
    let (mut h, _r) = harness(rig("pw.wake", &synth_layout(), Box::new(sleeper)));
    for _ in 0..4 {
        h.process();
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1, "asleep after the first");
    wake.store(true, Ordering::Relaxed);
    h.process();
    assert_eq!(calls.load(Ordering::Relaxed), 2, "woken by the request");
    for _ in 0..3 {
        h.process();
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "asleep again, request spent"
    );
}
