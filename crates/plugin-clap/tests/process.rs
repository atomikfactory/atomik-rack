//! Against the real CLAP backend: the device drives real
//! CLAP plugins through `clack-host`. Outputs equal the reference DSP of
//! `atomik-test-plugin-core` run directly, notes and expressions arrive in the plugin's
//! dialect, transport and silence flags are right, and a misbehaving plugin cannot break the
//! host.
// Justification: copies of the same f32 samples and exact dyadic values are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, ExprKind, NoteEvent, NoteKind, PPQ};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::PluginMessage;
use atomik_test_plugin_core as core;
use support::*;

const ID_GAIN: &str = "atomik.test.gain";
const ID_SYNTH: &str = "atomik.test.synth";
const ID_SHIFT: &str = "atomik.test.noteshift";

fn ramp(scale: f32) -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 100.0;
    }
    b
}

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

fn meters(h: &atomik_testkit::harness::DeviceHarness) -> [f32; 5] {
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    m
}

fn messages(rig: &ClapRig) -> Vec<PluginMessage> {
    std::iter::from_fn(|| rig.manager.try_recv()).collect()
}

// ----- activation and the fixed block ---------------------------------------------------------

#[test]
fn the_plugin_is_activated_at_the_session_rate_with_exactly_64_frames_per_block() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    assert_eq!(lib.probe(&*rig.instance, probe::MIN_FRAMES), 64);
    assert_eq!(lib.probe(&*rig.instance, probe::MAX_FRAMES), 64);
    assert_eq!(lib.probe_f64(&*rig.instance, probe::SAMPLE_RATE), SR);
    let mut h = rig.harness();
    h.process();
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::FRAMES), 64);
    assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), 2);
}

#[test]
fn the_render_mode_follows_the_run_mode() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let live = rig_with(&backend, ID_GAIN, RunMode::Realtime);
    assert_eq!(lib.probe(&*live.instance, probe::RENDER_MODE), 0);
    let offline = rig_with(&backend, ID_GAIN, RunMode::Offline);
    assert_eq!(lib.probe(&*offline.instance, probe::RENDER_MODE), 1);
}

// ----- the effect -------------------------------------------------------------------------------

#[test]
fn the_effect_output_equals_the_reference_dsp_run_directly() {
    let (backend, _) = backend_for("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    let mut h = rig.harness();
    let mut reference = core::GainFx::new(SR);
    h.set_param("p1", -4.5);
    h.set_param("p3", 37.0);
    h.set_param("p4", 0.75);
    reference.set_param(core::GainFx::GAIN, -4.5);
    reference.set_param(core::GainFx::LATENCY, 37.0);
    reference.set_param(core::GainFx::DUCK, 0.75);
    for n in 0..30 {
        let l = ramp(1.0 + n as f32 * 0.1);
        let r = ramp(-0.5 * n as f32);
        let sc = ramp(if n % 3 == 0 { 2.0 } else { 0.0 });
        *h.input_mut("in_l") = l;
        *h.input_mut("in_r") = r;
        *h.input_mut("sc_in_l") = sc;
        *h.input_mut("sc_in_r") = sc;
        h.process();
        let mut out = [[0.0; BATCH]; 2];
        reference.process(&[l, r], Some(&[sc, sc]), &mut out);
        assert_eq!(&h.output("out_l")[..], &out[0][..], "batch {n} left");
        assert_eq!(&h.output("out_r")[..], &out[1][..], "batch {n} right");
    }
    assert!(h.output("out_l").iter().any(|s| s.abs() > 0.1));
}

#[test]
fn silent_inputs_carry_the_constant_flag_on_every_channel() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    let mut h = rig.harness();
    h.process(); // nothing cabled
    assert_eq!(lib.probe(&*rig.instance, probe::MASK_MAIN), 0b11);
    assert_eq!(lib.probe(&*rig.instance, probe::MASK_SIDECHAIN), 0b11);
    h.input_mut("in_l")[3] = 1.0; // the main input is cabled
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::MASK_MAIN), 0);
    assert_eq!(lib.probe(&*rig.instance, probe::MASK_SIDECHAIN), 0b11);
    h.input_mut("sc_in_r")[0] = 1.0; // and the sidechain
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::MASK_SIDECHAIN), 0);
}

