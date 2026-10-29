//! Shared helpers for the model integration tests.
#![allow(dead_code)]

pub mod audio;
pub mod console;
pub mod console_ops;
pub mod container_ops;
pub mod plugin;
pub mod song;
pub mod workflow;

use atomik_device_sdk::Registry;
use atomik_model::{Cable, Command, DeviceKey, Document, SocketRef, SyncOp, empty_doc};

pub const SYNTH: &str = "atomik.poly-synth";
pub const DELAY: &str = "atomik.stereo-delay";
pub const MIXER: &str = "atomik.line-mixer";
pub const SEQ: &str = "atomik.step-sequencer";
pub const ARP: &str = "atomik.arpeggiator";
pub const CONTAINER: &str = "atomik.container";
pub const SPLIT: &str = "atomik.note-split";
pub const SAMPLER: &str = "atomik.sampler";
pub const COMP: &str = "atomik.compressor";

pub const CHANNEL: &str = "atomik.channel";
pub const MASTER: &str = "atomik.master";
pub const BUS: &str = "atomik.bus";

/// The built-in devices except the console devices, whatever `register_all` holds: without
/// `atomik.channel`, `AddDevice` creates no automatic channels (the original behaviour).
pub fn registry() -> Registry {
    let mut all = Registry::new();
    atomik_devices::register_all(&mut all).unwrap();
    let mut r = Registry::new();
    // The real audio track device (once it exists) is replaced by the stub of `audio`.
    for t in all
        .type_ids()
        .filter(|t| ![CHANNEL, BUS, MASTER, audio::AUDIO_TRACK].contains(t))
    {
        let factory = *all.get(t).unwrap();
        if all.takes_samples(t) {
            r.register_with_samples(factory).unwrap();
        } else if all.takes_curves(t) {
            r.register_with_curves(factory).unwrap();
        } else if all.takes_drums(t) {
            r.register_with_drums(factory).unwrap();
        } else if all.takes_player_data(t) {
            r.register_with_player_data(factory).unwrap();
        } else {
            r.register(factory).unwrap();
        }
    }
    r
}

/// [`registry`] plus the stub console devices (exact socket and param keys) and the
/// test instruments of [`console`].
pub fn console_registry() -> Registry {
    let mut r = registry();
    console::register_stubs(&mut r);
    r
}

/// The starter document with note tails off, as a project written before format 35 loads: the
/// tests that compare an older file's re-save with the current text use it.
pub fn legacy_project_doc() -> atomik_model::RackDoc {
    let mut doc = atomik_model::new_project_doc();
    doc.song.tails = false;
    doc
}

pub fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// `"device:socket"` to a socket reference.
pub fn sock(text: &str) -> SocketRef {
    let (d, s) = text.split_once(':').unwrap();
    SocketRef {
        device: key(d),
        socket: s.to_owned(),
    }
}

pub fn cable(from: &str, to: &str) -> Cable {
    Cable {
        from: sock(from),
        to: sock(to),
    }
}

pub fn add(index: usize, type_id: &str, k: Option<&str>) -> Command {
    Command::AddDevice {
        index,
        type_id: type_id.to_owned(),
        key: k.map(key),
    }
}

pub fn connect(from: &str, to: &str) -> Command {
    Command::Connect(cable(from, to))
}

/// A document with `synth`, `delay`, `mixer` (in that order) and no cables.
pub fn three_devices(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [(SYNTH, "synth"), (DELAY, "delay"), (MIXER, "mixer")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), reg).unwrap();
    }
    d
}

/// `three_devices` cabled synth → delay → mixer ch1 → interface out 1/2.
pub fn chain(reg: &Registry) -> Document {
    let mut d = three_devices(reg);
    for (f, t) in [
        ("synth:out_l", "delay:in_l"),
        ("synth:out_r", "delay:in_r"),
        ("delay:out_l", "mixer:ch1_in_l"),
        ("delay:out_r", "mixer:ch1_in_r"),
        ("mixer:master_l", "interface:out_1"),
        ("mixer:master_r", "interface:out_2"),
    ] {
        d.apply(connect(f, t), reg).unwrap();
    }
    d
}

pub fn keys(d: &Document) -> Vec<String> {
    d.doc().devices.iter().map(|x| x.key.0.clone()).collect()
}

pub fn routing() -> SyncOp {
    SyncOp::RoutingChanged
}

