//! Property tests of the workflow commands: random
//! sequences of clipboard and multi-device commands over a document with a console, tracks,
//! a container and plugins. Every accepted command is one undo step, every rejected one
//! changes nothing, undoing everything reproduces the starting text byte for byte, redoing
//! reproduces each state, a replay on a fresh document gives the same texts, and every state
//! loads back to the same text.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipId, ClipSnapshot, Command, DeviceKey, DeviceSnapshot, Document, NoteSnapshot,
    WorkflowCommand, empty_doc, load_project, save_project,
};
use common::plugin::*;
use common::song::*;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

fn reg() -> Registry {
    plugin_console_registry()
}

/// A rich starting document.
fn base(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, DELAY, Some("delay")),
            connect("synth:out_l", "delay:in_l"),
            connect("synth:out_r", "delay:in_r"),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(
                    BAR,
                    BAR,
                    vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.6)],
                ),
            ),
            add_clip(
                "synth",
                0,
                notes_clip(3 * BAR, BAR, vec![note(0, PPQ, 67, 0.5)]),
            ),
            add(2, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "isynth"),
            add_inner("c", 1, DELAY, "idelay"),
            connect("c/isynth:out_l", "c/idelay:in_l"),
            add_plugin(3, &synth_ref(), &synth_layout(), Some("ps")),
            add_track("ps"),
            add_clip("ps", 0, notes_clip(0, 2 * BAR, vec![note(0, PPQ, 50, 1.0)])),
            sync_state("ps", &[9, 8, 7]),
        ],
    );
    Document::new(d.doc().clone())
}

#[derive(Debug, Clone)]
enum Op {
    Duplicate(Vec<usize>, bool),
    Remove(Vec<usize>),
    Move(Vec<usize>, usize),
    Disconnect(usize),
    DuplicateTrack(usize),
    PasteDevices(Vec<usize>, usize, bool),
    PasteClips(Vec<usize>, i64, Option<usize>),
    DuplicateClips(Vec<usize>),
    Join(Vec<usize>),
    PasteNotes(usize, Vec<usize>, i64),
    DuplicateNotes(usize, Vec<usize>),
    AddDevice(usize),
    Connect(usize, usize),
}

fn sel() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..12, 1..4)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (sel(), any::<bool>()).prop_map(|(s, i)| Op::Duplicate(s, i)),
        sel().prop_map(Op::Remove),
        (sel(), 0usize..12).prop_map(|(s, t)| Op::Move(s, t)),
        (0usize..12).prop_map(Op::Disconnect),
        (0usize..12).prop_map(Op::DuplicateTrack),
        (sel(), 0usize..12, any::<bool>()).prop_map(|(s, a, i)| Op::PasteDevices(s, a, i)),
        (sel(), 0i64..12, proptest::option::of(0usize..3)).prop_map(|(s, b, t)| Op::PasteClips(
            s,
            b * BAR / 2,
            t
        )),
        sel().prop_map(Op::DuplicateClips),
        sel().prop_map(Op::Join),
        (0usize..6, sel(), 0i64..16).prop_map(|(c, n, t)| Op::PasteNotes(c, n, t * PPQ)),
        (0usize..6, sel()).prop_map(|(c, n)| Op::DuplicateNotes(c, n)),
        (0usize..4).prop_map(Op::AddDevice),
        (0usize..12, 0usize..12).prop_map(|(a, b)| Op::Connect(a, b)),
    ]
}

fn pick<T: Clone>(list: &[T], i: usize) -> Option<T> {
    (!list.is_empty()).then(|| list[i % list.len()].clone())
}

