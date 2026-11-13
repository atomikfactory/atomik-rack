//! The analyser feed: the spectrum and the phase meter panels get their data from the scope tap
//! of the device each frame, with the settings of the device, skipping a paused engine and a
//! bypassed device, and with a real offline engine the tone that plays is the tone that is
//! read; the headless probe returns the same numbers.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_dsp::spectrum::{Smoothing, Weighting};
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Note, SocketRef, SongCommand,
};

const SYNTH: &str = "atomik.poly-synth";
const SPECTRUM: &str = "atomik.spectrum";
const PHASE: &str = "atomik.phase-meter";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

fn set<S: EngineSink>(c: &mut AppCore<S>, k: &str, p: &str, v: f32) {
    c.apply_command(Command::SetParam {
        key: key(k),
        param: p.into(),
        value: v,
        merge: None,
    })
    .unwrap_or_else(|e| panic!("{k} {p}: {e}"));
}

fn connect<S: EngineSink>(c: &mut AppCore<S>, from: (&str, &str), to: (&str, &str)) {
    let r = |(d, s): (&str, &str)| SocketRef {
        device: key(d),
        socket: s.to_string(),
    };
    c.apply_command(Command::Connect(Cable {
        from: r(from),
        to: r(to),
    }))
    .unwrap_or_else(|e| panic!("{from:?} -> {to:?}: {e}"));
}

/// Scope frames for the fake sink: `frames` of a sine on both channels (right `gain` times the
/// left, inverted when negative), ending at `head`.
fn tone(hz: f64, frames: usize, head: u64, gain: f32) -> (Vec<f32>, Vec<f32>, u64) {
    let l: Vec<f32> = (0..frames)
        .map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin()) as f32)
        .collect();
    let r = l.iter().map(|v| v * gain).collect();
    (l, r, head)
}

fn data<S: EngineSink>(c: &AppCore<S>, k: &str) -> atomik_ui::analyser::AnalyserData {
    c.view()
        .analyser_data(&key(k))
        .expect("an analyser panel")
        .clone()
}

// ------------------------------------------------------------------ the feed from a fake tap

#[test]
fn the_spectrum_panel_gets_the_analysis_of_the_tap_each_frame() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    let id = c.device_id(&key("spec")).unwrap();
    c.frame();
    assert!(data(&c, "spec").trace.is_empty(), "nothing before a tap");
    c.sink_mut()
        .device_scopes
        .insert(id, tone(1000.0, 8192, 8192, 1.0));
    c.frame();
    let d = data(&c, "spec");
    assert_eq!(d.trace.len(), atomik_ui::analyser::TRACE_COLUMNS);
    assert!(!d.bands);
    assert!((d.peak_hz - 1000.0).abs() < 12.0, "{}", d.peak_hz);
    assert!((d.peak_db + 6.0).abs() < 1.5, "{}", d.peak_db);
    assert_eq!(d.peaks.len(), d.trace.len(), "peak hold is on by default");
    // The same write position: no new analysis (a paused engine costs nothing), even though the
    // frames behind it changed.
    c.sink_mut()
        .device_scopes
        .insert(id, tone(3000.0, 8192, 8192, 1.0));
    c.frame();
    assert!((data(&c, "spec").peak_hz - 1000.0).abs() < 12.0, "skipped");
    // A new write position is analysed (a second later, so the 150 ms average has moved on).
    c.sink_mut()
        .device_scopes
        .insert(id, tone(3000.0, 8192, 8192 + 48_000, 1.0));
    c.frame();
    assert!(
        (data(&c, "spec").peak_hz - 3000.0).abs() < 12.0,
        "{}",
        data(&c, "spec").peak_hz
    );
}

