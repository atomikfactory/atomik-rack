//! `Command::RelinkAsset`: every reference to one pool asset (audio clips, sampler, slicer
//! and drum zones, inside containers too) is swapped to another pooled asset as one undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, Command, ContainerCommand, Document, ModelError, RackDoc, SyncOp, ZoneDoc,
    empty_doc, load_project, save_project,
};
use common::audio::*;
use common::*;
use proptest::prelude::*;

fn relink(from: u8, to: u8) -> Command {
    Command::RelinkAsset {
        from: hash(from),
        to: hash(to),
    }
}

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo, "a rejected command records no step");
    e
}

fn set_zones(device: &str, zones: Vec<ZoneDoc>) -> Command {
    Command::SetZones {
        device: key(device),
        zones,
        merge: None,
    }
}

/// Assets 1 (480 000 frames) and 2 (96 000), plus 3 (500 000) as the replacement; an audio
/// track with a clip of asset 1 and one of asset 2; a sampler `s` with zones on assets 1 and 2;
/// a container `c` holding a sampler on asset 1.
fn scene(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(register(3, 500_000), reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, 1_000, 1), reg)
        .unwrap();
    d.apply(add_audio_clip("audio-track", 2_000, 1_000, 2), reg)
        .unwrap();
    d.apply(add_audio_clip("audio-track", 4_000, 1_000, 1), reg)
        .unwrap();
    d.apply(add(0, SAMPLER, Some("s")), reg).unwrap();
    d.apply(
        set_zones(
            "s",
            vec![
                ZoneDoc::new(hash(1), 480_000),
                ZoneDoc::new(hash(2), 96_000),
            ],
        ),
        reg,
    )
    .unwrap();
    d.apply(add(0, CONTAINER, Some("c")), reg).unwrap();
    d.apply(
        Command::Container(ContainerCommand::AddInner {
            container: key("c"),
            index: 0,
            type_id: SAMPLER.to_owned(),
            key: Some(key("in")),
            auto_split: false,
        }),
        reg,
    )
    .unwrap();
    d.apply(set_zones("c/in", vec![ZoneDoc::new(hash(1), 480_000)]), reg)
        .unwrap();
    d
}

fn clip_assets(doc: &RackDoc) -> Vec<String> {
    doc.song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .filter_map(|c| match &c.content {
            ClipContent::Audio(a) => Some(a.asset.clone()),
            _ => None,
        })
        .collect()
}

fn zone_assets(doc: &RackDoc, device: &str) -> Vec<String> {
    doc.resolve(&key(device))
        .unwrap()
        .zones
        .iter()
        .map(|z| z.asset.clone())
        .collect()
}

#[test]
fn relink_swaps_every_reference_kind_and_leaves_the_others() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    d.apply(relink(1, 3), &reg).unwrap();
    let doc = d.doc();
    assert_eq!(clip_assets(doc), vec![hash(3), hash(2), hash(3)]);
    assert_eq!(zone_assets(doc, "s"), vec![hash(3), hash(2)]);
    assert_eq!(zone_assets(doc, "c/in"), vec![hash(3)]);
    assert!(!doc.referenced_assets().contains(&hash(1)));
    assert!(doc.referenced_assets().contains(&hash(3)));
    // Everything else about the zones and clips is kept, including their frame ranges.
    let z = &doc.resolve(&key("s")).unwrap().zones[0];
    assert_eq!((z.start, z.end), (0, 480_000));
    // The old entry stays in the pool for undo (the history still references it).
    assert!(doc.assets.contains_key(&hash(1)));
    assert!(d.referenced_assets().contains(&hash(1)));
}

#[test]
fn relink_is_one_undo_step_and_undo_redo_restore_exactly() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    let before = d.doc().clone();
    let text_before = save_project(&before);
    d.apply(relink(1, 3), &reg).unwrap();
    let after = d.doc().clone();
    assert_ne!(after, before);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    assert_eq!(save_project(d.doc()), text_before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc(), &after);
    // One step: a second undo does not return to the state before the relink twice.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.undo(&reg).unwrap();
    assert_ne!(
        d.doc(),
        &before,
        "the step before the relink is a different one"
    );
}

#[test]
fn relink_reports_song_and_zone_changes_once() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    let ops = d.apply(relink(1, 3), &reg).unwrap();
    assert_eq!(ops.iter().filter(|o| **o == SyncOp::SongChanged).count(), 1);
    for device in ["s", "c/in"] {
        let want = SyncOp::ZonesChanged(key(device));
        assert_eq!(ops.iter().filter(|o| **o == want).count(), 1, "{device}");
    }
    let undo = d.undo(&reg).unwrap();
    assert!(undo.contains(&SyncOp::SongChanged));
    assert!(undo.contains(&SyncOp::ZonesChanged(key("s"))));
}

