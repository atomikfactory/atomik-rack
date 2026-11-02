//! Random container command sequences and the invariants of a document and its expansion,
//! shared by the command and format property tests.

use super::*;
use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_model::{ContainerCommand, DeviceKey, RackDoc};
use proptest::prelude::*;

/// Every invariant of the document and of its expansion.
pub fn assert_invariants(doc: &RackDoc, r: &Registry) {
    doc.check_containers(r)
        .unwrap_or_else(|e| panic!("document invalid: {e}"));
    let flat = doc.expand(r);
    // Keys are unique; every flat device has a registered type.
    let mut seen = std::collections::BTreeSet::new();
    for dev in &flat.devices {
        assert!(
            seen.insert(dev.key.clone()),
            "duplicate flat key {}",
            dev.key
        );
        assert!(r.get(&dev.type_id).is_some(), "{}", dev.type_id);
        assert_ne!(dev.type_id, CONTAINER, "containers vanish");
    }
    // Cables are valid for the engine: ends exist, direction and kind, one per plain input.
    let mut plain_inputs = std::collections::BTreeSet::new();
    let mut all = std::collections::BTreeSet::new();
    for c in &flat.cables {
        assert!(all.insert(c.clone()), "the same flat cable twice: {c:?}");
        let socket = |s: &atomik_model::SocketRef| {
            if s.device.0 == "interface" {
                return (SocketKind::Audio, Direction::Input);
            }
            let dev = flat
                .devices
                .iter()
                .find(|d| d.key == s.device)
                .unwrap_or_else(|| panic!("no flat device {}", s.device));
            let sd = r
                .get(&dev.type_id)
                .unwrap()
                .descriptor
                .socket_by_key(&s.socket)
                .unwrap_or_else(|| panic!("no socket {s}"));
            (sd.kind, sd.direction)
        };
        let (fk, fd) = socket(&c.from);
        let (tk, td) = socket(&c.to);
        if c.from.device.0 != "interface" {
            assert_eq!(fd, Direction::Output, "{c:?}");
        }
        if c.to.device.0 != "interface" {
            assert!(td.is_input(), "{c:?}");
            assert_eq!(fk, tk, "{c:?}");
            if td == Direction::Input {
                assert!(plain_inputs.insert(c.to.clone()), "input fed twice {c:?}");
            }
        }
    }
    // Acyclic at device level (the delays and synths here have a single stage).
    let mut order: Vec<&DeviceKey> = Vec::new();
    let mut indeg: std::collections::BTreeMap<&DeviceKey, usize> =
        flat.devices.iter().map(|d| (&d.key, 0)).collect();
    for c in &flat.cables {
        if c.from.device.0 != "interface" && c.to.device.0 != "interface" {
            *indeg.get_mut(&c.to.device).unwrap() += 1;
        }
    }
    let mut ready: Vec<&DeviceKey> = indeg
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(k, _)| *k)
        .collect();
    while let Some(k) = ready.pop() {
        order.push(k);
        for c in flat.cables.iter().filter(|c| &c.from.device == k) {
            if c.to.device.0 == "interface" {
                continue;
            }
            let n = indeg.get_mut(&c.to.device).unwrap();
            *n -= 1;
            if *n == 0 {
                ready.push(&c.to.device);
            }
        }
    }
    assert_eq!(order.len(), flat.devices.len(), "the flat graph is acyclic");
    // Every container has an alias; its split is in the list. A bypassed container (its own
    // switch, or a macro of an outer container) has no inside: the containers in it are not
    // expanded and nothing of it is in the flat list.
    fn containers(devs: &[atomik_model::RackDevice], prefix: &str, out: &mut Vec<String>) {
        for d in devs {
            if let Some(i) = &d.inner {
                let p = format!("{prefix}{}", d.key);
                containers(&i.devices, &format!("{p}/"), out);
                out.push(p);
            }
        }
    }
    let mut cs = Vec::new();
    containers(&doc.devices, "", &mut cs);
    for c in cs {
        let Some(alias) = flat.alias.get(&key(&c)) else {
            let inside = format!("{c}/");
            assert!(
                flat.devices.iter().all(|d| !d.key.0.starts_with(&inside)),
                "no alias for {c}, which is not bypassed away"
            );
            continue;
        };
        assert_eq!(alias, &key(&format!("{c}/~split")));
        assert!(flat.devices.iter().any(|d| &d.key == alias));
    }
}

#[derive(Debug, Clone)]
pub enum Op {
    AddTop(u8, u8),
    AddInner(u8, u8, u8, bool),
    Connect(u8, u8, u8, u8),
    Disconnect(u8, u8),
    Splits(u8, Vec<(u8, u8, u8, i8)>),
    Macro(u8, u8, u8, u8, u8, u8),
    Unmap(u8, u8, u8),
    Remove(u8),
    Move(u8, u8),
    Duplicate(u8),
    Open(u8, bool),
    Param(u8, u8, u8),
    Undo,
    Redo,
}

