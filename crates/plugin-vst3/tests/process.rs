//! Against both real backends: the device drives the
//! CLAP and the VST3 test plugins through the same `PluginProcessor` trait. Outputs equal the
//! reference DSP of `atomik-test-plugin-core` run directly, notes and expressions arrive in the
//! plugin's format, transport and silence flags are right, and a misbehaving plugin cannot break
//! the host.
// Justification: copies of the same f32 samples and exact dyadic values are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, Batch, ExprKind, NoteEvent, NoteKind, PPQ};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::PluginMessage;
use atomik_test_plugin_core as core;
use support::*;

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

fn messages(rig: &Rig) -> Vec<PluginMessage> {
    std::iter::from_fn(|| rig.manager.try_recv()).collect()
}

fn lib_of(k: &Kit) -> TestLib {
    TestLib::open_path(&k.path)
}

// ----- activation and the fixed block ---------------------------------------------------------

#[test]
fn the_plugin_is_activated_at_the_session_rate_with_exactly_64_frames_per_block() {
    for format in FORMATS {
        let k = kit(format);
        let lib = lib_of(&k);
        let mut rig = rig(&k, Plug::Gain);
        assert_eq!(
            lib.probe(&*rig.instance, probe::MAX_FRAMES),
            64,
            "{}",
            format.name()
        );
        assert_eq!(lib.probe_f64(&*rig.instance, probe::SAMPLE_RATE), SR);
        let mut h = rig.harness();
        // VST3 skips an effect with a zero tail and silent inputs, so give it a signal.
        *h.input_mut("in_l") = [0.1; BATCH];
        h.process();
        h.process();
        assert_eq!(lib.probe(&*rig.instance, probe::FRAMES), 64);
        assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), 2);
    }
}

#[test]
fn the_process_mode_follows_the_run_mode() {
    for (format, live, offline) in [(Format::Clap, 0, 1), (Format::Vst3, 0, 2)] {
        let k = kit(format);
        let lib = lib_of(&k);
        let a = rig_with(&k, Plug::Gain, RunMode::Realtime);
        assert_eq!(
            lib.probe(&*a.instance, probe::RENDER_MODE),
            live,
            "{}",
            format.name()
        );
        let b = rig_with(&k, Plug::Gain, RunMode::Offline);
        assert_eq!(
            lib.probe(&*b.instance, probe::RENDER_MODE),
            offline,
            "{}",
            format.name()
        );
    }
}

// ----- the effect -------------------------------------------------------------------------------

#[test]
fn the_effect_output_equals_the_reference_dsp_run_directly() {
    for format in FORMATS {
        let k = kit(format);
        let mut rig = rig(&k, Plug::Gain);
        let mut h = rig.harness();
        let mut reference = core::GainFx::new(SR);
        for (id, plain) in [
            (core::GainFx::GAIN, -4.5),
            (core::GainFx::LATENCY, 37.0),
            (core::GainFx::DUCK, 0.75),
        ] {
            let (host, dsp) = k.host_value(Plug::Gain, id, plain);
            h.set_param(&format!("p{id}"), host as f32);
            reference.set_param(id, dsp);
        }
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
            assert_eq!(
                &h.output("out_l")[..],
                &out[0][..],
                "{} batch {n} left",
                format.name()
            );
            assert_eq!(
                &h.output("out_r")[..],
                &out[1][..],
                "{} batch {n} right",
                format.name()
            );
        }
        assert!(h.output("out_l").iter().any(|s| s.abs() > 0.1));
    }
}

#[test]
fn silent_inputs_carry_the_silence_flag_on_every_channel() {
    for format in FORMATS {
        let k = kit(format);
        let lib = lib_of(&k);
        // Only the main input is cabled: the sidechain is flagged silent on both channels.
        let mut a = rig(&k, Plug::Gain);
        let mut h = a.harness();
        h.input_mut("in_l")[3] = 1.0;
        h.process();
        assert_eq!(
            lib.probe(&*a.instance, probe::MASK_MAIN),
            0,
            "{}",
            format.name()
        );
        assert_eq!(lib.probe(&*a.instance, probe::MASK_SIDECHAIN), 0b11);
        // Only the sidechain is cabled: the main input is flagged silent on both channels.
        let mut b = rig(&k, Plug::Gain);
        let mut h = b.harness();
        h.input_mut("sc_in_r")[0] = 1.0;
        h.process();
        assert_eq!(
            lib.probe(&*b.instance, probe::MASK_MAIN),
            0b11,
            "{}",
            format.name()
        );
        assert_eq!(lib.probe(&*b.instance, probe::MASK_SIDECHAIN), 0);
    }
}

