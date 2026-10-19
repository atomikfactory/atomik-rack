//! The drum machine's rack panel: the 4x4 pad grid, the knobs of the selected
//! pad, master level, the Edit button, drops onto pads and the background, the back panel with
//! 35 reachable sockets, and display-list goldens of both faces. Other devices' panels are
//! covered by their own goldens (they must not change).
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test drum_panel`.

mod support;

use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, Flip, PointerButton, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::drum::*;
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

fn rack() -> (RackDoc, RackView) {
    let (doc, reg) = (drum_doc(), registry());
    let v = view(&doc, &reg);
    (doc, v)
}

fn drum_only() -> RackDoc {
    let mut doc = drum_doc();
    doc.devices.retain(|d| d.key == key("drums"));
    doc.cables.clear();
    doc
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), doc, &reg);
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

/// A point of the drum panel that is neither a pad, a control, the plate nor a button: the
/// left rack ear.
fn body_point(v: &RackView) -> Point {
    let r = v.device_rect(&key("drums")).unwrap();
    v.rack_to_view(Point::new(r.x0 + 6.0, r.center().y))
}

fn auditions(a: &[UiAction]) -> Vec<(u8, u8, bool)> {
    a.iter()
        .filter_map(|x| match x {
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => {
                assert_eq!(device, &support::key("drums"));
                Some((*key, *velocity, *on))
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let doc = drum_only();
    let reg = registry();
    let mut v = view(&doc, &reg);
    v.set_selected_pad(&key("drums"), 3);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("drum_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_with_a_loaded_pad_selected_and_a_drop_hover_matches_golden() {
    let doc = drum_only();
    let reg = registry();
    let mut v = view(&doc, &reg);
    v.set_selected_pad(&key("drums"), 2);
    let on = v.pad_center(&key("drums"), 5).unwrap();
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on,
            paths: vec!["a.wav".into()],
        },
        &doc,
        &reg,
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("drum_panel_drop_1x.json", &dl.to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let doc = drum_only();
    let reg = registry();
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("drum_panel_back_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ front

#[test]
fn the_panel_shows_pad_labels_the_selected_pad_and_the_edit_button() {
    let (doc, mut v) = rack();
    let t = texts_of(&mut v, &doc);
    for s in [
        "PADS",
        "PAD 1",
        "MASTER",
        "EDIT",
        "kick_808",
        "snare_soft",
        "1",
        "16",
        "PAD 1  ·  C2",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(
        !t.iter().any(|x| x == "SAMPLES"),
        "no sample strip on the drum panel: {t:?}"
    );
    assert_eq!(v.selected_pad(&key("drums")), Some(1));
    assert_eq!(v.selected_pad(&key("synth")), None);
    // Three rack units, no 129-knob sprawl.
    let r = v.device_rect(&key("drums")).unwrap();
    assert!((r.height() - 3.0 * 76.0).abs() < 1e-9, "{r:?}");
}

#[test]
fn the_knobs_follow_the_selected_pad() {
    let (doc, mut v) = rack();
    let d = key("drums");
    for suffix in [
        "level", "pan", "tune", "fine", "decay", "mode", "choke", "velocity",
    ] {
        assert!(
            v.control_center(&d, &format!("pad1.{suffix}")).is_some(),
            "{suffix}"
        );
        assert!(v.control_center(&d, &format!("pad2.{suffix}")).is_none());
    }
    assert!(v.control_center(&d, "master.level").is_some());
    let at_pad1 = v.control_center(&d, "pad1.pan").unwrap();
    v.set_selected_pad(&d, 2);
    assert_eq!(v.selected_pad(&d), Some(2));
    assert!(v.control_center(&d, "pad1.pan").is_none());
    assert_eq!(
        v.control_center(&d, "pad2.pan"),
        Some(at_pad1),
        "same place, other pad"
    );
    // Only the selected pad's 8 knobs and the master are on the panel.
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "PAD 2"), "{t:?}");
    assert!(t.iter().any(|x| x == "PAD 2  ·  C#2"), "{t:?}");
    // Dragging a knob edits the selected pad's param.
    let reg = registry();
    let p = v.control_center(&d, "pad2.level").unwrap();
    let mut cmds = Vec::new();
    for e in [
        down(p),
        mv(p + atomik_ui::kurbo::Vec2::new(0.0, -20.0)),
        up(p),
    ] {
        cmds.extend(commands(&v.handle(&e, &doc, &reg)));
    }
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::SetParam { key: k, param, .. }
            if *k == d && param == "pad2.level")),
        "{cmds:?}"
    );
}

#[test]
fn clicking_a_pad_selects_it_and_auditions_it_until_release() {
    let (doc, mut v) = rack();
    let reg = registry();
    let d = key("drums");
    let p = v.pad_center(&d, 7).unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    assert_eq!(v.selected_pad(&d), Some(7));
    assert_eq!(auditions(&a), vec![(42, 100, true)]);
    assert!(a.contains(&UiAction::Select(d.clone())));
    assert!(commands(&a).is_empty());
    let a = v.handle(&up(p), &doc, &reg);
    assert_eq!(auditions(&a), vec![(42, 100, false)]);
    // Pad 16 is key 51; pad 1 is the bottom-left pad.
    let p16 = v.pad_center(&d, 16).unwrap();
    let a = click(&mut v, &doc, p16);
    assert_eq!(auditions(&a), vec![(51, 100, true), (51, 100, false)]);
    let (p1, p2, p5) = (
        v.pad_center(&d, 1).unwrap(),
        v.pad_center(&d, 2).unwrap(),
        v.pad_center(&d, 5).unwrap(),
    );
    assert!(
        p1.x < p2.x && (p1.y - p2.y).abs() < 1e-9,
        "numbers run left to right"
    );
    assert!(
        p5.y < p1.y && (p5.x - p1.x).abs() < 1e-9,
        "and upwards from the bottom"
    );
    // The selection is view state: it never reaches the document.
    assert_eq!(doc, drum_doc());
}

#[test]
fn the_edit_button_opens_the_step_editor_and_leaves_the_clip_to_the_app() {
    let (doc, mut v) = rack();
    let d = key("drums");
    let p = v.edit_button_center(&d).unwrap();
    let a = click(&mut v, &doc, p);
    assert!(
        a.contains(&UiAction::OpenStepEditor {
            device: d.clone(),
            clip: None
        }),
        "{a:?}"
    );
    assert!(!a.iter().any(|x| matches!(x, UiAction::OpenSampleEditor(_))));
    assert!(commands(&a).is_empty());
    v.set_flip(Flip::Back);
    assert!(v.edit_button_center(&d).is_none(), "front only");
}

#[test]
fn double_click_on_the_pad_grid_background_opens_the_editor_but_a_pad_does_not() {
    let (doc, mut v) = rack();
    let reg = registry();
    let d = key("drums");
    let open = UiAction::OpenStepEditor {
        device: d.clone(),
        clip: None,
    };
    let gap = v.pad_grid_background(&d).unwrap();
    let a = click(&mut v, &doc, gap);
    assert!(!a.contains(&open), "single click: {a:?}");
    let a = v.handle(&down_with(gap, PointerButton::Primary, 2), &doc, &reg);
    assert!(a.contains(&open), "{a:?}");
    let _ = v.handle(&up(gap), &doc, &reg);
    let pad = v.pad_center(&d, 4).unwrap();
    let a = v.handle(&down_with(pad, PointerButton::Primary, 2), &doc, &reg);
    assert!(
        !a.contains(&open),
        "a double-click on a pad only plays it: {a:?}"
    );
    let _ = v.handle(&up(pad), &doc, &reg);
    // The panel body does not open the sample editor for a drum machine.
    let body = body_point(&v);
    let a = v.handle(&down_with(body, PointerButton::Primary, 2), &doc, &reg);
    assert!(
        !a.iter().any(|x| matches!(
            x,
            UiAction::OpenSampleEditor(_) | UiAction::OpenStepEditor { .. }
        )),
        "{a:?}"
    );
}

#[test]
fn files_dropped_on_a_pad_load_that_pad_and_on_the_background_fill_empty_pads() {
    let (doc, mut v) = rack();
    let reg = registry();
    let d = key("drums");
    let paths: Vec<PathBuf> = vec!["a.wav".into(), "b.wav".into()];
    let on_pad = v.pad_center(&d, 9).unwrap();
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on_pad,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert_eq!(v.drop_pad(), Some((&d, 9)));
    assert_eq!(v.drop_hover(), Some(&d));
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: on_pad,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert_eq!(
        a,
        vec![UiAction::ImportSamples {
            paths: paths.clone(),
            device: d.clone(),
            at_key: Some(44),
        }]
    );
    assert_eq!((v.drop_pad(), v.drop_hover()), (None, None));
    // The background: the rack ear and the gap between pads.
    for bg in [body_point(&v), v.pad_grid_background(&d).unwrap()] {
        let _ = v.handle(
            &UiEvent::FileHover {
                pos: bg,
                paths: paths.clone(),
            },
            &doc,
            &reg,
        );
        assert_eq!(v.drop_pad(), None);
        assert_eq!(v.drop_hover(), Some(&d));
        let a = v.handle(
            &UiEvent::FileDrop {
                pos: bg,
                paths: paths.clone(),
            },
            &doc,
            &reg,
        );
        assert_eq!(
            a,
            vec![UiAction::ImportSamples {
                paths: paths.clone(),
                device: d.clone(),
                at_key: None
            }]
        );
    }
    // Cancel clears both indicators; a drop elsewhere (the synth) imports nothing.
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: on_pad,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!((v.drop_pad(), v.drop_hover()), (None, None));
    let synth = v.rack_to_view(Point::new(
        10.0,
        v.device_rect(&key("synth")).unwrap().center().y,
    ));
    let a = v.handle(&UiEvent::FileDrop { pos: synth, paths }, &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn the_selected_pad_is_not_lost_when_the_document_changes() {
    let (mut doc, mut v) = rack();
    let reg = registry();
    let d = key("drums");
    v.set_selected_pad(&d, 11);
    doc.devices[0].params.insert("master.level".into(), -12.0);
    v.sync(&doc, &reg);
    assert_eq!(v.selected_pad(&d), Some(11));
    assert!(v.control_center(&d, "pad11.decay").is_some());
}

// ------------------------------------------------------------------ back

#[test]
fn all_35_sockets_are_reachable_on_the_back_and_stay_inside_the_panel() {
    let (doc, mut v) = rack();
    v.set_flip(Flip::Back);
    let d = key("drums");
    let mut names = vec!["notes_in".to_string(), "out_l".into(), "out_r".into()];
    for n in 1..=16 {
        names.push(format!("pad{n}_out_l"));
        names.push(format!("pad{n}_out_r"));
    }
    assert_eq!(names.len(), 35);
    let panel = v.device_rect(&d).unwrap();
    let mut centres = Vec::new();
    for n in &names {
        let c = v
            .socket_center(&sref("drums", n))
            .unwrap_or_else(|| panic!("socket {n} has no jack on the back"));
        let rp = v.view_to_rack(c);
        assert!(panel.contains(rp), "{n} at {rp:?} is outside {panel:?}");
        centres.push((n.clone(), c));
    }
    for (i, (na, a)) in centres.iter().enumerate() {
        for (nb, b) in &centres[i + 1..] {
            assert!(a.distance(*b) > 30.0, "{na} and {nb} overlap: {a:?} {b:?}");
        }
    }
    let t = texts_of(&mut v, &doc);
    for s in ["PAD 1", "PAD 8", "PAD 9", "PAD 16"] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
}

#[test]
fn a_pad_output_can_be_cabled_to_a_mixer_input() {
    let (doc, mut v) = rack();
    let reg = registry();
    v.set_flip(Flip::Back);
    let from = v.socket_center(&sref("drums", "pad16_out_r")).unwrap();
    let to = v.socket_center(&sref("mixer", "ch2_in_l")).unwrap();
    let mut cmds = Vec::new();
    for e in [down(from), mv(from.midpoint(to)), mv(to), up(to)] {
        cmds.extend(commands(&v.handle(&e, &doc, &reg)));
    }
    assert!(
        cmds.iter().any(|c| matches!(c, Command::Connect(cab)
            if cab.from == sref("drums", "pad16_out_r") && cab.to == sref("mixer", "ch2_in_l"))),
        "{cmds:?}"
    );
}

#[test]
fn other_devices_have_no_drum_panel() {
    let (doc, mut v) = rack();
    let t = texts_of(&mut v, &doc);
    assert_eq!(
        t.iter().filter(|x| *x == "EDIT").count(),
        1,
        "only the drum machine: {t:?}"
    );
    assert_eq!(v.selected_pad(&key("synth")), None);
    assert!(v.pad_center(&key("synth"), 1).is_none());
}

// ------------------------------------------------------------------ TO CH

#[test]
fn the_to_channel_button_gives_the_selected_pad_its_own_channel() {
    let (doc, mut v) = rack();
    let d = key("drums");
    assert!(texts_of(&mut v, &doc).iter().any(|x| x == "TO CH"));
    v.set_selected_pad(&d, 3);
    let p = v.to_channel_button_center(&d).unwrap();
    let a = click(&mut v, &doc, p);
    assert_eq!(
        commands(&a),
        [Command::Console(
            atomik_model::ConsoleCommand::AddChannelFor {
                source: d.clone(),
                out_l: "pad3_out_l".into(),
                out_r: "pad3_out_r".into(),
                name: None,
            }
        )],
        "{a:?}"
    );
    v.set_flip(Flip::Back);
    assert!(v.to_channel_button_center(&d).is_none(), "front only");
}

#[test]
fn the_to_channel_button_is_dim_and_silent_when_the_pad_is_already_cabled() {
    let mut doc = drum_doc();
    doc.cables
        .push(cable(("drums", "pad3_out_l"), ("mixer", "ch2_in_l")));
    doc.cables.sort();
    let reg = registry();
    let mut v = view(&doc, &reg);
    let d = key("drums");
    v.set_selected_pad(&d, 3);
    let p = v.to_channel_button_center(&d).unwrap();
    assert!(commands(&click(&mut v, &doc, p)).is_empty());
    // Another pad is not cabled: the button works again.
    v.set_selected_pad(&d, 4);
    assert_eq!(commands(&click(&mut v, &doc, p)).len(), 1);
    // The dim state is a different picture.
    let mut cabled = DisplayList::new();
    v.set_selected_pad(&d, 3);
    v.paint(&mut cabled, &doc, &reg);
    let mut free = DisplayList::new();
    let mut doc2 = doc.clone();
    doc2.cables.retain(|c| c.from.socket != "pad3_out_l");
    v.sync(&doc2, &reg);
    v.paint(&mut free, &doc2, &reg);
    assert_ne!(cabled.to_json(), free.to_json());
}