#[test]
fn the_sidechain_ducks_the_effect_through_the_plugin() {
    let (backend, _) = backend_for("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    let mut h = rig.harness();
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [0.5; BATCH];
    h.process();
    let open = h.output("out_l")[BATCH - 1];
    *h.input_mut("sc_in_l") = [1.0; BATCH];
    *h.input_mut("sc_in_r") = [1.0; BATCH];
    h.process();
    let ducked = h.output("out_l")[BATCH - 1];
    assert!(ducked < open * 0.5, "{ducked} vs {open}");
}

#[test]
fn a_mono_plugin_gets_the_mean_of_the_cabled_inputs_and_answers_on_both_outputs() {
    let (backend, _) = backend_for("atomik-test-clap-variants");
    let mut rig = rig(&backend, "atomik.test.mono");
    let mut h = rig.harness();
    h.set_param("p1", 0.5);
    *h.input_mut("in_l") = ramp(1.0);
    *h.input_mut("in_r") = ramp(3.0);
    h.process();
    let want: Vec<f32> = ramp(1.0)
        .iter()
        .zip(ramp(3.0).iter())
        .map(|(a, b)| (a + b) * 0.5 * 0.5)
        .collect();
    assert_eq!(&h.output("out_l")[..], &want[..]);
    assert_eq!(&h.output("out_r")[..], &want[..]);
}

// ----- the instrument ---------------------------------------------------------------------------

fn core_note(offset: u16, event: core::Event) -> core::TimedEvent {
    core::TimedEvent { offset, event }
}

/// The scripted note stream of the synth tests: (batch, events).
fn script() -> Vec<(usize, Vec<NoteEvent>)> {
    vec![
        (
            0,
            vec![
                ev(
                    5,
                    NoteKind::On {
                        key: 60,
                        velocity: 0.5,
                    },
                ),
                ev(9, NoteKind::PitchBend { value: 0.5 }),
                ev(
                    12,
                    NoteKind::Cc {
                        controller: 1,
                        value: 64.0 / 127.0,
                    },
                ),
                ev(
                    20,
                    NoteKind::ChannelPressure {
                        value: 32.0 / 127.0,
                    },
                ),
                ev(
                    30,
                    NoteKind::PolyPressure {
                        key: 60,
                        value: 96.0 / 127.0,
                    },
                ),
            ],
        ),
        (
            1,
            vec![
                ev(
                    2,
                    NoteKind::On {
                        key: 67,
                        velocity: 1.0,
                    },
                ),
                ev(
                    10,
                    NoteKind::NoteExpr {
                        key: 60,
                        kind: ExprKind::Tuning,
                        value: 2.0,
                    },
                ),
                ev(
                    11,
                    NoteKind::NoteExpr {
                        key: 67,
                        kind: ExprKind::Pressure,
                        value: 0.25,
                    },
                ),
                ev(
                    12,
                    NoteKind::NoteExpr {
                        key: 67,
                        kind: ExprKind::Volume,
                        value: 0.5,
                    },
                ),
                ev(
                    13,
                    NoteKind::NoteExpr {
                        key: 60,
                        kind: ExprKind::Pan,
                        value: -0.25,
                    },
                ),
            ],
        ),
        (3, vec![ev(3, NoteKind::Off { key: 60 })]),
        (6, vec![ev(0, NoteKind::AllOff)]),
    ]
}

/// The same stream as the plugin's reference events. `ids` is true for the CLAP dialect (the
/// host assigns note ids 0, 1, ...), false for MIDI (no ids).
fn reference_events(ids: bool, with_expr: bool) -> Vec<(usize, Vec<core::TimedEvent>)> {
    use core::Event::*;
    let id = |n: i32| if ids { n } else { -1 };
    let mut v = vec![
        (
            0,
            vec![
                // MIDI carries 7 bits of velocity: 0.5 becomes 64/127.
                core_note(
                    5,
                    NoteOn {
                        key: 60,
                        velocity: if ids { 0.5 } else { 64.0 / 127.0 },
                        id: id(0),
                    },
                ),
                core_note(9, PitchBend(0.5)),
                core_note(
                    12,
                    Cc {
                        controller: 1,
                        value: 64.0 / 127.0,
                    },
                ),
                core_note(20, ChannelPressure(32.0 / 127.0)),
                core_note(
                    30,
                    PolyPressure {
                        key: 60,
                        id: -1,
                        value: 96.0 / 127.0,
                    },
                ),
            ],
        ),
        (
            1,
            vec![core_note(
                2,
                NoteOn {
                    key: 67,
                    velocity: 1.0,
                    id: id(1),
                },
            )],
        ),
        (3, vec![core_note(3, NoteOff { key: 60, id: id(0) })]),
        (
            6,
            vec![
                core_note(0, NoteOff { key: 67, id: id(1) }),
                core_note(
                    0,
                    Cc {
                        controller: 123,
                        value: 0.0,
                    },
                ),
            ],
        ),
    ];
    if with_expr {
        v[1].1.extend([
            core_note(
                10,
                Expr {
                    key: 60,
                    id: 0,
                    kind: core::ExprKind::Tuning,
                    value: 2.0,
                },
            ),
            core_note(
                11,
                Expr {
                    key: 67,
                    id: 1,
                    kind: core::ExprKind::Pressure,
                    value: 0.25,
                },
            ),
            core_note(
                12,
                Expr {
                    key: 67,
                    id: 1,
                    kind: core::ExprKind::Volume,
                    value: 0.5,
                },
            ),
            core_note(
                13,
                Expr {
                    key: 60,
                    id: 0,
                    kind: core::ExprKind::Pan,
                    value: -0.25,
                },
            ),
        ]);
    }
    v
}

fn run_synth(
    backend: &atomik_plugin_clap::ClapBackend,
    id: &str,
    reference: &[(usize, Vec<core::TimedEvent>)],
) -> (ClapRig, Vec<[Batch; 2]>) {
    let mut rig = rig(backend, id);
    let mut h = rig.harness();
    let mut synth = core::TestSynth::new(SR);
    let script = script();
    let mut got = Vec::new();
    for b in 0..14usize {
        for (batch, events) in &script {
            if *batch == b {
                for e in events {
                    h.push_note("notes_in", *e);
                }
            }
        }
        h.process();
        let events: Vec<core::TimedEvent> = reference
            .iter()
            .filter(|(batch, _)| *batch == b)
            .flat_map(|(_, e)| e.clone())
            .collect();
        let mut out = [[0.0; BATCH]; 2];
        synth.process(&events, &mut out);
        assert_eq!(&h.output("out_l")[..], &out[0][..], "batch {b} left");
        assert_eq!(&h.output("out_r")[..], &out[1][..], "batch {b} right");
        got.push(out);
    }
    (rig, got)
}

#[test]
fn the_synth_output_equals_the_reference_dsp_with_clap_notes_and_expressions() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let (rig, got) = run_synth(&backend, ID_SYNTH, &reference_events(true, true));
    assert!(
        got.iter().any(|o| o[0].iter().any(|s| s.abs() > 0.01)),
        "it sounds"
    );
    let dialect = lib.probe(&*rig.instance, probe::EVENT_DIALECT);
    assert_eq!(dialect & 2, 2, "CLAP note events were sent");
}

