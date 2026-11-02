//! The container's rack panel: layout, display-list goldens of both
//! faces, the OPEN, EDIT and TO CH buttons, double-click to enter, the macro knobs and their
//! names (rename in place), and the "Containers" group of the browser.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test container_panel`.

mod support;

use atomik_model::{Command, ConsoleCommand, ContainerCommand, Document};
use atomik_ui::autolayout::PanelSpec;
use atomik_ui::container_panel::{CONTAINER_UNITS, ContainerButton, container_layout};
use atomik_ui::kurbo::Point;
use atomik_ui::{Flip, Key, RackView, UiAction};
use support::container::*;
use support::*;

/// The rack with console devices hidden (this file is not about them).
fn view(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    plain_view(doc, reg)
}

fn setup() -> (Document, RackView) {
    let d = layered();
    let v = view(d.doc(), &registry());
    (d, v)
}

fn spec() -> PanelSpec {
    let reg = registry();
    let desc = reg.get(CONTAINER).unwrap().descriptor;
    container_layout(desc)
}

// ------------------------------------------------------------------ layout

#[test]
fn the_panel_is_two_units_with_eight_macro_knobs_and_names_below_them() {
    let s = spec();
    assert_eq!(s.height_units, CONTAINER_UNITS);
    let panel = s.container.expect("container panel");
    assert_eq!(s.front.len(), 8);
    let reg = registry();
    let desc = reg.get(CONTAINER).unwrap().descriptor;
    for (i, pc) in s.front.iter().enumerate() {
        assert_eq!(desc.params[pc.param].key, format!("macro{}", i + 1));
        assert!(
            !pc.rect.overlaps(panel.names[i]),
            "name {i} stays out of its knob"
        );
        assert!(pc.rect.x0 >= 0.0 && panel.names[i].x1 <= 760.0);
    }
    for w in panel.names.windows(2) {
        assert!(w[0].x1 <= w[1].x0, "names do not overlap");
    }
    let h = s.height();
    for b in [
        ContainerButton::Open,
        ContainerButton::Edit,
        ContainerButton::ToChannel,
    ] {
        let r = panel.button(b);
        assert!(r.y0 >= 0.0 && r.y1 <= h && r.width() > 20.0, "{b:?}");
    }
    assert!(
        panel.open.x1 <= panel.edit.x0,
        "OPEN and EDIT do not overlap"
    );
    // Five sockets on the back.
    assert_eq!(s.back.len(), 5);
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_matches_golden() {
    let (d, mut v) = setup();
    check(
        "container_panel_front_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_back_matches_golden() {
    let (d, mut v) = setup();
    v.set_flip(Flip::Back);
    check(
        "container_panel_back_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_panel_shows_names_counts_and_the_summary() {
    let (d, mut v) = setup();
    let dl = paint_rack(&mut v, d.doc());
    let texts = dl.texts();
    for t in [
        "OPEN",
        "EDIT",
        "TO CH",
        "Cutoff",
        "Macro 2",
        "Macro 8",
        "MACROS",
        "SPLITS",
        "CONTAINER",
        "4 devices",
        "2 splits · 1 mapped",
        "1",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
}

#[test]
fn an_empty_container_says_so() {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = Document::new(doc);
    apply(
        &mut d,
        Command::AddDevice {
            index: 0,
            type_id: CONTAINER.into(),
            key: Some(key("empty")),
        },
    );
    let mut v = view(d.doc(), &registry());
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(texts.contains("EMPTY") && texts.contains("double-click: unfold"));
    assert!(texts.contains("no splits"));
}

// ------------------------------------------------------------------ buttons

fn center(v: &RackView, b: ContainerButton) -> Point {
    v.container_button_center(&key("layer"), b)
        .unwrap_or_else(|| panic!("no button {b:?}"))
}

#[test]
fn open_enters_the_container_and_reports_the_path() {
    let (d, mut v) = setup();
    let p = center(&v, ContainerButton::Open);
    let a = click(&mut v, d.doc(), p);
    assert!(
        a.contains(&UiAction::EnterContainer {
            device: key("layer")
        }),
        "{a:?}"
    );
    assert_eq!(v.scope(), [key("layer")]);
    assert_eq!(v.scope_depth(), 1);
}

fn set_open_command(open: bool) -> Command {
    Command::Container(atomik_model::ContainerCommand::SetOpen {
        container: key("layer"),
        open,
    })
}

#[test]
fn the_arrow_unfolds_the_container_in_place() {
    let (d, mut v) = setup();
    let p = center(&v, ContainerButton::Fold);
    let a = click(&mut v, d.doc(), p);
    assert_eq!(commands(&a), vec![set_open_command(true)], "{a:?}");
    assert!(v.scope().is_empty(), "the rack does not change its view");
    assert!(
        a.iter()
            .all(|x| !matches!(x, UiAction::EnterContainer { .. })),
        "{a:?}"
    );
}

#[test]
fn the_arrow_of_an_unfolded_container_folds_it() {
    let (mut d, mut v) = setup();
    apply(&mut d, set_open_command(true));
    v.sync(d.doc(), &registry());
    let p = center(&v, ContainerButton::Fold);
    let a = click(&mut v, d.doc(), p);
    assert_eq!(commands(&a), vec![set_open_command(false)], "{a:?}");
}

#[test]
fn a_double_click_on_the_panel_outside_its_controls_unfolds_it_in_place() {
    let (d, mut v) = setup();
    let reg = registry();
    let desc = reg.get(CONTAINER).unwrap().descriptor;
    let s = container_layout(desc);
    let panel = s.container.unwrap();
    let r = v.device_rect(&key("layer")).unwrap();
    let p = v.rack_to_view(panel.summary.center() + atomik_ui::kurbo::Vec2::new(0.0, r.y0));
    let a = double_click(&mut v, d.doc(), p);
    assert_eq!(commands(&a), vec![set_open_command(true)], "{a:?}");
    assert!(v.scope().is_empty());
}

#[test]
fn a_double_click_on_a_knob_resets_it_and_does_not_enter() {
    let (d, mut v) = setup();
    let p = v.control_center(&key("layer"), "macro2").unwrap();
    let a = double_click(&mut v, d.doc(), p);
    assert!(
        a.iter()
            .all(|x| !matches!(x, UiAction::EnterContainer { .. })),
        "{a:?}"
    );
    assert!(v.scope().is_empty());
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: key("layer"),
            param: "macro2".into(),
            value: 0.0,
            merge: None
        }]
    );
}

#[test]
fn edit_opens_the_container_editor() {
    let (d, mut v) = setup();
    let p = center(&v, ContainerButton::Edit);
    let a = click(&mut v, d.doc(), p);
    assert!(
        a.contains(&UiAction::OpenContainerEditor {
            device: key("layer"),
            macros: None
        }),
        "{a:?}"
    );
    assert!(v.scope().is_empty(), "EDIT does not enter");
}

#[test]
fn to_ch_adds_a_channel_with_the_container_as_source_once() {
    let (mut d, mut v) = setup();
    let p = center(&v, ContainerButton::ToChannel);
    let a = click(&mut v, d.doc(), p);
    assert_eq!(
        commands(&a),
        vec![Command::Console(ConsoleCommand::AddChannel {
            source: key("layer")
        })]
    );
    apply(&mut d, commands(&a).remove(0));
    v.sync(d.doc(), &registry());
    // The container feeds a channel now: the button is dim and does nothing.
    let p = center(&v, ContainerButton::ToChannel);
    assert!(commands(&click(&mut v, d.doc(), p)).is_empty());
}

// ------------------------------------------------------------------ macro knobs

#[test]
fn dragging_a_macro_knob_sets_the_containers_macro_param_merged() {
    let (d, mut v) = setup();
    let reg = registry();
    let p = v.control_center(&key("layer"), "macro3").unwrap();
    let _ = v.handle(&down(p), d.doc(), &reg);
    let q = Point::new(p.x, p.y - 30.0);
    let a = v.handle(&mv(q), d.doc(), &reg);
    let end = v.handle(&up(q), d.doc(), &reg);
    let cmds = commands(&a);
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        Command::SetParam {
            key: k,
            param,
            value,
            merge,
        } => {
            assert_eq!(k, &key("layer"));
            assert_eq!(param, "macro3");
            assert!(*value > 0.0 && *value <= 1.0);
            assert!(merge.is_some());
        }
        c => panic!("{c:?}"),
    }
    assert!(end.contains(&UiAction::EndMerge));
}

