//! Random bus edits: whatever is accepted or refused, the console invariants hold, bus routing
//! never loops, every accepted command is one undo step, and undoing everything restores the
//! starting document (redoing everything restores the end).

mod common;

use atomik_model::console::{OutputDest, output_dest};
use atomik_model::{Command, ConsoleCommand, Document, new_project_doc};
use common::console_ops::assert_console_invariants;
use common::*;
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Create(u8, u8),
    Rename(u8, u8),
    Delete(u8),
    Reorder(u8, u8),
    Output(u8, u8),
    Route(Vec<u8>, u8),
    Remove(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..6, 0u8..8).prop_map(|(n, d)| Op::Create(n, d)),
        (0u8..8, 0u8..6).prop_map(|(b, n)| Op::Rename(b, n)),
        (0u8..8).prop_map(Op::Delete),
        (0u8..8, 0u8..8).prop_map(|(b, t)| Op::Reorder(b, t)),
        (0u8..12, 0u8..12).prop_map(|(s, d)| Op::Output(s, d)),
        (prop::collection::vec(0u8..12, 0..4), 0u8..6).prop_map(|(s, n)| Op::Route(s, n)),
        (0u8..8).prop_map(Op::Remove),
    ]
}

fn strip(doc: &atomik_model::RackDoc, i: u8) -> atomik_model::DeviceKey {
    let all: Vec<_> = doc
        .console
        .channels
        .iter()
        .map(|c| c.channel.clone())
        .chain(doc.console.buses.iter().map(|b| b.bus.clone()))
        .collect();
    all[usize::from(i) % all.len()].clone()
}

fn dest(doc: &atomik_model::RackDoc, d: u8) -> OutputDest {
    match d % 4 {
        0 => OutputDest::Master,
        1 if !doc.console.buses.is_empty() => OutputDest::Bus(
            doc.console.buses[usize::from(d / 4) % doc.console.buses.len()]
                .bus
                .clone(),
        ),
        _ => OutputDest::AudioIo(u16::from(d / 4) % 5),
    }
}

fn command(doc: &atomik_model::RackDoc, op: &Op) -> Command {
    let name = |n: &u8| format!("Bus {n}");
    let bus_key = |i: &u8| {
        if doc.console.buses.is_empty() {
            key("none")
        } else {
            doc.console.buses[usize::from(*i) % doc.console.buses.len()]
                .bus
                .clone()
        }
    };
    Command::Console(match op {
        Op::Create(n, d) => ConsoleCommand::CreateBus {
            name: name(n),
            dest: dest(doc, *d),
        },
        Op::Rename(b, n) => ConsoleCommand::RenameBus {
            bus: bus_key(b),
            name: name(n),
        },
        Op::Delete(b) => ConsoleCommand::DeleteBus { bus: bus_key(b) },
        Op::Reorder(b, t) => ConsoleCommand::ReorderBus {
            bus: bus_key(b),
            to: usize::from(*t),
        },
        Op::Output(s, d) => ConsoleCommand::SetOutput {
            strip: strip(doc, *s),
            dest: dest(doc, *d),
        },
        Op::Route(s, n) => ConsoleCommand::RouteToNewBus {
            strips: s.iter().map(|i| strip(doc, *i)).collect(),
            name: name(n),
        },
        Op::Remove(b) => return Command::RemoveDevice { key: bus_key(b) },
    })
}

fn acyclic(doc: &atomik_model::RackDoc) {
    for b in &doc.console.buses {
        let mut at = b.bus.clone();
        for _ in 0..=doc.console.buses.len() {
            match output_dest(doc, &at) {
                OutputDest::Bus(next) => at = next,
                _ => break,
            }
        }
        assert!(
            !matches!(output_dest(doc, &at), OutputDest::Bus(_)),
            "bus routing loops from {}",
            b.bus
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_bus_edits_keep_the_console_valid_and_undoable(ops in prop::collection::vec(op(), 0..14)) {
        let r = console_registry();
        let mut start = new_project_doc();
        start.output_channels = 8;
        let mut d = Document::new(start);
        run(&mut d, &r, [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, SYNTH, Some("b")),
            add(usize::MAX, SYNTH, Some("c")),
        ]);
        let base = d.doc().clone();
        let mut accepted = 0;
        for op in &ops {
            let c = command(d.doc(), op);
            let before = d.doc().clone();
            match d.apply(c, &r) {
                Ok(_) => accepted += 1,
                Err(_) => prop_assert_eq!(d.doc(), &before),
            }
            assert_console_invariants(d.doc());
            acyclic(d.doc());
        }
        let end = d.doc().clone();
        // The three setup steps stay; undo only the accepted ones.
        for _ in 0..accepted {
            prop_assert!(d.undo(&r).is_some());
        }
        prop_assert_eq!(d.doc(), &base);
        for _ in 0..accepted {
            prop_assert!(d.redo(&r).is_some());
        }
        prop_assert_eq!(d.doc(), &end);
    }
}
