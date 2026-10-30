//! (proptest): any command sequence applied then fully undone gives back the original
//! document; redoing everything gives the final one. `SyncOp`s replayed onto an engine mirror
//! keep it equal to the document after any apply/undo/redo sequence.
//!
//! Half the runs use a registry with the console devices, so instruments get
//! automatic channels and console commands join the mix. The mirror re-reads the console
//! and recomputes solo silence only on `ConsoleChanged`, so a missing op shows up as a
//! stale mirror; console invariants hold after every step.

mod common;

use atomik_device_sdk::{Direction, Registry};
use atomik_model::{
    Cable, Command, DeviceKey, Document, INTERFACE_KEY, RackDoc, SocketRef, SyncOp, empty_doc,
};
use common::audio::*;
use common::console_ops::*;
use common::*;
use proptest::prelude::*;
use std::collections::BTreeMap;

const TYPES: [&str; 3] = [SYNTH, DELAY, MIXER];

/// What the engine would hold: device instances with the params sent to them, the last
/// submitted routing and the tempo. Ordering rules are asserted while replaying.
#[derive(Debug)]
struct Mirror {
    devices: BTreeMap<DeviceKey, BTreeMap<String, f32>>,
    cables: Vec<Cable>,
    tempo: f64,
    console: atomik_model::Console,
    silence: Vec<(DeviceKey, bool)>,
}

impl Mirror {
    /// The app builds the engine from a loaded document.
    fn from_doc(doc: &RackDoc, reg: &Registry) -> Self {
        Self {
            devices: doc
                .devices
                .iter()
                .map(|d| (d.key.clone(), d.params.clone()))
                .collect(),
            cables: doc.cables.clone(),
            tempo: doc.tempo_bpm,
            console: doc.console.clone(),
            silence: doc.solo_silence(reg),
        }
    }

    /// Replays `ops` in order; routing is read from `doc` when it changes.
    fn replay(&mut self, ops: &[SyncOp], doc: &RackDoc, reg: &Registry) {
        for op in ops {
            match op {
                SyncOp::DeviceAdded(k) => {
                    assert!(!self.devices.contains_key(k), "{k} added twice");
                    self.devices.insert(k.clone(), BTreeMap::new());
                }
                SyncOp::DeviceRemoved(k) => {
                    assert!(self.devices.contains_key(k), "{k} removed but unknown");
                    assert!(
                        self.cables
                            .iter()
                            .all(|c| c.from.device != *k && c.to.device != *k),
                        "{k} removed while still routed: routing must come first"
                    );
                    self.devices.remove(k);
                }
                SyncOp::RoutingChanged => {
                    for c in &doc.cables {
                        for d in [&c.from.device, &c.to.device] {
                            assert!(
                                d.0 == INTERFACE_KEY || self.devices.contains_key(d),
                                "routing references {d} before it is added"
                            );
                        }
                    }
                    self.cables = doc.cables.clone();
                }
                SyncOp::ParamChanged { key, param, value } => {
                    self.devices
                        .get_mut(key)
                        .unwrap_or_else(|| panic!("param for unknown {key}"))
                        .insert(param.clone(), *value);
                }
                SyncOp::TempoChanged(t) => self.tempo = *t,
                SyncOp::ViewChanged
                | SyncOp::MidiMapsChanged
                | SyncOp::ZonesChanged(_)
                | SyncOp::PatternsChanged { .. }
                | SyncOp::MacrosChanged { .. }
                | SyncOp::PluginChanged { .. }
                | SyncOp::PluginStateChanged { .. }
                | SyncOp::SplitsChanged { .. }
                | SyncOp::FrozenChanged => {}
                // The song mirror is checked by the song property tests.
                SyncOp::SongChanged => {}
                SyncOp::ConsoleChanged => {
                    for c in &doc.console.channels {
                        assert!(
                            self.devices.contains_key(&c.channel),
                            "{} not added",
                            c.channel
                        );
                    }
                    self.console = doc.console.clone();
                    self.silence = doc.solo_silence(reg);
                }
            }
        }
    }

