//! The VST3 specifics: connection through proxies, split and
//! single-component plugins, bus negotiation and activation, parameter queues with offsets,
//! `IMidiMapping` and the dropped counter, note ids, the controller state sections, restarts,
//! the editor view and edits made in it, and the host objects a plugin talks to.
// Justification: exact dyadic values and copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{
    EventList, NativeParent, PluginEvent, PluginEventKind as K, PluginIo, PluginMessage,
    PluginProcessor, PluginTransport, ProcessStatus, SleepRule, TailLength,
};
use atomik_test_plugin_core as core;
use support::*;

struct Bufs {
    main_in: [Batch; 2],
    sidechain: [Batch; 2],
    main_out: [Batch; 2],
    out_events: EventList,
}

impl Bufs {
    fn new() -> Self {
        let mut main_in = [[0.0; BATCH]; 2];
        main_in[0][0] = 0.5; // a signal, so a VST3 effect with a zero tail is processed
        Self {
            main_in,
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
        n_out: usize,
    ) -> (ProcessStatus, u32) {
        self.out_events.clear();
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &self.main_in[..n_in],
            main_in_silent: n_in == 0,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut self.main_out[..n_out],
            events_in: events,
            events_out: &mut self.out_events,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        let status = p.process(&mut io);
        let _ = &self.sidechain;
        (status, io.dropped)
    }
}

fn ev(offset: u16, kind: K) -> PluginEvent {
    PluginEvent { offset, kind }
}

fn k() -> (Kit, TestLib) {
    let k = kit(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    (k, lib)
}

fn drain(m: &atomik_plugin_host::ManagerEnd) -> Vec<PluginMessage> {
    std::iter::from_fn(|| m.try_recv()).collect()
}

// ----- connection and the two class layouts -----------------------------------------------------

#[test]
fn a_split_plugin_is_connected_through_proxies_and_talks_across_them() {
    let kit = kit_variants(Format::Vst3);
    let lib = TestLib::open_path(&kit.path);
    let plugin = kit.variant_of_split();
    let r = rig_for(&*kit.backend, &plugin, RunMode::Realtime);
    let inst = r.instance;
    // The component and the controller were both connected, and the component's link message
    // (sent at activation) and the controller's answer both crossed the proxies.
    assert_eq!(lib.probe(&*inst, probe::CONNECTS), 2);
    assert!(lib.probe(&*inst, probe::MESSAGES) >= 2, "link and ack");
    // The controller adopted the component's probes: an editor opened on the split plugin is
    // visible through the component's handle.
    assert!(inst.has_gui());
}

#[test]
fn a_single_component_plugin_is_not_connected() {
    let (k, lib) = k();
    let (inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    assert_eq!(lib.probe(&*inst, probe::CONNECTS), 0);
    assert_eq!(lib.probe(&*inst, probe::MESSAGES), 0);
}

#[test]
fn the_controller_is_brought_in_step_with_the_component_when_the_plugin_is_created() {
    let (k, lib) = k();
    let (inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    assert!(lib.probe(&*inst, probe::STATE_SAVES) >= 1);
    assert!(lib.probe(&*inst, probe::CTRL_COMPONENT_STATES) >= 1);
}

// ----- buses -------------------------------------------------------------------------------------

#[test]
fn the_host_negotiates_arrangements_and_activates_exactly_the_buses_it_drives() {
    let (k, lib) = k();
    // bits: audio in 0, audio in 1 (sidechain), audio out 0, events in, events out
    let a = rig(&k, Plug::Gain);
    assert!(lib.probe(&*a.instance, probe::ARRANGEMENT_CALLS) >= 1);
    assert_eq!(lib.probe(&*a.instance, probe::BUS_ACTIVE), 0b00111);
    let s = rig(&k, Plug::Synth);
    assert_eq!(lib.probe(&*s.instance, probe::BUS_ACTIVE), 0b01100);
    let n = rig(&k, Plug::Shift);
    assert_eq!(lib.probe(&*n.instance, probe::BUS_ACTIVE), 0b11000);
}

#[test]
fn deactivating_clears_the_activation_and_a_second_activation_works() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    inst.deactivate(p);
    assert_eq!(lib.probe(&*inst, probe::DEACTIVATIONS), 1);
    assert_eq!(lib.probe(&*inst, probe::BUS_ACTIVE), 0);
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(lib.probe(&*inst, probe::ACTIVATIONS), 2);
    inst.deactivate(p);
}

// ----- the processor called directly ---------------------------------------------------------------

#[test]
fn the_vst3_sleep_rule_is_zero_tail_and_the_tail_follows_the_plugin() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(p.sleep_rule(), SleepRule::ZeroTail);
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let (status, _) = b.run(&mut *p, &[], 2, 2);
    assert_eq!(status, ProcessStatus::Continue);
    assert_eq!(p.tail_samples(), TailLength::Finite(0));
    b.run(&mut *p, &[ev(0, K::Param { id: 3, value: 37.0 })], 2, 2);
    assert_eq!(p.tail_samples(), TailLength::Finite(37));
    assert!(!p.wake_requested());
    p.reset();
    p.stop_processing();
    assert!(p.start_processing(), "a stopped processor starts again");
    inst.deactivate(p);
}

#[test]
fn an_infinite_tail_is_reported_as_infinite() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(
        &mut *p,
        &[ev(
            0,
            K::NoteOn {
                key: 60,
                velocity: 1.0,
                note_id: 0,
            },
        )],
        0,
        2,
    );
    assert_eq!(p.tail_samples(), TailLength::Infinite, "a sounding voice");
    inst.deactivate(p);
}

#[test]
fn parameter_changes_reach_the_plugin_as_points_at_their_offsets() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(
        &mut *p,
        &[
            ev(17, K::Param { id: 1, value: 0.25 }),
            ev(17, K::Param { id: 4, value: 0.5 }),
            ev(40, K::Param { id: 1, value: 0.5 }),
        ],
        2,
        2,
    );
    assert_eq!(lib.probe(&*inst, probe::PARAM_POINTS), 3);
    assert_eq!(lib.probe(&*inst, probe::LAST_POINT_OFFSET), 40);
    assert_eq!(lib.probe(&*inst, probe::EVENTS_IN), 3);
    inst.deactivate(p);
}

