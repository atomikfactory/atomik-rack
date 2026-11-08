//! The masking map in the app core: the analysis runs on a worker over the song and ranks the pair
//! that shares a register first, nothing in the song changes, and the de-mask puts a keyed
//! Dynamic EQ band on the covered channel in one undo step (and reuses the device the next time).

use super::*;
use crate::masking_worker::{MaskAnalysis, MaskTrack};
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_dsp::masking::{BANDS, PairScore, band_of};
use atomik_model::{
    Clip, ClipContent, ConsoleCommand, CreateOptions, CreateSource, InsertEffect, Note,
    WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn sustained(keys: &[u8]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            keys.iter()
                .map(|&k| Note::new(0, 2 * BAR, k, 0.9))
                .collect::<Vec<_>>()
                .into(),
        ),
    }
}

/// The starter project with synths `loud` and `quiet` playing the same notes, `loud` 18 dB louder.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    for (k, level) in [("loud", 0.0f32), ("quiet", -18.0)] {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 0.0),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
        ] {
            c.apply_command(Command::SetParam {
                key: key(k),
                param: p.into(),
                value: v,
                merge: None,
            })
            .unwrap();
        }
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: sustained(&[52, 59]),
        }))
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::SetSongEnd(2 * BAR)))
        .unwrap();
    c
}

fn channel(c: &AppCore<RecordingSink>, source: &str) -> DeviceKey {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("channel")
        .channel
        .clone()
}

/// A finished analysis naming the channels of `loud` and `quiet` (the loud one covers) with a
/// clash at about 250 Hz.
fn found(c: &AppCore<RecordingSink>) -> MaskAnalysis {
    let band = band_of(250.0);
    let mut bands = [0.0; BANDS];
    bands[band] = 80.0;
    let mut masker_db = [-90.0; BANDS];
    let mut masked_db = [-90.0; BANDS];
    masker_db[band] = -12.0;
    masked_db[band] = -30.0;
    MaskAnalysis {
        from_tick: 0,
        from_frame: 0,
        rate: 48_000,
        tracks: vec![
            MaskTrack {
                channel: channel(c, "loud"),
                name: "Loud".into(),
            },
            MaskTrack {
                channel: channel(c, "quiet"),
                name: "Quiet".into(),
            },
        ],
        pairs: vec![PairScore {
            masker: 0,
            masked: 1,
            bands,
            worst: band,
            score: 80.0,
            ranges: vec![(10, 40)],
            masker_db,
            masked_db,
        }],
    }
}

fn dynamic_eqs(c: &AppCore<RecordingSink>) -> Vec<DeviceKey> {
    c.doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.dynamic-eq")
        .map(|d| d.key.clone())
        .collect()
}

fn value(c: &AppCore<RecordingSink>, k: &DeviceKey, p: &str) -> f32 {
    c.doc().param_value(k, p, c.registry()).expect("param")
}

#[test]
fn the_analysis_ranks_the_pair_that_shares_a_register_and_changes_nothing() {
    let mut c = core();
    let before = c.doc().clone();
    assert!(c.masking().is_none());
    c.start_masking().expect("starts");
    assert!(c.masking_running());
    assert!(c.start_masking().is_err(), "one at a time");
    c.wait_background();
    assert!(!c.masking_running());
    let found = c.masking().expect("a result");
    assert_eq!(found.tracks.len(), 2);
    let top = found.pairs.first().expect("the pair clashes");
    assert_eq!((top.masker, top.masked), (0, 1));
    assert!(top.score > 30.0, "{}", top.score);
    assert!(!top.ranges.is_empty());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Masking map")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(*c.doc(), before, "an analysis never changes the song");
}

#[test]
fn the_analysis_needs_two_channels_and_a_song() {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    assert!(c.start_masking().is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("two")));
}

#[test]
fn a_new_project_drops_the_result() {
    let mut c = core();
    c.start_masking().unwrap();
    c.wait_background();
    assert!(c.masking().is_some());
    c.new_document();
    assert!(c.masking().is_none());
}

#[test]
fn locating_a_range_moves_the_playhead_to_its_first_block() {
    let mut c = core();
    let analysis = found(&c);
    c.set_masking_for_test(analysis);
    c.locate_masking(0, 0);
    let frame = 10 * atomik_dsp::masking::HOP as u64;
    let want = c.doc().song.frame_clock(48_000).tick_at(frame);
    assert!(want > 0);
    assert_eq!(c.position(), want);
    // A range or a pair that is not there does nothing.
    c.locate_masking(0, 5);
    c.locate_masking(7, 0);
    assert_eq!(c.position(), want);
}