    fn assert_matches(&self, doc: &RackDoc, reg: &Registry) {
        let doc_keys: Vec<_> = doc.devices.iter().map(|d| d.key.clone()).collect();
        let mut sorted = doc_keys.clone();
        sorted.sort();
        assert_eq!(self.devices.keys().cloned().collect::<Vec<_>>(), sorted);
        assert_eq!(self.cables, doc.cables);
        assert_eq!(self.tempo.to_bits(), doc.tempo_bpm.to_bits());
        assert_eq!(
            self.console, doc.console,
            "console change without ConsoleChanged"
        );
        assert_eq!(self.silence, doc.solo_silence(reg), "stale solo silence");
        assert_console_invariants(doc);
        assert_audio_invariants(doc);
        for d in &doc.devices {
            let desc = reg.get(&d.type_id).unwrap().descriptor;
            for p in desc.params {
                let engine = self.devices[&d.key]
                    .get(p.key)
                    .copied()
                    .unwrap_or(p.default);
                let model = doc.param_value(&d.key, p.key, reg).unwrap();
                assert_eq!(engine.to_bits(), model.to_bits(), "{} {}", d.key, p.key);
            }
        }
    }
}

/// A command in abstract form, resolved against the current document so that most of them
/// are valid (indices wrap around the current devices, sockets and params).
#[derive(Debug, Clone)]
enum Op {
    Add(usize, usize, Option<u8>),
    Remove(usize),
    Move(usize, usize),
    Fold(usize, bool),
    Rename(usize, u8),
    Color(usize, Option<[u8; 3]>),
    Connect(usize, usize, usize, usize),
    Disconnect(usize),
    Param(usize, usize, f32, Option<u64>),
    EndMerge,
    Tempo(f64),
    Console(ConsoleOp),
    Audio(AudioOp),
}

fn op() -> impl Strategy<Value = Op> {
    let i = || 0usize..64;
    prop_oneof![
        3 => (0usize..3, 0usize..8, proptest::option::of(0u8..4)).prop_map(|(t, i, k)| Op::Add(t, i, k)),
        1 => i().prop_map(Op::Remove),
        1 => (i(), 0usize..10).prop_map(|(d, t)| Op::Move(d, t)),
        1 => (i(), any::<bool>()).prop_map(|(d, f)| Op::Fold(d, f)),
        1 => (i(), any::<u8>()).prop_map(|(d, n)| Op::Rename(d, n)),
        1 => (i(), proptest::option::of(any::<[u8; 3]>())).prop_map(|(d, c)| Op::Color(d, c)),
        5 => (i(), i(), i(), i()).prop_map(|(a, b, c, d)| Op::Connect(a, b, c, d)),
        1 => i().prop_map(Op::Disconnect),
        4 => (i(), i(), -30_000f32..30_000f32, proptest::option::of(0u64..3))
            .prop_map(|(d, p, v, m)| Op::Param(d, p, v, m)),
        1 => Just(Op::EndMerge),
        1 => (20f64..=999f64).prop_map(Op::Tempo),
        4 => console_op().prop_map(Op::Console),
        3 => audio_op().prop_map(Op::Audio),
    ]
}

/// Sockets of `device` (or of the interface, as `out_1..out_N` inputs) in `direction`. Asking
/// for inputs also lists summing inputs.
fn sockets(doc: &RackDoc, reg: &Registry, device: &str, direction: Direction) -> Vec<String> {
    if device == INTERFACE_KEY {
        return match direction {
            Direction::Input | Direction::SummingInput => (1..=doc.output_channels)
                .map(|k| format!("out_{k}"))
                .collect(),
            Direction::Output => Vec::new(),
        };
    }
    let d = doc.device(&key(device)).unwrap();
    reg.get(&d.type_id)
        .unwrap()
        .descriptor
        .sockets
        .iter()
        .filter(|s| s.direction.is_input() == direction.is_input())
        .map(|s| s.key.to_owned())
        .collect()
}

