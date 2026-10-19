//! Property test of the master lock: over random command sequences on a new
//! project the master is never lost and never leaves index 0, every accepted command is one
//! undo step with identical text after undo and redo, a rejection changes nothing, undoing
//! everything returns the starter byte for byte, and every state loads back.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, DeviceSnapshot,
    Document, WorkflowCommand, load_project, new_project_doc, save_project,
};
use common::workflow::*;
use common::*;
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Add(usize, usize),
    Instrument(usize),
    Effect(usize, Option<usize>),
    Remove(usize),
    RemoveMany(Vec<usize>),
    DeleteMany(Vec<usize>),
    Move(usize, usize),
    MoveMany(Vec<usize>, usize),
    Duplicate(Vec<usize>),
    Paste(Vec<usize>, usize),
    RemoveChannel(usize),
    Undo,
    Redo,
}

fn sel() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..12, 1..4)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..4, 0usize..8).prop_map(|(t, i)| Op::Add(t, i)),
        (0usize..8).prop_map(Op::Instrument),
        (0usize..2, proptest::option::of(0usize..12)).prop_map(|(t, s)| Op::Effect(t, s)),
        (0usize..12).prop_map(Op::Remove),
        sel().prop_map(Op::RemoveMany),
        sel().prop_map(Op::DeleteMany),
        (0usize..12, 0usize..12).prop_map(|(k, to)| Op::Move(k, to)),
        (sel(), 0usize..12).prop_map(|(k, to)| Op::MoveMany(k, to)),
        sel().prop_map(Op::Duplicate),
        (sel(), 0usize..12).prop_map(|(k, at)| Op::Paste(k, at)),
        (0usize..6).prop_map(Op::RemoveChannel),
        Just(Op::Undo),
        Just(Op::Redo),
    ]
}

fn top(d: &Document) -> Vec<DeviceKey> {
    d.doc().devices.iter().map(|x| x.key.clone()).collect()
}

fn pick<T: Clone>(list: &[T], i: usize) -> Option<T> {
    (!list.is_empty()).then(|| list[i % list.len()].clone())
}

fn chosen(list: &[DeviceKey], sel: &[usize]) -> Vec<DeviceKey> {
    sel.iter().filter_map(|i| pick(list, *i)).collect()
}

fn resolve(op: &Op, d: &Document) -> Option<Command> {
    let keys = top(d);
    Some(match op {
        Op::Add(t, i) => add(*i, [SYNTH, DELAY, MIXER, CONTAINER][*t], None),
        Op::Instrument(i) => wf(WorkflowCommand::CreateInstrument {
            index: *i,
            source: CreateSource::Type(SYNTH.into()),
            key: None,
            options: CreateOptions::default(),
        }),
        Op::Effect(t, s) => wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Type([DELAY, "atomik.reverb"][*t].into()),
            target: s
                .and_then(|i| pick(&keys, i))
                .map_or(CreateTarget::Nothing, CreateTarget::Selected),
            key: None,
            options: CreateOptions::default(),
        }),
        Op::Remove(i) => Command::RemoveDevice {
            key: pick(&keys, *i)?,
        },
        Op::RemoveMany(s) => wf(WorkflowCommand::RemoveDevices {
            keys: chosen(&keys, s),
        }),
        Op::DeleteMany(s) => wf(WorkflowCommand::DeleteDevices {
            keys: chosen(&keys, s),
        }),
        Op::Move(k, to) => Command::MoveDevice {
            key: pick(&keys, *k)?,
            to: *to,
        },
        Op::MoveMany(s, to) => wf(WorkflowCommand::MoveDevices {
            keys: chosen(&keys, s),
            to: *to,
        }),
        Op::Duplicate(s) => wf(WorkflowCommand::DuplicateDevices {
            keys: chosen(&keys, s),
        }),
        Op::Paste(s, at) => wf(WorkflowCommand::PasteDevices {
            snapshot: DeviceSnapshot::capture(d.doc(), &chosen(&keys, s), false).ok()?,
            container: None,
            at: *at,
        }),
        Op::RemoveChannel(i) => {
            let channels: Vec<DeviceKey> = d
                .doc()
                .console
                .channels
                .iter()
                .map(|c| c.channel.clone())
                .collect();
            Command::Console(ConsoleCommand::RemoveChannel {
                channel: pick(&channels, *i)?,
            })
        }
        Op::Undo | Op::Redo => return None,
    })
}

fn master_kept(d: &Document) {
    let doc = d.doc();
    assert_eq!(doc.console.master, Some(key("master")), "console master");
    assert_eq!(doc.devices[0].key, key("master"), "the master stays on top");
    assert_eq!(
        doc.devices.iter().filter(|x| x.type_id == MASTER).count(),
        1,
        "exactly one master"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn the_master_is_never_lost_or_moved(ops in proptest::collection::vec(op(), 1..28)) {
        let r: Registry = console_registry();
        let mut d = Document::new(new_project_doc());
        let start = text(&d);
        master_kept(&d);
        for o in &ops {
            match o {
                Op::Undo => { d.undo(&r); }
                Op::Redo => { d.redo(&r); }
                _ => {
                    let Some(cmd) = resolve(o, &d) else { continue };
                    let before = text(&d);
                    match d.apply(cmd, &r) {
                        Ok(_) => {
                            let after = text(&d);
                            d.undo(&r).expect("a step to undo");
                            prop_assert_eq!(text(&d), before);
                            d.redo(&r).expect("a step to redo");
                            prop_assert_eq!(text(&d), after.clone());
                            let loaded = load_project(&after, &r).expect("a saved state loads");
                            prop_assert_eq!(save_project(&loaded), after);
                        }
                        Err(_) => prop_assert_eq!(text(&d), before, "a rejection changes nothing"),
                    }
                }
            }
            master_kept(&d);
        }
        while d.undo(&r).is_some() {}
        prop_assert_eq!(text(&d), start);
        master_kept(&d);
    }
}
