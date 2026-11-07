//! Snapshots taken in a song at one sample rate and pasted into a song at another: the audio is
//! pointed at converted copies, frame positions are scaled, ticks stay, and the paste is one
//! undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, ClipContent, ClipSnapshot, Command, DeviceSnapshot, Document, ModelError, RackDoc,
    RateCopy, SongCommand, WorkflowCommand, ZoneDoc, assets_off_rate,
};
use common::audio::*;
use common::*;

fn meta(frames: u64, rate: u32, name: &str) -> AssetMeta {
    AssetMeta {
        channels: 2,
        sample_rate: rate,
        frames,
        original_name: name.to_owned(),
    }
}

fn registered(d: &mut Document, reg: &Registry, n: u8, frames: u64, rate: u32) {
    d.apply(
        Command::RegisterAsset {
            hash: hash(n),
            meta: meta(frames, rate, &format!("a{n}.wav")),
        },
        reg,
    )
    .unwrap();
}

/// A song at 48 kHz: an audio clip of asset 1 (source offset 48 000, fade-in 4 800) and a
/// sampler whose zone uses asset 2 (96 000 frames, a start at 4 800).
fn source(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    registered(&mut d, reg, 1, 480_000, 48_000);
    registered(&mut d, reg, 2, 96_000, 48_000);
    d.apply(add_audio_clip("audio-track", 1_000, 40_000, 1), reg)
        .unwrap();
    let id = last(&d);
    let mut content = audio(1);
    content.source_offset = 48_000;
    content.fade_in = 4_800;
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id,
            start: 1_000,
            length: 40_000,
            muted: false,
            content,
            merge: None,
        }),
        reg,
    )
    .unwrap();
    d.apply(add(0, SAMPLER, Some("s")), reg).unwrap();
    let mut z = ZoneDoc::new(hash(2), 96_000);
    z.start = 4_800;
    d.apply(
        Command::SetZones {
            device: key("s"),
            zones: vec![z],
            merge: None,
        },
        reg,
    )
    .unwrap();
    d
}

/// The copies the app would make: asset 1 and 2 at 44.1 kHz as assets 11 and 12.
fn copies() -> Vec<RateCopy> {
    vec![
        RateCopy {
            from: hash(1),
            to: hash(11),
            meta: meta(441_000, 44_100, "a1.wav"),
        },
        RateCopy {
            from: hash(2),
            to: hash(12),
            meta: meta(88_200, 44_100, "a2.wav"),
        },
    ]
}

fn clip_audio(doc: &RackDoc) -> (i64, i64, atomik_model::AudioClipContent) {
    let c = &doc.song.tracks[0].lanes[0].clips[0];
    match &c.content {
        ClipContent::Audio(a) => (c.start, c.length, a.clone()),
        _ => panic!("not an audio clip"),
    }
}

#[test]
fn assets_off_rate_lists_the_ones_at_another_rate() {
    let reg = audio_registry();
    let d = source(&reg);
    let snap = ClipSnapshot::capture(d.doc(), &[atomik_model::ClipId(1)]).unwrap();
    assert_eq!(assets_off_rate(&snap.assets, 48_000), Vec::<String>::new());
    assert_eq!(assets_off_rate(&snap.assets, 44_100), vec![hash(1)]);
}

#[test]
fn a_clip_snapshot_points_at_the_copy_and_scales_its_frames() {
    let reg = audio_registry();
    let src = source(&reg);
    let id = src.doc().song.tracks[0].lanes[0].clips[0].id;
    let snap = ClipSnapshot::capture(src.doc(), &[id]).unwrap();
    let converted = snap.at_rate(44_100, &copies()).unwrap();
    // The original is untouched; the converted one lists only the copy it needs.
    assert!(snap.assets.contains_key(&hash(1)));
    assert_eq!(
        converted.assets.keys().cloned().collect::<Vec<_>>(),
        vec![hash(11)]
    );
    assert_eq!((converted.origin, converted.span), (snap.origin, snap.span));

    // Paste into a song that holds only the copies.
    let mut dst = with_audio_track(&reg);
    for c in copies() {
        dst.apply(
            Command::RegisterAsset {
                hash: c.to.clone(),
                meta: c.meta.clone(),
            },
            &reg,
        )
        .unwrap();
    }
    let before = dst.doc().clone();
    dst.apply(
        Command::Workflow(WorkflowCommand::PasteClips {
            track: None,
            lane: None,
            at_tick: 5_000,
            clips: converted,
        }),
        &reg,
    )
    .unwrap();
    let (start, length, a) = clip_audio(dst.doc());
    assert_eq!((start, length), (5_000, 40_000));
    assert_eq!(a.asset, hash(11));
    assert_eq!(a.source_offset, 44_100);
    assert_eq!(a.fade_in, 4_410);
    // One undo step takes the paste back.
    dst.undo().unwrap();
    assert_eq!(dst.doc(), &before);
    // The source song still names the original.
    let (_, _, original) = clip_audio(src.doc());
    assert_eq!(original.asset, hash(1));
}

#[test]
fn a_device_snapshot_converts_its_zones() {
    let reg = audio_registry();
    let src = source(&reg);
    let snap = DeviceSnapshot::capture(src.doc(), &[key("s")], false).unwrap();
    let converted = snap.at_rate(44_100, &copies()).unwrap();
    let z = &converted.devices[0].zones[0];
    assert_eq!(
        (z.asset.as_str(), z.start, z.end),
        (hash(12).as_str(), 4_410, 88_200)
    );
    assert!(converted.required_assets().contains(&hash(12)));
    assert!(!converted.required_assets().contains(&hash(2)));
}

#[test]
fn a_missing_copy_or_a_wrong_copy_is_refused() {
    let reg = audio_registry();
    let src = source(&reg);
    let id = src.doc().song.tracks[0].lanes[0].clips[0].id;
    let snap = ClipSnapshot::capture(src.doc(), &[id]).unwrap();
    // No copy for an asset at another rate.
    assert!(matches!(
        snap.at_rate(44_100, &[]),
        Err(ModelError::BadSong(_))
    ));
    // A copy at the wrong rate.
    let wrong = vec![RateCopy {
        from: hash(1),
        to: hash(11),
        meta: meta(441_000, 96_000, "a1.wav"),
    }];
    assert!(snap.at_rate(44_100, &wrong).is_err());
    // An unsupported rate.
    assert!(snap.at_rate(12_345, &copies()).is_err());
    // The same asset converted twice.
    let twice = vec![copies()[0].clone(), copies()[0].clone()];
    assert!(snap.at_rate(44_100, &twice).is_err());
}

#[test]
fn audio_already_at_the_rate_is_left_alone() {
    let reg = audio_registry();
    let src = source(&reg);
    let id = src.doc().song.tracks[0].lanes[0].clips[0].id;
    let snap = ClipSnapshot::capture(src.doc(), &[id]).unwrap();
    let same = snap.at_rate(48_000, &[]).unwrap();
    assert_eq!(same, snap);
}