#[test]
fn more_parameter_points_than_the_queue_holds_are_dropped_and_counted() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let events: Vec<PluginEvent> = (0..20u16)
        .map(|i| ev(i, K::Param { id: 1, value: 0.5 }))
        .collect();
    let (_, dropped) = b.run(&mut *p, &events, 2, 2);
    assert_eq!(dropped, 4, "a queue holds 16 points per block");
    inst.deactivate(p);
}

#[test]
fn midi_controllers_go_through_the_mapping_and_an_unmapped_one_is_counted() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let (_, dropped) = b.run(
        &mut *p,
        &[
            ev(1, K::PitchBend { value: 0.5 }),
            ev(
                2,
                K::Cc {
                    controller: 1,
                    value: 0.5,
                },
            ),
            ev(3, K::ChannelPressure { value: 0.25 }),
            ev(
                4,
                K::Cc {
                    controller: 123,
                    value: 0.0,
                },
            ),
        ],
        0,
        2,
    );
    assert_eq!(dropped, 0, "the four are mapped");
    assert_eq!(lib.probe(&*inst, probe::PARAM_POINTS), 4);
    let (_, dropped) = b.run(
        &mut *p,
        &[ev(
            1,
            K::Cc {
                controller: 7,
                value: 0.5,
            },
        )],
        0,
        2,
    );
    assert_eq!(dropped, 1, "CC 7 has no assignment");
    inst.deactivate(p);
    // A plugin with no mapping at all drops pitch bend too, but not the all-notes-off CC.
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Shift));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let (_, dropped) = b.run(
        &mut *p,
        &[
            ev(1, K::PitchBend { value: 0.5 }),
            ev(
                2,
                K::Cc {
                    controller: 123,
                    value: 0.0,
                },
            ),
        ],
        0,
        0,
    );
    assert_eq!(dropped, 1);
    inst.deactivate(p);
}

