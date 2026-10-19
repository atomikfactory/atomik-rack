//! Property test of the creation chain: random sequences of
//! create, auto-route, disconnect, delete (with bridging) and a few plain edits. Every accepted
//! command is one undo step, every rejected one changes nothing, undoing everything gives the
//! starting text byte for byte, redo reproduces each state, a replay on a fresh document gives
//! the same texts, and every state loads back to the same text.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    SongCommand, WorkflowCommand, empty_doc, load_project, save_project,
};
use common::plugin::*;
use common::song::assert_tempo_invariant;
use common::workflow::*;
use common::*;
use proptest::prelude::*;

fn reg() -> Registry {
    plugin_console_registry()
}

fn base(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            wf(WorkflowCommand::CreateInstrument {
                index: 0,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key("synth")),
                options: CreateOptions::default(),
            }),
            add(usize::MAX, DELAY, Some("delay")),
            add(usize::MAX, "atomik.reverb", Some("verb")),
        ],
    );
    Document::new(d.doc().clone())
}

#[derive(Debug, Clone)]
enum Op {
    Instrument(usize, usize, bool, bool, Vec<usize>),
    Effect(usize, Option<usize>, bool),
    PluginEffect(Option<usize>),
    PluginInstrument,
    AutoRoute(Vec<usize>),
    Disconnect(Vec<usize>),
    Delete(Vec<usize>),
    Remove(usize),
    RemoveChannel(usize),
    Arm(usize, bool),
    AddDevice(usize),
    Connect(usize, usize),
    SetReturn(usize, usize),
}

fn sel() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..16, 1..4)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..4, 0usize..6, any::<bool>(), any::<bool>(), sel())
            .prop_map(|(t, i, r, s, d)| Op::Instrument(t, i, r, s, d)),
        (0usize..4, proptest::option::of(0usize..16), any::<bool>())
            .prop_map(|(t, s, r)| Op::Effect(t, s, r)),
        proptest::option::of(0usize..16).prop_map(Op::PluginEffect),
        Just(Op::PluginInstrument),
        sel().prop_map(Op::AutoRoute),
        sel().prop_map(Op::Disconnect),
        sel().prop_map(Op::Delete),
        (0usize..16).prop_map(Op::Remove),
        (0usize..4).prop_map(Op::RemoveChannel),
        (0usize..16, any::<bool>()).prop_map(|(i, a)| Op::Arm(i, a)),
        (0usize..4).prop_map(Op::AddDevice),
        (0usize..16, 0usize..16).prop_map(|(a, b)| Op::Connect(a, b)),
        (0usize..8, 0usize..16).prop_map(|(s, e)| Op::SetReturn(s, e)),
    ]
}

fn pick<T: Clone>(list: &[T], i: usize) -> Option<T> {
    (!list.is_empty()).then(|| list[i % list.len()].clone())
}

fn top(d: &Document) -> Vec<DeviceKey> {
    d.doc().devices.iter().map(|x| x.key.clone()).collect()
}

fn chosen(list: &[DeviceKey], sel: &[usize]) -> Vec<DeviceKey> {
    sel.iter().filter_map(|i| pick(list, *i)).collect()
}

const INSTRUMENTS: [&str; 4] = [
    SYNTH,
    "test.silent-synth",
    "test.audio-only-synth",
    "atomik.sampler",
];
const EFFECTS: [&str; 4] = [DELAY, "atomik.reverb", MIXER, "atomik.compressor"];

