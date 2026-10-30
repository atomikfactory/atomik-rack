//! (proptest): any sequence of song and rack commands applied then fully undone gives
//! back the original document; redo gives the final one. SyncOps replayed onto an engine
//! mirror keep its song equal to the document's, and the song never references a device the
//! engine does not have. `song.tempo[0].bpm == tempo_bpm` holds after every step.
//!
//! The same with the console devices registered (automatic channels) and console
//! commands mixed in; console invariants hold after every step.
//!
//! A third registry also has the audio track device, the pool holds two assets, and
//! audio commands (tracks, clips, trims, inputs) are mixed in; audio invariants hold after
//! every step.

mod common;

use atomik_model::{DeviceKey, Document, RackDoc, Song, SyncOp};
use common::audio::*;
use common::console_ops::*;
use common::song::*;
use common::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

/// What the engine side holds: device instances, the last compiled song and the tempo.
#[derive(Debug)]
struct Mirror {
    devices: BTreeSet<DeviceKey>,
    song: Song,
    tempo: f64,
}

impl Mirror {
    fn from_doc(doc: &RackDoc) -> Self {
        Self {
            devices: doc.devices.iter().map(|d| d.key.clone()).collect(),
            song: doc.song.clone(),
            tempo: doc.tempo_bpm,
        }
    }

    fn replay(&mut self, ops: &[SyncOp], doc: &RackDoc) {
        for op in ops {
            match op {
                SyncOp::DeviceAdded(k) => assert!(self.devices.insert(k.clone())),
                SyncOp::DeviceRemoved(k) => {
                    assert!(
                        self.song.tracks.iter().all(|t| &t.device != k),
                        "{k} removed while the engine song still plays it"
                    );
                    assert!(self.devices.remove(k));
                }
                SyncOp::SongChanged => {
                    for t in &doc.song.tracks {
                        assert!(
                            self.devices.contains(&t.device),
                            "song references {} before it is added",
                            t.device
                        );
                    }
                    self.song = doc.song.clone();
                }
                SyncOp::TempoChanged(t) => {
                    self.tempo = *t;
                    self.song.tempo[0].bpm = *t;
                }
                SyncOp::RoutingChanged
                | SyncOp::ParamChanged { .. }
                | SyncOp::ViewChanged
                | SyncOp::ConsoleChanged
                | SyncOp::MidiMapsChanged
                | SyncOp::ZonesChanged(_)
                | SyncOp::PatternsChanged { .. }
                | SyncOp::MacrosChanged { .. }
                | SyncOp::PluginChanged { .. }
                | SyncOp::PluginStateChanged { .. }
                | SyncOp::SplitsChanged { .. }
                | SyncOp::FrozenChanged => {}
            }
        }
    }

    fn assert_matches(&self, doc: &RackDoc) {
        assert_eq!(
            self.devices,
            doc.devices.iter().map(|d| d.key.clone()).collect()
        );
        assert_eq!(self.tempo.to_bits(), doc.tempo_bpm.to_bits());
        assert_eq!(self.song, doc.song);
        assert_tempo_invariant(doc);
        assert_console_invariants(doc);
        assert_audio_invariants(doc);
        let mut ids = clip_ids(doc);
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "clip ids are unique");
        assert!(ids.iter().all(|id| id.0 < doc.song.next_clip_id));
        for t in &doc.song.tracks {
            assert!(doc.device(&t.device).is_some());
            for l in &t.lanes {
                let keys: Vec<_> = l.clips.iter().map(|c| (c.start, c.id)).collect();
                let mut sorted = keys.clone();
                sorted.sort();
                assert_eq!(keys, sorted, "clips sorted by (start, id)");
            }
        }
    }
}

/// A song/rack op, a console op or an audio op.
#[derive(Debug, Clone)]
enum AnyOp {
    Song(SongOp),
    Console(ConsoleOp),
    Audio(AudioOp),
}

fn any_op() -> impl Strategy<Value = AnyOp> {
    prop_oneof![
        3 => song_op().prop_map(AnyOp::Song),
        1 => console_op().prop_map(AnyOp::Console),
        2 => audio_op().prop_map(AnyOp::Audio),
    ]
}

/// Registry and start document of a run: 0 plain, 1 console devices, 2 console and audio track
/// (with two pooled assets).
fn setup(mode: u8) -> (atomik_device_sdk::Registry, Document) {
    let reg = match mode {
        0 => registry(),
        1 => console_registry(),
        _ => audio_registry(),
    };
    let mut d = song_start(&registry());
    if mode == 2 {
        pre_register(&mut d, &reg);
    }
    (reg, d)
}

