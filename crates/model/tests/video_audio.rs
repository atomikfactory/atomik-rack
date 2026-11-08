//! The film's own sound as an audio clip kept with the film: moving, trimming, relinking and
//! "keep with video" move and trim that clip in the same undo step, to the sample, and a drag
//! stays one step. The link is saved with the film and an older file opens without it.

mod common;

use atomik_model::{
    AudioLink, Command, Document, SyncOp, VideoClip, VideoCommand, VideoFacts, load_project,
    save_project, video_audio_placement,
};
use common::audio::*;
use common::workflow::{one_step, text};

const RATE: u32 = 48_000;

fn facts() -> VideoFacts {
    VideoFacts {
        duration_us: 60_000_000,
        fps_num: 25,
        fps_den: 1,
        width: 1280,
        height: 720,
        timecode: None,
        has_audio: true,
    }
}

fn frames_of(us: i64) -> i128 {
    (i128::from(us) * i128::from(RATE) + 500_000).div_euclid(1_000_000)
}

/// A song with a 60 s film, a 20 s track of asset 1 on an audio track (placed by the model's own
/// arithmetic for a film at 0 and a 0 lead), and the link made.
fn kept(lead_us: i64, keep: bool) -> (Document, atomik_device_sdk::Registry, atomik_model::ClipId) {
    let reg = audio_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(register(1, 48_000 * 20), &reg).unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    let film = VideoClip::new("films/scene.mov", facts());
    d.apply(Command::Video(VideoCommand::Set(film.clone())), &reg)
        .unwrap();
    let at = video_audio_placement(&d.doc().song, RATE, &film, lead_us, 48_000 * 20).unwrap();
    d.apply(
        add_audio_clip_with(
            "audio-track",
            at.start,
            at.length,
            atomik_model::AudioClipContent {
                source_offset: at.source_offset,
                ..audio(1)
            },
        ),
        &reg,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    d.apply(
        Command::Video(VideoCommand::LinkAudio(Some(AudioLink {
            clip: id,
            lead_us,
            keep,
        }))),
        &reg,
    )
    .unwrap();
    (d, reg, id)
}

fn video(d: &Document) -> VideoClip {
    d.doc().video.clone().unwrap()
}

/// The song frame the asset's frame 0 plays on.
fn asset_zero(d: &Document, id: atomik_model::ClipId) -> i128 {
    let c = doc_clip(d.doc(), id);
    let atomik_model::ClipContent::Audio(a) = &c.content else {
        panic!("an audio clip");
    };
    i128::from(d.doc().song.frame_at(c.start, RATE)) - i128::from(a.source_offset)
}

fn clip_state(d: &Document, id: atomik_model::ClipId) -> (i64, i64, u64) {
    let c = doc_clip(d.doc(), id);
    let atomik_model::ClipContent::Audio(a) = &c.content else {
        panic!("an audio clip");
    };
    (c.start, c.length, a.source_offset)
}

#[test]
fn moving_the_film_moves_its_sound_to_the_sample_in_one_undo_step() {
    let (mut d, reg, id) = kept(0, true);
    assert_eq!(asset_zero(&d, id), 0);
    let ops = one_step(
        &mut d,
        &reg,
        Command::Video(VideoCommand::Move {
            start_us: 1_234_567,
            merge: None,
        }),
    );
    assert!(ops.contains(&SyncOp::ViewChanged));
    assert!(ops.contains(&SyncOp::SongChanged), "the clip moved too");
    assert_eq!(d.undo_label(), Some("Move Video"));
    assert_eq!(asset_zero(&d, id), frames_of(1_234_567));
    // Another tempo: the same frames.
    let (mut d, reg, id) = kept(0, true);
    d.apply(
        Command::Song(atomik_model::SongCommand::SetTempoMap(vec![
            atomik_model::TempoEvent::step(0, 97.0),
        ])),
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 7_654_321,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(asset_zero(&d, id), frames_of(7_654_321));
}

#[test]
fn undo_puts_back_the_film_and_the_sound_together() {
    let (mut d, reg, id) = kept(0, true);
    let before = (video(&d), clip_state(&d, id), text(&d));
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 3_000_000,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_ne!(clip_state(&d, id), before.1);
    d.undo().unwrap();
    assert_eq!((video(&d), clip_state(&d, id), text(&d)), before);
    d.redo().unwrap();
    assert_eq!(video(&d).start_us, 3_000_000);
    assert_eq!(asset_zero(&d, id), frames_of(3_000_000));
}

#[test]
fn a_drag_of_the_film_is_one_undo_step_for_the_film_and_the_sound() {
    let (mut d, reg, id) = kept(0, true);
    let before = text(&d);
    let depth = {
        let mut c = d.clone();
        let mut n = 0;
        while c.undo().is_some() {
            n += 1;
        }
        n
    };
    for us in [500_000, 900_001, 1_700_003, -2_000_000] {
        d.apply(
            Command::Video(VideoCommand::Move {
                start_us: us,
                merge: Some(4),
            }),
            &reg,
        )
        .unwrap();
        assert_eq!(asset_zero(&d, id), frames_of(us));
    }
    let mut c = d.clone();
    let mut n = 0;
    while c.undo().is_some() {
        n += 1;
    }
    assert_eq!(n, depth + 1, "the whole drag is one step");
    d.undo().unwrap();
    assert_eq!(text(&d), before);
    d.redo().unwrap();
    assert_eq!(asset_zero(&d, id), frames_of(-2_000_000));
}

#[test]
fn trimming_shows_only_the_sound_inside_the_trim_and_undoes_with_the_film() {
    let (mut d, reg, id) = kept(0, true);
    let before = (video(&d), clip_state(&d, id));
    // Cut the first 5 s and everything after 15 s of the film.
    d.apply(
        Command::Video(VideoCommand::Trim {
            in_us: 5_000_000,
            out_us: 15_000_000,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.undo_label(), Some("Trim Video"));
    assert_eq!(video(&d).start_us, 5_000_000, "the picture keeps its place");
    // The sound still sits where it was (frame 0 of the asset at song frame 0)...
    assert_eq!(asset_zero(&d, id), 0);
    // ...and only the trimmed 10 s of it plays.
    let (start, length, offset) = clip_state(&d, id);
    assert_eq!(offset, 5 * 48_000);
    let song = &d.doc().song;
    assert_eq!(song.frame_at(start, RATE), 5 * 48_000);
    assert_eq!(song.frame_at(start + length, RATE), 15 * 48_000);
    d.undo().unwrap();
    assert_eq!((video(&d), clip_state(&d, id)), before);
}

#[test]
fn the_lead_of_the_track_is_kept_when_the_film_moves() {
    let (mut d, reg, id) = kept(500_000, true);
    assert_eq!(asset_zero(&d, id), 24_000);
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 2_000_000,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(asset_zero(&d, id), 96_000 + 24_000);
}

#[test]
fn a_clip_that_is_not_kept_stays_and_keeping_it_again_lines_it_up() {
    let (mut d, reg, id) = kept(0, true);
    one_step(&mut d, &reg, Command::Video(VideoCommand::KeepAudio(false)));
    let still = clip_state(&d, id);
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 4_000_000,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(clip_state(&d, id), still, "the free clip does not follow");
    assert_eq!(video(&d).start_us, 4_000_000);
    // Keeping it again puts the sound under the picture, in one step with the flag.
    let before = text(&d);
    one_step(&mut d, &reg, Command::Video(VideoCommand::KeepAudio(true)));
    assert_eq!(asset_zero(&d, id), frames_of(4_000_000));
    assert_ne!(text(&d), before);
    assert!(video(&d).audio_link.unwrap().keep);
}

#[test]
fn a_removed_or_missing_clip_does_not_stop_the_film_from_moving() {
    let (mut d, reg, id) = kept(0, true);
    d.apply(sc_remove(id), &reg).unwrap();
    assert!(d.doc().song.clip(id).is_none());
    d.apply(
        Command::Video(VideoCommand::Move {
            start_us: 1_000_000,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(video(&d).start_us, 1_000_000);
}

fn sc_remove(id: atomik_model::ClipId) -> Command {
    Command::Song(atomik_model::SongCommand::RemoveClip { id })
}

#[test]
fn the_link_is_saved_with_the_film_and_an_old_file_has_none() {
    let (d, reg, id) = kept(-7_000, true);
    let saved = text(&d);
    assert!(saved.contains("\"audio_link\""), "{saved}");
    let back = load_project(&saved, &reg).unwrap();
    assert_eq!(
        back.video.as_ref().unwrap().audio_link,
        Some(AudioLink {
            clip: id,
            lead_us: -7_000,
            keep: true
        })
    );
    assert_eq!(save_project(&back), saved, "save, load and save agree");
    // A film saved before the link existed opens with none.
    let mut plain = Document::new(atomik_model::empty_doc());
    plain
        .apply(
            Command::Video(VideoCommand::Set(VideoClip::new("a.mov", facts()))),
            &reg,
        )
        .unwrap();
    assert!(!text(&plain).contains("audio_link"));
    assert_eq!(
        load_project(&text(&plain), &reg)
            .unwrap()
            .video
            .unwrap()
            .audio_link,
        None
    );
}