/// Resolves `op` into a command; `None` if it cannot target anything (e.g. empty rack).
fn resolve(op: &Op, doc: &RackDoc, reg: &Registry) -> Option<Command> {
    let n = doc.devices.len();
    let dev = |i: usize| (n > 0).then(|| doc.devices[i % n].key.clone());
    Some(match op {
        Op::Add(t, i, k) => Command::AddDevice {
            index: *i,
            type_id: TYPES[*t].into(),
            key: k.map(|k| key(&format!("dev{k}"))),
        },
        Op::Remove(d) => Command::RemoveDevice { key: dev(*d)? },
        Op::Move(d, to) => Command::MoveDevice {
            key: dev(*d)?,
            to: *to,
        },
        Op::Fold(d, f) => Command::SetFolded {
            key: dev(*d)?,
            folded: *f,
        },
        Op::Rename(d, x) => Command::Rename {
            key: dev(*d)?,
            name: format!("Name {x}"),
        },
        Op::Color(d, c) => Command::SetColor {
            key: dev(*d)?,
            color: *c,
        },
        Op::Connect(a, sa, b, sb) => {
            let from = dev(*a)?;
            // Destination: a device, or the interface one time in four.
            let to = if b % 4 == 3 {
                DeviceKey(INTERFACE_KEY.into())
            } else {
                dev(*b)?
            };
            let outs = sockets(doc, reg, &from.0, Direction::Output);
            let ins = sockets(doc, reg, &to.0, Direction::Input);
            if outs.is_empty() || ins.is_empty() {
                return None;
            }
            Command::Connect(Cable {
                from: SocketRef {
                    device: from,
                    socket: outs[sa % outs.len()].clone(),
                },
                to: SocketRef {
                    device: to,
                    socket: ins[sb % ins.len()].clone(),
                },
            })
        }
        Op::Disconnect(c) => {
            if doc.cables.is_empty() {
                return None;
            }
            Command::Disconnect {
                to: doc.cables[c % doc.cables.len()].to.clone(),
            }
        }
        Op::Param(d, p, v, m) => {
            let k = dev(*d)?;
            let ty = &doc.device(&k).unwrap().type_id;
            let params = reg.get(ty).unwrap().descriptor.params;
            Command::SetParam {
                key: k,
                param: params[p % params.len()].key.into(),
                value: *v,
                merge: *m,
            }
        }
        Op::Tempo(t) => Command::SetTempo(*t),
        Op::Console(c) => return resolve_console(c, doc),
        Op::Audio(a) => return resolve_audio(a, doc),
        Op::EndMerge => return None,
    })
}

fn start(chain_start: bool, reg: &Registry) -> Document {
    let mut d = if chain_start {
        // A fresh document (empty history) holding the cabled chain.
        // Built without console devices; console runs then add channels by command.
        Document::new(chain(&registry()).doc().clone())
    } else {
        Document::new(empty_doc())
    };
    // The pool of the random audio ops (not an undo step; only audio registries have the type).
    if reg.get(AUDIO_TRACK).is_some() {
        pre_register(&mut d, reg);
    }
    d
}

/// 0 plain, 1 console devices, 2 console devices and the audio track.
fn registry_for(mode: u8) -> Registry {
    match mode {
        0 => registry(),
        1 => console_registry(),
        _ => audio_registry(),
    }
}

