//! Property tests of the rack columns: random sequences of the column and slot commands
//! (with the flat commands mixed in) over a document with a console, an instrument and a
//! container keep the column invariant after every accepted command, change nothing when a
//! command is rejected, round-trip through the project file, and undo and redo reproduce every
//! state byte for byte.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, DeviceSnapshot, Document, RackSlot, WorkflowCommand,
    load_project, new_project_doc, save_project,
};
use common::plugin::plugin_console_registry;
use common::*;
use proptest::prelude::*;

fn reg() -> Registry {
    plugin_console_registry()
}

fn base(r: &Registry) -> Document {
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        r,
        [
            add(1, DELAY, Some("fx1")),
            add(2, MIXER, Some("mix")),
            add(3, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "ix"),
            add_inner("c", 1, DELAY, "iy"),
            Command::AddRackColumn,
            Command::AddDeviceAt {
                slot: RackSlot {
                    column: 1,
                    index: 0,
                },
                type_id: DELAY.into(),
                key: Some(key("fx2")),
            },
        ],
    );
    Document::new(d.doc().clone())
}

#[derive(Debug, Clone)]
enum Op {
    AddFlat(usize),
    AddAt(u16, usize),
    MoveFlat(usize, usize),
    MoveSlot(Vec<usize>, u16, usize),
    MoveFlatGroup(Vec<usize>, usize),
    AddColumn,
    RemoveColumn(u16),
    Remove(usize),
    PasteAt(Vec<usize>, u16, usize),
    PasteFlat(Vec<usize>, usize),
    Duplicate(Vec<usize>),
    EffectAt(u16, usize),
    InstrumentAt(u16, usize),
    InnerMove(usize, usize),
}

fn op() -> impl Strategy<Value = Op> {
    let picks = || proptest::collection::vec(0usize..12, 1..4);
    prop_oneof![
        (0usize..14).prop_map(Op::AddFlat),
        (0u16..5, 0usize..8).prop_map(|(c, i)| Op::AddAt(c, i)),
        (0usize..12, 0usize..14).prop_map(|(k, t)| Op::MoveFlat(k, t)),
        (picks(), 0u16..5, 0usize..8).prop_map(|(k, c, i)| Op::MoveSlot(k, c, i)),
        (picks(), 0usize..14).prop_map(|(k, t)| Op::MoveFlatGroup(k, t)),
        Just(Op::AddColumn),
        (0u16..5).prop_map(Op::RemoveColumn),
        (0usize..12).prop_map(Op::Remove),
        (picks(), 0u16..5, 0usize..8).prop_map(|(k, c, i)| Op::PasteAt(k, c, i)),
        (picks(), 0usize..14).prop_map(|(k, a)| Op::PasteFlat(k, a)),
        picks().prop_map(Op::Duplicate),
        (0u16..5, 0usize..8).prop_map(|(c, i)| Op::EffectAt(c, i)),
        (0u16..5, 0usize..8).prop_map(|(c, i)| Op::InstrumentAt(c, i)),
        (0usize..4, 0usize..4).prop_map(|(a, b)| Op::InnerMove(a, b)),
    ]
}

fn top_keys(d: &Document, picks: &[usize]) -> Vec<atomik_model::DeviceKey> {
    let all = &d.doc().devices;
    let mut out: Vec<atomik_model::DeviceKey> = picks
        .iter()
        .map(|p| all[p % all.len()].key.clone())
        .collect();
    out.dedup();
    out
}