#[test]
fn the_settings_of_the_device_shape_the_analysis() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 8192u64;
    let mut feed = |c: &mut AppCore<RecordingSink>, frames: (Vec<f32>, Vec<f32>, u64)| {
        head += 4096;
        c.sink_mut()
            .device_scopes
            .insert(id, (frames.0, frames.1, head));
        c.frame();
    };
    // 1/3 octave bands: 31 of them, the 1 kHz band is the loudest.
    set(&mut c, "spec", "mode", 1.0);
    set(&mut c, "spec", "averaging", 0.0);
    feed(&mut c, {
        let t = tone(1000.0, 8192, 0, 1.0);
        (t.0, t.1, 0)
    });
    let d = data(&c, "spec");
    assert!(d.bands && d.trace.len() == 31, "{}", d.trace.len());
    let loudest = d
        .trace
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert_eq!(loudest, 17, "the band centred on 1 kHz");
    // Side of identical channels is silence; the sum is not.
    set(&mut c, "spec", "mode", 0.0);
    set(&mut c, "spec", "channel", 3.0);
    feed(&mut c, {
        let t = tone(1000.0, 8192, 0, 1.0);
        (t.0, t.1, 0)
    });
    assert!(
        data(&c, "spec").peak_db < -150.0,
        "{}",
        data(&c, "spec").peak_db
    );
    set(&mut c, "spec", "channel", 4.0);
    feed(&mut c, {
        let t = tone(1000.0, 8192, 0, 1.0);
        (t.0, t.1, 0)
    });
    assert!(data(&c, "spec").peak_db > -10.0);
    // No peak hold: no peaks handed to the view.
    set(&mut c, "spec", "hold", 0.0);
    feed(&mut c, {
        let t = tone(1000.0, 8192, 0, 1.0);
        (t.0, t.1, 0)
    });
    assert!(data(&c, "spec").peaks.is_empty());
}

#[test]
fn a_bypassed_analyser_keeps_its_last_picture() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    let id = c.device_id(&key("spec")).unwrap();
    c.sink_mut()
        .device_scopes
        .insert(id, tone(1000.0, 8192, 8192, 1.0));
    c.frame();
    let live = data(&c, "spec");
    set(&mut c, "spec", "bypass", 1.0);
    c.sink_mut()
        .device_scopes
        .insert(id, tone(5000.0, 8192, 16_384, 1.0));
    c.frame();
    assert_eq!(data(&c, "spec"), live, "frozen while bypassed");
    set(&mut c, "spec", "bypass", 0.0);
    c.frame();
    assert!(
        (data(&c, "spec").peak_hz - 5000.0).abs() < 12.0,
        "and live again"
    );
}

#[test]
fn the_phase_meter_gets_decimated_scope_frames_and_its_meter_values() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, PHASE, "ph");
    let id = c.device_id(&key("ph")).unwrap();
    let mut values = vec![0.0; 16];
    values[atomik_ui::fx::phase::CORRELATION] = 0.8;
    c.sink_mut().device_values.insert(id, (values, 1));
    c.sink_mut()
        .device_scopes
        .insert(id, tone(440.0, 2048, 2048, -1.0));
    c.frame();
    let d = data(&c, "ph");
    assert_eq!(d.scope_l.len(), 1024, "every second frame of 2048");
    assert_eq!(d.scope_r.len(), 1024);
    assert!(
        d.scope_l
            .iter()
            .zip(&d.scope_r)
            .all(|(l, r)| l.to_bits() == (-r).to_bits())
    );
    assert!(d.trace.is_empty(), "no spectrum for a phase meter");
    let shown = c.view().device_meters(&key("ph")).to_vec();
    assert_eq!(shown.len(), 16);
    assert!((shown[atomik_ui::fx::phase::CORRELATION] - 0.8).abs() < 0.011);
}