#[test]
fn the_sidechain_ducks_the_effect_through_the_plugin() {
    for format in FORMATS {
        let k = kit(format);
        let mut rig = rig(&k, Plug::Gain);
        let mut h = rig.harness();
        *h.input_mut("in_l") = [0.5; BATCH];
        *h.input_mut("in_r") = [0.5; BATCH];
        h.process();
        let open = h.output("out_l")[BATCH - 1];
        *h.input_mut("sc_in_l") = [1.0; BATCH];
        *h.input_mut("sc_in_r") = [1.0; BATCH];
        h.process();
        let ducked = h.output("out_l")[BATCH - 1];
        assert!(ducked < open * 0.5, "{}: {ducked} vs {open}", format.name());
    }
}

#[test]
fn a_mono_plugin_gets_the_mean_of_the_cabled_inputs_and_answers_on_both_outputs() {
    for format in FORMATS {
        let k = kit_variants(format);
        let mut rig = rig_for(&*k.backend, &k.variant(Variant::Mono), RunMode::Realtime);
        let mut h = rig.harness();
        // Level 0.5: VST3 takes it normalised over 0 to 2.
        let host = if format == Format::Vst3 { 0.25 } else { 0.5 };
        h.set_param("p1", host);
        *h.input_mut("in_l") = ramp(1.0);
        *h.input_mut("in_r") = ramp(3.0);
        h.process();
        let want: Vec<f32> = ramp(1.0)
            .iter()
            .zip(ramp(3.0).iter())
            .map(|(a, b)| (a + b) * 0.5 * 0.5)
            .collect();
        assert_eq!(&h.output("out_l")[..], &want[..], "{}", format.name());
        assert_eq!(&h.output("out_r")[..], &want[..]);
    }
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

/// The same stream as the plugin's reference events in the format's own dialect: host note ids
/// 0, 1, ... Both formats pass velocity, controllers and pressure as exact floats; VST3 delivers
/// per-note pressure as a poly pressure event and has no key on expressions, and its parameter
/// changes (controllers) sort before notes at an equal offset.
fn reference_events(format: Format) -> Vec<(usize, Vec<core::TimedEvent>)> {
    use core::Event::*;
    let vst3 = format == Format::Vst3;
    let mut v = vec![
        (
            0,
            vec![
                core_note(
                    5,
                    NoteOn {
                        key: 60,
                        velocity: 0.5,
                        id: 0,
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
                        id: if vst3 { 0 } else { -1 },
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
                    id: 1,
                },
            )],
        ),
        (3, vec![core_note(3, NoteOff { key: 60, id: 0 })]),
        (
            6,
            if vst3 {
                vec![
                    core_note(
                        0,
                        Cc {
                            controller: 123,
                            value: 0.0,
                        },
                    ),
                    core_note(0, NoteOff { key: 67, id: 1 }),
                ]
            } else {
                vec![
                    core_note(0, NoteOff { key: 67, id: 1 }),
                    core_note(
                        0,
                        Cc {
                            controller: 123,
                            value: 0.0,
                        },
                    ),
                ]
            },
        ),
    ];
    let expr = |offset, key, id, kind, value| {
        core_note(
            offset,
            Expr {
                key,
                id,
                kind,
                value,
            },
        )
    };
    v[1].1.extend(if vst3 {
        vec![
            expr(10, 0, 0, core::ExprKind::Tuning, 2.0),
            core_note(
                11,
                PolyPressure {
                    key: 67,
                    id: 1,
                    value: 0.25,
                },
            ),
            expr(12, 0, 1, core::ExprKind::Volume, 0.5),
            expr(13, 0, 0, core::ExprKind::Pan, -0.25),
        ]
    } else {
        vec![
            expr(10, 60, 0, core::ExprKind::Tuning, 2.0),
            expr(11, 67, 1, core::ExprKind::Pressure, 0.25),
            expr(12, 67, 1, core::ExprKind::Volume, 0.5),
            expr(13, 60, 0, core::ExprKind::Pan, -0.25),
        ]
    });
    v
}

#[test]
fn the_synth_output_equals_the_reference_dsp_with_notes_controllers_and_expressions() {
    for format in FORMATS {
        let k = kit(format);
        let mut r = rig(&k, Plug::Synth);
        let mut h = r.harness();
        let mut synth = core::TestSynth::new(SR);
        let script = script();
        let reference = reference_events(format);
        let mut sounded = false;
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
            assert_eq!(
                &h.output("out_l")[..],
                &out[0][..],
                "{} batch {b} left",
                format.name()
            );
            assert_eq!(
                &h.output("out_r")[..],
                &out[1][..],
                "{} batch {b} right",
                format.name()
            );
            sounded |= out[0].iter().any(|s| s.abs() > 0.01);
        }
        assert!(sounded, "it sounds");
        assert_eq!(meters(&h)[4], 0.0, "{}: nothing was dropped", format.name());
    }
}

