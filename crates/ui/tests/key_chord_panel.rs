//! The Key & Chord rack panel: the generated layout with the FROM CLIP button, display-list
//! goldens (button enabled and disabled), and the click that copies the scale of the clip open
//! in the piano roll into `scale.root` and `scale.kind` as one command.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test key_chord_panel`.

mod support;

use atomik_model::theory::{Scale, ScaleKind};
use atomik_model::{Command, RackDoc};
use atomik_ui::autolayout::CONTENT_X;
use atomik_ui::key_chord_panel::{KEY_CHORD_TYPE, key_chord_layout};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{DisplayList, RackView, UiAction};
use std::path::PathBuf;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
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

fn key_chord_only() -> RackDoc {
    let mut doc = reference_doc();
    doc.devices = vec![device("kc", KEY_CHORD_TYPE, "Key & Chord")];
    doc.cables.clear();
    doc
}

fn setup() -> (RackDoc, RackView) {
    let doc = key_chord_only();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), doc, &reg);
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn commands(actions: &[UiAction]) -> Vec<&Command> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(c) => Some(c),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_without_a_clip_scale_matches_golden() {
    let (doc, mut v) = setup();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("key_chord_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_with_a_clip_scale_matches_golden() {
    let (doc, mut v) = setup();
    v.set_clip_scale(Some(Scale::new(2, ScaleKind::Dorian)));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("key_chord_panel_clip_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_and_the_button_overlaps_nothing() {
    let reg = registry();
    let desc = reg.get(KEY_CHORD_TYPE).unwrap().descriptor;
    let spec = key_chord_layout(desc);
    // `bypass` is the strip switch; the other nine are controls.
    let mut keys: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    keys.sort_unstable();
    let mut want: Vec<&str> = desc
        .params
        .iter()
        .map(|p| p.key)
        .filter(|k| *k != "bypass")
        .collect();
    want.sort_unstable();
    assert_eq!(keys, want);
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    let button = spec.key_chord.expect("key chord panel").from_clip;
    let content = Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    assert!(
        button.x0 >= content.x0 && button.x1 <= content.x1 && button.y0 >= 0.0,
        "{button:?} outside {content:?}"
    );
    assert!(button.y1 <= content.y1, "{button:?} outside {content:?}");
    for c in &spec.front {
        let r = c.rect;
        let overlap = button.x0 < r.x1 && r.x0 < button.x1 && button.y0 < r.y1 && r.y0 < button.y1;
        assert!(!overlap, "{button:?} overlaps {r:?}");
    }
    assert!(spec.seq.is_none() && spec.arp.is_none() && spec.samples.is_none());
}

#[test]
fn the_panel_shows_the_sections_and_the_button() {
    let (doc, mut v) = setup();
    let t = texts_of(&mut v, &doc);
    for s in ["SCALE", "CHORD", "FROM CLIP"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
}

// ------------------------------------------------------------------ FROM CLIP

#[test]
fn from_clip_sends_one_batch_of_root_and_kind() {
    let (doc, mut v) = setup();
    // D dorian: root 2, kind index 5.
    v.set_clip_scale(Some(Scale::new(2, ScaleKind::Dorian)));
    let p = v.from_clip_button_center(&key("kc")).expect("button");
    let a = click(&mut v, &doc, p);
    let cmds = commands(&a);
    assert_eq!(cmds.len(), 1, "{a:?}");
    let Command::Batch(items) = cmds[0] else {
        panic!("a batch: {:?}", cmds[0])
    };
    let set = |param: &str, value: f32| Command::SetParam {
        key: key("kc"),
        param: param.into(),
        value,
        merge: None,
    };
    assert_eq!(items, &vec![set("scale.root", 2.0), set("scale.kind", 5.0)]);
}

#[test]
fn from_clip_is_disabled_without_a_clip_scale() {
    let (doc, mut v) = setup();
    let p = v.from_clip_button_center(&key("kc")).expect("button");
    assert!(commands(&click(&mut v, &doc, p)).is_empty());
    v.set_clip_scale(Some(Scale::new(0, ScaleKind::Major)));
    assert_eq!(commands(&click(&mut v, &doc, p)).len(), 1);
    v.set_clip_scale(None);
    assert!(commands(&click(&mut v, &doc, p)).is_empty());
}
