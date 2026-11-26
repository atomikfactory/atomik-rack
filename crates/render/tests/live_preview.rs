//! A scratch project played by hand: silence until a note is sent, sound while it is held, the
//! same bits for the same notes at the same frames, and a project with no track on a device that
//! takes notes refused.

use atomik_core::NoteKind;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::live_preview::LivePreview;

const SYNTH: &str = "atomik.poly-synth";
const BLOCK: usize = 128;

fn scratch(track: bool) -> atomik_model::RackDoc {
    let registry = default_registry();
    let mut doc = Document::new(empty_doc());
    let key = DeviceKey("synth".to_string());
    doc.apply(
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key.clone()),
        },
        &registry,
    )
    .unwrap();
    if track {
        doc.apply(
            Command::Song(SongCommand::AddTrack {
                device: key.clone(),
            }),
            &registry,
        )
        .unwrap();
        doc.apply(
            Command::Song(SongCommand::AddClip {
                device: key,
                lane: 0,
                clip: Clip {
                    id: ClipId(0),
                    start: 0,
                    length: 1920,
                    offset: 0,
                    loop_length: None,
                    muted: false,
                    scale: None,
                    param_lanes: Vec::new(),
                    midi: Default::default(),
                    groove: None,
                    follow_song_key: true,
                    controllers: Vec::new(),
                    name: None,
                    color: None,
                    grid: None,
                    content: ClipContent::Notes(Vec::new().into()),
                },
            }),
            &registry,
        )
        .unwrap();
    }
    doc.doc().clone()
}

fn peak(planes: &[Vec<f32>]) -> f32 {
    planes.iter().flatten().fold(0.0f32, |p, s| p.max(s.abs()))
}

fn play(notes: &[(usize, NoteKind)], blocks: usize) -> Vec<f32> {
    let mut live = LivePreview::build(
        &scratch(true),
        &default_registry(),
        None,
        Default::default(),
    )
    .expect("the scratch project builds");
    assert_eq!(live.sample_rate(), 48_000);
    let mut left = Vec::new();
    for block in 0..blocks {
        for (at, kind) in notes {
            if *at == block {
                live.note(*kind);
            }
        }
        let planes = live.render(BLOCK).expect("renders");
        assert_eq!(planes.len(), 2);
        left.extend_from_slice(&planes[0]);
    }
    left
}

#[test]
fn it_is_silent_until_a_note_arrives_then_sounds_and_ends_with_the_note() {
    let on = NoteKind::On {
        key: 60,
        velocity: 0.8,
    };
    let off = NoteKind::Off { key: 60 };
    let left = play(&[(20, on), (120, off)], 600);
    let at = |from: usize, to: usize| peak(&[left[from * BLOCK..to * BLOCK].to_vec()]);
    assert!(at(0, 20) < 1e-9, "silent before the note");
    assert!(at(21, 120) > 0.01, "sounding while the key is held");
    assert!(
        at(560, 600) < at(21, 120) * 0.1,
        "the release dies away after the key goes up"
    );
}

#[test]
fn the_same_notes_at_the_same_frames_give_the_same_bits() {
    let on = NoteKind::On {
        key: 64,
        velocity: 0.7,
    };
    let a = play(&[(3, on)], 100);
    let b = play(&[(3, on)], 100);
    assert_eq!(a, b);
}

#[test]
fn a_project_with_no_track_is_refused() {
    let err = LivePreview::build(
        &scratch(false),
        &default_registry(),
        None,
        Default::default(),
    )
    .expect_err("no track");
    assert!(err.to_string().contains("takes notes"), "{err}");
}
