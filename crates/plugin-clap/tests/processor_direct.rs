//! The CLAP processor called directly through `PluginProcessor`, without a `PluginDevice`:
//! start, stop and reset, the tail, the sleep rule, the wake request, the output events, and
//! the events that do not fit.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind as K, PluginIo, PluginProcessor, PluginTransport,
    ProcessStatus, SleepRule, TailLength,
};
use support::*;

const PKG: &str = "atomik-test-clap";

struct Bufs {
    main_in: [Batch; 2],
    sidechain: [Batch; 2],
    main_out: [Batch; 2],
    out_events: EventList,
}

impl Bufs {
    fn new() -> Self {
        Self {
            main_in: [[0.0; BATCH]; 2],
            sidechain: [[0.0; BATCH]; 2],
            main_out: [[0.0; BATCH]; 2],
            out_events: EventList::with_capacity(64),
        }
    }

    fn run(
        &mut self,
        p: &mut dyn PluginProcessor,
        events: &[PluginEvent],
        n_in: usize,
        sidechain: bool,
        n_out: usize,
    ) -> (ProcessStatus, u32) {
        self.out_events.clear();
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &self.main_in[..n_in],
            main_in_silent: n_in == 0,
            sidechain: if sidechain { &self.sidechain[..] } else { &[] },
            sidechain_silent: true,
            main_out: &mut self.main_out[..n_out],
            events_in: events,
            events_out: &mut self.out_events,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        let status = p.process(&mut io);
        (status, io.dropped)
    }
}

fn ev(offset: u16, kind: K) -> PluginEvent {
    PluginEvent { offset, kind }
}

#[test]
fn the_tail_follows_the_plugin_and_the_sleep_rule_is_the_plugins() {
    let (backend, _) = backend_for(PKG);
    let (mut inst, _manager) = bare(&backend, "atomik.test.gain");
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(p.sleep_rule(), SleepRule::PluginDecides);
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let (status, _) = b.run(&mut *p, &[], 2, true, 2);
    assert_eq!(status, ProcessStatus::Continue);
    assert_eq!(p.tail_samples(), TailLength::Finite(0));
    b.run(
        &mut *p,
        &[ev(0, K::Param { id: 3, value: 37.0 })],
        2,
        true,
        2,
    );
    assert_eq!(
        p.tail_samples(),
        TailLength::Finite(37),
        "the plugin's tail extension"
    );
    p.reset();
    p.stop_processing();
    // A processor that was stopped starts again.
    assert!(p.start_processing());
    inst.deactivate(p);
}

#[test]
fn a_wake_request_is_reported_once() {
    let lib = TestLib::open(PKG);
    let (backend, _) = backend_for(PKG);
    let (mut inst, _manager) = bare(&backend, "atomik.test.synth");
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(!p.wake_requested());
    lib.command(&*inst, command::REQUEST_PROCESS, 0, 0);
    assert!(p.wake_requested());
    assert!(!p.wake_requested(), "reported once");
    lib.command(&*inst, command::REQUEST_FLUSH, 0, 0);
    assert!(p.wake_requested(), "a flush request wakes it too");
    inst.deactivate(p);
}

#[test]
fn a_note_effects_output_events_come_back_with_their_offsets_and_overflow_is_dropped() {
    let (backend, _) = backend_for(PKG);
    let (mut inst, _manager) = bare(&backend, "atomik.test.noteshift");
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let events = [
        ev(
            2,
            K::NoteOn {
                key: 60,
                velocity: 0.5,
                note_id: 0,
            },
        ),
        ev(
            9,
            K::NoteOff {
                key: 60,
                note_id: 0,
            },
        ),
    ];
    let (_, dropped) = b.run(&mut *p, &events, 0, false, 0);
    assert_eq!(dropped, 0);
    let out: Vec<_> = b
        .out_events
        .as_slice()
        .iter()
        .map(|e| (e.offset, e.kind))
        .collect();
    assert_eq!(
        out,
        vec![
            (
                2,
                K::NoteOn {
                    key: 60,
                    velocity: 0.5,
                    note_id: 0
                }
            ),
            (
                9,
                K::NoteOff {
                    key: 60,
                    note_id: 0
                }
            ),
        ]
    );
    // More notes than the output list can hold: the extra ones are counted, never grown.
    let mut tiny = Bufs::new();
    tiny.out_events = EventList::with_capacity(2);
    let many: Vec<PluginEvent> = (0..10u8)
        .flat_map(|i| {
            [ev(
                u16::from(i),
                K::NoteOn {
                    key: 40 + i,
                    velocity: 0.5,
                    note_id: i32::from(i) + 1,
                },
            )]
        })
        .collect();
    tiny.run(&mut *p, &many, 0, false, 0);
    assert_eq!(tiny.out_events.len(), 2);
    assert!(tiny.out_events.dropped() > 0);
    inst.deactivate(p);
}

#[test]
fn events_the_plugin_cannot_take_are_counted_in_the_dropped_figure() {
    // The synth accepts CLAP and MIDI; the midisynth only MIDI: an expression is dropped.
    let (backend, _) = backend_for("atomik-test-clap-variants");
    let (mut inst, _manager) = bare(&backend, "atomik.test.midisynth");
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let events = [
        ev(
            0,
            K::NoteOn {
                key: 60,
                velocity: 1.0,
                note_id: 0,
            },
        ),
        ev(
            1,
            K::NoteExpr {
                key: 60,
                note_id: 0,
                kind: atomik_core::ExprKind::Tuning,
                value: 1.0,
            },
        ),
        ev(2, K::PitchBend { value: 0.5 }),
    ];
    let (_, dropped) = b.run(&mut *p, &events, 0, false, 2);
    assert_eq!(dropped, 1);
    inst.deactivate(p);
}
