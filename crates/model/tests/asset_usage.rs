//! The sample pool's usage counts and the removal of an unused sample.

mod common;

use atomik_model::{AssetRemoval, AssetUse, Command, Document, ZoneDoc};
use common::audio::*;
use common::*;

const SAMPLER: &str = "atomik.sampler";

fn with_sampler(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = with_audio_track(reg);
    d.apply(
        Command::AddDevice {
            index: usize::MAX,
            type_id: SAMPLER.into(),
            key: Some(key("smp")),
        },
        reg,
    )
    .unwrap();
    d
}

fn zones_on(d: &mut Document, reg: &atomik_device_sdk::Registry, n: &[u8]) {
    d.apply(
        Command::SetZones {
            device: key("smp"),
            zones: n.iter().map(|n| ZoneDoc::new(hash(*n), 96_000)).collect(),
            merge: None,
        },
        reg,
    )
    .unwrap();
}

#[test]
fn every_pool_asset_has_a_count_by_the_kind_of_use() {
    let reg = audio_registry();
    let mut d = with_sampler(&reg);
    d.apply(register(3, 96_000), &reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, 960, 1), &reg)
        .unwrap();
    d.apply(add_audio_clip("audio-track", 960, 960, 1), &reg)
        .unwrap();
    zones_on(&mut d, &reg, &[2, 2, 1]);
    let usage = d.doc().asset_usage();
    assert_eq!(usage.len(), 3);
    assert_eq!(
        usage[&hash(1)],
        AssetUse {
            clips: 2,
            zones: 1,
            ..AssetUse::default()
        }
    );
    assert_eq!(usage[&hash(2)].zones, 2);
    assert!(usage[&hash(1)].assigned() && usage[&hash(2)].assigned());
    assert!(
        !usage[&hash(3)].assigned(),
        "an unused sample is unassigned"
    );
    assert_eq!(usage[&hash(3)].total(), 0);
}

#[test]
fn assigned_follows_undo_and_redo() {
    let reg = audio_registry();
    let mut d = with_sampler(&reg);
    zones_on(&mut d, &reg, &[1]);
    assert!(d.doc().asset_usage()[&hash(1)].assigned());
    d.undo(&reg).unwrap();
    assert!(!d.doc().asset_usage()[&hash(1)].assigned());
    d.redo(&reg).unwrap();
    assert!(d.doc().asset_usage()[&hash(1)].assigned());
}

#[test]
fn an_unused_sample_leaves_the_pool_unless_undo_needs_it() {
    let reg = audio_registry();
    let mut d = with_sampler(&reg);
    zones_on(&mut d, &reg, &[1]);
    assert_eq!(d.remove_asset(&hash(1)), Err(AssetRemoval::InUse));
    assert_eq!(d.remove_asset(&hash(9)), Err(AssetRemoval::Unknown));
    // Undone: nothing uses it, but redo would.
    d.undo(&reg).unwrap();
    assert_eq!(d.remove_asset(&hash(1)), Err(AssetRemoval::History));
    // Never used: gone at once.
    assert!(d.doc().assets.contains_key(&hash(2)));
    d.remove_asset(&hash(2)).unwrap();
    assert!(!d.doc().assets.contains_key(&hash(2)));
    assert!(!d.doc().asset_usage().contains_key(&hash(2)));
}
