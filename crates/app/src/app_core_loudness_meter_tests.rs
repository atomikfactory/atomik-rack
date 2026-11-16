//! The loudness meter in the app: its published values reach the view in the units of its panel,
//! its history comes from the newest frames of its tap and starts over when the device is reset,
//! a real engine and the headless probe read the tone that plays, and the readout names the
//! values.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_devices::loudness_meter as dev;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Note, SocketRef, SongCommand,
};
use atomik_ui::fx::{FxKind, loudness as slot};

const LOUD: &str = "atomik.loudness-meter";
const SYNTH: &str = "atomik.poly-synth";

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

fn data<S: EngineSink>(c: &AppCore<S>) -> atomik_ui::analyser::AnalyserData {
    c.view()
        .analyser_data(&key("m"))
        .expect("an analyser panel")
        .clone()
}

/// The published values of a meter that has seen `points` history points since reset `epoch`.
fn values(integrated: f32, points: usize, epoch: f32) -> Vec<f32> {
    let mut v = vec![0.0; dev::METERS];
    v[slot::MOMENTARY] = -13.0;
    v[slot::SHORT_TERM] = -14.0;
    v[slot::INTEGRATED] = integrated;
    v[slot::LRA] = 6.34;
    v[slot::TRUE_PEAK] = -0.7;
    v[slot::PLR] = 13.3;
    v[slot::TARGET] = -14.0;
    v[slot::MAX_TP] = -1.0;
    v[slot::EPOCH] = epoch;
    v[slot::HISTORY_POINTS] = points as f32;
    v[slot::TIME] = 12_345.4;
    v[slot::BINS + 3] = 0.5;
    v
}

/// Tap frames: `n` points whose short-term value is `-30 + i` and momentary `-20 - i`, ending at
/// `head`.
fn frames(n: usize, head: u64) -> (Vec<f32>, Vec<f32>, u64) {
    (
        (0..n).map(|i| -30.0 + i as f32).collect(),
        (0..n).map(|i| -20.0 - i as f32).collect(),
        head,
    )
}

#[test]
fn the_published_values_reach_the_view_in_the_units_of_its_panel() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, LOUD, "m");
    let id = c.device_id(&key("m")).unwrap();
    assert_eq!(FxKind::LoudnessMeter.meter_count(), dev::METERS);
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.23, 0, 1.0), 1));
    c.frame();
    let shown = c.view().device_meters(&key("m")).to_vec();
    assert_eq!(shown.len(), dev::METERS);
    assert_eq!(shown[slot::INTEGRATED], -14.2);
    assert_eq!(shown[slot::LRA], 6.3);
    assert_eq!(shown[slot::PLR], 13.3);
    assert_eq!(shown[slot::TARGET], -14.0);
    // A programme time of hours is a whole number of seconds, not clamped to 10000.
    assert_eq!(shown[slot::TIME], 12_345.0);
    assert_eq!(shown[slot::BINS + 3], 0.5);
    // A later frame replaces them.
    c.sink_mut()
        .device_values
        .insert(id, (values(-20.0, 0, 1.0), 2));
    c.frame();
    assert_eq!(c.view().device_meters(&key("m"))[slot::INTEGRATED], -20.0);
}

#[test]
fn the_history_is_the_newest_frames_of_the_tap_since_the_last_reset() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, LOUD, "m");
    let id = c.device_id(&key("m")).unwrap();
    c.frame();
    assert!(data(&c).loud_short.is_empty(), "nothing before a tap");
    // Ten points in a tap that also holds older ones: only the ten of this epoch are shown.
    let (l, r, head) = frames(40, 40);
    c.sink_mut()
        .device_scopes
        .insert(id, (l.clone(), r.clone(), head));
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.0, 10, 1.0), 1));
    c.frame();
    let d = data(&c);
    assert_eq!(d.loud_short, l[30..].to_vec());
    assert_eq!(d.loud_momentary, r[30..].to_vec());
    // More points arrive (the head moves).
    let (l, r, head) = frames(60, 50);
    c.sink_mut()
        .device_scopes
        .insert(id, (l.clone(), r.clone(), head));
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.0, 20, 1.0), 2));
    c.frame();
    assert_eq!(data(&c).loud_short, l[40..].to_vec());
    // The same head and the same epoch: nothing is redone.
    c.sink_mut()
        .device_scopes
        .insert(id, (vec![0.0; 60], vec![0.0; 60], 50));
    c.frame();
    assert_eq!(data(&c).loud_short, l[40..].to_vec());
    // A reset writes no frame, but the counter changes: the view starts over at once.
    c.sink_mut()
        .device_values
        .insert(id, (values(-120.0, 0, 2.0), 3));
    c.frame();
    let d = data(&c);
    assert!(d.loud_short.is_empty() && d.loud_momentary.is_empty());
    // And the points that follow are the new epoch's.
    let (l, r, head) = frames(60, 53);
    c.sink_mut().device_scopes.insert(id, (l.clone(), r, head));
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.0, 3, 2.0), 4));
    c.frame();
    assert_eq!(data(&c).loud_short, l[57..].to_vec());
}