fn step(d: &mut Document, mirror: &mut Mirror, op: &AnyOp, reg: &atomik_device_sdk::Registry) {
    if matches!(op, AnyOp::Song(SongOp::EndMerge)) {
        d.end_merge();
        return;
    }
    let cmd = match op {
        AnyOp::Song(op) => resolve_song(op, d.doc(), reg),
        AnyOp::Console(op) => resolve_console(op, d.doc()),
        AnyOp::Audio(op) => resolve_audio(op, d.doc()),
    };
    let Some(cmd) = cmd else {
        return;
    };
    let before = d.doc().clone();
    match d.apply(cmd, reg) {
        Ok(ops) => mirror.replay(&ops, d.doc()),
        Err(_) => assert_eq!(d.doc(), &before),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn ac_s1_apply_undo_all_redo_all(mode in 0u8..3, ops in prop::collection::vec(any_op(), 0..60)) {
        let (reg, mut d) = setup(mode);
        let original = d.doc().clone();
        let mut mirror = Mirror::from_doc(&original);
        for op in &ops {
            step(&mut d, &mut mirror, op, &reg);
            mirror.assert_matches(d.doc());
        }
        let last = d.doc().clone();
        while let Some(sync) = d.undo(&reg) {
            mirror.replay(&sync, d.doc());
            mirror.assert_matches(d.doc());
        }
        prop_assert_eq!(d.doc(), &original);
        while let Some(sync) = d.redo(&reg) {
            mirror.replay(&sync, d.doc());
            mirror.assert_matches(d.doc());
        }
        prop_assert_eq!(d.doc(), &last);
    }

    #[test]
    fn ac_s1_mirror_follows_mixed_apply_undo_redo(mode in 0u8..3, ops in prop::collection::vec((0u8..10, any_op()), 0..80)) {
        let (reg, mut d) = setup(mode);
        let mut mirror = Mirror::from_doc(d.doc());
        for (choice, op) in &ops {
            match choice {
                0 | 1 => {
                    if let Some(sync) = d.undo(&reg) {
                        mirror.replay(&sync, d.doc());
                    }
                }
                2 => {
                    if let Some(sync) = d.redo(&reg) {
                        mirror.replay(&sync, d.doc());
                    }
                }
                _ => step(&mut d, &mut mirror, op, &reg),
            }
            mirror.assert_matches(d.doc());
        }
    }
}

/// The generator is only useful if it reaches every command, both accepted and rejected.
#[test]
fn generator_exercises_every_song_command() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use std::collections::BTreeMap;
    let reg = registry();
    let mut runner = TestRunner::deterministic();
    let mut accepted: BTreeMap<String, usize> = BTreeMap::new();
    let mut rejected = 0usize;
    for _ in 0..64 {
        let ops = prop::collection::vec(song_op(), 60)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let mut d = song_start(&reg);
        for op in &ops {
            let Some(cmd) = resolve_song(op, d.doc(), &reg) else {
                continue;
            };
            let name = match &cmd {
                atomik_model::Command::Song(s) => format!("{s:?}"),
                other => format!("{other:?}"),
            };
            let name = name.split([' ', '(', '{']).next().unwrap().to_owned();
            match d.apply(cmd, &reg) {
                Ok(_) => *accepted.entry(name).or_default() += 1,
                Err(_) => rejected += 1,
            }
        }
    }
    for kind in [
        "AddTrack",
        "RemoveTrack",
        "SetTrackFlags",
        "AddAutomationLane",
        "AddClip",
        "RemoveClip",
        "SetClipGeometry",
        "SetClipContent",
        "SplitClip",
        "DuplicateClip",
        "SetTempoMap",
        "SetMeter",
        "SetLoop",
        "SetSongEnd",
        "AddDevice",
        "RemoveDevice",
        "SetTempo",
    ] {
        assert!(
            accepted.get(kind).copied().unwrap_or(0) >= 3,
            "{kind} accepted too rarely: {accepted:?}"
        );
    }
    assert!(rejected > 50, "rejections are exercised ({rejected})");
}

/// The audio generator reaches every audio command, accepted and rejected.
#[test]
fn generator_exercises_every_audio_command() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use std::collections::BTreeMap;
    let mut runner = TestRunner::deterministic();
    let mut accepted: BTreeMap<String, usize> = BTreeMap::new();
    let mut rejected = 0usize;
    for _ in 0..64 {
        let ops = prop::collection::vec(any_op(), 60)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let (reg, mut d) = setup(2);
        for op in &ops {
            let AnyOp::Audio(op) = op else {
                if let AnyOp::Song(op) = op
                    && let Some(cmd) = resolve_song(op, d.doc(), &reg)
                {
                    let _ = d.apply(cmd, &reg);
                }
                continue;
            };
            let Some(cmd) = resolve_audio(op, d.doc()) else {
                continue;
            };
            let name = match &cmd {
                atomik_model::Command::Song(s) => format!("{s:?}"),
                other => format!("{other:?}"),
            };
            let name = name.split([' ', '(', '{']).next().unwrap().to_owned();
            match d.apply(cmd, &reg) {
                Ok(_) => *accepted.entry(name).or_default() += 1,
                Err(_) => rejected += 1,
            }
        }
    }
    for kind in [
        "AddAudioTrack",
        "AddAudioClip",
        "SetAudioClip",
        "SetInputChannels",
        "RegisterAsset",
    ] {
        assert!(
            accepted.get(kind).copied().unwrap_or(0) >= 3,
            "{kind} accepted too rarely: {accepted:?}"
        );
    }
    assert!(rejected > 50, "rejections are exercised ({rejected})");
}
