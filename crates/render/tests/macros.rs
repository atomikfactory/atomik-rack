//! Macro banks in an offline render: a lane on a macro renders bit for bit like the same lane
//! written by hand on the driven param, a static macro value like the param set to the same value,
//! a bank without assignments (or a project from before the format) bounces exactly as the
//! project without it, and the render does not depend on the block size or the worker count.

use atomik_core::PPQ;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Command, Curve, DeviceKey, Document, MacroAssign,
    MacroCommand, Note, RackDoc, SongCommand, assign_value, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const CUTOFF: &str = "filter.cutoff";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn clip(start: i64, length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content,
    }
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A synth `lead` with a one-bar note and a channel, the song two bars long.
fn song() -> Document {
    let mut d = Document::new(empty_doc());
    let length = 4 * PPQ;
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("lead")),
        },
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: clip(
                0,
                length,
                ClipContent::Notes(vec![Note::new(0, length, 60, 0.8)]),
            ),
        }),
        Command::Song(SongCommand::SetSongEnd(2 * length)),
    ] {
        apply(&mut d, c);
    }
    d
}

fn points() -> Vec<AutomationPoint> {
    vec![
        AutomationPoint {
            tick: 0,
            value: 0.0,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 2 * PPQ,
            value: 0.75,
            curve: Curve::Linear,
        },
        AutomationPoint {
            tick: 3 * PPQ,
            value: 0.2,
            curve: Curve::Step,
        },
    ]
}

fn lane_on(d: &mut Document, device: &str, param: &str, points: Vec<AutomationPoint>) {
    apply(
        d,
        Command::Song(SongCommand::AddAutomationLane {
            device: key(device),
            param: param.into(),
        }),
    );
    let lane = d
        .doc()
        .song
        .track(&key(device))
        .unwrap()
        .automation_lane(param)
        .unwrap();
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: key(device),
            lane,
            clip: clip(0, 4 * PPQ, ClipContent::Automation(points)),
        }),
    );
}

fn assign(d: &mut Document, min: Option<f32>, max: Option<f32>, invert: bool) -> DeviceKey {
    apply(
        d,
        Command::Macro(MacroCommand::Assign {
            bank: None,
            index: 0,
            device: key("lead"),
            param: CUTOFF.into(),
            min,
            max,
            invert,
        }),
    );
    d.doc().banks().next().unwrap().key.clone()
}

