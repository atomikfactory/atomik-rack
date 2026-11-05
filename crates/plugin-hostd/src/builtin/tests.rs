//! The `null` plugins driven directly, without a child process: their layouts, state, params
//! and what each processor does with a block.

use super::*;
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{EventList, InstanceId, PluginEvent, PluginTransport, plugin_channel};

fn plugin(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

fn instance(id: &str) -> Box<dyn PluginInstance> {
    let (_device, manager) = plugin_channel(InstanceId(1));
    BuiltinBackend
        .instantiate(&plugin(id), HostHandle::new(manager.messenger()))
        .expect("a null plugin")
}

fn config() -> ActivateConfig {
    ActivateConfig {
        sample_rate: 48_000.0,
        frames: 64,
        mode: RunMode::Realtime,
    }
}

fn ramp() -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = i as f32 / BATCH as f32 + 0.25;
    }
    b
}

fn event(offset: u16, kind: PluginEventKind) -> PluginEvent {
    PluginEvent { offset, kind }
}

/// Processes one block with `inputs` and `events`; returns the output channels.
fn run(
    processor: &mut dyn PluginProcessor,
    inputs: &[Batch],
    events: &[PluginEvent],
    outputs: usize,
) -> Vec<Batch> {
    let mut out = vec![[0.0; BATCH]; outputs];
    let mut aux: Vec<Batch> = Vec::new();
    let mut events_out = EventList::with_capacity(8);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: inputs,
        main_in_silent: inputs.is_empty(),
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut out,
        aux_out: &mut aux,
        events_in: events,
        events_out: &mut events_out,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    let status = processor.process(&mut io);
    assert_eq!(status, ProcessStatus::Continue);
    out
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

fn peak(b: &Batch) -> f32 {
    b.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn every_null_plugin_id_is_known_and_others_are_not() {
    for id in [
        "null.gain",
        "null.latency",
        "null.crash",
        "null.hang",
        "null.hang-activate",
        "null.synth",
        "null.noisy",
    ] {
        assert!(Kind::of(id).is_some(), "{id}");
    }
    assert_eq!(Kind::of("null.nothing"), None);
    assert_eq!(Kind::of(""), None);
}

#[test]
fn layouts_follow_the_kind() {
    let synth = layout_of(Kind::Synth, "null.synth");
    assert_eq!(synth.kind_hint, PluginKindHint::Instrument);
    assert!(synth.notes_in);
    assert!(synth.main_in.is_none());
    assert_eq!(synth.main_out, Some(Channels::Stereo));

    let gain = layout_of(Kind::Gain, "null.gain");
    assert_eq!(gain.kind_hint, PluginKindHint::Effect);
    assert!(!gain.notes_in);
    assert_eq!(gain.main_in, Some(Channels::Stereo));
    assert_eq!(gain.params.len(), 1);
    assert_eq!(gain.params[0].name, "Pass");
    assert!(gain.params[0].stepped);

    for kind in [Kind::Latency, Kind::Crash, Kind::Hang, Kind::Noisy] {
        assert!(layout_of(kind, "null.other").params.is_empty());
    }
}

#[test]
fn the_backend_creates_known_plugins_and_refuses_the_rest() {
    let backend = BuiltinBackend;
    assert_eq!(backend.format(), PluginFormat::Clap);
    assert!(matches!(
        backend.scan_file(Path::new("any.clap")),
        Err(BackendError::Unsupported(_))
    ));
    let (_device, manager) = plugin_channel(InstanceId(2));
    let host = HostHandle::new(manager.messenger());
    assert!(matches!(
        backend.instantiate(&plugin("null.missing"), host.clone()),
        Err(BackendError::UnknownPlugin(id)) if id == "null.missing"
    ));
    let gain = backend.instantiate(&plugin("null.gain"), host).unwrap();
    assert_eq!(gain.layout().name, "null.gain");
}

#[test]
fn only_the_latency_plugin_reports_latency() {
    assert_eq!(instance("null.latency").latency_frames(), LATENCY);
    assert_eq!(instance("null.gain").latency_frames(), 0);
    assert_eq!(instance("null.synth").latency_frames(), 0);
}

#[test]
fn state_is_the_pass_value_and_bad_state_is_refused() {
    let mut a = instance("null.gain");
    let mut b = instance("null.gain");
    let one = PluginStateBlob(1.0f64.to_bits().to_le_bytes().to_vec());
    a.restore_state(&one).unwrap();
    let captured = a.capture_state().unwrap();
    assert_eq!(captured, one);
    b.restore_state(&captured).unwrap();
    assert_eq!(b.capture_state().unwrap(), one);

    // The wrong length and a non-number leave the state alone.
    assert!(matches!(
        b.restore_state(&PluginStateBlob(vec![1, 2, 3])),
        Err(BackendError::Failed(_))
    ));
    let nan = PluginStateBlob(f64::NAN.to_bits().to_le_bytes().to_vec());
    assert!(matches!(
        b.restore_state(&nan),
        Err(BackendError::Failed(_))
    ));
    assert_eq!(b.capture_state().unwrap(), one);
}

#[test]
fn param_text_is_only_for_the_gain_pass_param() {
    let mut gain = instance("null.gain");
    assert_eq!(gain.param_text(0, 1.0).as_deref(), Some("pass"));
    assert_eq!(gain.param_text(0, 0.0).as_deref(), Some("half"));
    assert_eq!(gain.param_text(1, 1.0), None);
    assert_eq!(instance("null.latency").param_text(0, 1.0), None);
}

#[test]
fn the_null_plugins_have_no_editor() {
    let mut gain = instance("null.gain");
    assert!(matches!(
        gain.open_gui(NativeParent::Win32(0), 1.0),
        Err(BackendError::Unsupported(_))
    ));
    gain.close_gui();
    gain.poll();
}

#[test]
fn the_gain_plugin_halves_until_pass_is_set() {
    let mut gain = instance("null.gain");
    let mut processor = gain.activate(&config()).unwrap();
    assert!(processor.start_processing());
    let input = [ramp(), ramp()];

    let out = run(&mut *processor, &input, &[], 2);
    for ch in &out {
        for (o, i) in ch.iter().zip(&input[0]) {
            assert!(same(*o, i * 0.5));
        }
    }

    // A Pass event at offset 32 switches the second half of the block to unity.
    let set = event(32, PluginEventKind::Param { id: 0, value: 1.0 });
    let out = run(&mut *processor, &input, &[set], 2);
    assert!(same(out[0][31], input[0][31] * 0.5));
    assert!(same(out[0][32], input[0][32]));
    assert!(same(out[1][63], input[1][63]));

    // The value is kept for the next block and for the captured state.
    let out = run(&mut *processor, &input, &[], 2);
    assert!(same(out[0][0], input[0][0]));
    gain.deactivate(processor);
    let state = gain.capture_state().unwrap();
    assert_eq!(state.0, 1.0f64.to_bits().to_le_bytes().to_vec());
}

#[test]
fn the_gain_plugin_ignores_other_params_and_non_numbers() {
    let mut gain = instance("null.gain");
    let mut processor = gain.activate(&config()).unwrap();
    let input = [ramp(), ramp()];
    let events = [
        event(0, PluginEventKind::Param { id: 7, value: 1.0 }),
        event(
            4,
            PluginEventKind::Param {
                id: 0,
                value: f64::NAN,
            },
        ),
        event(8, PluginEventKind::PitchBend { value: 0.5 }),
    ];
    let out = run(&mut *processor, &input, &events, 2);
    assert!(same(out[0][63], input[0][63] * 0.5));
    processor.stop_processing();
    processor.reset();
}

#[test]
fn the_gain_plugin_feeds_a_mono_input_to_both_channels_and_skips_no_input() {
    let mut gain = instance("null.gain");
    let mut processor = gain.activate(&config()).unwrap();
    let mono = [ramp()];
    let out = run(&mut *processor, &mono, &[], 2);
    assert!(same(out[0][10], out[1][10]));
    assert!(same(out[0][10], mono[0][10] * 0.5));

    // With no input the output stays as it arrived (zero).
    let out = run(&mut *processor, &[], &[], 2);
    assert!(out.iter().all(|c| peak(c) < 1e-9));
}

#[test]
fn the_passthrough_plugins_copy_the_input() {
    for id in ["null.latency", "null.noisy"] {
        let mut plugin = instance(id);
        let mut processor = plugin.activate(&config()).unwrap();
        assert!(processor.start_processing());
        let stereo = [ramp(), ramp()];
        let out = run(&mut *processor, &stereo, &[], 2);
        assert_eq!(out[0], stereo[0], "{id}");
        assert_eq!(out[1], stereo[1], "{id}");
        let out = run(&mut *processor, &[ramp()], &[], 2);
        assert_eq!(out[1], ramp(), "{id} mono input");
        let out = run(&mut *processor, &[], &[], 2);
        assert!(out.iter().all(|c| peak(c) < 1e-9), "{id} no input");
        processor.stop_processing();
        processor.reset();
        plugin.poll();
        plugin.deactivate(processor);
    }
}

#[test]
fn the_crash_plugin_passes_batches_until_its_count_is_reached() {
    // The count is read from the environment at activation; the processor is built directly so
    // that no test has to change the environment or abort the test process.
    let mut crashing = PassProcessor {
        crash_after: Some(3),
        batches: 0,
    };
    let input = [ramp(), ramp()];
    for _ in 0..3 {
        let out = run(&mut crashing, &input, &[], 2);
        assert_eq!(out[0], input[0]);
    }
    assert_eq!(crashing.batches, 3);

    // Activated with the default (an unset or unusable count) it is a crash plugin at batch 0.
    let mut plugin = instance("null.crash");
    let _processor = plugin.activate(&config()).unwrap();
}

#[test]
fn the_hang_plugin_activates_and_starts_but_would_block_in_process() {
    let mut hang = instance("null.hang");
    let mut processor = hang.activate(&config()).unwrap();
    assert!(processor.start_processing());
    processor.stop_processing();
    processor.reset();
}

fn synth_block(processor: &mut dyn PluginProcessor, events: &[PluginEvent]) -> Vec<Batch> {
    run(processor, &[], events, 2)
}

#[test]
fn the_synth_sounds_while_a_note_is_held_and_stops_at_note_off() {
    let mut synth = instance("null.synth");
    let mut processor = synth.activate(&config()).unwrap();
    assert!(processor.start_processing());

    assert!(peak(&synth_block(&mut *processor, &[])[0]) < 1e-9);

    let on = event(
        10,
        PluginEventKind::NoteOn {
            key: 69,
            velocity: 1.0,
            note_id: 5,
        },
    );
    let out = synth_block(&mut *processor, &[on]);
    assert!(out[0][..10].iter().all(|s| s.abs() < 1e-9));
    assert!(peak(&out[0]) > 0.05);
    assert_eq!(out[0], out[1]);

    // Still sounding in the next block, and a note off for another id changes nothing.
    let other = event(
        0,
        PluginEventKind::NoteOff {
            key: 69,
            note_id: 9,
        },
    );
    assert!(peak(&synth_block(&mut *processor, &[other])[0]) > 0.05);

    let off = event(
        0,
        PluginEventKind::NoteOff {
            key: 69,
            note_id: 5,
        },
    );
    assert!(peak(&synth_block(&mut *processor, &[off])[0]) < 1e-9);
}

#[test]
fn the_synth_ignores_other_events_and_reset_cuts_every_voice() {
    let mut synth = instance("null.synth");
    let mut processor = synth.activate(&config()).unwrap();
    let events = [
        event(0, PluginEventKind::PitchBend { value: 0.5 }),
        event(
            1,
            PluginEventKind::Cc {
                controller: 1,
                value: 0.5,
            },
        ),
        event(
            2,
            PluginEventKind::NoteOn {
                key: 60,
                velocity: 0.5,
                note_id: 1,
            },
        ),
    ];
    assert!(peak(&synth_block(&mut *processor, &events)[0]) > 0.01);
    processor.reset();
    assert!(peak(&synth_block(&mut *processor, &[])[0]) < 1e-9);
}

#[test]
fn the_synth_steals_the_first_voice_when_all_are_busy() {
    let mut synth = SynthProcessor::new(48_000.0);
    for k in 0..VOICES {
        synth.note_on(40 + k as u8, 1.0, k as i32);
    }
    assert!(synth.voices.iter().all(|v| v.active));
    synth.note_on(100, 1.0, 99);
    assert_eq!(synth.voices[0].key, 100);
    assert_eq!(synth.voices[0].note_id, 99);
    assert!(synth.voices[1..].iter().all(|v| v.key != 100));
    // A 440 Hz key steps 440 / rate per frame.
    synth.note_on(69, 1.0, 1);
    let a = synth.voices.iter().find(|v| v.key == 69).unwrap();
    assert!((a.step - 440.0 / 48_000.0).abs() < 1e-12);
}