/// Top-level keys, or the inner keys of the first container when `inner`.
fn scope_keys(d: &Document, inner: bool) -> Vec<DeviceKey> {
    let doc = d.doc();
    if inner {
        doc.devices
            .iter()
            .find(|x| x.inner.is_some())
            .map(|c| {
                c.inner
                    .as_ref()
                    .unwrap()
                    .devices
                    .iter()
                    .map(|x| key(&format!("{}/{}", c.key, x.key)))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        doc.devices.iter().map(|x| x.key.clone()).collect()
    }
}

/// [`scope_keys`] without the console devices, which are never copied.
fn copyable_keys(d: &Document, inner: bool) -> Vec<DeviceKey> {
    let doc = d.doc();
    scope_keys(d, inner)
        .into_iter()
        .filter(|k| {
            doc.device(k)
                .is_none_or(|x| x.type_id != CHANNEL && x.type_id != MASTER)
        })
        .collect()
}

fn chosen(list: &[DeviceKey], sel: &[usize]) -> Vec<DeviceKey> {
    sel.iter().filter_map(|i| pick(list, *i)).collect()
}

fn clip_list(d: &Document) -> Vec<ClipId> {
    clip_ids(d.doc())
}

fn resolve(op: &Op, d: &Document) -> Option<Command> {
    let top = scope_keys(d, false);
    Some(match op {
        Op::Duplicate(s, inner) => {
            let list = copyable_keys(d, *inner);
            let keys = chosen(&list, s);
            if keys.is_empty() {
                return None;
            }
            wf(WorkflowCommand::DuplicateDevices { keys, route: false })
        }
        Op::Remove(s) => wf(WorkflowCommand::RemoveDevices {
            keys: chosen(&top, s),
        }),
        Op::Move(s, to) => wf(WorkflowCommand::MoveDevices {
            route: false,
            keys: chosen(&top, s),
            to: *to,
        }),
        Op::Disconnect(i) => wf(WorkflowCommand::DisconnectDevice {
            key: pick(&top, *i)?,
        }),
        Op::DuplicateTrack(i) => {
            let tracks: Vec<DeviceKey> = d
                .doc()
                .song
                .tracks
                .iter()
                .map(|t| t.device.clone())
                .collect();
            wf(WorkflowCommand::DuplicateTrack {
                device: pick(&tracks, *i)?,
            })
        }
        Op::PasteDevices(s, at, inner) => {
            let list = copyable_keys(d, *inner);
            let keys = chosen(&list, s);
            let snapshot = DeviceSnapshot::capture(d.doc(), &keys, !*inner && at % 2 == 0).ok()?;
            let container = (*inner).then(|| key("c"));
            wf(WorkflowCommand::PasteDevices {
                route: false,
                snapshot,
                container,
                at: *at,
            })
        }
        Op::PasteClips(s, at, track) => {
            let all = clip_list(d);
            let ids: Vec<ClipId> = s.iter().filter_map(|i| pick(&all, *i)).collect();
            let clips = ClipSnapshot::capture(d.doc(), &ids).ok()?;
            let tracks: Vec<DeviceKey> = d
                .doc()
                .song
                .tracks
                .iter()
                .map(|t| t.device.clone())
                .collect();
            wf(WorkflowCommand::PasteClips {
                track: track.and_then(|i| pick(&tracks, i)),
                lane: None,
                at_tick: *at,
                clips,
            })
        }
        Op::DuplicateClips(s) => {
            let all = clip_list(d);
            wf(WorkflowCommand::DuplicateClips {
                ids: s.iter().filter_map(|i| pick(&all, *i)).collect(),
            })
        }
        Op::Join(s) => {
            let all = clip_list(d);
            wf(WorkflowCommand::JoinClips {
                ids: s.iter().filter_map(|i| pick(&all, *i)).collect(),
            })
        }
        Op::PasteNotes(c, n, at) => {
            let all = clip_list(d);
            let id = pick(&all, *c)?;
            let (_, _, clip) = d.doc().song.clip(id)?;
            let notes = NoteSnapshot::capture(clip, n).ok()?;
            wf(WorkflowCommand::PasteNotes {
                clip: id,
                at_tick: *at,
                notes,
            })
        }
        Op::DuplicateNotes(c, n) => {
            let all = clip_list(d);
            wf(WorkflowCommand::DuplicateNotes {
                clip: pick(&all, *c)?,
                indices: n.clone(),
            })
        }
        Op::AddDevice(t) => add(usize::MAX, [SYNTH, DELAY, CONTAINER, SAMPLER][*t], None),
        Op::Connect(a, b) => {
            let (from, to) = (pick(&top, *a)?, pick(&top, *b)?);
            Command::Connect(cable(&format!("{from}:out_l"), &format!("{to}:in_l")))
        }
    })
}

fn invariants(d: &Document, r: &Registry) {
    d.doc().check_containers(r).expect("containers stay valid");
    let doc = d.doc();
    // Every track belongs to a device, every console channel to a device.
    for t in &doc.song.tracks {
        assert!(doc.device(&t.device).is_some(), "track of a missing device");
    }
    for c in &doc.console.channels {
        assert!(doc.device(&c.channel).is_some());
    }
    // Clip ids are unique and below the next id.
    let ids = clip_ids(doc);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "clip ids are unique");
    assert!(ids.iter().all(|i| i.0 < doc.song.next_clip_id));
    assert_tempo_invariant(doc);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn workflow_sequences_undo_and_redo_byte_for_byte(ops in proptest::collection::vec(op(), 1..22)) {
        let r = reg();
        let start = base(&r);
        let mut d = Document::new(start.doc().clone());
        let mut states = vec![text(&d)];
        let mut accepted: Vec<Command> = Vec::new();
        for o in &ops {
            let Some(cmd) = resolve(o, &d) else { continue };
            let before = text(&d);
            match d.apply(cmd.clone(), &r) {
                Ok(_) => {
                    invariants(&d, &r);
                    // One undo step, and redo gives the same bytes.
                    let after = text(&d);
                    d.undo(&r).expect("a step to undo");
                    prop_assert_eq!(text(&d), before.clone());
                    d.redo(&r).expect("a step to redo");
                    prop_assert_eq!(text(&d), after.clone());
                    // The saved text loads back to the same text (the format is unchanged).
                    let loaded = load_project(&after, &r).expect("a saved state loads");
                    prop_assert_eq!(save_project(&loaded), after.clone());
                    states.push(after);
                    accepted.push(cmd);
                }
                Err(_) => prop_assert_eq!(text(&d), before, "a rejected command changes nothing"),
            }
        }
        // Undo everything: each state comes back.
        for k in (0..accepted.len()).rev() {
            d.undo(&r).expect("undo");
            prop_assert_eq!(text(&d), states[k].clone(), "undo to state {}", k);
        }
        prop_assert_eq!(text(&d), text(&start));
        prop_assert!(!d.can_undo());
        // Redo everything: every state equals the first-time state.
        for (k, s) in states.iter().enumerate().skip(1) {
            d.redo(&r).expect("redo");
            prop_assert_eq!(text(&d), s.clone(), "redo to state {}", k);
        }
        // A replay on a fresh document gives the very same texts (no hidden state).
        let mut fresh = Document::new(start.doc().clone());
        for (k, cmd) in accepted.iter().enumerate() {
            fresh.apply(cmd.clone(), &r).expect("replay");
            prop_assert_eq!(text(&fresh), states[k + 1].clone(), "replay of step {}", k);
        }
    }
}

#[test]
fn a_fixed_mix_of_commands_survives_undo_all_and_redo_all() {
    // A deterministic sequence that hits every command once, for a quick regression signal.
    let r = reg();
    let start = base(&r);
    let mut d = Document::new(start.doc().clone());
    let mut states = vec![text(&d)];
    let sequence = [
        Op::Duplicate(vec![0, 1], false),
        Op::DuplicateTrack(0),
        Op::PasteDevices(vec![2], 0, false),
        Op::PasteClips(vec![0, 1], BAR, None),
        Op::DuplicateClips(vec![0]),
        Op::Join(vec![0, 1]),
        Op::PasteNotes(0, vec![0], 5 * PPQ),
        Op::DuplicateNotes(0, vec![0]),
        Op::Move(vec![0], 5),
        Op::Disconnect(1),
        Op::Duplicate(vec![0], true),
        Op::PasteDevices(vec![0], 0, true),
        Op::Remove(vec![1, 2]),
    ];
    for o in &sequence {
        let cmd = resolve(o, &d).unwrap_or_else(|| panic!("{o:?} resolves in {:?}", keys(&d)));
        d.apply(cmd.clone(), &r)
            .unwrap_or_else(|e| panic!("{o:?}: {cmd:?} rejected: {e}"));
        invariants(&d, &r);
        states.push(text(&d));
    }
    for k in (0..sequence.len()).rev() {
        d.undo(&r).unwrap();
        assert_eq!(text(&d), states[k]);
    }
    for (k, s) in states.iter().enumerate().skip(1) {
        d.redo(&r).unwrap();
        assert_eq!(&text(&d), s, "redo {k}");
    }
}
