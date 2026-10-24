//! The analyser feed: the spectrum and the phase meter panels get their data from the scope tap
//! of the device each frame, with the settings of the device, skipping a paused engine and a
//! bypassed device, and with a real offline engine the tone that plays is the tone that is
//! read; the headless probe returns the same numbers.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
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
        controllers: Vec::new(),
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
            ClipContent::Notes(vec![Note::new(0, beats * PPQ, 69, 0.9)]),
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
            ClipContent::Notes(vec![Note::new(0, 8 * PPQ, 69, 0.9)]),
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