/// Applies `op` (if it resolves) and replays its ops onto the mirror. Rejected commands must
/// leave the document unchanged.
fn step(d: &mut Document, mirror: &mut Mirror, op: &Op, reg: &Registry) -> bool {
    if matches!(op, Op::EndMerge) {
        d.end_merge();
        return false;
    }
    let Some(cmd) = resolve(op, d.doc(), reg) else {
        return false;
    };
    let before = d.doc().clone();
    match d.apply(cmd, reg) {
        Ok(ops) => {
            mirror.replay(&ops, d.doc(), reg);
            true
        }
        Err(_) => {
            assert_eq!(d.doc(), &before);
            false
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn ac_u1_apply_undo_all_redo_all(
        chain_start in any::<bool>(),
        mode in 0u8..3,
        ops in prop::collection::vec(op(), 0..40),
    ) {
        let reg = registry_for(mode);
        let mut d = start(chain_start, &reg);
        let original = d.doc().clone();
        let mut mirror = Mirror::from_doc(&original, &reg);
        for op in &ops {
            step(&mut d, &mut mirror, op, &reg);
            mirror.assert_matches(d.doc(), &reg);
        }
        let last = d.doc().clone();
        while let Some(sync) = d.undo(&reg) {
            mirror.replay(&sync, d.doc(), &reg);
            mirror.assert_matches(d.doc(), &reg);
        }
        prop_assert_eq!(d.doc(), &original);
        while let Some(sync) = d.redo(&reg) {
            mirror.replay(&sync, d.doc(), &reg);
            mirror.assert_matches(d.doc(), &reg);
        }
        prop_assert_eq!(d.doc(), &last);
        let mut sorted = last.cables.clone();
        sorted.sort();
        prop_assert_eq!(&last.cables, &sorted, "cables stay in canonical order");
    }

    #[test]
    fn mirror_follows_mixed_apply_undo_redo(
        chain_start in any::<bool>(),
        mode in 0u8..3,
        ops in prop::collection::vec((0u8..10, op()), 0..60),
    ) {
        let reg = registry_for(mode);
        let mut d = start(chain_start, &reg);
        let mut mirror = Mirror::from_doc(d.doc(), &reg);
        for (choice, op) in &ops {
            match choice {
                0 | 1 => {
                    if let Some(sync) = d.undo(&reg) {
                        mirror.replay(&sync, d.doc(), &reg);
                    }
                }
                2 => {
                    if let Some(sync) = d.redo(&reg) {
                        mirror.replay(&sync, d.doc(), &reg);
                    }
                }
                _ => {
                    step(&mut d, &mut mirror, op, &reg);
                }
            }
            mirror.assert_matches(d.doc(), &reg);
        }
    }
}

/// The console part of the generator reaches every console command, accepted and rejected.
#[test]
fn generator_exercises_every_console_command() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let reg = console_registry();
    let mut runner = TestRunner::deterministic();
    let mut accepted: BTreeMap<String, usize> = BTreeMap::new();
    let mut rejected = 0usize;
    for _ in 0..64 {
        let ops = prop::collection::vec(op(), 40)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let mut d = start(true, &reg);
        for op in &ops {
            let Some(cmd) = resolve(op, d.doc(), &reg) else {
                continue;
            };
            let name = match &cmd {
                Command::Console(c) => format!("{c:?}"),
                Command::SetParam { param, .. } if param == "solo" => "Solo".into(),
                other => format!("{other:?}"),
            };
            let name = name.split([' ', '(', '{']).next().unwrap().to_owned();
            match d.apply(cmd, &reg) {
                Ok(_) => *accepted.entry(name).or_default() += 1,
                Err(_) => rejected += 1,
            }
        }
    }
    for kind in [
        "AddChannel",
        "RemoveChannel",
        "MoveChannel",
        "SetReturn",
        "SetCompensation",
        "ClearSolos",
        "ClearMutes",
        "Solo",
    ] {
        assert!(
            accepted.get(kind).copied().unwrap_or(0) >= 3,
            "{kind} accepted too rarely: {accepted:?}"
        );
    }
    assert!(rejected > 50, "rejections are exercised ({rejected})");
}