fn name_point(v: &RackView, n: u8) -> Point {
    v.macro_name_center(&key("layer"), n)
        .unwrap_or_else(|| panic!("no name {n}"))
}

#[test]
fn a_double_click_on_a_name_renames_the_macro_in_place() {
    let (d, mut v) = setup();
    let p = name_point(&v, 1);
    let _ = double_click(&mut v, d.doc(), p);
    assert_eq!(v.renaming_macro(&key("layer")), Some((1, "")));
    // The panel shows the text with a caret.
    type_keys(&mut v, d.doc(), "Drive");
    assert_eq!(v.renaming_macro(&key("layer")), Some((1, "Drive")));
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(texts.contains("Drive|"));
    let a = send_key(&mut v, d.doc(), Key::Enter);
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::RenameMacro {
                container: key("layer"),
                index: 1,
                name: "Drive".into()
            }
        ))]
    );
    assert_eq!(v.renaming_macro(&key("layer")), None);
}

#[test]
fn renaming_starts_from_the_stored_name_and_escape_or_a_click_elsewhere_cancels() {
    let (d, mut v) = setup();
    let p = name_point(&v, 0);
    let _ = double_click(&mut v, d.doc(), p);
    assert_eq!(v.renaming_macro(&key("layer")), Some((0, "Cutoff")));
    assert!(send_key(&mut v, d.doc(), Key::Escape).is_empty());
    assert_eq!(v.renaming_macro(&key("layer")), None);
    // An unchanged name emits nothing.
    let _ = double_click(&mut v, d.doc(), p);
    assert!(send_key(&mut v, d.doc(), Key::Enter).is_empty());
    // A click elsewhere cancels.
    let _ = double_click(&mut v, d.doc(), p);
    let _ = click(&mut v, d.doc(), Point::new(5.0, 5.0));
    assert_eq!(v.renaming_macro(&key("layer")), None);
}