#[test]
fn a_midi_only_plugin_gets_midi_one_events_and_loses_the_expressions() {
    let (backend, _) = backend_for("atomik-test-clap-variants");
    let lib = TestLib::open("atomik-test-clap-variants");
    let mut r = rig(&backend, "atomik.test.midisynth");
    let mut h = r.harness();
    // Notes, bend, CC and pressure in MIDI; the expression is dropped and counted.
    let mut synth = core::TestSynth::new(SR);
    let script = script();
    let reference = reference_events(false, false);
    for b in 0..8usize {
        for (batch, events) in &script {
            if *batch == b {
                for e in events {
                    h.push_note("notes_in", *e);
                }
            }
        }
        h.process();
        let events: Vec<core::TimedEvent> = reference
            .iter()
            .filter(|(batch, _)| *batch == b)
            .flat_map(|(_, e)| e.clone())
            .collect();
        let mut out = [[0.0; BATCH]; 2];
        synth.process(&events, &mut out);
        assert_eq!(&h.output("out_l")[..], &out[0][..], "batch {b}");
    }
    assert_eq!(
        lib.probe(&*r.instance, probe::EVENT_DIALECT),
        1,
        "MIDI only, no CLAP note events"
    );
    assert_eq!(
        meters(&h)[4],
        4.0,
        "the four expressions were dropped and counted"
    );
}