#[test]
fn de_masking_adds_a_keyed_band_to_the_covered_channel_in_one_undo_step() {
    let mut c = core();
    let analysis = found(&c);
    c.set_masking_for_test(analysis);
    let (loud, quiet) = (channel(&c, "loud"), channel(&c, "quiet"));
    let before = c.doc().clone();
    let report = c.demask(0, band_of(250.0)).expect("de-masks");
    assert!(report.added);
    assert_eq!(report.band, 2);
    assert_eq!(
        report.range_db, -6.0,
        "half the 18 dB lead is above the cap"
    );
    let eq = report.device.clone();
    // The device is on the covered channel chain, nowhere else.
    assert_eq!(dynamic_eqs(&c), vec![eq.clone()]);
    assert_eq!(
        atomik_model::insert_chain(c.doc(), c.registry(), &quiet),
        vec![eq.clone()]
    );
    assert!(atomik_model::insert_chain(c.doc(), c.registry(), &loud).is_empty());
    // The key is cabled from the covering channel insert send.
    for side in ["l", "r"] {
        assert!(
            c.doc().cables.iter().any(|x| x.from.device == loud
                && x.from.socket == format!("insert_send_{side}")
                && x.to.device == eq
                && x.to.socket == format!("sc_in_{side}")),
            "key {side}"
        );
    }
    // The band: on, at the clash band, listening to the key, a cut with a threshold under the
    // covering track level there.
    assert_eq!(value(&c, &eq, "band2.on"), 1.0);
    assert_eq!(value(&c, &eq, "band2.source"), 1.0);
    let freq = value(&c, &eq, "band2.freq");
    assert!((200.0..320.0).contains(&freq), "{freq}");
    assert_eq!(value(&c, &eq, "band2.range"), report.range_db);
    assert_eq!(value(&c, &eq, "band2.threshold"), -18.0);
    assert_eq!(value(&c, &eq, "band2.gain"), 0.0);
    assert_eq!(value(&c, &eq, "band3.on"), 0.0);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("De-masked Quiet under Loud")),
        "{:?}",
        c.last_rejection()
    );
    // One undo step takes the device, the band and the cables away together.
    assert!(c.undo());
    assert_eq!(*c.doc(), before);
    assert!(c.redo());
    assert_eq!(dynamic_eqs(&c), vec![eq]);
}

#[test]
fn a_second_de_mask_uses_the_device_and_the_next_free_band_in_one_step() {
    let mut c = core();
    let analysis = found(&c);
    c.set_masking_for_test(analysis);
    let first = c.demask(0, band_of(250.0)).expect("first");
    let after_first = c.doc().clone();
    let second = c.demask(0, band_of(400.0)).expect("second");
    assert!(!second.added);
    assert_eq!(second.device, first.device);
    assert_eq!(second.band, 3);
    assert_eq!(dynamic_eqs(&c).len(), 1);
    let cables = c
        .doc()
        .cables
        .iter()
        .filter(|x| x.to.device == first.device && x.to.socket.starts_with("sc_in"))
        .count();
    assert_eq!(cables, 2, "the key is not cabled twice");
    assert_eq!(value(&c, &second.device, "band3.on"), 1.0);
    assert!(c.undo());
    assert_eq!(*c.doc(), after_first, "the second de-mask is one step too");
}

#[test]
fn de_masking_refuses_what_it_cannot_do_and_leaves_the_song_alone() {
    let mut c = core();
    assert!(c.demask(0, 5).is_err(), "no analysis yet");
    let analysis = found(&c);
    c.set_masking_for_test(analysis);
    let before = c.doc().clone();
    assert!(c.demask(3, 5).is_err());
    assert!(c.demask(0, BANDS).is_err());
    assert_eq!(*c.doc(), before);
    // A track that is gone.
    c.apply_command(Command::RemoveDevice { key: key("quiet") })
        .unwrap();
    let removed = c.doc().clone();
    assert!(c.demask(0, band_of(250.0)).is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("no longer")));
    assert_eq!(*c.doc(), removed);
}

#[test]
fn a_key_from_another_track_is_not_taken_over() {
    let mut c = core();
    let analysis = found(&c);
    c.set_masking_for_test(analysis);
    let quiet = channel(&c, "quiet");
    // A Dynamic EQ on the covered chain with no key: the de-mask reuses it.
    c.apply_command(Command::Console(ConsoleCommand::SetInsert {
        channel: quiet,
        slot: 0,
        effect: Some(InsertEffect::New {
            type_id: "atomik.dynamic-eq".into(),
        }),
    }))
    .unwrap();
    let existing = dynamic_eqs(&c)[0].clone();
    let report = c.demask(0, band_of(250.0)).expect("reuses the free device");
    assert!(!report.added);
    assert_eq!(report.device, existing);
    // Another track keys it now: a de-mask from a third track adds a device of its own.
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key("third")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    let mut analysis = found(&c);
    analysis.tracks[0].channel = channel(&c, "third");
    c.set_masking_for_test(analysis);
    let report = c.demask(0, band_of(250.0)).expect("a new device");
    assert!(report.added);
    assert_ne!(report.device, existing);
    assert_eq!(dynamic_eqs(&c).len(), 2);
}