#[test]
fn note_ids_are_the_hosts_and_expressions_need_the_plugins_support() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(
        &mut *p,
        &[ev(
            0,
            K::NoteOn {
                key: 64,
                velocity: 0.75,
                note_id: 41,
            },
        )],
        0,
        2,
    );
    assert_eq!(lib.probe(&*inst, probe::LAST_NOTE_ID), 41);
    inst.deactivate(p);
    // The note effect has no expression controller: an expression is dropped, a poly pressure
    // is not (it is a plain event).
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Shift));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let (_, dropped) = b.run(
        &mut *p,
        &[
            ev(
                0,
                K::NoteExpr {
                    key: 60,
                    note_id: 1,
                    kind: atomik_core::ExprKind::Tuning,
                    value: 1.0,
                },
            ),
            ev(
                1,
                K::PolyPressure {
                    key: 60,
                    note_id: 1,
                    value: 0.5,
                },
            ),
        ],
        0,
        0,
    );
    assert_eq!(dropped, 1);
    inst.deactivate(p);
}

#[test]
fn events_that_do_not_fit_the_list_are_dropped_and_counted() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let events: Vec<PluginEvent> = (0..4_700u32)
        .map(|i| {
            ev(
                0,
                K::NoteOn {
                    key: (i % 100) as u8,
                    velocity: 0.5,
                    note_id: i as i32,
                },
            )
        })
        .collect();
    let (_, dropped) = b.run(&mut *p, &events, 0, 2);
    assert_eq!(dropped, 4_700 - 4_608, "4 096 + 512 events fit a block");
    inst.deactivate(p);
}

#[test]
fn a_note_effects_output_events_come_back_with_their_offsets() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Shift));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(
        &mut *p,
        &[
            ev(2, K::Param { id: 1, value: 29.0 }), // +5 semitones (index 29 of -24..24)
            ev(
                2,
                K::NoteOn {
                    key: 60,
                    velocity: 0.5,
                    note_id: 3,
                },
            ),
            ev(
                20,
                K::NoteOff {
                    key: 60,
                    note_id: 3,
                },
            ),
        ],
        0,
        0,
    );
    let got: Vec<PluginEvent> = b.out_events.as_slice().to_vec();
    assert_eq!(
        got,
        vec![
            ev(
                2,
                K::NoteOn {
                    key: 65,
                    velocity: 0.5,
                    note_id: 3
                }
            ),
            ev(
                20,
                K::NoteOff {
                    key: 65,
                    note_id: 3
                }
            ),
        ]
    );
    inst.deactivate(p);
}

// ----- state: the two sections ---------------------------------------------------------------------

#[test]
fn the_state_holds_the_component_and_the_controller_sections_and_restores_both() {
    let (k, lib) = k();
    let (mut a, _ma) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let before = lib.probe(&*a, probe::CTRL_STATE_SAVES);
    let blob = a.capture_state().unwrap();
    assert_eq!(&blob.0[..4], b"AVS1");
    assert_eq!(lib.probe(&*a, probe::CTRL_STATE_SAVES), before + 1);
    let (mut b, _mb) = bare(&*k.backend, &k.plugin(Plug::Synth));
    let (loads, ctrl_loads, comp_states) = (
        lib.probe(&*b, probe::STATE_LOADS),
        lib.probe(&*b, probe::CTRL_STATE_LOADS),
        lib.probe(&*b, probe::CTRL_COMPONENT_STATES),
    );
    b.restore_state(&blob).unwrap();
    assert_eq!(
        lib.probe(&*b, probe::STATE_LOADS),
        loads + 1,
        "component setState"
    );
    assert_eq!(
        lib.probe(&*b, probe::CTRL_COMPONENT_STATES),
        comp_states + 1,
        "controller setComponentState"
    );
    assert_eq!(
        lib.probe(&*b, probe::CTRL_STATE_LOADS),
        ctrl_loads + 1,
        "controller setState"
    );
}

