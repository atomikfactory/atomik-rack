//! A muted note is silent in the bounce (and so live: both play what `flatten_notes` gives),
//! and the same project with the note unmuted is not.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SongCommand, empty_doc,
    load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};

fn song(muted: bool) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let synth = DeviceKey("synth".into());
    let length = 2 * PPQ;
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
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![Note {
                    muted,
                    ..Note::new(0, PPQ, 69, 1.0)
                }]),
            },
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    d.doc().clone()
}

fn peak(doc: &RackDoc) -> f32 {
    let r = render_doc(doc, &default_registry(), &ProjectOptions::default()).unwrap();
    r.channels
        .iter()
        .flatten()
        .fold(0.0_f32, |m, x| m.max(x.abs()))
}

#[test]
fn a_muted_note_is_silent_and_the_same_note_unmuted_is_not() {
    let audible = peak(&song(false));
    assert!(audible > 0.01, "the unmuted note sounds: {audible}");
    assert_eq!(peak(&song(true)), 0.0, "the muted note is silent");
}

#[test]
fn a_muted_note_survives_save_and_load_and_stays_silent() {
    let doc = song(true);
    let text = save_project(&doc);
    let back = load_project(&text, &default_registry()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(peak(&back), 0.0);
}