pub fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Op::AddTop(a, b)),
        6 => (any::<u8>(), any::<u8>(), any::<u8>(), any::<bool>())
            .prop_map(|(a, b, c, d)| Op::AddInner(a, b, c, d)),
        10 => (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(a, b, c, d)| Op::Connect(a, b, c, d)),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Op::Disconnect(a, b)),
        4 => (
            any::<u8>(),
            proptest::collection::vec((any::<u8>(), 0u8..=127, 0u8..=127, -50i8..=50), 0..4)
        )
            .prop_map(|(a, v)| Op::Splits(a, v)),
        5 => (any::<u8>(), 0u8..9, any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(a, b, c, d, e, f)| Op::Macro(a, b, c, d, e, f)),
        2 => (any::<u8>(), 0u8..8, any::<u8>()).prop_map(|(a, b, c)| Op::Unmap(a, b, c)),
        1 => any::<u8>().prop_map(Op::Remove),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Op::Move(a, b)),
        2 => any::<u8>().prop_map(Op::Duplicate),
        1 => (any::<u8>(), any::<bool>()).prop_map(|(a, b)| Op::Open(a, b)),
        3 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| Op::Param(a, b, c)),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

/// Every device path of `doc` (containers included), depth first in document order.
pub fn all_paths(doc: &RackDoc) -> Vec<(String, bool)> {
    fn walk(devs: &[atomik_model::RackDevice], prefix: &str, out: &mut Vec<(String, bool)>) {
        for d in devs {
            let p = format!("{prefix}{}", d.key);
            out.push((p.clone(), d.inner.is_some()));
            if let Some(i) = &d.inner {
                walk(&i.devices, &format!("{p}/"), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.devices, "", &mut out);
    out
}

pub fn resolve_op(op: &Op, doc: &RackDoc, r: &Registry) -> Option<Command> {
    const TYPES: [&str; 4] = [SYNTH, DELAY, CONTAINER, SEQ];
    let paths = all_paths(doc);
    let containers: Vec<&(String, bool)> = paths.iter().filter(|(_, c)| *c).collect();
    let pick =
        |i: u8, list: &[(String, bool)]| list.get(usize::from(i) % list.len().max(1)).cloned();
    let _sockets_unused = |path: &str, want_output: bool| -> Vec<String> {
        doc.resolve(&key(path))
            .and_then(|d| r.get(&d.type_id))
            .map(|f| {
                f.descriptor
                    .sockets
                    .iter()
                    .filter(|s| (s.direction == Direction::Output) == want_output)
                    .map(|s| s.key.to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    match op {
        Op::AddTop(t, i) => Some(add(
            usize::from(*i) % (doc.devices.len() + 1),
            TYPES[usize::from(*t) % TYPES.len()],
            None,
        )),
        Op::AddInner(c, t, i, auto) => {
            let (c, _) = pick(
                *c,
                &containers.iter().map(|x| (*x).clone()).collect::<Vec<_>>(),
            )?;
            Some(Command::Container(ContainerCommand::AddInner {
                container: key(&c),
                index: usize::from(*i) % 6,
                type_id: TYPES[usize::from(*t) % TYPES.len()].to_owned(),
                key: None,
                auto_split: *auto,
            }))
        }
        Op::Connect(a, sa, b, sb) => {
            // Two ends in one scope (the top level or a container, boundary included).
            let mut scopes: Vec<Option<String>> = vec![None];
            scopes.extend(containers.iter().map(|(p, _)| Some(p.clone())));
            let scope = scopes[usize::from(*a) % scopes.len()].clone();
            let prefix = scope.as_ref().map(|s| format!("{s}/")).unwrap_or_default();
            let devices = match &scope {
                None => doc
                    .devices
                    .iter()
                    .map(|d| (d.key.0.clone(), d.type_id.clone()))
                    .collect(),
                Some(s) => doc
                    .resolve(&key(s))?
                    .inner
                    .as_ref()?
                    .devices
                    .iter()
                    .map(|d| (d.key.0.clone(), d.type_id.clone()))
                    .collect::<Vec<_>>(),
            };
            // (local key, output sockets, input sockets)
            let mut ends: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
            for (k, t) in &devices {
                let desc = r.get(t)?.descriptor;
                let sock_keys = |output: bool| -> Vec<String> {
                    desc.sockets
                        .iter()
                        .filter(|s| (s.direction == Direction::Output) == output)
                        .map(|s| s.key.to_owned())
                        .collect()
                };
                ends.push((k.clone(), sock_keys(true), sock_keys(false)));
            }
            if scope.is_some() {
                ends.push((
                    "boundary".into(),
                    vec!["in_l".into(), "in_r".into()],
                    vec!["out_l".into(), "out_r".into()],
                ));
            }
            if ends.is_empty() {
                return None;
            }
            let from = &ends[usize::from(*sb) % ends.len()];
            let to = &ends[usize::from(*sa).wrapping_add(usize::from(*b)) % ends.len()];
            if from.1.is_empty() || to.2.is_empty() {
                return None;
            }
            Some(Command::Connect(atomik_model::Cable {
                from: sock(&format!(
                    "{prefix}{}:{}",
                    from.0,
                    from.1[usize::from(*b) % from.1.len()]
                )),
                to: sock(&format!(
                    "{prefix}{}:{}",
                    to.0,
                    to.2[usize::from(*sa) % to.2.len()]
                )),
            }))
        }
        Op::Disconnect(c, s) => {
            let (c, _) = pick(
                *c,
                &containers.iter().map(|x| (*x).clone()).collect::<Vec<_>>(),
            )?;
            let inner = doc.resolve(&key(&c))?.inner.as_ref()?;
            let cable = inner
                .cables
                .get(usize::from(*s) % inner.cables.len().max(1))?;
            Some(Command::Disconnect {
                to: sock(&format!("{c}/{}:{}", cable.to.device, cable.to.socket)),
            })
        }
        Op::Splits(c, list) => {
            let (c, _) = pick(
                *c,
                &containers.iter().map(|x| (*x).clone()).collect::<Vec<_>>(),
            )?;
            let inner = doc.resolve(&key(&c))?.inner.as_ref()?;
            let devs: Vec<&str> = inner.devices.iter().map(|d| d.key.0.as_str()).collect();
            let splits = list
                .iter()
                .map(|(t, lo, hi, tr)| {
                    let target = devs
                        .get(usize::from(*t) % devs.len().max(1))
                        .copied()
                        .unwrap_or("none");
                    split(target, *lo.min(hi), *lo.max(hi), *tr)
                })
                .collect();
            Some(set_splits(&c, splits))
        }
        Op::Macro(c, m, t, p, lo, hi) => {
            let (c, _) = pick(
                *c,
                &containers.iter().map(|x| (*x).clone()).collect::<Vec<_>>(),
            )?;
            let inner = doc.resolve(&key(&c))?.inner.as_ref()?;
            let dev = inner
                .devices
                .get(usize::from(*t) % inner.devices.len().max(1))?;
            let desc = r.get(&dev.type_id)?.descriptor;
            let param = desc.params.get(usize::from(*p) % desc.params.len())?;
            Some(set_macro(
                &c,
                *m,
                target(
                    &dev.key.0,
                    param.key,
                    f32::from(*lo) / 255.0,
                    f32::from(*hi) / 255.0,
                ),
            ))
        }
        Op::Unmap(c, m, t) => {
            let (c, _) = pick(
                *c,
                &containers.iter().map(|x| (*x).clone()).collect::<Vec<_>>(),
            )?;
            let inner = doc.resolve(&key(&c))?.inner.as_ref()?;
            let targets = &inner.macros[usize::from(*m)].targets;
            let t = targets.get(usize::from(*t) % targets.len().max(1))?;
            Some(Command::Container(ContainerCommand::RemoveMacroTarget {
                container: key(&c),
                index: *m,
                device: t.device.clone(),
                param: t.param.clone(),
            }))
        }
        Op::Remove(i) => {
            let (p, _) = pick(*i, &paths)?;
            Some(Command::RemoveDevice { key: key(&p) })
        }
        Op::Move(i, to) => {
            let (p, _) = pick(*i, &paths)?;
            Some(Command::MoveDevice {
                key: key(&p),
                to: usize::from(*to) % 5,
            })
        }
        Op::Duplicate(i) => {
            let (p, _) = pick(*i, &paths)?;
            Some(Command::Container(ContainerCommand::DuplicateContainer {
                container: key(&p),
            }))
        }
        Op::Open(i, open) => {
            let owned: Vec<(String, bool)> = containers.iter().map(|x| (*x).clone()).collect();
            let (p, _) = pick(*i, &owned)?;
            Some(Command::Container(ContainerCommand::SetOpen {
                container: key(&p),
                open: *open,
            }))
        }
        Op::Param(i, p, v) => {
            let (path, _) = pick(*i, &paths)?;
            let d = doc.resolve(&key(&path))?;
            let desc = r.get(&d.type_id)?.descriptor;
            let param = desc.params.get(usize::from(*p) % desc.params.len())?;
            Some(set_param(&path, param.key, f32::from(*v) / 255.0))
        }
        Op::Undo | Op::Redo => None,
    }
}