#[test]
fn note_ons_and_offs_arrive_at_their_exact_frames() {
    let (backend, _) = backend_for("atomik-test-clap");
    let mut rig = rig(&backend, ID_SYNTH);
    let mut h = rig.harness();
    h.push_note(
        "notes_in",
        ev(
            17,
            NoteKind::On {
                key: 69,
                velocity: 1.0,
            },
        ),
    );
    h.process();
    let out = h.output("out_l");
    assert!(out[..17].iter().all(|s| *s == 0.0), "silent before the On");
    assert!(out[17..].iter().any(|s| s.abs() > 0.0), "sounding after it");
}

// ----- the note effect --------------------------------------------------------------------------

#[test]
fn a_note_effect_sends_its_notes_to_notes_out() {
    let (backend, _) = backend_for("atomik-test-clap");
    let mut rig = rig(&backend, ID_SHIFT);
    let mut h = rig.harness();
    h.set_param("p1", 5.0);
    h.push_note(
        "notes_in",
        ev(
            3,
            NoteKind::On {
                key: 60,
                velocity: 0.5,
            },
        ),
    );
    h.push_note("notes_in", ev(40, NoteKind::Off { key: 60 }));
    h.process();
    let out: Vec<_> = h.notes_out("notes_out").to_vec();
    assert_eq!(
        out,
        vec![
            ev(
                3,
                NoteKind::On {
                    key: 65,
                    velocity: 0.5
                }
            ),
            ev(40, NoteKind::Off { key: 65 }),
        ]
    );
}

// ----- transport ---------------------------------------------------------------------------------

#[test]
fn the_transport_reaches_the_plugin_through_a_locate_and_a_loop_wrap() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    let mut h = rig.harness();
    let has = |flags: u64, bit: u32| flags & u64::from(bit) != 0;
    const PLAYING: u32 = 1 << 4;
    const LOOP: u32 = 1 << 6;
    {
        let t = h.time_mut();
        t.playing = true;
        t.tick = 4.0 * PPQ as f64;
        t.tempo = atomik_core::Tempo::new(133.0).unwrap();
        t.ticks_per_frame = 133.0 * PPQ as f64 / (60.0 * SR);
        t.time_signature = (3, 4);
        t.loop_ticks = Some((2.0 * PPQ as f64, 6.0 * PPQ as f64));
    }
    h.process();
    let flags = lib.probe(&*rig.instance, probe::TRANSPORT_FLAGS);
    assert!(has(flags, PLAYING) && has(flags, LOOP), "{flags:#b}");
    assert_eq!(lib.probe_f64(&*rig.instance, probe::TEMPO), 133.0);
    assert_eq!(lib.probe_f64(&*rig.instance, probe::BEATS), 4.0);
    assert_eq!(lib.probe(&*rig.instance, probe::TSIG_NUM), 3);
    assert_eq!(lib.probe(&*rig.instance, probe::TSIG_DEN), 4);
    assert_eq!(lib.probe_f64(&*rig.instance, probe::LOOP_START), 2.0);
    assert_eq!(lib.probe_f64(&*rig.instance, probe::LOOP_END), 6.0);
    let seconds = lib.probe_f64(&*rig.instance, probe::SECONDS);
    assert!((seconds - 4.0 * 60.0 / 133.0).abs() < 1e-6, "{seconds}");
    // A locate: the position jumps and the plugin sees the jump.
    h.time_mut().tick = 0.5 * PPQ as f64;
    h.process();
    assert_eq!(lib.probe_f64(&*rig.instance, probe::BEATS), 0.5);
    // Stopped: the playing flag goes.
    h.time_mut().playing = false;
    h.process();
    let flags = lib.probe(&*rig.instance, probe::TRANSPORT_FLAGS);
    assert!(!has(flags, PLAYING), "{flags:#b}");
    // The steady time is the engine's frame counter.
    let frame = h.frame();
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::STEADY), frame);
}

