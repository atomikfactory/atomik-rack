//! `Command::ConvertSampleRate` and `SetOutputChannels`: one undo step each, positions in ticks
//! unchanged, frame positions scaled.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, ClipContent, Command, Document, GainPoint, ModelError, RackDoc, SongCommand,
    ZoneDoc, load_project, save_project,
};
use common::audio::*;
use common::*;

fn registered(d: &mut Document, reg: &Registry, n: u8, frames: u64, rate: u32) {
    d.apply(
        Command::RegisterAsset {
            hash: hash(n),
            meta: AssetMeta {
                channels: 2,
                sample_rate: rate,
                frames,
                original_name: format!("a{n}.wav"),
            },
        },
        reg,
    )
    .unwrap();
}

/// Assets 1 and 2 at 48 kHz, their 44.1 kHz copies 11 and 12; a clip of asset 1 with fades,
/// an envelope and a source offset; a sampler on asset 2.
fn scene(reg: &Registry) -> Document {
    let mut d = with_audio_track(reg);
    registered(&mut d, reg, 11, 441_000, 44_100);
    registered(&mut d, reg, 12, 88_200, 44_100);
    registered(&mut d, reg, 21, 960_000, 96_000);
    registered(&mut d, reg, 22, 192_000, 96_000);
    d.apply(add_audio_clip("audio-track", 1_000, 2_000, 1), reg)
        .unwrap();
    let id = last(&d);
    let mut content = audio(1);
    content.source_offset = 48_000;
    content.fade_in = 4_800;
    content.fade_out = 9_600;
    content.envelope = vec![
        GainPoint {
            frame: 0,
            gain_db: 0.0,
        },
        GainPoint {
            frame: 48_000,
            gain_db: -6.0,
        },
    ];
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id,
            start: 1_000,
            length: 2_000,
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

fn convert(rate: u32) -> Command {
    Command::ConvertSampleRate {
        rate,
        assets: vec![(hash(1), hash(11)), (hash(2), hash(12))],
    }
}

fn first_clip(doc: &RackDoc) -> (i64, i64, atomik_model::AudioClipContent) {
    let c = &doc.song.tracks[0].lanes[0].clips[0];
    match &c.content {
        ClipContent::Audio(a) => (c.start, c.length, a.clone()),
        _ => panic!("not an audio clip"),
    }
}

#[test]
fn conversion_keeps_ticks_and_scales_frames_in_one_step() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    let before = d.doc().clone();
    d.apply(convert(44_100), &reg).unwrap();
    let doc = d.doc();
    assert_eq!(doc.sample_rate, 44_100);
    let (start, length, a) = first_clip(doc);
    assert_eq!((start, length), (1_000, 2_000));
    assert_eq!(a.asset, hash(11));
    assert_eq!(a.source_offset, 44_100);
    assert_eq!((a.fade_in, a.fade_out), (4_410, 8_820));
    assert_eq!(a.envelope[1].frame, 44_100);
    let z = &doc.resolve(&key("s")).unwrap().zones[0];
    assert_eq!(
        (z.asset.as_str(), z.start, z.end),
        (hash(12).as_str(), 4_410, 88_200)
    );
    assert!(!doc.referenced_assets().contains(&hash(1)));
    // One undo step restores the rate, the assets and every position.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().sample_rate, 44_100);
}

#[test]
fn conversion_round_trips_through_the_project_format() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    d.apply(
        Command::ConvertSampleRate {
            rate: 96_000,
            assets: vec![(hash(1), hash(21)), (hash(2), hash(22))],
        },
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(back.sample_rate, 96_000);
    assert_eq!(first_clip(&back).2.source_offset, 96_000);
}

#[test]
fn conversion_rejects_bad_requests_and_changes_nothing() {
    let reg = audio_registry();
    let mut d = scene(&reg);
    let before = d.doc().clone();
    let mut check = |cmd: Command| {
        let e = d.apply(cmd, &reg).expect_err("must be rejected");
        assert_eq!(d.doc(), &before);
        e
    };
    // The current rate, an unsupported one.
    assert!(matches!(check(convert(48_000)), ModelError::BadSong(_)));
    assert!(matches!(check(convert(22_050)), ModelError::OutOfRange(_)));
    // A used asset with no copy.
    assert!(matches!(
        check(Command::ConvertSampleRate {
            rate: 44_100,
            assets: vec![(hash(1), hash(11))],
        }),
        ModelError::BadSong(_)
    ));
    // A copy at the wrong rate (asset 2 is itself at 48 kHz).
    assert!(matches!(
        check(Command::ConvertSampleRate {
            rate: 44_100,
            assets: vec![(hash(1), hash(11)), (hash(2), hash(1))],
        }),
        ModelError::BadSong(_)
    ));
}

#[test]
fn output_channels_change_in_one_step_and_drop_the_cables_above() {
    let reg = audio_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(Command::SetOutputChannels(8), &reg).unwrap();
    assert_eq!(d.doc().output_channels, 8);
    let n = d.doc().cables.len();
    d.apply(Command::SetOutputChannels(1), &reg).unwrap();
    assert_eq!(d.doc().output_channels, 1);
    assert!(d.doc().cables.len() <= n);
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.to.socket != "out_2" && c.to.socket != "out_3")
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().output_channels, 8);
    assert!(matches!(
        d.apply(Command::SetOutputChannels(0), &reg),
        Err(ModelError::OutOfRange(_))
    ));
    assert!(matches!(
        d.apply(Command::SetOutputChannels(33), &reg),
        Err(ModelError::OutOfRange(_))
    ));
}