#[test]
fn relink_with_only_clips_or_only_zones_works() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(register(3, 500_000), &reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, 1_000, 1), &reg)
        .unwrap();
    let ops = d.apply(relink(1, 3), &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    assert_eq!(clip_assets(d.doc()), vec![hash(3)]);

    let mut d = Document::new(empty_doc());
    for (n, frames) in [(1, 1000), (3, 2000)] {
        d.apply(register(n, frames), &reg).unwrap();
    }
    d.apply(add(0, SAMPLER, Some("s")), &reg).unwrap();
    d.apply(set_zones("s", vec![ZoneDoc::new(hash(1), 1000)]), &reg)
        .unwrap();
    let ops = d.apply(relink(1, 3), &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::ZonesChanged(key("s"))]);
    assert_eq!(zone_assets(d.doc(), "s"), vec![hash(3)]);
}

#[test]
fn relink_refusals_change_nothing() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    // Unknown source, unknown target, same asset, an asset nothing uses.
    assert!(matches!(
        rejects(&mut d, relink(9, 3), &reg),
        ModelError::MissingAsset(_)
    ));
    assert!(matches!(
        rejects(&mut d, relink(1, 9), &reg),
        ModelError::MissingAsset(_)
    ));
    assert!(matches!(
        rejects(&mut d, relink(1, 1), &reg),
        ModelError::BadSong(_)
    ));
    assert!(matches!(
        rejects(&mut d, relink(3, 1), &reg),
        ModelError::BadSong(_)
    ));
    // A malformed id.
    let bad = Command::RelinkAsset {
        from: "abc".into(),
        to: hash(3),
    };
    assert!(matches!(rejects(&mut d, bad, &reg), ModelError::BadSong(_)));
}

#[test]
fn relink_to_a_shorter_asset_is_refused_when_a_zone_or_clip_offset_needs_the_length() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    // Asset 4 has 1000 frames: the zones span 480 000.
    d.apply(register(4, 1_000), &reg).unwrap();
    assert!(matches!(
        rejects(&mut d, relink(1, 4), &reg),
        ModelError::BadZones(_)
    ));
    // A clip that starts 5 000 frames in cannot move to a 1 000-frame asset either.
    let mut d = with_audio_track(&reg);
    d.apply(register(4, 1_000), &reg).unwrap();
    let mut a = audio(1);
    a.source_offset = 5_000;
    d.apply(add_audio_clip_with("audio-track", 0, 1_000, a), &reg)
        .unwrap();
    assert!(matches!(
        rejects(&mut d, relink(1, 4), &reg),
        ModelError::BadSong(_)
    ));
}

#[test]
fn relink_is_a_step_inside_the_history_and_clears_redo_like_any_edit() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    d.apply(relink(1, 3), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert!(d.can_redo());
    d.apply(relink(2, 3), &reg).unwrap();
    assert!(!d.can_redo());
}

#[test]
fn a_relinked_document_saves_and_loads_back() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    d.apply(relink(1, 3), &reg).unwrap();
    let mut saved = d.doc().clone();
    saved.prune_unreferenced_assets();
    assert!(!saved.assets.contains_key(&hash(1)));
    let text = save_project(&saved);
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(save_project(&back), text);
}

#[derive(Debug, Clone)]
enum Op {
    Relink(u8, u8),
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (1u8..=4, 1u8..=4).prop_map(|(a, b)| Op::Relink(a, b)),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Over random relink/undo/redo runs: a rejection changes nothing, an accepted relink is
    /// one step whose undo restores the document byte for byte, every reference stays pooled,
    /// and undoing everything returns the starting document.
    #[test]
    fn relink_runs_keep_the_document_valid(ops in proptest::collection::vec(op(), 1..24)) {
        let reg = audio_registry();
        let mut d = scene(&reg);
        d.apply(register(4, 480_000), &reg).unwrap();
        let floor = {
            let mut probe = d.clone();
            while probe.undo(&reg).is_some() {}
            save_project(probe.doc())
        };
        for op in ops {
            match op {
                Op::Relink(a, b) => {
                    let before = save_project(d.doc());
                    match d.apply(relink(a, b), &reg) {
                        Ok(_) => {
                            let after = save_project(d.doc());
                            d.undo(&reg).unwrap();
                            prop_assert_eq!(save_project(d.doc()), before);
                            d.redo(&reg).unwrap();
                            prop_assert_eq!(save_project(d.doc()), after);
                        }
                        Err(_) => prop_assert_eq!(save_project(d.doc()), before),
                    }
                }
                Op::Undo => { d.undo(&reg); }
                Op::Redo => { d.redo(&reg); }
            }
            assert_audio_invariants(d.doc());
            let text = save_project(d.doc());
            prop_assert_eq!(save_project(&load_project(&text, &reg).unwrap()), text);
        }
        while d.undo(&reg).is_some() {}
        prop_assert_eq!(save_project(d.doc()), floor);
    }
}