// ----- sleep and wake ------------------------------------------------------------------------------

#[test]
fn the_synth_sleeps_when_idle_and_wakes_for_a_note_and_for_a_request() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let mut rig = rig(&backend, ID_SYNTH);
    let mut h = rig.harness();
    h.push_note(
        "notes_in",
        ev(
            0,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        ),
    );
    h.process();
    h.push_note("notes_in", ev(0, NoteKind::Off { key: 60 }));
    for _ in 0..40 {
        h.process();
    }
    let calls = lib.probe(&*rig.instance, probe::PROCESS_CALLS);
    assert!(calls < 41, "the plugin slept: {calls} calls in 41 batches");
    assert_eq!(meters(&h)[2], 1.0, "state sleeping");
    for _ in 0..10 {
        h.process();
    }
    assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), calls);
    // `request_process` from another thread wakes it for one batch.
    let handle = rig.instance.native_handle();
    let path = lib.path.clone();
    std::thread::spawn(move || {
        // SAFETY: the library is loaded by the test and the handle is the live instance.
        let l = unsafe { libloading::Library::new(&path) }.unwrap();
        // SAFETY: the symbol is exported by the test library with this signature.
        let f: libloading::Symbol<'_, unsafe extern "C" fn(usize, u32, u64, u64) -> u64> =
            unsafe { l.get(b"atomik_test_command\0") }.unwrap();
        // SAFETY: the handle is a live instance of this library.
        unsafe { f(handle, command::REQUEST_PROCESS, 0, 0) };
    })
    .join()
    .unwrap();
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), calls + 1);
    // A note wakes it for good.
    h.push_note(
        "notes_in",
        ev(
            0,
            NoteKind::On {
                key: 64,
                velocity: 1.0,
            },
        ),
    );
    h.process();
    assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), calls + 2);
}

// ----- GUI edits ----------------------------------------------------------------------------------------

#[test]
fn a_gui_edit_in_the_plugin_reaches_the_app_as_a_gesture() {
    let (backend, _) = backend_for("atomik-test-clap");
    let lib = TestLib::open("atomik-test-clap");
    let mut rig = rig(&backend, ID_GAIN);
    let mut h = rig.harness();
    h.process();
    let _ = messages(&rig);
    lib.command(&*rig.instance, command::GUI_EDIT, 1, (-12.0f64).to_bits());
    h.process();
    let msgs = messages(&rig);
    let edits: Vec<_> = msgs
        .iter()
        .filter_map(|m| match m {
            PluginMessage::ParamEdit { id, value, phase } => Some((*id, *value, *phase)),
            _ => None,
        })
        .collect();
    use atomik_plugin_host::ParamPhase::{Begin, Change, End};
    assert_eq!(edits.len(), 3, "{msgs:?}");
    assert!(matches!(edits[0], (1, v, Begin) if v.is_nan()));
    assert_eq!(edits[1], (1, -12.0, Change));
    assert!(matches!(edits[2], (1, v, End) if v.is_nan()));
    // The plugin applied the edit: its output is 12 dB down.
    *h.input_mut("in_l") = [1.0; BATCH];
    *h.input_mut("in_r") = [1.0; BATCH];
    h.process();
    let want = 10f32.powf(-12.0 / 20.0);
    assert!((h.output("out_l")[10] - want).abs() < 1e-6);
}

// ----- plugins that misbehave ----------------------------------------------------------------------------