#[test]
fn garbage_is_not_a_state_and_the_plugin_is_left_alone() {
    let (k, lib) = k();
    let (mut a, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let loads = lib.probe(&*a, probe::STATE_LOADS);
    let err = a
        .restore_state(&atomik_device_sdk::PluginStateBlob(b"nonsense".to_vec()))
        .unwrap_err();
    assert!(
        matches!(err, atomik_plugin_host::BackendError::Failed(_)),
        "{err:?}"
    );
    assert_eq!(lib.probe(&*a, probe::STATE_LOADS), loads);
    // A well-formed container whose component section the plugin refuses.
    let mut blob = b"AVS1".to_vec();
    blob.extend_from_slice(&3u32.to_le_bytes());
    blob.extend_from_slice(b"bad");
    blob.extend_from_slice(&0u32.to_le_bytes());
    let err = a
        .restore_state(&atomik_device_sdk::PluginStateBlob(blob))
        .unwrap_err();
    assert!(err.to_string().contains("refused"), "{err}");
}

// ----- restartComponent -------------------------------------------------------------------------------

#[test]
fn restart_requests_become_the_matching_messages() {
    let (k, lib) = k();
    let r = rig(&k, Plug::Gain);
    let _ = drain(&r.manager);
    let cases: [(u32, PluginMessage); 4] = [
        (command::LATENCY_CHANGED, PluginMessage::LatencyChanged),
        (command::REQUEST_RESTART, PluginMessage::RequestRestart),
        (command::MARK_DIRTY, PluginMessage::StateDirty),
        (command::PARAM_VALUES_CHANGED, PluginMessage::StateDirty),
    ];
    for (cmd, want) in cases {
        lib.command(&*r.instance, cmd, 0, 0);
        let got = drain(&r.manager);
        assert!(got.contains(&want), "command {cmd}: {got:?}");
    }
    lib.command(&*r.instance, command::MIDI_MAPPING_CHANGED, 0, 0);
    assert!(drain(&r.manager).contains(&PluginMessage::RequestRestart));
    lib.command(&*r.instance, command::IO_CHANGED, 0, 0);
    let got = drain(&r.manager);
    assert!(
        got.contains(&PluginMessage::RequestRestart) && got.contains(&PluginMessage::RescanParams),
        "{got:?}"
    );
}

#[test]
fn a_param_titles_change_gives_a_new_table_at_the_next_activation() {
    let (k, lib) = k();
    let (mut inst, m) = bare(&*k.backend, &k.plugin(Plug::Params));
    assert_eq!(inst.layout().params.len(), 2_000);
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    lib.command(&*inst, command::RESCAN_PARAMS, 0, 0);
    assert!(drain(&m).contains(&PluginMessage::RescanParams));
    // The restart: deactivate, the plugin's table is re-read, then activate again.
    inst.deactivate(p);
    assert!(inst.take_layout_changed(), "the layout changed");
    assert!(!inst.take_layout_changed(), "reported once");
    assert_eq!(inst.layout().params.len(), 2_001);
    assert_eq!(inst.layout().params[2_000].id, core::ParamBank::EXTRA_ID);
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    inst.deactivate(p);
}

#[test]
fn a_changed_latency_is_read_at_the_next_activation() {
    let (k, lib) = k();
    let (mut inst, m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(inst.latency_frames(), 0);
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(&mut *p, &[ev(0, K::Param { id: 3, value: 64.0 })], 2, 2);
    // The plugin tells the host (restartComponent), the host restarts it and asks again.
    assert_eq!(inst.latency_frames(), 0, "read at activation, not before");
    let _ = drain(&m);
    lib.command(&*inst, command::LATENCY_CHANGED, 0, 0);
    assert!(drain(&m).contains(&PluginMessage::LatencyChanged));
    p.stop_processing();
    inst.deactivate(p);
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(inst.latency_frames(), 64);
    inst.deactivate(p);
}

#[test]
fn a_state_restored_into_a_running_plugin_changes_its_latency_at_once() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert_eq!(inst.latency_frames(), 0);
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::LATENCY, 64.0);
    inst.restore_state(&wrap_state(Format::Vst3, &fx.save_state()))
        .unwrap();
    assert_eq!(
        inst.latency_frames(),
        64,
        "the latency a state brings is read while the plugin is active"
    );
    inst.deactivate(p);
}

// ----- the controller follows the rack -------------------------------------------------------------

#[test]
fn values_the_rack_set_before_the_processor_went_back_reach_the_controller_at_deactivation() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(&mut *p, &[ev(0, K::Param { id: 1, value: 0.75 })], 2, 2);
    // No poll ran: the value still waits in the mirror ring when the processor goes back.
    let before = lib.probe(&*inst, probe::CTRL_SET_PARAMS);
    p.stop_processing();
    inst.deactivate(p);
    assert_eq!(
        lib.probe(&*inst, probe::CTRL_SET_PARAMS),
        before + 1,
        "deactivation hands the controller the value the rack set"
    );
    assert_eq!(
        f64::from_bits(lib.probe(&*inst, probe::LAST_SET_VALUE)),
        0.75
    );
}

