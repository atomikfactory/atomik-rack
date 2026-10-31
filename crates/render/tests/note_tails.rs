//! Note tails: with the song flag a note rings out past the end of its clip; without it (every
//! project written before format 35) the note is cut at the clip end and the bounce is what it
//! was.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand, empty_doc,
    load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};

/// A synth with a one-beat clip holding a note four beats long; the song is eight beats.
fn song(tails: bool) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let synth = DeviceKey("synth".into());
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
        Command::Song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
        Command::Song(SongCommand::AddClip {
            device: synth.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 4 * PPQ, 69, 1.0)].into()),
            },
        }),
        Command::Song(SongCommand::SetSongEnd(8 * PPQ)),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    let mut doc = d.doc().clone();
    doc.song_mut().tails = tails;
    doc
}

/// The summed magnitude of the left channel between beats `from` and `to` (120 BPM, 48 kHz).
fn energy(doc: &RackDoc, from: usize, to: usize) -> f32 {
    let r = render_doc(doc, &default_registry(), &ProjectOptions::default()).unwrap();
    let per_beat = 24_000;
    r.channels[0][from * per_beat..to * per_beat]
        .iter()
        .map(|x| x.abs())
        .sum()
}

#[test]
fn a_note_rings_past_its_clip_end_with_tails_and_not_without() {
    let cut = energy(&song(false), 3, 4);
    let ringing = energy(&song(true), 3, 4);
    assert!(
        ringing > 100.0,
        "the note still sounds in beat 4: {ringing}"
    );
    assert!(
        cut < ringing * 0.01,
        "the cut note has died away: {cut} vs {ringing}"
    );
}

#[test]
fn the_first_beat_is_the_same_either_way_and_the_flag_survives_save_and_load() {
    let (off, on) = (song(false), song(true));
    let first = |d: &RackDoc| {
        let r = render_doc(d, &default_registry(), &ProjectOptions::default()).unwrap();
        r.channels[0][..24_000].to_vec()
    };
    assert_eq!(
        first(&off),
        first(&on),
        "the note sounds the same until the clip ends"
    );
    let text = save_project(&on);
    let back = load_project(&text, &default_registry()).unwrap();
    assert!(back.song.tails);
    assert_eq!(back, on);
    assert!(
        !load_project(&save_project(&off), &default_registry())
            .unwrap()
            .song
            .tails
    );
}