#[test]
fn removing_and_adding_analysers_keeps_the_feed_consistent() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "a");
    add(&mut c, PHASE, "b");
    assert_eq!(c.analyser_devices().len(), 2);
    c.apply_command(Command::RemoveDevice { key: key("a") })
        .unwrap();
    assert_eq!(c.analyser_devices().len(), 1);
    c.frame();
    add(&mut c, SPECTRUM, "a");
    let id = c.device_id(&key("a")).unwrap();
    c.sink_mut()
        .device_scopes
        .insert(id, tone(1000.0, 8192, 8192, 1.0));
    c.frame();
    assert!((data(&c, "a").peak_hz - 1000.0).abs() < 12.0);
    c.undo();
    c.frame();
}

// ------------------------------------------------------------------ a real engine

fn clip(length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content,
    }
}

/// A sine synth playing A4 (440 Hz) for `beats` beats straight into a spectrum and a phase
/// meter (cabled after it, so both see the same audio).
fn tone_rack(c: &mut AppCore<OfflineSink>, beats: i64) {
    add(c, SYNTH, "synth");
    add(c, SPECTRUM, "spec");
    add(c, PHASE, "ph");
    set(c, "synth", "osc1.wave", 0.0);
    for side in ["l", "r"] {
        connect(
            c,
            ("synth", &format!("out_{side}")),
            ("spec", &format!("in_{side}")),
        );
        connect(
            c,
            ("synth", &format!("out_{side}")),
            ("ph", &format!("in_{side}")),
        );
    }
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: clip(
            beats * PPQ,
            ClipContent::Notes(vec![Note::new(0, beats * PPQ, 69, 0.9)].into()),
        ),
    }))
    .unwrap();
}

#[test]
fn a_real_engine_feeds_the_tone_it_plays_to_both_panels() {
    let mut c = AppCore::new(OfflineSink::new());
    tone_rack(&mut c, 8);
    c.transport_action(atomik_ui::TransportAction::Play);
    for _ in 0..12 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    let d = data(&c, "spec");
    assert!(
        (d.peak_hz - 440.0).abs() < 15.0,
        "A4 reads {} Hz",
        d.peak_hz
    );
    assert!(d.peak_db > -30.0, "{}", d.peak_db);
    let ph = data(&c, "ph");
    assert_eq!(ph.scope_l.len(), 1024);
    assert!(
        ph.scope_l.iter().any(|v| v.abs() > 0.01),
        "the scope sees the tone"
    );
    let m = c.view().device_meters(&key("ph")).to_vec();
    assert!(
        m[atomik_ui::fx::phase::CORRELATION] > 0.9,
        "a centred voice is mono: {m:?}"
    );
    assert!(m[atomik_ui::fx::phase::PEAK_L] > -30.0, "{m:?}");
    // The panels were repainted: texts show the readout.
    let _ = c.handle_event(&atomik_ui::UiEvent::Resize(atomik_ui::kurbo::Size::new(
        820.0, 3_000.0,
    )));
    let mut dl = atomik_ui::DisplayList::new();
    c.paint(&mut dl);
    let texts: Vec<&str> = dl.texts();
    assert!(texts.iter().any(|t| t.starts_with("PEAK 4")), "{texts:?}");
    assert!(texts.contains(&"CORRELATION"), "{texts:?}");
}

#[test]
fn the_probe_reads_the_same_tone_without_an_audio_stream() {
    let mut c = AppCore::new(RecordingSink::new());
    tone_rack_for_probe(&mut c);
    let readouts = c.probe_meters(48_000, None).expect("a probe");
    assert_eq!(readouts.len(), 2);
    let spec = readouts.iter().find(|r| r.key == key("spec")).unwrap();
    let value = |r: &MeterReadout, n: &str| r.values.iter().find(|(k, _)| *k == n).unwrap().1;
    assert!((value(spec, "peak_hz") - 440.0).abs() < 15.0, "{spec:?}");
    assert_eq!(spec.bands.len(), 31);
    let loudest = spec
        .bands
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(
        (loudest.0 - 398.1).abs() < 0.5,
        "440 Hz is in the 400 Hz band: {loudest:?}"
    );
    let ph = readouts.iter().find(|r| r.key == key("ph")).unwrap();
    assert!(value(ph, "correlation") > 0.9, "{ph:?}");
    assert_eq!(ph.values.len(), PHASE_VALUE_NAMES.len());
    // The probe fed the view too: the panels show what it read.
    assert!((data(&c, "spec").peak_hz - 440.0).abs() < 15.0);
    assert!(!data(&c, "ph").scope_l.is_empty());
    // One device only, and errors that say what to do.
    let one = c.probe_meters(48_000, Some(&key("ph"))).unwrap();
    assert_eq!(one.len(), 1);
    let err = c.probe_meters(48_000, Some(&key("synth"))).unwrap_err();
    assert!(err.contains("not a spectrum or phase meter"), "{err}");
}

