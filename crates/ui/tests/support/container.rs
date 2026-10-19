//! Container fixtures: a document with a container `layer` (a synth, a second synth
//! `pad`, a delay cabled to the boundary output, an uncabled reverb, a split per synth and a
//! macro on the synth's cutoff) next to a top-level synth, and the helpers the container tests
//! share.

use super::*;
use atomik_model::{Command, ContainerCommand, Document, MacroTarget, NoteSplit};

pub const CONTAINER: &str = "atomik.container";
pub const SYNTH: &str = "atomik.poly-synth";
pub const DELAY: &str = "atomik.stereo-delay";
pub const REVERB: &str = "atomik.reverb";

pub fn apply(d: &mut Document, c: Command) {
    let reg = registry();
    d.apply(c, &reg)
        .unwrap_or_else(|e| panic!("fixture command refused: {e}"));
}

pub fn add_inner(d: &mut Document, container: &str, index: usize, type_id: &str, k: &str) {
    apply(
        d,
        Command::Container(ContainerCommand::AddInner {
            container: key(container),
            index,
            type_id: type_id.to_string(),
            key: Some(key(k)),
            auto_split: false,
        }),
    );
}

pub fn connect(d: &mut Document, from: (&str, &str), to: (&str, &str)) {
    apply(d, Command::Connect(cable(from, to)));
}

pub fn split(target: &str, lo: u8, hi: u8, transpose: i8) -> NoteSplit {
    NoteSplit {
        target: key(target),
        key_lo: lo,
        key_hi: hi,
        vel_lo: 1,
        vel_hi: 127,
        transpose,
    }
}

pub fn target(device: &str, param: &str, min: f32, max: f32) -> MacroTarget {
    MacroTarget {
        device: key(device),
        param: param.to_string(),
        min,
        max,
    }
}

/// A top-level synth `lead` and a container `layer` (named "Layer").
pub fn layered() -> Document {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = Document::new(doc);
    apply(
        &mut d,
        Command::AddDevice {
            index: 0,
            type_id: SYNTH.into(),
            key: Some(key("lead")),
        },
    );
    apply(
        &mut d,
        Command::AddDevice {
            index: 1,
            type_id: CONTAINER.into(),
            key: Some(key("layer")),
        },
    );
    apply(
        &mut d,
        Command::Rename {
            key: key("layer"),
            name: "Layer".into(),
        },
    );
    add_inner(&mut d, "layer", 0, SYNTH, "synth");
    add_inner(&mut d, "layer", 1, SYNTH, "pad");
    add_inner(&mut d, "layer", 2, DELAY, "delay");
    add_inner(&mut d, "layer", 3, REVERB, "reverb");
    connect(&mut d, ("layer/synth", "out_l"), ("layer/delay", "in_l"));
    connect(&mut d, ("layer/synth", "out_r"), ("layer/delay", "in_r"));
    connect(
        &mut d,
        ("layer/delay", "out_l"),
        ("layer/boundary", "out_l"),
    );
    connect(
        &mut d,
        ("layer/delay", "out_r"),
        ("layer/boundary", "out_r"),
    );
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetSplits {
            container: key("layer"),
            splits: vec![split("synth", 0, 59, 0), split("pad", 60, 127, 12)],
            merge: None,
        }),
    );
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: target("synth", "filter.cutoff", 200.0, 8000.0),
            merge: None,
        }),
    );
    apply(
        &mut d,
        Command::Container(ContainerCommand::RenameMacro {
            container: key("layer"),
            index: 0,
            name: "Cutoff".into(),
        }),
    );
    d
}

/// [`layered`] with a container `fx` inside `layer`, and one `deep` inside `layer/fx`, and one
/// `deeper` inside `layer/fx/deep` (three levels).
pub fn nested() -> Document {
    let mut d = layered();
    add_inner(&mut d, "layer", 4, CONTAINER, "fx");
    add_inner(&mut d, "layer/fx", 0, CONTAINER, "deep");
    add_inner(&mut d, "layer/fx/deep", 0, DELAY, "echo");
    for (k, name) in [("layer/fx", "FX"), ("layer/fx/deep", "Deep")] {
        apply(
            &mut d,
            Command::Rename {
                key: key(k),
                name: name.into(),
            },
        );
    }
    d
}

pub fn path(segments: &[&str]) -> Vec<DeviceKey> {
    segments.iter().map(|s| key(s)).collect()
}

// ------------------------------------------------------------------ gesture helpers

use atomik_model::RackDoc;
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use std::path::PathBuf;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

/// Compares `actual` with the golden `name` (regenerate with `ATOMIK_UPDATE_GOLDEN=1`).
pub fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

/// A press and release at `p` with `clicks` (1 or 2) and `button`.
pub fn press_with(
    v: &mut RackView,
    doc: &RackDoc,
    p: Point,
    button: PointerButton,
    clicks: u8,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button,
            modifiers,
            clicks,
        },
        doc,
        &reg,
    );
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

pub fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    press_with(v, doc, p, PointerButton::Primary, 1, Modifiers::NONE)
}

pub fn double_click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    press_with(v, doc, p, PointerButton::Primary, 2, Modifiers::NONE)
}

pub fn right_click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    press_with(v, doc, p, PointerButton::Secondary, 1, Modifiers::NONE)
}

pub fn type_keys(v: &mut RackView, doc: &RackDoc, text: &str) {
    let reg = registry();
    for ch in text.chars() {
        let _ = v.handle(&key_event(atomik_ui::Key::Char(ch)), doc, &reg);
    }
}

pub fn key_event(k: atomik_ui::Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

pub fn send_key(v: &mut RackView, doc: &RackDoc, k: atomik_ui::Key) -> Vec<UiAction> {
    v.handle(&key_event(k), doc, &registry())
}

pub fn paint_rack(v: &mut RackView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

/// A fresh synced view of `doc`, 1000 x 1200 logical, entered into `scope`.
pub fn scoped_view(doc: &RackDoc, scope: &[&str]) -> RackView {
    let reg = registry();
    // The rack with console devices hidden.
    let mut v = plain_view(doc, &reg);
    v.set_scope(&path(scope));
    v.sync(doc, &reg);
    v
}
