//! Sample edits as render operations through the core: every edit equals a naive reference
//! on the sampled audio, the zones keep their audio positions, and one undo step restores zone and
//! asset reference.

use super::sampling_tests::{key, pump, ready, sample_into, zones};
use super::*;
use crate::offline_sink::OfflineSink;
use atomik_audio_file::{AudioData, Bundle};
use atomik_device_sdk::LoopMode;
use atomik_model::ZoneDoc;
use atomik_ui::SampleOp;

/// A sampler holding one zone over a 3096-frame sample (4096 frames recorded, 1000 of silence
/// before the signal), with a loop, and the sample's audio.
fn sampled() -> (AppCore<OfflineSink>, ZoneDoc, AudioData) {
    let mut core = ready(2, "atomik.sampler");
    sample_into(&mut core, None);
    pump(&mut core, 4_096, 64);
    sample_into(&mut core, None);
    let mut zone = zones(&core).remove(0);
    zone.start = 100;
    zone.end = 2_000;
    zone.loop_mode = LoopMode::Forward;
    zone.loop_start = 500;
    zone.loop_end = 1_500;
    core.apply_command(Command::SetZones {
        device: key("dev"),
        zones: vec![zone.clone()],
        merge: None,
    })
    .unwrap();
    let data = load(&core, &zone.asset);
    assert_eq!(data.frames(), 3_096);
    (core, zone, data)
}

fn load(core: &AppCore<OfflineSink>, hash: &str) -> AudioData {
    Bundle::open(core.bundle_root().unwrap())
        .unwrap()
        .load(hash)
        .unwrap()
}

fn edit(core: &mut AppCore<OfflineSink>, all_uses: bool, op: SampleOp) {
    core.handle_action(UiAction::EditSample {
        device: key("dev"),
        zone: 0,
        all_uses,
        edit: op,
    });
    core.wait_background();
}

fn bits(d: &AudioData) -> Vec<Vec<u32>> {
    d.channels
        .iter()
        .map(|c| c.iter().map(|s| s.to_bits()).collect())
        .collect()
}

#[test]
fn a_crop_makes_a_new_asset_and_the_zone_follows_the_audio() {
    let (mut core, zone, old) = sampled();
    let depth = core.document().undo_depth();
    edit(
        &mut core,
        false,
        SampleOp::Crop {
            from: 200,
            to: 2_500,
        },
    );
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    let now = zones(&core).remove(0);
    assert_ne!(now.asset, zone.asset);
    let new = load(&core, &now.asset);
    let expect: Vec<Vec<f32>> = old
        .channels
        .iter()
        .map(|c| c[200..2_500].to_vec())
        .collect();
    assert_eq!(
        bits(&new),
        bits(&AudioData::new(old.sample_rate, expect).unwrap())
    );
    assert_eq!(
        (now.start, now.end),
        (0, 1_800),
        "100..2000 shifted by 200 and cut"
    );
    assert_eq!((now.loop_start, now.loop_end), (300, 1_300));
    assert_eq!(core.doc().assets[&now.asset].frames, 2_300);
    assert_eq!(core.document().undo_depth(), depth + 1, "one undo step");

    // Undo restores zone and asset reference; redo brings the edit back.
    assert!(core.undo());
    assert_eq!(zones(&core)[0], zone);
    assert!(core.redo());
    assert_eq!(zones(&core)[0], now);
    // The superseded asset stays while the history needs it, and a save without the history
    // would not keep the one nothing references.
    core.document.prune_unreferenced_assets();
    assert!(core.doc().assets.contains_key(&zone.asset));
    assert!(!core.doc().referenced_assets().contains(&zone.asset));
}

#[test]
fn a_reverse_mirrors_the_audio_and_the_markers_stay_on_the_same_material() {
    let (mut core, zone, old) = sampled();
    edit(&mut core, false, SampleOp::Reverse { from: 0, to: 3_096 });
    let now = zones(&core).remove(0);
    let new = load(&core, &now.asset);
    for (n, o) in new.channels.iter().zip(&old.channels) {
        let mut rev = o.clone();
        rev.reverse();
        assert_eq!(n, &rev);
    }
    assert_eq!((now.start, now.end), (3_096 - 2_000, 3_096 - 100));
    assert_eq!((now.loop_start, now.loop_end), (3_096 - 1_500, 3_096 - 500));
    // The first frame of the zone is the old last frame of it, mirrored.
    assert_eq!(
        new.channels[0][now.start as usize].to_bits(),
        old.channels[0][zone.end as usize - 1].to_bits()
    );
    assert!(core.undo());
    assert_eq!(zones(&core)[0], zone);
}