#[test]
fn note_ons_and_offs_arrive_at_their_exact_frames() {
    for format in FORMATS {
        let k = kit(format);
        let mut rig = rig(&k, Plug::Synth);
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
        assert!(
            out[..17].iter().all(|s| *s == 0.0),
            "{}: silent before the On",
            format.name()
        );
        assert!(out[17..].iter().any(|s| s.abs() > 0.0), "sounding after it");
    }
}

// ----- the note effect --------------------------------------------------------------------------

#[test]
fn a_note_effect_sends_its_notes_to_notes_out() {
    for format in FORMATS {
        let k = kit(format);
        let mut rig = rig(&k, Plug::Shift);
        let mut h = rig.harness();
        let (host, _) = k.host_value(Plug::Shift, core::NoteShift::SEMITONES, 5.0);
        h.set_param("p1", host as f32);
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
            ],
            "{}",
            format.name()
        );
    }
}

// ----- transport ---------------------------------------------------------------------------------

#[test]
fn the_transport_reaches_the_plugin_through_a_locate_and_a_loop_wrap() {
    for format in FORMATS {
        let k = kit(format);
        let lib = lib_of(&k);
        let mut rig = rig(&k, Plug::Gain);
        let mut h = rig.harness();
        *h.input_mut("in_l") = [0.1; BATCH];
        // CLAP flags: playing bit 4, loop bit 6; VST3 `ProcessContext::state`: 1 and 2.
        let (playing, looping): (u64, u64) = match format {
            Format::Clap => (1 << 4, 1 << 6),
            Format::Vst3 => (1 << 1, 1 << 2),
        };
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
        assert!(
            flags & playing != 0 && flags & looping != 0,
            "{}: {flags:#b}",
            format.name()
        );
        assert_eq!(lib.probe_f64(&*rig.instance, probe::TEMPO), 133.0);
        assert_eq!(lib.probe_f64(&*rig.instance, probe::BEATS), 4.0);
        assert_eq!(lib.probe(&*rig.instance, probe::TSIG_NUM), 3);
        assert_eq!(lib.probe(&*rig.instance, probe::TSIG_DEN), 4);
        assert_eq!(lib.probe_f64(&*rig.instance, probe::LOOP_START), 2.0);
        assert_eq!(lib.probe_f64(&*rig.instance, probe::LOOP_END), 6.0);
        let seconds_at = 4.0 * 60.0 / 133.0;
        match format {
            Format::Clap => {
                let seconds = lib.probe_f64(&*rig.instance, probe::SECONDS);
                assert!((seconds - seconds_at).abs() < 1e-6, "{seconds}");
            }
            Format::Vst3 => {
                let samples = lib.probe(&*rig.instance, probe::SECONDS) as f64;
                assert!((samples - seconds_at * SR).abs() <= 1.0, "{samples}");
            }
        }
        // A locate: the position jumps and the plugin sees the jump.
        h.time_mut().tick = 0.5 * PPQ as f64;
        h.process();
        assert_eq!(lib.probe_f64(&*rig.instance, probe::BEATS), 0.5);
        // Stopped: the playing flag goes.
        h.time_mut().playing = false;
        h.process();
        let flags = lib.probe(&*rig.instance, probe::TRANSPORT_FLAGS);
        assert_eq!(flags & playing, 0, "{}: {flags:#b}", format.name());
        // The steady time is the engine frame counter.
        let frame = h.frame();
        h.process();
        assert_eq!(lib.probe(&*rig.instance, probe::STEADY), frame);
    }
}