fn command(d: &Document, op: &Op) -> Option<Command> {
    let slot = |column, index| RackSlot { column, index };
    let n = d.doc().devices.len();
    Some(match op {
        Op::AddFlat(i) => Command::AddDevice {
            index: *i,
            type_id: DELAY.into(),
            key: None,
        },
        Op::AddAt(c, i) => Command::AddDeviceAt {
            slot: slot(*c, *i),
            type_id: DELAY.into(),
            key: None,
        },
        Op::MoveFlat(k, to) => Command::MoveDevice {
            key: d.doc().devices[k % n].key.clone(),
            to: *to,
        },
        Op::MoveSlot(k, c, i) => wf_cmd(WorkflowCommand::MoveDevicesToSlot {
            route: false,
            keys: top_keys(d, k),
            slot: slot(*c, *i),
        }),
        Op::MoveFlatGroup(k, to) => wf_cmd(WorkflowCommand::MoveDevices {
            route: false,
            keys: top_keys(d, k),
            to: *to,
        }),
        Op::AddColumn => Command::AddRackColumn,
        Op::RemoveColumn(c) => Command::RemoveRackColumn { column: *c },
        Op::Remove(k) => Command::RemoveDevice {
            key: d.doc().devices[k % n].key.clone(),
        },
        Op::PasteAt(k, c, i) => {
            let s = DeviceSnapshot::capture(d.doc(), &top_keys(d, k), false).ok()?;
            wf_cmd(WorkflowCommand::PasteDevicesAt {
                route: false,
                snapshot: s,
                container: None,
                slot: slot(*c, *i),
            })
        }
        Op::PasteFlat(k, at) => {
            let s = DeviceSnapshot::capture(d.doc(), &top_keys(d, k), false).ok()?;
            wf_cmd(WorkflowCommand::PasteDevices {
                route: false,
                snapshot: s,
                container: None,
                at: *at,
            })
        }
        Op::Duplicate(k) => wf_cmd(WorkflowCommand::DuplicateDevices {
            route: false,
            keys: top_keys(d, k),
        }),
        Op::EffectAt(c, i) => wf_cmd(WorkflowCommand::CreateEffectAt {
            slot: slot(*c, *i),
            source: CreateSource::Type(DELAY.into()),
            key: None,
            options: CreateOptions::default(),
        }),
        Op::InstrumentAt(c, i) => wf_cmd(WorkflowCommand::CreateInstrumentAt {
            slot: slot(*c, *i),
            source: CreateSource::Type(SYNTH.into()),
            key: None,
            options: CreateOptions::default(),
        }),
        Op::InnerMove(a, b) => wf_cmd(WorkflowCommand::MoveDevicesToSlot {
            route: false,
            keys: vec![key(["c/ix", "c/iy"][a % 2])],
            slot: slot(3, *b),
        }),
    })
}

fn wf_cmd(c: WorkflowCommand) -> Command {
    Command::Workflow(c)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn columns_stay_sorted_and_every_step_undoes_redoes_and_round_trips(
        ops in proptest::collection::vec(op(), 1..28)
    ) {
        let r = reg();
        let mut d = base(&r);
        d.doc().check_columns().unwrap();
        let mut states = vec![save_project(d.doc())];
        for op in &ops {
            let Some(c) = command(&d, op) else { continue };
            let before = save_project(d.doc());
            match d.apply(c, &r) {
                Ok(_) => {
                    if let Err(e) = d.doc().check_columns() {
                        prop_assert!(false, "{op:?} broke the invariant: {e}");
                    }
                    let text = save_project(d.doc());
                    let back = load_project(&text, &r).unwrap();
                    prop_assert_eq!(save_project(&back), text.clone());
                    prop_assert_eq!(&back.devices, &d.doc().devices);
                    prop_assert_eq!(back.columns, d.doc().columns);
                    states.push(text);
                }
                Err(_) => prop_assert_eq!(save_project(d.doc()), before),
            }
        }
        let last = d.doc().clone();
        for want in states.iter().rev().skip(1) {
            d.undo(&r).unwrap();
            d.doc().check_columns().unwrap();
            prop_assert_eq!(&save_project(d.doc()), want);
        }
        prop_assert!(!d.can_undo());
        for want in states.iter().skip(1) {
            d.redo(&r).unwrap();
            d.doc().check_columns().unwrap();
            prop_assert_eq!(&save_project(d.doc()), want);
        }
        prop_assert_eq!(d.doc(), &last);
    }
}