fn assignment(d: &Document) -> MacroAssign {
    d.doc().macros.assigns[0].clone()
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
    bounce_with(doc, 64, 0)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// Asserts two renders are the same bit for bit, naming the first frame that differs.
fn same(a: &[f32], b: &[f32], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: length");
    if let Some(i) = (0..a.len()).find(|&i| a[i].to_bits() != b[i].to_bits()) {
        panic!(
            "{what}: first difference at frame {i}: {} against {}",
            a[i], b[i]
        );
    }
}

fn cutoff_descriptor() -> atomik_core::ParamDescriptor {
    *default_registry()
        .get("atomik.poly-synth")
        .unwrap()
        .descriptor
        .param_by_key(CUTOFF)
        .unwrap()
}

/// What `d` would write by hand on the driven param for the macro lane `points`.
fn hand_made(d: &Document, points: &[AutomationPoint]) -> Vec<AutomationPoint> {
    let a = assignment(d);
    let p = cutoff_descriptor();
    points
        .iter()
        .map(|pt| AutomationPoint {
            value: assign_value(&a, pt.value, &p),
            ..*pt
        })
        .collect()
}

#[test]
fn automation_of_a_macro_renders_like_the_same_automation_on_the_param() {
    for (min, max, invert) in [
        (None, None, false),
        (Some(300.0), Some(2400.0), false),
        (Some(300.0), Some(2400.0), true),
        (Some(5000.0), Some(400.0), false),
    ] {
        let mut m = song();
        let bank = assign(&mut m, min, max, invert);
        lane_on(&mut m, bank.as_str(), "macro1", points());
        // Before its first point the lane has the value the macro gave the param at the start.
        let start = m
            .doc()
            .driven_value(&default_registry(), &key("lead"), CUTOFF)
            .unwrap();
        let mut hand = song();
        apply(
            &mut hand,
            Command::SetParam {
                key: key("lead"),
                param: CUTOFF.into(),
                value: start,
                merge: None,
            },
        );
        lane_on(&mut hand, "lead", CUTOFF, hand_made(&m, &points()));
        let a = bounce(m.doc());
        let b = bounce(hand.doc());
        assert!(a.iter().any(|s| *s != 0.0), "the song makes sound");
        same(&a, &b, &format!("{min:?} {max:?} {invert}"));
    }
}

#[test]
fn the_automated_macro_really_moves_the_sound() {
    // The lane is not a no-op: the same project without the lane sounds different.
    let mut m = song();
    let bank = assign(&mut m, Some(300.0), Some(2400.0), false);
    let still = bounce(m.doc());
    lane_on(&mut m, bank.as_str(), "macro1", points());
    assert!(
        bits(&still) != bits(&bounce(m.doc())),
        "the lane changes the render"
    );
}

#[test]
fn a_static_macro_renders_like_the_param_set_to_the_same_value() {
    for position in [0.0f32, 0.3, 0.6, 1.0] {
        let mut m = song();
        let bank = assign(&mut m, Some(300.0), Some(2400.0), true);
        apply(
            &mut m,
            Command::SetParam {
                key: bank,
                param: "macro1".into(),
                value: position,
                merge: None,
            },
        );
        let value = assign_value(&assignment(&m), position, &cutoff_descriptor());
        let mut hand = song();
        apply(
            &mut hand,
            Command::SetParam {
                key: key("lead"),
                param: CUTOFF.into(),
                value,
                merge: None,
            },
        );
        same(
            &bounce(m.doc()),
            &bounce(hand.doc()),
            &format!("position {position}"),
        );
    }
}

#[test]
fn a_bank_without_assignments_renders_like_the_project_without_it() {
    let plain = song();
    let mut with = song();
    apply(
        &mut with,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.macro-bank".into(),
            key: Some(key("bank")),
        },
    );
    apply(
        &mut with,
        Command::SetParam {
            key: key("bank"),
            param: "macro3".into(),
            value: 0.9,
            merge: None,
        },
    );
    same(
        &bounce(plain.doc()),
        &bounce(with.doc()),
        "a bank without assignments",
    );
}

#[test]
fn assigning_and_removing_leaves_the_render_as_it_was() {
    let plain = song();
    let mut m = song();
    assign(&mut m, None, None, false);
    let id = assignment(&m).id;
    apply(&mut m, Command::Macro(MacroCommand::Remove { id }));
    // The param keeps the value the macro gave it, which was the value it had.
    let before = bounce(plain.doc());
    let after = bounce(m.doc());
    same(&before, &after, "assigned and removed");
    // And an undo of the assignment is the original document.
    let mut n = song();
    let original = n.doc().clone();
    assign(&mut n, None, None, false);
    n.undo(&default_registry()).unwrap();
    assert_eq!(n.doc(), &original);
    same(&bounce(n.doc()), &before, "assigned and undone");
}

#[test]
fn the_render_does_not_depend_on_the_block_size_or_the_workers() {
    let mut m = song();
    let bank = assign(&mut m, Some(300.0), Some(2400.0), true);
    lane_on(&mut m, bank.as_str(), "macro1", points());
    let reference = bounce_with(m.doc(), 64, 0);
    for (block, workers) in [(128, 2), (333, 3), (1024, 1)] {
        same(
            &bounce_with(m.doc(), block, workers),
            &reference,
            &format!("block {block} workers {workers}"),
        );
    }
}

#[test]
fn a_saved_and_loaded_project_renders_the_same() {
    let mut m = song();
    let bank = assign(&mut m, Some(300.0), Some(2400.0), true);
    lane_on(&mut m, bank.as_str(), "macro1", points());
    let text = save_project(m.doc());
    let loaded = load_project(&text, &default_registry()).unwrap();
    assert_eq!(&loaded, m.doc());
    same(&bounce(&loaded), &bounce(m.doc()), "saved and loaded");
}

#[test]
fn a_version_28_file_without_macros_bounces_as_it_always_did() {
    let d = song();
    let text = save_project(d.doc());
    assert!(!text.contains("\"macros\""));
    let old = text.replace("\"version\": 37", "\"version\": 28");
    let loaded = load_project(&old, &default_registry()).unwrap();
    same(&bounce(&loaded), &bounce(d.doc()), "a version 28 file");
}
