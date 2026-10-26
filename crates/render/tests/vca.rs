//! VCA groups reach the audio exactly as their members' own controls would: a group at -6 dB
//! bounces like its members' faders at -6 dB, a group at 0 dB (or no group) is bit for bit the
//! plain project, a group's mute and solo are the members' own, and the bounce does not depend
//! on the block size or the worker count.

use atomik_core::PPQ;
use atomik_dsp::fader_pos_for_db;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, Note, RackDoc,
    SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// Three synth tracks (`lead`, `pad`, `bass`) with a one-bar note each, so channels `channel`,
/// `channel-2` and `channel-3`.
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
    d.apply(Command::Song(SongCommand::SetSongEnd(4 * PPQ)), &registry)
        .unwrap();
    d
}

fn bounce_with(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn bounce(doc: &RackDoc) -> Vec<f32> {
    bounce_with(doc, 512, 0)
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

fn level(db: f32) -> Command {
    console(ConsoleCommand::SetVcaLevel {
        vca: 1,
        db,
        merge: None,
    })
}

const ALL: [&str; 3] = ["channel", "channel-2", "channel-3"];

fn grouped(members: &[&str]) -> Document {
    let mut d = song();
    apply(
        &mut d,
        console(ConsoleCommand::CreateVca {
            name: "Group".into(),
            members: members.iter().map(|m| key(m)).collect(),
        }),
    );
    d
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn peak(a: &[f32]) -> f32 {
    a.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn a_group_at_zero_db_or_no_group_is_bit_identical_to_the_plain_project() {
    let plain = bounce(song().doc());
    assert!(peak(&plain) > 0.05, "the song is not silent");
    let d = grouped(&ALL);
    assert_eq!(bits(&bounce(d.doc())), bits(&plain), "a group at 0 dB");
    // Saved and loaded with the group, and as a project saved before the format had groups.
    let text = save_project(d.doc());
    let loaded = load_project(&text, &default_registry()).unwrap();
    assert_eq!(bits(&bounce(&loaded)), bits(&plain));
    let old = save_project(song().doc()).replace("\"version\": 32", "\"version\": 25");
    let loaded = load_project(&old, &default_registry()).unwrap();
    assert_eq!(bits(&bounce(&loaded)), bits(&plain), "a version 25 file");
}

#[test]
fn a_group_at_minus_six_db_bounces_like_its_members_faders_at_minus_six_db() {
    let pos = fader_pos_for_db(-6.0);
    let mut by_faders = song();
    for c in ALL {
        apply(&mut by_faders, set(c, "fader", pos));
    }
    let mut by_group = grouped(&ALL);
    apply(&mut by_group, level(-6.0));
    let plain = bounce(song().doc());
    let a = bounce(by_faders.doc());
    let b = bounce(by_group.doc());
    assert!(
        (peak(&b) / peak(&plain) - 10f32.powf(-6.0 / 20.0)).abs() < 1e-3,
        "{} {}",
        peak(&b),
        peak(&plain)
    );
    assert!(
        max_diff(&a, &b) < 1e-4 * peak(&plain),
        "{}",
        max_diff(&a, &b)
    );
}

#[test]
fn only_the_members_follow_the_group() {
    let mut d = grouped(&["channel", "channel-2"]);
    apply(&mut d, level(-60.0));
    // The two members are off; the bass alone is what a project with them muted plays.
    let mut bass = song();
    for c in ["channel", "channel-2"] {
        apply(&mut bass, set(c, "mute", 1.0));
    }
    let a = bounce(d.doc());
    let b = bounce(bass.doc());
    assert!(peak(&a) > 0.02);
    assert!(max_diff(&a, &b) < 1e-5, "{}", max_diff(&a, &b));
}

#[test]
fn the_offset_adds_to_the_members_own_fader() {
    let mut own = song();
    apply(&mut own, set("channel", "fader", fader_pos_for_db(-3.0)));
    let mut both = grouped(&["channel"]);
    apply(&mut both, set("channel", "fader", fader_pos_for_db(-3.0)));
    apply(&mut both, level(-3.0));
    let mut sum = song();
    apply(&mut sum, set("channel", "fader", fader_pos_for_db(-6.0)));
    let a = bounce(both.doc());
    let b = bounce(sum.doc());
    assert!(max_diff(&a, &b) < 1e-4 * peak(&a), "{}", max_diff(&a, &b));
    assert!(
        max_diff(&a, &bounce(own.doc())) > 1e-3 * peak(&a),
        "the group changed the level"
    );
}

#[test]
fn a_group_mute_is_the_members_own_mute_bit_for_bit() {
    let mut by_members = song();
    for c in ["channel", "channel-3"] {
        apply(&mut by_members, set(c, "mute", 1.0));
    }
    let mut by_group = grouped(&["channel", "channel-3"]);
    apply(
        &mut by_group,
        console(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
    );
    let a = bounce(by_members.doc());
    assert!(peak(&a) > 0.02);
    assert_eq!(bits(&bounce(by_group.doc())), bits(&a));
}

#[test]
fn a_group_solo_is_the_members_own_solo_bit_for_bit() {
    let mut by_members = song();
    for c in ["channel", "channel-3"] {
        apply(&mut by_members, set(c, "solo", 1.0));
    }
    let mut by_group = grouped(&["channel", "channel-3"]);
    apply(
        &mut by_group,
        console(ConsoleCommand::SetVcaSolo { vca: 1, on: true }),
    );
    let a = bounce(by_members.doc());
    let plain = bounce(song().doc());
    assert!(max_diff(&a, &plain) > 1e-3, "the solo silenced the pad");
    assert_eq!(bits(&bounce(by_group.doc())), bits(&a));
}

#[test]
fn a_group_solo_beats_a_member_mute_and_the_group_mute() {
    let mut d = grouped(&["channel"]);
    for c in [
        set("channel", "mute", 1.0),
        console(ConsoleCommand::SetVcaMute { vca: 1, on: true }),
        console(ConsoleCommand::SetVcaSolo { vca: 1, on: true }),
    ] {
        apply(&mut d, c);
    }
    let mut only = song();
    apply(&mut only, set("channel", "solo", 1.0));
    assert_eq!(bits(&bounce(d.doc())), bits(&bounce(only.doc())));
}

#[test]
fn the_bounce_with_a_group_does_not_depend_on_the_block_or_the_workers() {
    let mut d = grouped(&ALL);
    for c in [level(-4.25), set("channel-2", "fader", 0.6)] {
        apply(&mut d, c);
    }
    let base = bits(&bounce_with(d.doc(), 64, 0));
    for (block, workers) in [(512, 0), (333, 0), (64, 2), (512, 3)] {
        assert_eq!(
            bits(&bounce_with(d.doc(), block, workers)),
            base,
            "block {block}, workers {workers}"
        );
    }
}
