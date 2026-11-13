//! `AddStemTracks`: the tracks a separated audio clip becomes, in one undo step.

mod common;

use atomik_model::{ClipContent, Command, ModelError, SongCommand, StemTrack};
use common::audio::*;
use common::song::*;
use common::*;

const NAMES: [&str; 4] = ["Take Vocals", "Take Drums", "Take Bass", "Take Other"];

/// A document with an audio track holding one clip of asset 1 (somewhat edited, so that a copy
/// that forgets a setting shows) and the four stem files 11..14 in the pool, as long as asset 1.
fn rack() -> (atomik_device_sdk::Registry, atomik_model::Document) {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    for n in 11..=14 {
        d.apply(register(n, 480_000), &reg).unwrap();
    }
    let device = d.doc().song.tracks[0].device.0.clone();
    d.apply(add_audio_clip(&device, BAR, 2 * BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let mut content = audio(1);
    content.source_offset = 4800;
    content.gain_db = -3.0;
    content.fade_in = 240;
    content.fade_out = 480;
    set_audio_content(&mut d, &reg, id, content);
    (reg, d)
}

fn stems() -> Vec<StemTrack> {
    NAMES
        .iter()
        .zip(11u8..)
        .map(|(name, n)| StemTrack {
            name: (*name).into(),
            asset: hash(n),
        })
        .collect()
}

fn command(clip: atomik_model::ClipId, mute_original: bool) -> Command {
    sc(SongCommand::AddStemTracks {
        clip,
        stems: stems(),
        mute_original,
    })
}

#[test]
fn each_stem_is_a_track_under_the_original_with_the_same_clip() {
    let (reg, mut d) = rack();
    let original = clip_ids(d.doc())[0];
    d.apply(command(original, true), &reg).unwrap();
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), 5);
    let source = doc_clip(doc, original).clone();
    let ClipContent::Audio(source_audio) = &source.content else {
        panic!("audio")
    };
    for (i, name) in NAMES.iter().enumerate() {
        // Right after the original's track, in the order of the stems.
        let track = &doc.song.tracks[i + 1];
        assert!(track.is_audio());
        assert_eq!(&doc.device(&track.device).unwrap().name, name);
        let clip = &track.lanes[0].clips[0];
        assert_eq!((clip.start, clip.length), (BAR, 2 * BAR));
        assert!(!clip.muted);
        let ClipContent::Audio(a) = &clip.content else {
            panic!("audio")
        };
        assert_eq!(a.asset, hash(11 + i as u8));
        // Everything but the file is the original's.
        let mut same = a.clone();
        same.asset = source_audio.asset.clone();
        assert_eq!(&same, source_audio);
        assert!(doc.console.channel_of(&track.device).is_some(), "a channel");
    }
}

#[test]
fn the_original_is_muted_only_when_asked() {
    let (reg, mut d) = rack();
    let original = clip_ids(d.doc())[0];
    d.apply(command(original, false), &reg).unwrap();
    assert!(!doc_clip(d.doc(), original).muted);
    let (reg, mut d) = rack();
    let original = clip_ids(d.doc())[0];
    d.apply(command(original, true), &reg).unwrap();
    assert!(doc_clip(d.doc(), original).muted);
}

#[test]
fn it_is_one_undo_step_that_removes_everything_and_redo_brings_it_back() {
    let (reg, mut d) = rack();
    let original = clip_ids(d.doc())[0];
    let before = d.doc().clone();
    d.apply(command(original, true), &reg).unwrap();
    let after = d.doc().clone();
    assert_eq!(d.undo_label(), Some("Separate Stems"));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
    d.redo().unwrap();
    assert_eq!(d.doc(), &after);
}

#[test]
fn bad_input_changes_nothing() {
    let (reg, mut d) = rack();
    let original = clip_ids(d.doc())[0];
    let before = d.doc().clone();
    // A stem file that is not in the pool.
    let e = d
        .apply(
            sc(SongCommand::AddStemTracks {
                clip: original,
                stems: vec![StemTrack {
                    name: "A".into(),
                    asset: hash(99),
                }],
                mute_original: false,
            }),
            &reg,
        )
        .expect_err("missing asset");
    assert!(matches!(e, ModelError::MissingAsset(_)));
    // The second stem is bad: the first track is not left behind.
    let e = d.apply(
        sc(SongCommand::AddStemTracks {
            clip: original,
            stems: vec![
                StemTrack {
                    name: "A".into(),
                    asset: hash(11),
                },
                StemTrack {
                    name: "B".into(),
                    asset: hash(99),
                },
            ],
            mute_original: true,
        }),
        &reg,
    );
    assert!(e.is_err());
    // No stems, a clip that is not there.
    assert!(
        d.apply(
            sc(SongCommand::AddStemTracks {
                clip: original,
                stems: vec![],
                mute_original: false,
            }),
            &reg
        )
        .is_err()
    );
    assert!(
        d.apply(command(atomik_model::ClipId(9999), false), &reg)
            .is_err()
    );
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_note_clip_cannot_be_separated() {
    let (reg, mut d) = rack();
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, 480, 60, 0.8)])),
        &reg,
    )
    .unwrap();
    let notes = last(&d);
    assert!(d.apply(command(notes, false), &reg).is_err());
}
