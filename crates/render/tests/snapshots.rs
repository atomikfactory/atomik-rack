//! Mixer snapshots reach the audio exactly as the mix they hold: recalling a snapshot bounces
//! bit for bit like a project whose controls were set to that mix by hand, a compare flip
//! bounces as either side, and a project without snapshots (or with snapshots it does not
//! recall) is bit for bit the plain project, including a version 26 file.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, Note, RackDoc,
    SnapshotScope, SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// Three synth tracks (`lead`, `pad`, `bass`) with a one-bar note each, so channels `channel`,
/// `channel-2` and `channel-3`, and an effect `comp` on the insert of the first.
fn song() -> Document {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    for (k, midi) in [("lead", 72u8), ("pad", 64), ("bass", 36)] {
        let length = 4 * PPQ;
        for c in [
            Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(key(k)),
            },
            Command::Song(SongCommand::AddTrack { device: key(k) }),
            Command::Song(SongCommand::AddClip {
                device: key(k),
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
                    content: ClipContent::Notes(vec![Note::new(0, length, midi, 0.8)]),
                },
            }),
        ] {
            d.apply(c.clone(), &registry)
                .unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.compressor".into(),
            key: Some(key("comp")),
        },
    );
    apply(
        &mut d,
        console(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: Some(atomik_model::InsertEffect::Existing(key("comp"))),
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(4 * PPQ)));
    d
}

fn bounce(doc: &RackDoc) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn console(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn set(k: &str, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    }
}

fn store(name: &str) -> Command {
    console(ConsoleCommand::StoreSnapshot { name: name.into() })
}

fn recall(id: u32) -> Command {
    console(ConsoleCommand::RecallSnapshot {
        id,
        scope: SnapshotScope::ALL,
    })
}

/// A mix: faders, a pan, a mute and an effect bypassed, a send level.
fn mix_a(d: &mut Document) {
    for c in [
        set("channel", "fader", 0.45),
        set("channel", "pan", -0.6),
        set("channel-2", "mute", 1.0),
        set("channel-3", "fader", 0.6),
        set("channel-3", "send1.on", 1.0),
        set("channel-3", "send1.level", 0.4),
        set("master", "fader", 0.7),
        set("comp", "bypass", 1.0),
    ] {
        apply(d, c);
    }
}

/// Another mix.
fn mix_b(d: &mut Document) {
    for c in [
        set("channel", "fader", 0.8),
        set("channel-2", "fader", 0.3),
        set("channel-2", "mute", 0.0),
        set("channel-3", "pan", 0.5),
        set("comp", "threshold", -30.0),
        set("comp", "ratio", 8.0),
        set("master", "fader", 0.5),
    ] {
        apply(d, c);
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn peak(a: &[f32]) -> f32 {
    a.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn a_recalled_snapshot_bounces_exactly_like_the_mix_set_by_hand() {
    let mut by_hand = song();
    mix_a(&mut by_hand);
    let want = bounce(by_hand.doc());
    assert!(peak(&want) > 0.01, "the song is not silent");

    let mut d = song();
    mix_a(&mut d);
    apply(&mut d, store("A"));
    mix_b(&mut d);
    let other = bounce(d.doc());
    assert_ne!(bits(&other), bits(&want), "the second mix sounds different");
    apply(&mut d, recall(1));
    assert_eq!(bits(&bounce(d.doc())), bits(&want), "recalled A");
    // Undo puts the second mix back, bit for bit.
    d.undo(&default_registry()).unwrap();
    assert_eq!(bits(&bounce(d.doc())), bits(&other), "undone");
    // And a saved and loaded project plays the same.
    apply(&mut d, recall(1));
    let loaded = load_project(&save_project(d.doc()), &default_registry()).unwrap();
    assert_eq!(bits(&bounce(&loaded)), bits(&want), "saved and loaded");
}

#[test]
fn a_compare_flip_bounces_as_either_side() {
    let mut a_side = song();
    mix_a(&mut a_side);
    let a = bounce(a_side.doc());

    let mut d = song();
    mix_a(&mut d);
    apply(&mut d, store("A"));
    mix_b(&mut d);
    let working = bounce(d.doc());
    apply(&mut d, console(ConsoleCommand::Compare { id: 1 }));
    assert_eq!(bits(&bounce(d.doc())), bits(&a), "on the snapshot");
    for _ in 0..2 {
        apply(&mut d, console(ConsoleCommand::Compare { id: 1 }));
        assert_eq!(bits(&bounce(d.doc())), bits(&working), "on the working mix");
        apply(&mut d, console(ConsoleCommand::Compare { id: 1 }));
        assert_eq!(bits(&bounce(d.doc())), bits(&a), "on the snapshot again");
    }
}

#[test]
fn a_scoped_recall_bounces_as_those_controls_by_hand() {
    // Only the mutes and the solos of the first mix: channel-2 muted, nothing else changes.
    let mut a = song();
    mix_a(&mut a);
    let mut d = song();
    mix_a(&mut d);
    apply(&mut d, store("A"));
    mix_b(&mut d);
    apply(
        &mut d,
        console(ConsoleCommand::RecallSnapshot {
            id: 1,
            scope: SnapshotScope::MUTES,
        }),
    );
    let mut by_hand = song();
    mix_a(&mut by_hand);
    mix_b(&mut by_hand);
    apply(&mut by_hand, set("channel-2", "mute", 1.0));
    assert_eq!(bits(&bounce(d.doc())), bits(&bounce(by_hand.doc())));
}

#[test]
fn a_project_without_snapshots_is_bit_identical_to_the_plain_project() {
    let plain = bounce(song().doc());
    assert!(peak(&plain) > 0.01);
    // Snapshots that are stored but not recalled change nothing.
    let mut d = song();
    apply(&mut d, store("A"));
    apply(&mut d, store("B"));
    assert_eq!(bits(&bounce(d.doc())), bits(&plain), "stored only");
    // A project saved before the format had snapshots plays as it did.
    let old = save_project(song().doc()).replace("\"version\": 34", "\"version\": 26");
    let loaded = load_project(&old, &default_registry()).unwrap();
    assert_eq!(bits(&bounce(&loaded)), bits(&plain), "a version 26 file");
    // Recalling a snapshot of the plain mix after edits comes back to the plain bounce (the
    // snapshot holds the strips and the bypass, not the parameters of the effects).
    apply(&mut d, set("channel", "fader", 0.3));
    apply(&mut d, set("channel-2", "pan", 0.7));
    apply(&mut d, set("master", "fader", 0.5));
    apply(&mut d, recall(1));
    assert_eq!(
        bits(&bounce(d.doc())),
        bits(&plain),
        "recalled the plain mix"
    );
}