#[test]
fn macro_names_are_cut_at_24_characters_and_can_be_cleared() {
    let (d, mut v) = setup();
    let p = name_point(&v, 0);
    let _ = double_click(&mut v, d.doc(), p);
    for _ in 0..6 {
        let _ = send_key(&mut v, d.doc(), Key::Backspace);
    }
    type_keys(&mut v, d.doc(), &"x".repeat(40));
    assert_eq!(
        v.renaming_macro(&key("layer")).unwrap().1.chars().count(),
        24
    );
    for _ in 0..24 {
        let _ = send_key(&mut v, d.doc(), Key::Backspace);
    }
    let a = send_key(&mut v, d.doc(), Key::Enter);
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::RenameMacro {
                container: key("layer"),
                index: 0,
                name: String::new()
            }
        ))],
        "an empty name clears the macro name"
    );
}

#[test]
fn a_rename_golden() {
    let (d, mut v) = setup();
    let p = name_point(&v, 2);
    let _ = double_click(&mut v, d.doc(), p);
    type_keys(&mut v, d.doc(), "Space");
    check(
        "container_panel_rename_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

// ------------------------------------------------------------------ misc

#[test]
fn a_folded_container_has_no_buttons() {
    let (mut d, mut v) = setup();
    apply(
        &mut d,
        Command::SetFolded {
            key: key("layer"),
            folded: true,
        },
    );
    v.sync(d.doc(), &registry());
    assert!(
        v.container_button_center(&key("layer"), ContainerButton::Open)
            .is_none()
    );
}