/// `AddInner` without an automatic split (`container` is a path key, `k` a local key).
pub fn add_inner(container: &str, index: usize, type_id: &str, k: &str) -> Command {
    Command::Container(atomik_model::ContainerCommand::AddInner {
        container: key(container),
        index,
        type_id: type_id.to_owned(),
        key: Some(key(k)),
        auto_split: false,
    })
}

/// Applies `commands` in order, panicking on the first rejection.
pub fn run(d: &mut Document, reg: &Registry, commands: impl IntoIterator<Item = Command>) {
    for c in commands {
        d.apply(c.clone(), reg)
            .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    }
}

/// A full-velocity-range split to `target` with the given key range and transpose.
pub fn split(target: &str, key_lo: u8, key_hi: u8, transpose: i8) -> atomik_model::NoteSplit {
    atomik_model::NoteSplit {
        target: key(target),
        key_lo,
        key_hi,
        vel_lo: 1,
        vel_hi: 127,
        transpose,
    }
}

/// A macro target.
pub fn target(device: &str, param: &str, min: f32, max: f32) -> atomik_model::MacroTarget {
    atomik_model::MacroTarget {
        device: key(device),
        param: param.to_owned(),
        min,
        max,
    }
}

/// `SetMacro` (macro `index` is 0-based).
pub fn set_macro(container: &str, index: u8, t: atomik_model::MacroTarget) -> Command {
    Command::Container(atomik_model::ContainerCommand::SetMacro {
        container: key(container),
        index,
        target: t,
        merge: None,
    })
}

pub fn set_splits(container: &str, splits: Vec<atomik_model::NoteSplit>) -> Command {
    Command::Container(atomik_model::ContainerCommand::SetSplits {
        container: key(container),
        splits,
        merge: None,
    })
}

pub fn set_param(k: &str, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: param.to_owned(),
        value,
        merge: None,
    }
}

/// A document with a container `c` holding a synth `synth` and a delay `delay`, cabled
/// `synth -> delay -> boundary out`.
pub fn container_doc(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        reg,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, DELAY, "delay"),
            connect("c/synth:out_l", "c/delay:in_l"),
            connect("c/synth:out_r", "c/delay:in_r"),
            connect("c/delay:out_l", "c/boundary:out_l"),
            connect("c/delay:out_r", "c/boundary:out_r"),
        ],
    );
    d
}

/// The text of an older project file as today's canonical save writes it: the version number
/// `from` becomes the current one and the single `"meter": [n, d]` pair becomes the version 13
/// list of changes. (The rest of a file only differs when the format changed.)
pub fn as_current(file: &str, from: u32) -> String {
    let version = |n: u32| format!("\"version\": {n}");
    let file = file.replacen(&version(from), &version(atomik_model::PROJECT_VERSION), 1);
    let Some(start) = file.find("\"meter\": [") else {
        return file;
    };
    let rest = &file[start + "\"meter\": [".len()..];
    let end = rest.find("],").expect("a meter line ends with `],`");
    let pair: Vec<&str> = rest[..end].split(", ").collect();
    if pair.len() != 2 || pair[0].starts_with('{') {
        return file;
    }
    let list = format!(
        "\"meter\": [\n      {{\"bar\": 1, \"num\": {}, \"den\": {}}}\n    ],",
        pair[0], pair[1]
    );
    format!("{}{}{}", &file[..start], list, &rest[end + "],".len()..])
}

/// The inverse of [`as_current`] for a single-meter file: today's save as the version `to` file
/// (for regenerating the frozen fixtures).
pub fn as_old(text: &str, to: u32) -> String {
    let version = |n: u32| format!("\"version\": {n}");
    let text = text.replacen(&version(atomik_model::PROJECT_VERSION), &version(to), 1);
    let Some(start) = text.find("\"meter\": [\n") else {
        return text;
    };
    let rest = &text[start..];
    let end = rest.find("],").expect("a meter list ends with `],`");
    let inner = &rest[..end];
    let num = inner
        .split("\"num\": ")
        .nth(1)
        .and_then(|s| s.split(',').next());
    let den = inner
        .split("\"den\": ")
        .nth(1)
        .and_then(|s| s.split('}').next());
    let (Some(num), Some(den)) = (num, den) else {
        return text;
    };
    format!(
        "{}\"meter\": [{num}, {den}],{}",
        &text[..start],
        &rest[end + "],".len()..]
    )
}
