//! Property tests for plugin devices: random command sequences with
//! plugin devices keep every invariant of the document, its expansion and its text, and a
//! stand-in for a missing plugin keeps its `plugin` object byte for byte across any edits.

mod common;

use atomik_device_sdk::Registry;
use atomik_device_sdk::{PluginLayout, PluginRef, is_plugin_type_id};
use atomik_model::{
    Command, Document, PLUGIN_GESTURE_MERGE_BASE, PluginCommand, RackDoc, SongCommand,
    load_project, register_stand_ins, save_project,
};
use common::container_ops::{Op, all_paths, assert_invariants, op, resolve_op};
use common::plugin::*;
use common::*;
use proptest::prelude::*;

fn kinds() -> [(PluginRef, PluginLayout); 4] {
    [
        (synth_ref(), synth_layout()),
        (fx_ref(), fx_layout()),
        (vst3_ref(), vst3_layout()),
        (notes_ref(), notes_layout()),
    ]
}

/// Every plugin invariant of a document and of its text.
fn assert_plugin_invariants(doc: &RackDoc, r: &Registry) {
    fn walk(devs: &[atomik_model::RackDevice], prefix: &str, r: &Registry) {
        for d in devs {
            let path = format!("{prefix}{}", d.key);
            assert_eq!(
                d.plugin.is_some(),
                is_plugin_type_id(&d.type_id),
                "{path}: a plugin doc exactly on plugin types"
            );
            if let Some(p) = &d.plugin {
                assert_eq!(p.type_id(), d.type_id, "{path}");
                assert!(p.pinned.len() <= atomik_model::MAX_PINNED_PARAMS, "{path}");
                let mut sorted = p.pinned.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(sorted.len(), p.pinned.len(), "{path}: pins are unique");
                assert!(r.is_plugin(&d.type_id), "{path}: registered");
                if let Some(s) = &p.state {
                    assert!(s.0.len() as u64 <= atomik_model::MAX_PLUGIN_STATE_BYTES);
                }
            }
            if let Some(i) = &d.inner {
                walk(&i.devices, &format!("{path}/"), r);
            }
        }
    }
    walk(&doc.devices, "", r);
    // The text is canonical and reloads to the compacted document, with the plugins
    // installed and as stand-ins.
    let text = save_project(doc);
    let back = load_project(&text, r).unwrap_or_else(|e| panic!("reload failed: {e}\n{text}"));
    assert_eq!(back, doc.compacted());
    assert_eq!(save_project(&back), text);
    let bare = registry();
    let stand = load_project(&text, &bare).unwrap_or_else(|e| panic!("stand-in load: {e}"));
    assert_eq!(stand, back, "installed or stand-in: the same document");
    assert_eq!(save_project(&stand), text);
}

#[derive(Debug, Clone)]
enum POp {
    AddTop(u8, u8),
    AddInner(u8, u8, u8),
    Sync(u8, u8),
    Layout(u8),
    Pins(u8, u8),
    Power(u8, bool),
    Run(u8, bool),
    Relink(u8, u8),
    Gesture(u8, u8, u8, u8),
    Param(u8, u8, u8),
    Remove(u8),
    Lane(u8, u8),
    Undo,
    Redo,
    Container(Op),
}

fn pop() -> impl Strategy<Value = POp> {
    prop_oneof![
        4 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| POp::AddTop(a, b)),
        4 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| POp::AddInner(a, b, c)),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| POp::Sync(a, b)),
        1 => any::<u8>().prop_map(POp::Layout),
        3 => (any::<u8>(), 0u8..20).prop_map(|(a, b)| POp::Pins(a, b)),
        2 => (any::<u8>(), any::<bool>()).prop_map(|(a, b)| POp::Power(a, b)),
        2 => (any::<u8>(), any::<bool>()).prop_map(|(a, b)| POp::Run(a, b)),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| POp::Relink(a, b)),
        4 => (any::<u8>(), any::<u8>(), any::<u8>(), 0u8..3)
            .prop_map(|(a, b, c, d)| POp::Gesture(a, b, c, d)),
        3 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| POp::Param(a, b, c)),
        1 => any::<u8>().prop_map(POp::Remove),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| POp::Lane(a, b)),
        2 => Just(POp::Undo),
        1 => Just(POp::Redo),
        6 => op().prop_map(POp::Container),
    ]
}