#[test]
fn a_bypassed_loudness_meter_keeps_its_picture() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, LOUD, "m");
    let id = c.device_id(&key("m")).unwrap();
    let (l, r, head) = frames(40, 40);
    c.sink_mut().device_scopes.insert(id, (l, r, head));
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.0, 10, 1.0), 1));
    c.frame();
    let live = data(&c);
    assert_eq!(live.loud_short.len(), 10);
    set(&mut c, "m", "bypass", 1.0);
    let (l, r, head) = frames(40, 90);
    c.sink_mut().device_scopes.insert(id, (l, r, head));
    c.sink_mut()
        .device_values
        .insert(id, (values(-14.0, 30, 1.0), 2));
    c.frame();
    assert_eq!(data(&c), live);
}

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

/// A sine synth playing A4 for 8 beats (4 s at 120 bpm) straight into the loudness meter.
fn tone_rack<S: EngineSink>(c: &mut AppCore<S>) {
    add(c, SYNTH, "synth");
    add(c, LOUD, "m");
    set(c, "synth", "osc1.wave", 0.0);
    for side in ["l", "r"] {
        connect(
            c,
            ("synth", &format!("out_{side}")),
            ("m", &format!("in_{side}")),
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
fn a_real_engine_feeds_the_tone_it_plays() {
    let mut c = AppCore::new(OfflineSink::new());
    tone_rack(&mut c);
    c.transport_action(atomik_ui::TransportAction::Play);
    for _ in 0..40 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    let m = c.view().device_meters(&key("m")).to_vec();
    assert_eq!(m.len(), dev::METERS);
    assert!(
        m[slot::INTEGRATED] > -50.0 && m[slot::INTEGRATED] < 0.0,
        "{m:?}"
    );
    assert!(m[slot::TRUE_PEAK] > -40.0, "{m:?}");
    assert!(m[slot::TIME] >= 3.0, "{m:?}");
    let d = data(&c);
    assert!(!d.loud_short.is_empty(), "history points after the settle");
    assert_eq!(d.loud_short.len(), d.loud_momentary.len());
    assert!(d.loud_short.iter().all(|v| *v < 6.0));
}

#[test]
fn the_probe_reads_the_tone_and_names_the_values() {
    let mut c = AppCore::new(RecordingSink::new());
    tone_rack(&mut c);
    let readouts = c.probe_meters(5 * 48_000, None).expect("a probe");
    assert_eq!(readouts.len(), 1);
    let r = &readouts[0];
    assert_eq!(r.type_id, LOUD);
    assert_eq!(r.values.len(), LOUDNESS_VALUE_NAMES.len());
    let value = |n: &str| r.values.iter().find(|(k, _)| *k == n).unwrap().1;
    assert!(value("integrated_lufs") > -50.0, "{r:?}");
    assert!(value("true_peak_dbtp") > -40.0, "{r:?}");
    assert_eq!(value("target_lufs"), -14.0);
    assert_eq!(value("max_true_peak_dbtp"), -1.0);
    assert!(value("history_points") >= 2.0, "{r:?}");
    assert!(value("programme_seconds") > 4.9, "{r:?}");
    // The probe filled the panel too.
    let d = data(&c);
    assert_eq!(d.loud_short.len() as f32, value("history_points"));
    // The names and the device's indices agree.
    assert_eq!(
        LOUDNESS_VALUE_NAMES[dev::value::INTEGRATED],
        "integrated_lufs"
    );
    assert_eq!(LOUDNESS_VALUE_NAMES[dev::value::TIME], "programme_seconds");
    assert_eq!(LOUDNESS_VALUE_NAMES.len(), dev::value::BINS);
}