/// The tone rack on a core without an engine (the probe renders offline by itself).
fn tone_rack_for_probe(c: &mut AppCore<RecordingSink>) {
    add(c, SYNTH, "synth");
    add(c, SPECTRUM, "spec");
    add(c, PHASE, "ph");
    set(c, "synth", "osc1.wave", 0.0);
    for side in ["l", "r"] {
        connect(
            c,
            ("synth", &format!("out_{side}")),
            ("spec", &format!("in_{side}")),
        );
        connect(
            c,
            ("synth", &format!("out_{side}")),
            ("ph", &format!("in_{side}")),
        );
    }
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: clip(
            8 * PPQ,
            ClipContent::Notes(vec![Note::new(0, 8 * PPQ, 69, 0.9)].into()),
        ),
    }))
    .unwrap();
}

#[test]
fn the_probe_refuses_a_project_without_analysers() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "synth");
    let err = c.probe_meters(4_800, None).unwrap_err();
    assert!(err.contains("no spectrum or phase meter"), "{err}");
}

// ------------------------------------------------------------------ the tuner device

const TUNER: &str = "atomik.tuner";

#[test]
fn the_tuner_panel_gets_the_pitch_of_the_tap_against_its_reference() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, TUNER, "tun");
    let id = c.device_id(&key("tun")).unwrap();
    // 110 Hz is A2 exactly against the default 440 Hz.
    c.sink_mut()
        .device_scopes
        .insert(id, tone(110.0, 8192, 8192, 1.0));
    c.frame();
    let d = data(&c, "tun");
    assert!(d.tuner_on);
    let r = d.tuner.expect("a note");
    assert_eq!(r.note.key, 45);
    assert!(r.note.cents.abs() < 1.0, "{}", r.note.cents);
    assert_eq!(d.tuner_ref_hz, 0.0, "in tune: the panel shows its own ref");
    // The same write position is not analysed again; a new one is, against the new reference.
    set(&mut c, "tun", "ref", 446.0);
    c.sink_mut()
        .device_scopes
        .insert(id, tone(110.0, 8192, 8192 + 4096, 1.0));
    c.frame();
    let r = data(&c, "tun").tuner.expect("a note");
    let want = -1200.0 * (446.0f32 / 440.0).log2();
    assert!(
        (r.note.cents - want).abs() < 1.5,
        "{} vs {want}",
        r.note.cents
    );
    // Silence reads nothing (after the hold).
    c.sink_mut()
        .device_scopes
        .insert(id, (vec![0.0; 8192], vec![0.0; 8192], 8192 + 4096 + 96_000));
    c.frame();
    assert!(data(&c, "tun").tuner.is_none());
}

#[test]
fn the_master_tune_moves_the_tuner_reference() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, TUNER, "tun");
    c.apply_command(Command::SetMasterTune(20.0)).unwrap();
    let id = c.device_id(&key("tun")).unwrap();
    // A2 tuned 20 cents up is in tune for a project tuned 20 cents up.
    let hz = 110.0 * 2f64.powf(20.0 / 1200.0);
    c.sink_mut()
        .device_scopes
        .insert(id, tone(hz, 8192, 8192, 1.0));
    c.frame();
    let d = data(&c, "tun");
    let r = d.tuner.expect("a note");
    assert!(r.note.cents.abs() < 1.5, "{}", r.note.cents);
    assert!((d.tuner_ref_hz - 445.1).abs() < 0.2, "{}", d.tuner_ref_hz);
}