/// Paths of the plugin devices of `doc`.
fn plugin_paths(doc: &RackDoc) -> Vec<String> {
    all_paths(doc)
        .into_iter()
        .filter(|(p, _)| doc.resolve(&key(p)).is_some_and(|d| d.plugin.is_some()))
        .map(|(p, _)| p)
        .collect()
}

fn param_command(
    doc: &RackDoc,
    r: &Registry,
    path: &str,
    p: u8,
    v: u8,
    gesture: Option<u8>,
) -> Option<Command> {
    let dev = doc.resolve(&key(path))?;
    let desc = r.get(&dev.type_id)?.descriptor;
    let param = desc.params.get(usize::from(p) % desc.params.len())?;
    let value = param.min + (param.max - param.min) * (f32::from(v) / 255.0);
    Some(Command::SetParam {
        key: key(path),
        param: param.key.to_owned(),
        value,
        merge: gesture.map(|g| PLUGIN_GESTURE_MERGE_BASE + u64::from(g)),
    })
}

fn resolve(op: &POp, doc: &RackDoc, r: &Registry) -> Option<Command> {
    let plugins = plugin_paths(doc);
    let pick = |i: u8| plugins.get(usize::from(i) % plugins.len().max(1)).cloned();
    let containers: Vec<String> = all_paths(doc)
        .into_iter()
        .filter(|(_, c)| *c)
        .map(|(p, _)| p)
        .collect();
    match op {
        POp::AddTop(t, i) => {
            let (p, l) = &kinds()[usize::from(*t) % 4];
            Some(add_plugin(
                usize::from(*i) % (doc.devices.len() + 1),
                p,
                l,
                None,
            ))
        }
        POp::AddInner(c, t, i) => {
            let c = containers.get(usize::from(*c) % containers.len().max(1))?;
            let (p, l) = &kinds()[usize::from(*t) % 4];
            Some(add_plugin_inner(
                c,
                usize::from(*i) % 4,
                p,
                l,
                None,
                *i % 2 == 0,
            ))
        }
        POp::Sync(d, n) => {
            let blob: Vec<u8> = (0..usize::from(*n) * 3).map(|i| (i * 7) as u8).collect();
            Some(sync_state(&pick(*d)?, &blob))
        }
        POp::Layout(d) => {
            let path = pick(*d)?;
            let p = doc.resolve(&key(&path))?.plugin.as_ref()?;
            let layout = kinds()
                .into_iter()
                .find(|(k, _)| k.id == p.id)
                .map(|(_, l)| l)?;
            Some(Command::Plugin(PluginCommand::SyncLayout {
                device: key(&path),
                layout,
            }))
        }
        POp::Pins(d, n) => {
            let path = pick(*d)?;
            let p = doc.resolve(&key(&path))?.plugin.as_ref()?;
            let ids: Vec<u32> = p
                .layout
                .params
                .iter()
                .map(|x| x.id)
                .rev()
                .take(usize::from(*n))
                .collect();
            Some(set_pinned(&path, &ids))
        }
        POp::Power(d, on) => Some(Command::Plugin(PluginCommand::SetPower {
            device: key(&pick(*d)?),
            powered: *on,
        })),
        POp::Run(d, on) => Some(Command::Plugin(PluginCommand::SetRunMode {
            device: key(&pick(*d)?),
            run_on_audio_thread: *on,
        })),
        POp::Relink(d, t) => {
            let (p, l) = &kinds()[usize::from(*t) % 4];
            Some(relink(&pick(*d)?, p, l, Some("hint")))
        }
        POp::Gesture(d, p, v, g) => param_command(doc, r, &pick(*d)?, *p, *v, Some(*g)),
        POp::Param(d, p, v) => param_command(doc, r, &pick(*d)?, *p, *v, None),
        POp::Remove(d) => Some(Command::RemoveDevice {
            key: key(&pick(*d)?),
        }),
        POp::Lane(d, p) => {
            let path = pick(*d)?;
            let dev = doc.device(&key(&path))?;
            let desc = r.get(&dev.type_id)?.descriptor;
            let param = desc.params.get(usize::from(*p) % desc.params.len())?;
            Some(Command::Song(SongCommand::AddAutomationLane {
                device: key(&path),
                param: param.key.to_owned(),
            }))
        }
        POp::Container(o) => resolve_op(o, doc, r),
        POp::Undo | POp::Redo => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn random_sequences_with_plugins_keep_every_invariant(ops in proptest::collection::vec(pop(), 1..28)) {
        let r = plugin_registry();
        let mut d = Document::new(atomik_model::empty_doc());
        for o in &ops {
            match o {
                POp::Undo | POp::Container(Op::Undo) => { d.undo(&r); }
                POp::Redo | POp::Container(Op::Redo) => { d.redo(&r); }
                other => {
                    if let Some(c) = resolve(other, d.doc(), &r) {
                        let before = d.doc().clone();
                        if d.apply(c, &r).is_err() {
                            prop_assert_eq!(d.doc(), &before, "a rejected command changes nothing");
                        }
                    }
                }
            }
            assert_invariants(d.doc(), &r);
            assert_plugin_invariants(d.doc(), &r);
        }
        // Undo everything: the empty document comes back.
        while d.undo(&r).is_some() {}
        prop_assert_eq!(d.doc(), &atomik_model::empty_doc());
    }
}

// ---- stand-ins ------------------------------------------------------------------------------------

fn plugin_texts(text: &str) -> Vec<String> {
    // The shared helper lives in plugin_format.rs; this copy keeps the files independent.
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find("\"plugin\": {") {
        let start = from + at + "\"plugin\": ".len();
        let (mut depth, mut in_str, mut esc) = (0, false, false);
        let mut end = start;
        for (i, &b) in bytes[start..].iter().enumerate() {
            if in_str {
                match (esc, b) {
                    (true, _) => esc = false,
                    (false, b'\\') => esc = true,
                    (false, b'"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        // Byte for byte, but for the indentation (a moved device sits at another depth) and the
        // order of the devices.
        let object: Vec<&str> = text[start..end].lines().map(str::trim_start).collect();
        out.push(object.join(
            "
",
        ));
        from = end;
    }
    out.sort();
    out
}

#[derive(Debug, Clone)]
enum SOp {
    Rename(u8, u8),
    Fold(u8, bool),
    Color(u8),
    Move(u8, u8),
    Param(u8, u8, u8),
    Connect(u8, u8, u8, u8),
    Disconnect(u8),
    AddBuiltin(u8),
    RemoveBuiltin(u8),
    Tempo(u8),
    Undo,
    Redo,
}

fn sop() -> impl Strategy<Value = SOp> {
    prop_oneof![
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| SOp::Rename(a, b)),
        2 => (any::<u8>(), any::<bool>()).prop_map(|(a, b)| SOp::Fold(a, b)),
        1 => any::<u8>().prop_map(SOp::Color),
        2 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| SOp::Move(a, b)),
        4 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| SOp::Param(a, b, c)),
        6 => (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(a, b, c, d)| SOp::Connect(a, b, c, d)),
        2 => any::<u8>().prop_map(SOp::Disconnect),
        1 => any::<u8>().prop_map(SOp::AddBuiltin),
        1 => any::<u8>().prop_map(SOp::RemoveBuiltin),
        1 => any::<u8>().prop_map(SOp::Tempo),
        2 => Just(SOp::Undo),
        2 => Just(SOp::Redo),
    ]
}

fn resolve_stand_in(op: &SOp, doc: &RackDoc, r: &Registry) -> Option<Command> {
    let devs = &doc.devices;
    let pick = |i: u8| {
        devs.get(usize::from(i) % devs.len().max(1))
            .map(|d| d.key.0.clone())
    };
    let builtin: Vec<&str> = devs
        .iter()
        .filter(|d| d.plugin.is_none() && d.inner.is_none())
        .map(|d| d.key.0.as_str())
        .collect();
    match op {
        SOp::Rename(i, n) => Some(Command::Rename {
            key: key(&pick(*i)?),
            name: format!("name {n}"),
        }),
        SOp::Fold(i, f) => Some(Command::SetFolded {
            key: key(&pick(*i)?),
            folded: *f,
        }),
        SOp::Color(i) => Some(Command::SetColor {
            key: key(&pick(*i)?),
            color: Some([*i, 2, 3]),
        }),
        SOp::Move(i, to) => Some(Command::MoveDevice {
            key: key(&pick(*i)?),
            to: usize::from(*to) % 6,
        }),
        SOp::Param(i, p, v) => {
            let path = pick(*i)?;
            let dev = doc.device(&key(&path))?;
            let desc = r.get(&dev.type_id)?.descriptor;
            let param = desc.params.get(usize::from(*p) % desc.params.len())?;
            Some(set_param(
                &path,
                param.key,
                param.min + (param.max - param.min) * (f32::from(*v) / 255.0),
            ))
        }
        SOp::Connect(a, sa, b, sb) => {
            let from = devs.get(usize::from(*a) % devs.len().max(1))?;
            let to = devs.get(usize::from(*b) % devs.len().max(1))?;
            let outs: Vec<&str> = r
                .get(&from.type_id)?
                .descriptor
                .sockets
                .iter()
                .filter(|s| s.direction == atomik_device_sdk::Direction::Output)
                .map(|s| s.key)
                .collect();
            let ins: Vec<&str> = r
                .get(&to.type_id)?
                .descriptor
                .sockets
                .iter()
                .filter(|s| s.direction.is_input())
                .map(|s| s.key)
                .collect();
            if outs.is_empty() || ins.is_empty() {
                return None;
            }
            Some(connect(
                &format!("{}:{}", from.key, outs[usize::from(*sa) % outs.len()]),
                &format!("{}:{}", to.key, ins[usize::from(*sb) % ins.len()]),
            ))
        }
        SOp::Disconnect(i) => {
            let c = doc.cables.get(usize::from(*i) % doc.cables.len().max(1))?;
            Some(Command::Disconnect { to: c.to.clone() })
        }
        SOp::AddBuiltin(t) => Some(add(
            usize::from(*t) % 4,
            [SYNTH, DELAY, MIXER, ARP][usize::from(*t) % 4],
            None,
        )),
        SOp::RemoveBuiltin(i) => {
            let k = builtin.get(usize::from(*i) % builtin.len().max(1))?;
            Some(Command::RemoveDevice { key: key(k) })
        }
        SOp::Tempo(t) => Some(Command::SetTempo(60.0 + f64::from(*t))),
        SOp::Undo | SOp::Redo => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_stand_in_keeps_its_plugin_objects_byte_for_byte_across_edits(ops in proptest::collection::vec(sop(), 1..30)) {
        let file = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin-v9.atomik"),
        )
        .unwrap()
        .replace("\r\n", "\n");
        let original = plugin_texts(&file);
        let bare = registry();
        let doc = load_project(&file, &bare).unwrap();
        let mut r = registry();
        register_stand_ins(&doc, &mut r).unwrap();
        let mut d = Document::new(doc);
        for o in &ops {
            match o {
                SOp::Undo => { d.undo(&r); }
                SOp::Redo => { d.redo(&r); }
                other => {
                    if let Some(c) = resolve_stand_in(other, d.doc(), &r) {
                        let _ = d.apply(c, &r);
                    }
                }
            }
            let text = save_project(d.doc());
            prop_assert_eq!(plugin_texts(&text), original.clone(), "after {:?}", o);
            // The stand-in devices are still there, and the text reloads.
            for k in ["lead", "verb", "layer"] {
                prop_assert!(d.doc().device(&key(k)).is_some(), "{}", k);
            }
            let again = load_project(&text, &r).unwrap();
            prop_assert_eq!(save_project(&again), text);
        }
    }
}