#[test]
fn values_the_rack_sets_reach_the_controller_on_the_main_thread_so_an_editor_shows_them() {
    let (k, lib) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    let before = lib.probe(&*inst, probe::CTRL_SET_PARAMS);
    b.run(
        &mut *p,
        &[
            ev(0, K::Param { id: 1, value: 0.25 }),
            ev(3, K::Param { id: 3, value: 37.0 }),
        ],
        2,
        2,
    );
    // Nothing moves on the audio thread; the main thread applies them in `poll`.
    assert_eq!(lib.probe(&*inst, probe::CTRL_SET_PARAMS), before);
    inst.poll();
    assert_eq!(lib.probe(&*inst, probe::CTRL_SET_PARAMS), before + 2);
    assert_eq!(lib.probe(&*inst, probe::LAST_SET_ID), 3);
    assert_eq!(
        f64::from_bits(lib.probe(&*inst, probe::LAST_SET_VALUE)),
        37.0 / 512.0,
        "a stepped value is mirrored as its normalised value"
    );
    inst.poll();
    assert_eq!(
        lib.probe(&*inst, probe::CTRL_SET_PARAMS),
        before + 2,
        "once"
    );
    inst.deactivate(p);
}

#[test]
fn a_state_taken_right_after_a_rack_edit_holds_the_edit_in_the_controller_section() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let mut b = Bufs::new();
    b.run(&mut *p, &[ev(0, K::Param { id: 1, value: 0.75 })], 2, 2);
    let blob = inst.capture_state().unwrap().0;
    // `AVS1`, the component section, then the controller section: `CT01`, a count, then the
    // normalised values of the listed params in order (gain first).
    let rest = &blob[4..];
    let comp_len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
    let ctrl = &rest[4 + comp_len..];
    let ctrl_len = u32::from_le_bytes(ctrl[..4].try_into().unwrap()) as usize;
    let ctrl = &ctrl[4..4 + ctrl_len];
    assert_eq!(&ctrl[..4], b"CT01");
    assert_eq!(u32::from_le_bytes(ctrl[4..8].try_into().unwrap()), 4);
    assert_eq!(f64::from_le_bytes(ctrl[8..16].try_into().unwrap()), 0.75);
    inst.deactivate(p);
}

// ----- parameter text -------------------------------------------------------------------------------

#[test]
fn the_param_text_comes_from_the_controller_for_the_hosts_value() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    // Gain 0.25 normalised is -32 dB in the test plugin's three-decimal text.
    assert_eq!(inst.param_text(1, 0.25).as_deref(), Some("-32.000"));
    // A stepped param is asked for by step index: latency step 37 is 37.
    assert_eq!(inst.param_text(3, 37.0).as_deref(), Some("37.000"));
    assert_eq!(
        inst.param_text(99, 0.5),
        None,
        "an unknown param has no text"
    );
}

// ----- the editor -----------------------------------------------------------------------------------

fn parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xABCD)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xABCD)
    } else {
        NativeParent::X11(0xABCD)
    }
}