#[test]
fn a_real_engine_feeds_the_tuner_the_note_it_plays_and_the_probe_reports_it() {
    let mut c = AppCore::new(OfflineSink::new());
    add(&mut c, SYNTH, "synth");
    add(&mut c, TUNER, "tun");
    set(&mut c, "synth", "osc1.wave", 0.0);
    for side in ["l", "r"] {
        connect(
            &mut c,
            ("synth", &format!("out_{side}")),
            ("tun", &format!("in_{side}")),
        );
    }
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: clip(
            8 * PPQ,
            ClipContent::Notes(vec![Note::new(0, 8 * PPQ, 57, 0.9)].into()),
        ),
    }))
    .unwrap();
    c.transport_action(atomik_ui::TransportAction::Play);
    for _ in 0..12 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    let r = data(&c, "tun").tuner.expect("A3 is heard");
    assert_eq!(r.note.key, 57);
    assert!(r.note.cents.abs() < 5.0, "{}", r.note.cents);
}

#[test]
fn the_probe_reads_the_pitch_of_a_tuner_device() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "synth");
    add(&mut c, TUNER, "tun");
    set(&mut c, "synth", "osc1.wave", 0.0);
    for side in ["l", "r"] {
        connect(
            &mut c,
            ("synth", &format!("out_{side}")),
            ("tun", &format!("in_{side}")),
        );
    }
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("synth"),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("synth"),
        lane: 0,
        clip: clip(
            8 * PPQ,
            ClipContent::Notes(vec![Note::new(0, 8 * PPQ, 69, 0.9)].into()),
        ),
    }))
    .unwrap();
    let readouts = c.probe_meters(48_000, None).expect("a probe");
    assert_eq!(readouts.len(), 1);
    let value = |n: &str| readouts[0].values.iter().find(|(k, _)| *k == n).unwrap().1;
    assert_eq!(value("key"), 69.0);
    assert!(value("cents").abs() < 5.0 && (value("hz") - 440.0).abs() < 3.0);
    assert!(data(&c, "tun").tuner.is_some(), "the probe fed the panel");
}

// ------------------------------------------------------------------ meter follow-ups

/// Feeds `frames` of a tone to a spectrum device, the tap head moving on by `advance`.
fn feed_tone(
    c: &mut AppCore<RecordingSink>,
    id: DeviceInstanceId,
    hz: f64,
    head: &mut u64,
    advance: u64,
) {
    *head += advance;
    let t = tone(hz, 16_384, *head, 1.0);
    c.sink_mut().device_scopes.insert(id, (t.0, t.1, *head));
    c.frame();
}

fn unique_file(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-meter-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("spectrum-reference.json")
}

#[test]
fn the_largest_transform_smoothing_and_weighting_reach_the_analysis_settings() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    let desc = c.registry().descriptor(SPECTRUM).unwrap();
    let settings = |c: &AppCore<RecordingSink>| {
        analysers::spectrum_settings(desc, c.doc().resolve(&key("spec")).unwrap())
    };
    let s = settings(&c);
    assert_eq!(s.size, 4096);
    assert_eq!(s.smoothing, Smoothing::Off);
    assert_eq!(s.weighting, Weighting::Flat);
    set(&mut c, "spec", "size", 4.0);
    set(&mut c, "spec", "smoothing", 3.0);
    set(&mut c, "spec", "weighting", 1.0);
    let s = settings(&c);
    assert_eq!(s.size, 16_384);
    assert_eq!(s.smoothing, Smoothing::Third);
    assert_eq!(s.weighting, Weighting::K);
    // The spectrogram mode shows the fine trace, never the bands.
    set(&mut c, "spec", "mode", 2.0);
    assert!(!settings(&c).bands);
    set(&mut c, "spec", "mode", 1.0);
    assert!(settings(&c).bands);
    // A 16384-point analysis reads the whole tap and finds the tone to a fraction of a bin.
    set(&mut c, "spec", "mode", 0.0);
    set(&mut c, "spec", "weighting", 0.0);
    set(&mut c, "spec", "smoothing", 0.0);
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 0;
    feed_tone(&mut c, id, 1000.0, &mut head, 16_384);
    let d = data(&c, "spec");
    assert!((d.peak_hz - 1000.0).abs() < 3.0, "{}", d.peak_hz);
}