// ----- sleep ---------------------------------------------------------------------------------------

#[test]
fn an_idle_synth_stops_being_processed_and_a_note_wakes_it() {
    for format in FORMATS {
        let k = kit(format);
        let lib = lib_of(&k);
        let mut rig = rig(&k, Plug::Synth);
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
        for _ in 0..830 {
            h.process();
        }
        let calls = lib.probe(&*rig.instance, probe::PROCESS_CALLS);
        assert!(
            calls < 831,
            "{}: the plugin slept: {calls} calls in 831 batches (a VST3 instrument sleeps after a second of silence)",
            format.name()
        );
        assert_eq!(meters(&h)[2], 1.0, "state sleeping");
        for _ in 0..10 {
            h.process();
        }
        assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), calls);
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
        assert_eq!(lib.probe(&*rig.instance, probe::PROCESS_CALLS), calls + 1);
        assert_eq!(meters(&h)[2], 0.0, "running again");
    }
}

// ----- plugins that misbehave ----------------------------------------------------------------------

fn failed_state(format: Format, v: Variant) -> (Vec<PluginMessage>, [f32; BATCH], f32) {
    let k = kit_variants(format);
    let mut rig = rig_for(&*k.backend, &k.variant(v), RunMode::Realtime);
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
    for format in FORMATS {
        let (msgs, out, state) = failed_state(format, Variant::Error);
        assert_eq!(state, 3.0, "{}: failed", format.name());
        assert!(out.iter().all(|s| *s == 0.25), "dry");
        let failures = msgs
            .iter()
            .filter(|m| matches!(m, PluginMessage::Failed { .. }))
            .count();
        assert_eq!(failures, 1, "one message: {msgs:?}");
    }
}

#[test]
fn a_plugin_that_produces_non_finite_output_becomes_failed() {
    for format in FORMATS {
        let (msgs, out, state) = failed_state(format, Variant::Nan);
        assert_eq!(state, 3.0, "{}", format.name());
        assert!(out.iter().all(|s| *s == 0.25));
        assert!(
            msgs.iter()
                .any(|m| matches!(m, PluginMessage::Failed { .. }))
        );
    }
}

#[test]
fn a_plugin_that_cannot_start_processing_becomes_failed() {
    for format in FORMATS {
        let (_, out, state) = failed_state(format, Variant::StartFails);
        assert_eq!(state, 3.0, "{}", format.name());
        assert!(out.iter().all(|s| *s == 0.25));
    }
}

#[test]
fn a_plugin_that_cannot_activate_is_an_error_not_a_crash() {
    for format in FORMATS {
        let k = kit_variants(format);
        let (mut instance, _m) = bare(&*k.backend, &k.variant(Variant::ActivateFails));
        let err = instance
            .activate(&activate_config(RunMode::Realtime))
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(err, atomik_plugin_host::BackendError::Failed(_)),
            "{}: {err:?}",
            format.name()
        );
    }
}

// ----- allocation ----------------------------------------------------------------------------------

#[test]
fn processing_real_plugins_never_allocates() {
    for format in FORMATS {
        let k = kit(format);
        // `DeviceHarness::process` runs under the allocation guard; this drives every plugin
        // with notes, param bursts and a sidechain.
        let mut synth = rig(&k, Plug::Synth);
        let mut hs = synth.harness();
        let mut fx = rig(&k, Plug::Gain);
        let mut hf = fx.harness();
        let mut shift = rig(&k, Plug::Shift);
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
            let (host, _) = k.host_value(Plug::Gain, core::GainFx::GAIN, -((n % 12) as f64));
            hf.set_param("p1", host as f32);
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
}