#[test]
fn the_editor_is_created_scaled_sized_attached_resized_and_closed() {
    let (k, lib) = k();
    let (mut inst, m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    assert!(inst.has_gui());
    let (w, h) = inst.open_gui(parent(), 1.5).unwrap();
    assert_eq!((w, h), (320, 200), "the plugin's own size");
    let calls = lib.probe(&*inst, probe::GUI_CALLS);
    assert_eq!(calls & 1, 1, "attached");
    assert_eq!(calls & 4, 4, "getSize");
    assert_eq!(calls & 16, 16, "setFrame");
    assert_eq!(lib.probe(&*inst, probe::GUI_PARENT), 0xABCD);
    if cfg!(target_os = "macos") {
        assert_eq!(calls & 32, 0, "no content scale on macOS");
    } else {
        assert_eq!(calls & 32, 32, "content scale");
        assert_eq!(lib.probe_f64(&*inst, probe::GUI_SCALE), 1.5);
    }
    // The window resizes: the view is asked, constrained and told.
    let (w, h) = inst.resize_gui(500, 40).unwrap();
    assert_eq!(
        (w, h),
        (500, 80),
        "the plugin raised the height to its minimum"
    );
    assert_eq!(lib.probe(&*inst, probe::GUI_WIDTH), 500);
    assert_eq!(lib.probe(&*inst, probe::GUI_HEIGHT), 80);
    // The plugin asks for a size from another thread: the message arrives for the main thread.
    let handle = inst.native_handle();
    let path = lib.path.clone();
    let _ = drain(&m);
    std::thread::spawn(move || {
        let l = TestLib::open_path(&path);
        l.command_raw(handle, command::GUI_REQUEST_RESIZE, 410, 330);
    })
    .join()
    .unwrap();
    assert!(drain(&m).contains(&PluginMessage::GuiResize { w: 410, h: 330 }));
    // Opening twice is refused; closing removes the view.
    assert!(inst.open_gui(parent(), 1.0).is_err());
    inst.close_gui();
    assert_eq!(lib.probe(&*inst, probe::GUI_CALLS) & 2, 2, "removed");
    // And it can be opened again.
    inst.open_gui(parent(), 1.0).unwrap();
    inst.close_gui();
}

#[test]
fn a_plugin_without_an_editor_has_no_gui_and_a_refused_parent_is_an_error() {
    let (k, _) = k();
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    assert!(!inst.has_gui());
    assert!(matches!(
        inst.open_gui(parent(), 1.0),
        Err(atomik_plugin_host::BackendError::Unsupported(_))
    ));
    assert!(inst.resize_gui(10, 10).is_err());
    // The wrong window system for this OS is refused by the view.
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    let wrong = if cfg!(windows) {
        NativeParent::Cocoa(1)
    } else {
        NativeParent::Win32(1)
    };
    assert!(matches!(
        inst.open_gui(wrong, 1.0),
        Err(atomik_plugin_host::BackendError::Unsupported(_))
    ));
}

#[test]
fn an_edit_in_the_editor_reaches_the_app_as_a_gesture_and_the_processor_as_a_param() {
    let (k, lib) = k();
    let mut r = rig(&k, Plug::Gain);
    let mut h = r.harness();
    *h.input_mut("in_l") = [1.0; BATCH];
    *h.input_mut("in_r") = [1.0; BATCH];
    h.process();
    let _ = drain(&r.manager);
    let n = core::normalized_from_plain(-64.0, 64.0, -12.0);
    lib.command(&*r.instance, command::GUI_EDIT, 1, n.to_bits());
    let msgs = drain(&r.manager);
    use atomik_plugin_host::ParamPhase::{Begin, Change, End};
    let edits: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            PluginMessage::ParamEdit { id, value, phase } => Some((*id, *value, *phase)),
            _ => None,
        })
        .collect();
    assert_eq!(edits.len(), 3, "{msgs:?}");
    assert!(matches!(edits[0], (1, v, Begin) if v.is_nan()));
    assert_eq!(edits[1], (1, n, Change), "VST3 edits are normalised");
    assert!(matches!(edits[2], (1, v, End) if v.is_nan()));
    // The processor heard it through the host: the output is 12 dB down.
    h.process();
    let want = 10f32.powf(-12.0 / 20.0);
    assert!(
        (h.output("out_l")[10] - want).abs() < 1e-4,
        "{}",
        h.output("out_l")[10]
    );
}

#[test]
fn an_edit_of_a_stepped_param_is_reported_as_its_step_index() {
    let (k, lib) = k();
    let r = rig(&k, Plug::Gain);
    let _ = drain(&r.manager);
    lib.command(
        &*r.instance,
        command::GUI_EDIT,
        3,
        (37.0f64 / 512.0).to_bits(),
    );
    let value = drain(&r.manager)
        .into_iter()
        .find_map(|m| match m {
            PluginMessage::ParamEdit {
                value,
                phase: atomik_plugin_host::ParamPhase::Change,
                ..
            } => Some(value),
            _ => None,
        })
        .unwrap();
    assert_eq!(value, 37.0);
}