fn failed_state(id: &str) -> (Vec<PluginMessage>, [f32; BATCH], f32) {
    let (backend, _) = backend_for("atomik-test-clap-variants");
    let mut rig = rig(&backend, id);
    let mut h = rig.harness();
    *h.input_mut("in_l") = [0.25; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    for _ in 0..4 {
        h.process();
    }
    let out = *h.output("out_l");
    let state = meters(&h)[2];
    (messages(&rig), out, state)
}

#[test]
fn a_plugin_that_returns_an_error_status_becomes_failed_and_the_effect_goes_dry() {
    let (msgs, out, state) = failed_state("atomik.test.error");
    assert_eq!(state, 3.0, "failed");
    assert!(out.iter().all(|s| *s == 0.25), "dry");
    let failures = msgs
        .iter()
        .filter(|m| matches!(m, PluginMessage::Failed { .. }))
        .count();
    assert_eq!(failures, 1, "one message: {msgs:?}");
}

#[test]
fn a_plugin_that_produces_non_finite_output_becomes_failed() {
    let (msgs, out, state) = failed_state("atomik.test.nan");
    assert_eq!(state, 3.0);
    assert!(out.iter().all(|s| *s == 0.25));
    assert!(
        msgs.iter()
            .any(|m| matches!(m, PluginMessage::Failed { .. }))
    );
}

#[test]
fn a_plugin_that_cannot_start_processing_becomes_failed() {
    let (_, out, state) = failed_state("atomik.test.start-fails");
    assert_eq!(state, 3.0);
    assert!(out.iter().all(|s| *s == 0.25));
}

#[test]
fn a_plugin_that_cannot_activate_is_an_error_not_a_crash() {
    use atomik_plugin_host::{HostHandle, InstanceId, PluginBackend, plugin_channel};
    let (backend, _) = backend_for("atomik-test-clap-variants");
    let (_d, manager) = plugin_channel(InstanceId(900));
    let mut instance = backend
        .instantiate(
            &clap_ref("atomik.test.activate-fails"),
            HostHandle::new(manager.messenger()),
        )
        .expect("it creates");
    let err = instance
        .activate(&activate_config(RunMode::Realtime))
        .map(|_| ())
        .unwrap_err();
    assert!(
        matches!(err, atomik_plugin_host::BackendError::Failed(_)),
        "{err:?}"
    );
}

// ----- allocation ----------------------------------------------------------------------------------------

#[test]
fn processing_real_clap_plugins_never_allocates() {
    let (backend, _) = backend_for("atomik-test-clap");
    // `DeviceHarness::process` runs under the allocation guard; this drives every plugin with
    // notes, param bursts and a sidechain.
    let mut synth = rig(&backend, ID_SYNTH);
    let mut hs = synth.harness();
    let mut fx = rig(&backend, ID_GAIN);
    let mut hf = fx.harness();
    let mut shift = rig(&backend, ID_SHIFT);
    let mut hn = shift.harness();
    for n in 0..200u32 {
        hs.push_note(
            "notes_in",
            ev(
                0,
                NoteKind::On {
                    key: 40 + (n % 30) as u8,
                    velocity: 0.8,
                },
            ),
        );
        hs.push_note("notes_in", ev(20, NoteKind::PitchBend { value: 0.25 }));
        hs.push_note(
            "notes_in",
            ev(
                30,
                NoteKind::NoteExpr {
                    key: 40 + (n % 30) as u8,
                    kind: ExprKind::Tuning,
                    value: 1.0,
                },
            ),
        );
        hs.set_param("p2", (n % 10) as f32 / 10.0);
        hs.process();
        *hf.input_mut("in_l") = ramp(1.0);
        *hf.input_mut("sc_in_l") = ramp(0.5);
        hf.set_param("p1", -((n % 12) as f32));
        hf.process();
        hn.push_note(
            "notes_in",
            ev(
                1,
                NoteKind::On {
                    key: 50,
                    velocity: 0.5,
                },
            ),
        );
        hn.push_note("notes_in", ev(9, NoteKind::Off { key: 50 }));
        hn.process();
    }
}