#[test]
fn the_k_weighting_changes_what_the_spectrum_reads() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    set(&mut c, "spec", "averaging", 0.0);
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 0;
    feed_tone(&mut c, id, 4000.0, &mut head, 8192);
    let flat = data(&c, "spec").peak_db;
    set(&mut c, "spec", "weighting", 1.0);
    feed_tone(&mut c, id, 4000.0, &mut head, 8192);
    let weighted = data(&c, "spec").peak_db;
    // The shelf of the K-weighting: about +4 dB at 4 kHz.
    let expected = atomik_dsp::k_weighting::k_weighting_db(4000.0, 48_000.0) as f32;
    assert!(expected > 3.5);
    assert!(
        (weighted - flat - expected).abs() < 0.3,
        "{flat} -> {weighted}"
    );
}

#[test]
fn the_spectrogram_scrolls_with_the_audio_and_not_with_the_frames() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    set(&mut c, "spec", "averaging", 0.0);
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 0;
    feed_tone(&mut c, id, 1000.0, &mut head, 8192);
    assert!(
        data(&c, "spec").spectrogram.is_empty(),
        "not in the trace mode"
    );
    set(&mut c, "spec", "mode", 2.0);
    // The first feed in the mode has no time behind it: an empty image of the right size.
    feed_tone(&mut c, id, 1000.0, &mut head, 3600);
    let d = data(&c, "spec");
    assert_eq!((d.spectrogram_cols, d.spectrogram_rows), (120, 80));
    assert_eq!(d.spectrogram.len(), 120 * 80);
    assert!((d.spectrogram_span_s - 6.0).abs() < 1e-3);
    // 75 ms is 3600 frames: each feed of that length adds one row, the newest first.
    feed_tone(&mut c, id, 1000.0, &mut head, 3600);
    feed_tone(&mut c, id, 1000.0, &mut head, 3599);
    let d = data(&c, "spec");
    let rows_written = (0..80)
        .filter(|r| d.spectrogram[r * 120..(r + 1) * 120].iter().any(|v| *v > 0))
        .count();
    assert_eq!(rows_written, 2, "one row per 75 ms of audio");
    let newest = &d.spectrogram[..120];
    let loudest = newest
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .unwrap()
        .0;
    // 1 kHz is 55 % of the way along the log axis of 20 Hz to 20 kHz: column 66 of 120.
    assert!((64..=68).contains(&loudest), "{loudest}");
    // A tone that moves shows as a different newest row, the old one one row down.
    feed_tone(&mut c, id, 4000.0, &mut head, 3600);
    let d = data(&c, "spec");
    let moved = d.spectrogram[..120]
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .unwrap()
        .0;
    assert!(moved > loudest + 10, "{moved}");
    let before = d.spectrogram[120..240]
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .unwrap()
        .0;
    assert!((64..=68).contains(&before), "{before}");
    // Leaving the mode empties the image.
    set(&mut c, "spec", "mode", 0.0);
    feed_tone(&mut c, id, 1000.0, &mut head, 3600);
    let d = data(&c, "spec");
    assert!(d.spectrogram.is_empty() && d.spectrogram_rows == 0);
}