#[test]
fn normalise_and_fades_keep_the_positions() {
    let (mut core, zone, old) = sampled();
    edit(
        &mut core,
        false,
        SampleOp::Normalise {
            from: 1_000,
            to: 1_100,
        },
    );
    let now = zones(&core).remove(0);
    let new = load(&core, &now.asset);
    let peak = old
        .channels
        .iter()
        .flat_map(|c| c[1_000..1_100].iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = 1.0 / peak;
    for (n, o) in new.channels.iter().zip(&old.channels) {
        for (a, b) in n.iter().zip(o) {
            assert!((a - b * gain).abs() < 1e-5);
        }
    }
    assert_eq!((now.start, now.end, now.loop_start), (100, 2_000, 500));
    assert!(core.undo());

    edit(&mut core, false, SampleOp::FadeIn { from: 100, to: 600 });
    let faded = load(&core, &zones(&core)[0].asset);
    assert_eq!(faded.channels[0][100], 0.0);
    assert_eq!(faded.channels[0][700], old.channels[0][700]);
    assert!(core.undo());
    edit(
        &mut core,
        false,
        SampleOp::FadeOut {
            from: 1_000,
            to: 1_500,
        },
    );
    let faded = load(&core, &zones(&core)[0].asset);
    assert_eq!(faded.channels[0][1_499], 0.0);
    assert_eq!(faded.channels[0][999], old.channels[0][999]);
    assert!(zones(&core)[0].asset != zone.asset);
}

#[test]
fn the_scope_is_the_zone_or_every_zone_playing_the_same_asset() {
    let (mut core, zone, _) = sampled();
    // A second zone of the same device and a zone of a second sampler play the same asset.
    let mut second = zone.clone();
    second.key_lo = 0;
    core.apply_command(Command::SetZones {
        device: key("dev"),
        zones: vec![zone.clone(), second.clone()],
        merge: None,
    })
    .unwrap();
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.sampler".into(),
        key: Some(key("other")),
    })
    .unwrap();
    core.apply_command(Command::SetZones {
        device: key("other"),
        zones: vec![zone.clone()],
        merge: None,
    })
    .unwrap();
    edit(&mut core, false, SampleOp::FadeIn { from: 0, to: 100 });
    let mine = zones(&core);
    assert_ne!(mine[0].asset, zone.asset);
    assert_eq!(mine[1].asset, zone.asset, "the other zone is left alone");
    let other = core.doc().device(&key("other")).unwrap().zones.clone();
    assert_eq!(other[0].asset, zone.asset);
    assert!(core.undo());

    let depth = core.document().undo_depth();
    edit(&mut core, true, SampleOp::FadeIn { from: 0, to: 100 });
    let mine = zones(&core);
    let other = core.doc().device(&key("other")).unwrap().zones.clone();
    assert_eq!(mine[0].asset, mine[1].asset);
    assert_eq!(mine[0].asset, other[0].asset);
    assert_ne!(mine[0].asset, zone.asset);
    assert_eq!(
        core.document().undo_depth(),
        depth + 1,
        "all uses are one step"
    );
    assert!(core.undo());
    assert_eq!(zones(&core), vec![zone.clone(), second]);
    assert_eq!(
        core.doc().device(&key("other")).unwrap().zones[0].asset,
        zone.asset
    );
}

#[test]
fn a_bad_range_or_a_missing_zone_changes_nothing() {
    let (mut core, zone, _) = sampled();
    let before = core.doc().clone();
    edit(&mut core, false, SampleOp::Crop { from: 500, to: 500 });
    assert!(core.last_rejection().unwrap().contains("Select"));
    edit(&mut core, false, SampleOp::Crop { from: 0, to: 9_999 });
    assert_eq!(core.doc(), &before);
    core.handle_action(UiAction::EditSample {
        device: key("dev"),
        zone: 7,
        all_uses: false,
        edit: SampleOp::Reverse { from: 0, to: 10 },
    });
    core.wait_background();
    assert!(core.last_rejection().unwrap().contains("zone is gone"));
    // A crop that leaves nothing of the zone drops it, in one step that undo restores.
    edit(
        &mut core,
        false,
        SampleOp::Crop {
            from: 2_500,
            to: 3_000,
        },
    );
    assert!(zones(&core).is_empty());
    assert!(core.undo());
    assert_eq!(zones(&core)[0], zone);
}