fn resolve(op: &Op, d: &Document) -> Option<Command> {
    let keys = top(d);
    Some(match op {
        Op::Instrument(t, i, route, skip, dis) => wf(WorkflowCommand::CreateInstrument {
            index: *i,
            source: CreateSource::Type(INSTRUMENTS[*t].into()),
            key: None,
            options: CreateOptions {
                route: *route,
                skip_track: *skip,
                disarm: chosen(&keys, dis),
            },
        }),
        Op::Effect(t, s, route) => wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Type(EFFECTS[*t].into()),
            target: s
                .and_then(|i| pick(&keys, i))
                .map_or(CreateTarget::Nothing, CreateTarget::Selected),
            key: None,
            options: CreateOptions {
                route: *route,
                ..CreateOptions::default()
            },
        }),
        Op::PluginEffect(s) => wf(WorkflowCommand::CreateEffect {
            source: CreateSource::Plugin {
                plugin: fx_ref(),
                layout: fx_layout(),
                path_hint: None,
            },
            target: s
                .and_then(|i| pick(&keys, i))
                .map_or(CreateTarget::Nothing, CreateTarget::Selected),
            key: None,
            options: CreateOptions::default(),
        }),
        Op::PluginInstrument => wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Plugin {
                plugin: synth_ref(),
                layout: synth_layout(),
                path_hint: None,
            },
            key: None,
            options: CreateOptions::default(),
        }),
        Op::AutoRoute(s) => wf(WorkflowCommand::AutoRoute {
            devices: chosen(&keys, s),
            options: CreateOptions::default(),
        }),
        Op::Disconnect(s) => wf(WorkflowCommand::DisconnectDevices {
            keys: chosen(&keys, s),
        }),
        Op::Delete(s) => wf(WorkflowCommand::DeleteDevices {
            keys: chosen(&keys, s),
        }),
        Op::Remove(i) => wf(WorkflowCommand::DeleteDevices {
            keys: vec![pick(&keys, *i)?],
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
        Op::Arm(i, armed) => {
            let tracks: Vec<DeviceKey> = d
                .doc()
                .song
                .tracks
                .iter()
                .map(|t| t.device.clone())
                .collect();
            Command::Song(SongCommand::SetTrackFlags {
                device: pick(&tracks, *i)?,
                muted: false,
                solo: false,
                armed: *armed,
            })
        }
        Op::AddDevice(t) => add(usize::MAX, [SYNTH, DELAY, CONTAINER, MIXER][*t], None),
        Op::Connect(a, b) => {
            let (from, to) = (pick(&keys, *a)?, pick(&keys, *b)?);
            Command::Connect(cable(&format!("{from}:out_l"), &format!("{to}:in_l")))
        }
        Op::SetReturn(slot, e) => Command::Console(ConsoleCommand::SetReturn {
            slot: *slot,
            effect: pick(&keys, *e),
        }),
    })
}

fn invariants(d: &Document, r: &Registry) {
    d.doc().check_containers(r).expect("containers stay valid");
    let doc = d.doc();
    for t in &doc.song.tracks {
        assert!(doc.device(&t.device).is_some(), "track of a missing device");
    }
    for c in &doc.console.channels {
        assert!(doc.device(&c.channel).is_some());
        if let Some(s) = &c.source {
            assert!(doc.device(s).is_some(), "channel source exists");
        }
    }
    for c in &doc.cables {
        for e in [&c.from.device, &c.to.device] {
            assert!(
                e.0 == "interface" || doc.device(e).is_some(),
                "cable end {e} exists"
            );
        }
    }
    assert_tempo_invariant(doc);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn creation_sequences_undo_and_redo_byte_for_byte(ops in proptest::collection::vec(op(), 1..24)) {
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
                    let after = text(&d);
                    d.undo(&r).expect("a step to undo");
                    prop_assert_eq!(text(&d), before.clone());
                    d.redo(&r).expect("a step to redo");
                    prop_assert_eq!(text(&d), after.clone());
                    let loaded = load_project(&after, &r).expect("a saved state loads");
                    prop_assert_eq!(save_project(&loaded), after.clone());
                    states.push(after);
                    accepted.push(cmd);
                }
                Err(_) => prop_assert_eq!(text(&d), before, "a rejected command changes nothing"),
            }
        }
        for k in (0..accepted.len()).rev() {
            d.undo(&r).expect("undo");
            prop_assert_eq!(text(&d), states[k].clone(), "undo to state {}", k);
        }
        prop_assert_eq!(text(&d), text(&start));
        prop_assert!(!d.can_undo());
        for (k, s) in states.iter().enumerate().skip(1) {
            d.redo(&r).expect("redo");
            prop_assert_eq!(text(&d), s.clone(), "redo to state {}", k);
        }
        let mut fresh = Document::new(start.doc().clone());
        for (k, cmd) in accepted.iter().enumerate() {
            fresh.apply(cmd.clone(), &r).expect("replay");
            prop_assert_eq!(text(&fresh), states[k + 1].clone(), "replay of step {}", k);
        }
    }
}