#[test]
fn a_rising_edge_of_capture_stores_the_bands_and_the_reference_choice_draws_them() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SPECTRUM, "spec");
    set(&mut c, "spec", "averaging", 0.0);
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 0;
    assert!(c.spectrum_reference().is_none());
    // Nothing to capture before there is a picture, or from silence.
    set(&mut c, "spec", "reference.capture", 1.0);
    c.frame();
    assert!(c.spectrum_reference().is_none());
    set(&mut c, "spec", "reference.capture", 0.0);
    c.frame();
    let silence = (vec![0.0f32; 8192], vec![0.0f32; 8192]);
    head += 8192;
    c.sink_mut()
        .device_scopes
        .insert(id, (silence.0, silence.1, head));
    c.frame();
    set(&mut c, "spec", "reference.capture", 1.0);
    c.frame();
    assert!(c.spectrum_reference().is_none());
    assert!(c.last_rejection().unwrap().contains("no signal"));
    set(&mut c, "spec", "reference.capture", 0.0);
    c.frame();
    // A tone: the capture holds its bands, with the 1 kHz band the loudest.
    feed_tone(&mut c, id, 1000.0, &mut head, 8192);
    assert!(data(&c, "spec").reference.is_empty(), "the choice is off");
    set(&mut c, "spec", "reference.capture", 1.0);
    c.frame();
    let stored = *c.spectrum_reference().expect("captured");
    let loudest = stored
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap()
        .0;
    assert_eq!(loudest, 17);
    // Staying on is not another capture: a new tone does not replace it.
    feed_tone(&mut c, id, 4000.0, &mut head, 8192);
    assert_eq!(c.spectrum_reference(), Some(&stored));
    set(&mut c, "spec", "reference.capture", 0.0);
    c.frame();
    // Showing it, as stored and level matched (while the engine stands still).
    set(&mut c, "spec", "reference", 1.0);
    c.frame();
    let shown = data(&c, "spec").reference;
    assert_eq!(shown.len(), 31);
    assert!((shown[17] - stored[17]).abs() < 1e-3, "as captured");
    set(&mut c, "spec", "reference", 2.0);
    c.frame();
    let matched = data(&c, "spec").reference;
    assert_eq!(matched.len(), 31);
    // The live tone is at 4 kHz now: the matched curve is shifted to the level of the signal.
    assert!(
        matched
            .iter()
            .zip(&shown)
            .any(|(m, s)| (m - s).abs() > 0.5 && *s > -150.0),
        "matched differs from stored"
    );
    // The spectrogram has no level axis to draw it on.
    set(&mut c, "spec", "mode", 2.0);
    c.frame();
    assert!(data(&c, "spec").reference.is_empty());
    set(&mut c, "spec", "mode", 0.0);
    set(&mut c, "spec", "reference", 0.0);
    c.frame();
    assert!(data(&c, "spec").reference.is_empty());
}

#[test]
fn the_stored_curve_is_kept_in_a_file_and_read_again() {
    let file = unique_file("keep");
    let mut c = AppCore::new(RecordingSink::new());
    c.set_reference_path(Some(file.clone()));
    assert!(c.spectrum_reference().is_none());
    add(&mut c, SPECTRUM, "spec");
    let id = c.device_id(&key("spec")).unwrap();
    let mut head = 0;
    feed_tone(&mut c, id, 1000.0, &mut head, 8192);
    set(&mut c, "spec", "reference.capture", 1.0);
    c.frame();
    let stored = *c.spectrum_reference().unwrap();
    assert!(file.exists());
    assert!(!file.with_extension("json.tmp").exists());
    // Another session reads it.
    let mut other = AppCore::new(RecordingSink::new());
    other.set_reference_path(Some(file.clone()));
    let back = other.spectrum_reference().unwrap();
    for (a, b) in stored.iter().zip(back) {
        assert!((a - b).abs() < 0.006);
    }
    // A damaged file is ignored and the next capture replaces it.
    std::fs::write(&file, "garbage").unwrap();
    other.set_reference_path(Some(file.clone()));
    assert!(other.spectrum_reference().is_none());
    // A newer file is left alone, even by a capture.
    std::fs::write(&file, "{\"version\":9,\"bands\":[]}").unwrap();
    c.set_reference_path(Some(file.clone()));
    set(&mut c, "spec", "reference.capture", 0.0);
    c.frame();
    set(&mut c, "spec", "reference.capture", 1.0);
    c.frame();
    assert!(c.spectrum_reference().is_some(), "kept for this session");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\"version\":9,\"bands\":[]}"
    );
    assert!(c.last_rejection().unwrap().contains("newer version"));
    std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
}

fn phase_values(short: f32, integrated: f32) -> Vec<f32> {
    let mut v = vec![0.0; 16];
    v[atomik_ui::fx::phase::MOMENTARY] = short;
    v[atomik_ui::fx::phase::SHORT_TERM] = short;
    v[atomik_ui::fx::phase::INTEGRATED] = integrated;
    v
}

#[test]
fn the_phase_meter_keeps_a_loudness_history_and_the_range_after_the_windows_fill() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, PHASE, "ph");
    let id = c.device_id(&key("ph")).unwrap();
    let mut head = 0u64;
    let mut second = |c: &mut AppCore<RecordingSink>, short: f32, integrated: f32| {
        head += 48_000;
        let t = tone(440.0, 2048, head, 1.0);
        c.sink_mut().device_scopes.insert(id, (t.0, t.1, head));
        c.sink_mut()
            .device_values
            .insert(id, (phase_values(short, integrated), head));
        c.frame();
    };
    // The first three seconds are the meter's windows filling: not in the history.
    for _ in 0..3 {
        second(&mut c, -45.0, -120.0);
    }
    assert!(data(&c, "ph").loudness_history.is_empty());
    assert_eq!(data(&c, "ph").lra, None);
    // Then a programme that swings between -26 and -14.
    for i in 0..60 {
        let short = if i % 2 == 0 { -26.0 } else { -14.0 };
        second(&mut c, short, -18.0);
    }
    let d = data(&c, "ph");
    assert_eq!(d.loudness_history.len(), 60);
    assert!((d.loudness_history[0] + 26.0).abs() < 1e-3);
    assert!((d.loudness_history[1] + 14.0).abs() < 1e-3);
    let lra = d.lra.expect("a range");
    assert!((lra - 12.0).abs() < 0.5, "{lra}");
    // The meter's reset (the integrated loudness falls to silence) starts the history again.
    second(&mut c, -20.0, -120.0);
    let d = data(&c, "ph");
    assert!(
        d.loudness_history.is_empty(),
        "{:?}",
        d.loudness_history.len()
    );
    assert_eq!(d.lra, None);
    // A paused engine adds nothing.
    for _ in 0..3 {
        second(&mut c, -20.0, -20.0);
    }
    let before = data(&c, "ph").loudness_history.len();
    c.frame();
    c.frame();
    assert_eq!(data(&c, "ph").loudness_history.len(), before);
}

#[test]
fn a_bypassed_phase_meter_keeps_its_history() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, PHASE, "ph");
    let id = c.device_id(&key("ph")).unwrap();
    let mut head = 0u64;
    for _ in 0..8 {
        head += 48_000;
        let t = tone(440.0, 2048, head, 1.0);
        c.sink_mut().device_scopes.insert(id, (t.0, t.1, head));
        c.sink_mut()
            .device_values
            .insert(id, (phase_values(-20.0, -20.0), head));
        c.frame();
    }
    let live = data(&c, "ph");
    assert!(!live.loudness_history.is_empty());
    set(&mut c, "ph", "bypass", 1.0);
    head += 48_000 * 10;
    let t = tone(440.0, 2048, head, 1.0);
    c.sink_mut().device_scopes.insert(id, (t.0, t.1, head));
    c.frame();
    assert_eq!(data(&c, "ph"), live);
}
