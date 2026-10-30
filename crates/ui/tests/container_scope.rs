//! The rack view inside a container: enter and leave (OPEN,
//! double-click, breadcrumb, Back, Esc, Backspace), the boundary panels and cables, path-addressed
//! commands, the add menu filter, the "map to macro" menu, locked controls, and the vanishing
//! container. Display-list goldens `container_scope_*`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test container_scope`.

mod support;

use atomik_model::{Command, ContainerCommand, Document, MacroTarget};
use atomik_ui::breadcrumb::CrumbHit;
use atomik_ui::container_panel::ContainerButton;
use atomik_ui::kurbo::Point;
use atomik_ui::{Flip, Key, PointerButton, RackView, UiAction};
use support::container::*;
use support::*;

/// The rack with console devices hidden (this file is not about them).
fn view(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    plain_view(doc, reg)
}

fn enter(d: &Document, v: &mut RackView, container: &str) -> Vec<UiAction> {
    let p = v
        .container_button_center(&key(container), ContainerButton::Open)
        .expect("OPEN button");
    click(v, d.doc(), p)
}

fn labels(v: &RackView) -> Vec<String> {
    v.breadcrumb_labels()
}

fn keys_in_view(v: &RackView) -> Vec<String> {
    v.panel_revisions().into_iter().map(|(k, _)| k.0).collect()
}

// ------------------------------------------------------------------ entering and leaving

#[test]
fn the_inside_shows_the_boundary_panels_around_the_inner_devices_in_rack_order() {
    let d = layered();
    let mut v = view(d.doc(), &registry());
    assert_eq!(keys_in_view(&v), ["interface", "lead", "layer"]);
    let _ = enter(&d, &mut v, "layer");
    assert_eq!(
        keys_in_view(&v),
        [
            "boundary",
            "synth",
            "pad",
            "delay",
            "reverb",
            "boundary~out"
        ]
    );
    assert_eq!(labels(&v), ["Project", "Layer"]);
    // The INPUT panel is at the top, the OUTPUT panel under the last device.
    let top = v.device_rect(&key("boundary")).unwrap();
    let bottom = v.device_rect(&key("boundary~out")).unwrap();
    let last = v.device_rect(&key("reverb")).unwrap();
    assert!(top.y1 <= v.device_rect(&key("synth")).unwrap().y0 + 1e-6);
    assert!(bottom.y0 >= last.y1 - 1e-6);
}

#[test]
fn leaving_restores_the_top_level_exactly() {
    let d = layered();
    let reg = registry();
    let mut v = view(d.doc(), &reg);
    let before = v.panel_revisions().len();
    let _ = enter(&d, &mut v, "layer");
    let p = v.breadcrumb_center(CrumbHit::Segment(0)).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert_eq!(a, vec![UiAction::ExitContainer { levels: 1 }]);
    assert!(v.scope().is_empty());
    assert_eq!(v.panel_revisions().len(), before);
    assert!(v.breadcrumb_center(CrumbHit::Back).is_none());
}

#[test]
fn back_esc_and_backspace_go_up_one_level() {
    let d = nested();
    let reg = registry();
    // Back button.
    let mut v = scoped_view(d.doc(), &["layer", "fx"]);
    let p = v.breadcrumb_center(CrumbHit::Back).unwrap();
    assert_eq!(
        click(&mut v, d.doc(), p),
        vec![UiAction::ExitContainer { levels: 1 }]
    );
    assert_eq!(v.scope(), [key("layer")]);
    // Esc with nothing selected.
    let a = send_key(&mut v, d.doc(), Key::Escape);
    assert_eq!(a, vec![UiAction::ExitContainer { levels: 1 }]);
    assert!(v.scope().is_empty());
    // Esc does nothing at the top level; Backspace with a selection deletes it instead.
    assert!(send_key(&mut v, d.doc(), Key::Escape).is_empty());
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.plate_center(&key("synth")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    assert_eq!(v.selected(), Some(&key("synth")));
    assert!(
        send_key(&mut v, d.doc(), Key::Escape)
            .iter()
            .all(|a| !matches!(a, UiAction::ExitContainer { .. })),
        "Esc with a selection does not leave"
    );
    // It clears the selection instead; a second Esc then goes up a level.
    assert!(v.selection().is_empty());
    let p = v.plate_center(&key("synth")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    assert_eq!(v.selected(), Some(&key("synth")));
    // Deleting is the `edit.delete` command now (the key resolves to it in the shell), and its
    // device key is lifted out of the scope like any other action.
    let a = v.command("edit.delete", None, d.doc(), &reg);
    assert_eq!(
        commands(a.actions()),
        vec![Command::RemoveDevice {
            key: key("layer/synth")
        }]
    );
    assert!(
        commands(&send_key(&mut v, d.doc(), Key::Backspace)).is_empty(),
        "the key itself no longer deletes"
    );
    // Backspace with nothing selected leaves.
    let mut v = scoped_view(d.doc(), &["layer"]);
    let a = v.handle(&key_event(Key::Backspace), d.doc(), &reg);
    assert_eq!(a, vec![UiAction::ExitContainer { levels: 1 }]);
    assert!(v.scope().is_empty());
}

#[test]
fn a_breadcrumb_segment_goes_up_several_levels() {
    let d = nested();
    let mut v = scoped_view(d.doc(), &["layer", "fx", "deep"]);
    assert_eq!(labels(&v), ["Project", "Layer", "FX", "Deep"]);
    let p = v.breadcrumb_center(CrumbHit::Segment(1)).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert_eq!(a, vec![UiAction::ExitContainer { levels: 2 }]);
    assert_eq!(v.scope(), [key("layer")]);
    // The current segment does nothing.
    let p = v.breadcrumb_center(CrumbHit::Segment(1)).unwrap();
    assert!(click(&mut v, d.doc(), p).is_empty());
    assert_eq!(v.scope(), [key("layer")]);
}

#[test]
fn entering_a_nested_container_reports_its_path() {
    let d = nested();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let a = enter(&d, &mut v, "fx");
    assert!(
        a.contains(&UiAction::EnterContainer {
            device: key("layer/fx")
        }),
        "{a:?}"
    );
    assert_eq!(v.scope(), [key("layer"), key("fx")]);
    assert_eq!(keys_in_view(&v), ["boundary", "deep", "boundary~out"]);
}

#[test]
fn the_view_pops_to_the_nearest_scope_that_still_exists() {
    let mut d = nested();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer", "fx", "deep"]);
    // Remove "fx": the view falls back to "layer".
    apply(
        &mut d,
        Command::RemoveDevice {
            key: key("layer/fx"),
        },
    );
    v.sync(d.doc(), &reg);
    assert_eq!(v.scope(), [key("layer")]);
    // The next event reports it.
    let a = v.handle(&mv(Point::new(1.0, 1.0)), d.doc(), &reg);
    assert!(a.contains(&UiAction::ExitContainer { levels: 2 }), "{a:?}");
    // Remove "layer" too: back to the top level.
    apply(&mut d, Command::RemoveDevice { key: key("layer") });
    v.sync(d.doc(), &reg);
    assert!(v.scope().is_empty());
    assert_eq!(keys_in_view(&v), ["interface", "lead"]);
    // Undo brings the container back; the view stays where it is.
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.scope().is_empty());
}

#[test]
fn set_scope_accepts_a_path_and_a_missing_scope_is_left_on_the_next_sync() {
    let d = nested();
    let reg = registry();
    let mut v = view(d.doc(), &reg);
    v.set_scope(&[key("layer/fx")]);
    v.sync(d.doc(), &reg);
    assert_eq!(v.scope(), [key("layer"), key("fx")]);
    v.set_scope(&[key("nope")]);
    v.sync(d.doc(), &reg);
    assert!(v.scope().is_empty());
}

// ------------------------------------------------------------------ path-addressed commands

#[test]
fn commands_inside_a_scope_carry_path_keys() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    // A knob drag.
    let p = v.control_center(&key("pad"), "filter.resonance").unwrap();
    let _ = v.handle(&down(p), d.doc(), &reg);
    let a = v.handle(&mv(Point::new(p.x, p.y - 20.0)), d.doc(), &reg);
    let c = commands(&a);
    assert_eq!(c.len(), 1);
    match &c[0] {
        Command::SetParam { key: k, param, .. } => {
            assert_eq!(k, &key("layer/pad"));
            assert_eq!(param, "filter.resonance");
        }
        c => panic!("{c:?}"),
    }
    let _ = v.handle(&up(p), d.doc(), &reg);
    // Selecting, folding and reordering use path keys as well; the view's own selection is the
    // local key.
    let fold = v.fold_button_center(&key("pad")).unwrap();
    let a = click(&mut v, d.doc(), fold);
    assert_eq!(
        commands(&a),
        vec![Command::SetFolded {
            key: key("layer/pad"),
            folded: true
        }]
    );
    let plate = v.plate_center(&key("delay")).unwrap();
    let a = click(&mut v, d.doc(), plate);
    assert_eq!(a, vec![UiAction::Select(key("layer/delay"))]);
    assert_eq!(v.selected(), Some(&key("delay")));
    // Drag the delay plate above the synth.
    let top = v.device_rect(&key("synth")).unwrap().y0 + 2.0;
    let q = v.rack_to_view(Point::new(380.0, top));
    let _ = v.handle(&down(plate), d.doc(), &reg);
    let _ = v.handle(&mv(q), d.doc(), &reg);
    let a = v.handle(&up(q), d.doc(), &reg);
    assert_eq!(
        commands(&a),
        vec![Command::MoveDevice {
            key: key("layer/delay"),
            to: 0
        }]
    );
}

#[test]
fn every_emitted_command_is_accepted_by_the_model() {
    let mut d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    for dev in ["synth", "pad", "delay", "reverb"] {
        let p = v.control_center(&key(dev), "mix");
        let Some(p) = p.or_else(|| v.control_center(&key(dev), "level")) else {
            continue;
        };
        let _ = v.handle(&down(p), d.doc(), &reg);
        let a = v.handle(&mv(Point::new(p.x, p.y - 15.0)), d.doc(), &reg);
        let end = v.handle(&up(p), d.doc(), &reg);
        for c in commands(&a) {
            d.apply(c, &reg).unwrap_or_else(|e| panic!("{dev}: {e}"));
        }
        assert!(end.contains(&UiAction::EndMerge));
    }
}

// ------------------------------------------------------------------ boundary and cables

#[test]
fn the_boundary_sockets_take_part_in_cables() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_flip(Flip::Back);
    let shown = v.visible_cables(d.doc());
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert!(shown.contains(&cable(("delay", "out_l"), ("boundary", "out_l"))));
    for s in [
        ("boundary", "in_l"),
        ("boundary", "in_r"),
        ("boundary", "out_l"),
        ("boundary", "out_r"),
    ] {
        assert!(v.socket_center(&sref(s.0, s.1)).is_some(), "{s:?}");
    }
    // Path keys address the same sockets.
    assert!(
        v.socket_center(&sref("layer/boundary", "in_l")).is_some(),
        "path key accepted"
    );
    let input = v.socket_center(&sref("boundary", "in_l")).unwrap();
    let output = v.socket_center(&sref("boundary", "out_l")).unwrap();
    assert!(input.y < output.y, "INPUT above OUTPUT");
}

#[test]
fn dragging_from_the_input_boundary_connects_with_path_keys() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_flip(Flip::Back);
    let from = v.socket_center(&sref("boundary", "in_l")).unwrap();
    let to = v.socket_center(&sref("reverb", "in_l")).unwrap();
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    let a = v.handle(&up(to), d.doc(), &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Batch(vec![
            Command::Connect(cable(("layer/boundary", "in_l"), ("layer/reverb", "in_l"))),
            Command::Connect(cable(("layer/boundary", "in_r"), ("layer/reverb", "in_r"))),
        ])]
    );
}

#[test]
fn replacing_a_cable_into_the_output_boundary_disconnects_then_connects() {
    let mut d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_flip(Flip::Back);
    let from = v.socket_center(&sref("reverb", "out_l")).unwrap();
    let to = v.socket_center(&sref("boundary", "out_l")).unwrap();
    let _ = v.handle(&down(from), d.doc(), &reg);
    let _ = v.handle(&mv(to), d.doc(), &reg);
    let a = v.handle(&up(to), d.doc(), &reg);
    let c = commands(&a);
    assert_eq!(
        c,
        vec![Command::Batch(vec![
            Command::Disconnect {
                to: sref("layer/boundary", "out_l")
            },
            Command::Connect(cable(
                ("layer/reverb", "out_l"),
                ("layer/boundary", "out_l")
            )),
        ])]
    );
    for c in c {
        d.apply(c, &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
    assert_eq!(v.visible_cables(d.doc()).len(), 4);
}

#[test]
fn a_cable_drag_from_a_connected_input_detaches_it_with_path_keys() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_flip(Flip::Back);
    let p = v.socket_center(&sref("boundary", "out_r")).unwrap();
    // The cable is picked up, and cut only when it is dropped on nothing.
    let a = v.handle(&down(p), d.doc(), &reg);
    assert!(commands(&a).is_empty());
    let nowhere = atomik_ui::kurbo::Point::new(2.0, 2.0);
    let _ = v.handle(&mv(nowhere), d.doc(), &reg);
    let a = v.handle(&up(nowhere), d.doc(), &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Disconnect {
            to: sref("layer/boundary", "out_r")
        }]
    );
}

// ------------------------------------------------------------------ the add menu

#[test]
fn the_browser_inside_a_container_adds_inner_devices_at_the_scope_index() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_browser_visible(true);
    let rows = v.browser_rows(&reg);
    assert!(!rows.contains(&"atomik.channel") && !rows.contains(&"atomik.master"));
    assert!(!rows.contains(&"atomik.audio-track") && !rows.contains(&"atomik.note-split"));
    assert!(rows.contains(&"atomik.container"), "depth 1 may nest");
    // Select the synth, then double-click the poly synth row.
    let plate = v.plate_center(&key("synth")).unwrap();
    let _ = click(&mut v, d.doc(), plate);
    let i = rows.iter().position(|r| *r == "atomik.poly-synth").unwrap();
    let row = v.browser_row_center(i).unwrap();
    let a = press_with(
        &mut v,
        d.doc(),
        row,
        PointerButton::Primary,
        2,
        atomik_ui::Modifiers::NONE,
    );
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::AddInner {
            container: key("layer"),
            index: 1,
            type_id: "atomik.poly-synth".into(),
            key: None,
            auto_split: true,
        })]
    );
    // An effect gets no auto split.
    let i = rows
        .iter()
        .position(|r| *r == "atomik.stereo-delay")
        .unwrap();
    let row = v.browser_row_center(i).unwrap();
    let a = press_with(
        &mut v,
        d.doc(),
        row,
        PointerButton::Primary,
        2,
        atomik_ui::Modifiers::NONE,
    );
    match &commands(&a)[0] {
        Command::Container(ContainerCommand::AddInner { auto_split, .. }) => {
            assert!(!auto_split);
        }
        c => panic!("{c:?}"),
    }
}

#[test]
fn the_nesting_limit_removes_containers_from_the_browser_and_says_why() {
    let d = nested();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer", "fx", "deep"]);
    v.set_browser_visible(true);
    let rows = v.browser_rows(&reg);
    assert!(!rows.contains(&"atomik.container"));
    assert!(rows.contains(&"atomik.poly-synth"));
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(
        texts.contains("Containers nest 3 levels deep at most"),
        "{texts}"
    );
    // Two levels deep still offers a container; the top level too.
    let mut v = scoped_view(d.doc(), &["layer", "fx"]);
    v.set_browser_visible(true);
    assert!(v.browser_rows(&reg).contains(&"atomik.container"));
    let mut v = view(d.doc(), &reg);
    v.set_browser_visible(true);
    assert!(v.browser_rows(&reg).contains(&"atomik.container"));
}

// ------------------------------------------------------------------ macros: the context menu

fn rows(v: &RackView) -> Vec<String> {
    v.macro_menu_rows().into_iter().map(|(l, _)| l).collect()
}

#[test]
fn a_right_click_on_a_free_knob_offers_the_eight_macros_and_maps_with_the_descriptor_range() {
    let d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.control_center(&key("pad"), "filter.resonance").unwrap();
    assert!(right_click(&mut v, d.doc(), p).is_empty());
    let r = rows(&v);
    assert_eq!(r.len(), 8);
    assert_eq!(r[0], "Map to macro 1 (Cutoff)");
    assert_eq!(r[1], "Map to macro 2");
    assert_eq!(r[7], "Map to macro 8");
    let (_, at) = v.macro_menu_rows().remove(2);
    let a = click(&mut v, d.doc(), at);
    let desc = reg.get(SYNTH).unwrap().descriptor;
    let p = desc.param_by_key("filter.resonance").unwrap();
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::SetMacro {
                container: key("layer"),
                index: 2,
                target: MacroTarget {
                    device: key("pad"),
                    param: "filter.resonance".into(),
                    min: p.min,
                    max: p.max,
                },
                merge: None,
            }
        ))]
    );
    assert!(rows(&v).is_empty(), "the menu closed");
}

#[test]
fn a_press_outside_the_menu_or_escape_closes_it_without_a_command() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.control_center(&key("pad"), "filter.resonance").unwrap();
    let _ = right_click(&mut v, d.doc(), p);
    assert!(!rows(&v).is_empty());
    assert!(send_key(&mut v, d.doc(), Key::Escape).is_empty());
    assert!(rows(&v).is_empty());
    let _ = right_click(&mut v, d.doc(), p);
    assert!(click(&mut v, d.doc(), Point::new(5.0, 700.0)).is_empty());
    assert!(rows(&v).is_empty());
    // The top level has no such menu: a right-click on a knob only asks for the MIDI menu of
    // the control (the shell opens it), and no rows open here.
    let reg = registry();
    let mut v = view(d.doc(), &reg);
    let p = v.control_center(&key("lead"), "filter.resonance").unwrap();
    let actions = right_click(&mut v, d.doc(), p);
    assert!(
        matches!(&actions[..], [UiAction::ParamTouched { .. }, UiAction::ControlMenu { device, param, .. }]
            if *device == key("lead") && param == "filter.resonance"),
        "{actions:?}"
    );
    assert!(rows(&v).is_empty());
}

#[test]
fn a_mapped_control_is_locked_and_its_menu_offers_unmap_and_edit_range() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let reg = registry();
    let p = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    // A drag changes nothing.
    let _ = v.handle(&down(p), d.doc(), &reg);
    let a = v.handle(&mv(Point::new(p.x, p.y - 30.0)), d.doc(), &reg);
    let _ = v.handle(&up(p), d.doc(), &reg);
    assert!(commands(&a).is_empty(), "{a:?}");
    // So do a double-click (reset), the wheel and the keys.
    assert!(commands(&double_click(&mut v, d.doc(), p)).is_empty());
    let w = v.handle(
        &atomik_ui::UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
            pos: p,
            modifiers: atomik_ui::Modifiers::NONE,
        },
        d.doc(),
        &reg,
    );
    assert!(commands(&w).is_empty());
    // The menu.
    let _ = right_click(&mut v, d.doc(), p);
    assert_eq!(rows(&v), ["Edit range", "Unmap"]);
    let (_, unmap) = v.macro_menu_rows().remove(1);
    let a = click(&mut v, d.doc(), unmap);
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::RemoveMacroTarget {
                container: key("layer"),
                index: 0,
                device: key("synth"),
                param: "filter.cutoff".into(),
            }
        ))]
    );
    let _ = right_click(&mut v, d.doc(), p);
    let (_, edit) = v.macro_menu_rows().remove(0);
    let a = click(&mut v, d.doc(), edit);
    assert_eq!(
        a,
        vec![UiAction::OpenContainerEditor {
            device: key("layer"),
            macros: Some(0)
        }]
    );
}

#[test]
fn unmapping_releases_the_lock() {
    let mut d = layered();
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    apply(
        &mut d,
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: key("layer"),
            index: 0,
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
    );
    v.sync(d.doc(), &reg);
    let p = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let _ = v.handle(&down(p), d.doc(), &reg);
    let a = v.handle(&mv(Point::new(p.x, p.y - 30.0)), d.doc(), &reg);
    assert_eq!(commands(&a).len(), 1);
}

#[test]
fn an_outer_macro_that_drives_an_inner_containers_macro_locks_it_there() {
    let mut d = nested();
    let reg = registry();
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 3,
            target: target("fx", "macro1", 0.0, 1.0),
            merge: None,
        }),
    );
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.control_center(&key("fx"), "macro1").unwrap();
    let _ = v.handle(&down(p), d.doc(), &reg);
    let a = v.handle(&mv(Point::new(p.x, p.y - 30.0)), d.doc(), &reg);
    assert!(commands(&a).is_empty(), "locked");
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(texts.contains("M4"), "{texts}");
    // A macro of a container around the one in view locks without offering an unmap there.
    let mut d2 = nested();
    apply(
        &mut d2,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 1,
            target: target("fx/deep", "macro1", 0.0, 1.0),
            merge: None,
        }),
    );
    let mut v = scoped_view(d2.doc(), &["layer", "fx"]);
    let p = v.control_center(&key("deep"), "macro1").unwrap();
    let _ = right_click(&mut v, d2.doc(), p);
    assert!(rows(&v).is_empty(), "owned by another container: no menu");
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_scope_front_matches_golden() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    check(
        "container_scope_front_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_scope_back_matches_golden() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_flip(Flip::Back);
    check(
        "container_scope_back_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_macro_menu_matches_golden() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.control_center(&key("pad"), "filter.resonance").unwrap();
    let _ = right_click(&mut v, d.doc(), p);
    check(
        "container_scope_menu_1x.json",
        &paint_rack(&mut v, d.doc()).to_json(),
    );
}

#[test]
fn the_top_level_view_is_not_changed_by_containers_elsewhere() {
    // A container-free document paints exactly as before (the existing goldens pin it); a
    // document with a container paints no breadcrumb at the top level.
    let d = layered();
    let mut v = view(d.doc(), &registry());
    let texts = paint_rack(&mut v, d.doc()).texts().join("|");
    assert!(!texts.contains("BACK") && !texts.contains("Project"));
}

#[test]
fn the_scope_paints_the_breadcrumb() {
    let d = nested();
    let mut v = scoped_view(d.doc(), &["layer", "fx"]);
    let dl = paint_rack(&mut v, d.doc());
    let texts = dl.texts();
    for t in ["< BACK", "Project", "Layer", "FX", ">"] {
        assert!(texts.contains(&t), "{t}: {texts:?}");
    }
}

// ------------------------------------------------------------------ inner devices keep their panels

#[test]
fn the_editors_meters_and_drops_address_inner_devices_by_path_key() {
    use atomik_ui::SeqButton;
    let mut d = layered();
    for k in ["synth", "pad", "delay", "reverb"] {
        apply(
            &mut d,
            Command::RemoveDevice {
                key: key(&format!("layer/{k}")),
            },
        );
    }
    for (i, (t, k)) in [
        ("atomik.compressor", "comp"),
        ("atomik.step-sequencer", "seq"),
        ("atomik.sampler", "samp"),
        ("atomik.drum-machine", "drums"),
    ]
    .iter()
    .enumerate()
    {
        add_inner(&mut d, "layer", i, t, k);
    }
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    // The meter feed takes the path key (and the local one).
    let rev = |v: &RackView| -> u64 {
        v.panel_revisions()
            .into_iter()
            .find(|(k, _)| *k == key("comp"))
            .unwrap()
            .1
    };
    let before = rev(&v);
    v.set_device_meter_values(&key("layer/comp"), &[0.5, 0.25, -6.0]);
    assert_ne!(rev(&v), before, "a path key reaches the inner compressor");
    assert!(!v.device_meters(&key("layer/comp")).is_empty());
    // The sequencer's EDIT.
    let p = v
        .seq_button_center(&key("layer/seq"), SeqButton::Edit)
        .unwrap();
    let a = click(&mut v, d.doc(), p);
    assert_eq!(
        a,
        vec![
            UiAction::Select(key("layer/seq")),
            UiAction::OpenSeqEditor {
                device: key("layer/seq")
            }
        ]
    );
    // The sampler's EDIT and a file drop.
    let p = v.edit_button_center(&key("layer/samp")).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert!(
        a.contains(&UiAction::OpenSampleEditor(key("layer/samp"))),
        "{a:?}"
    );
    let a = v.handle(
        &atomik_ui::UiEvent::FileDrop {
            pos: p,
            paths: vec!["kick.wav".into()],
        },
        d.doc(),
        &reg,
    );
    assert!(
        matches!(&a[..], [UiAction::ImportSamples { device, .. }] if *device == key("layer/samp")),
        "{a:?}"
    );
    // A drum pad auditions the inner machine; TO CH explains instead of failing.
    let p = v.pad_center(&key("layer/drums"), 3).unwrap();
    let a = v.handle(&down(p), d.doc(), &reg);
    assert!(
        a.iter().any(|x| matches!(x,
            UiAction::AuditionVelocity { device, on: true, .. } if *device == key("layer/drums"))),
        "{a:?}"
    );
    let _ = v.handle(&up(p), d.doc(), &reg);
    let p = v.to_channel_button_center(&key("drums")).unwrap();
    let a = click(&mut v, d.doc(), p);
    assert!(
        commands(&a).is_empty(),
        "no console command for an inner device: {a:?}"
    );
}

#[test]
fn the_compressor_sidechain_helper_works_inside_a_container_with_path_keys() {
    let mut d = layered();
    for k in ["pad", "delay", "reverb"] {
        apply(
            &mut d,
            Command::RemoveDevice {
                key: key(&format!("layer/{k}")),
            },
        );
    }
    add_inner(&mut d, "layer", 1, "atomik.compressor", "comp");
    let reg = registry();
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.set_scroll(f64::MAX);
    let p = v.sc_button_center(&key("comp")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    let labels = v.sc_menu_labels();
    assert!(labels.len() >= 2, "{labels:?}");
    // Pick the synth as the source.
    let i = labels
        .iter()
        .position(|l| l.contains("Poly Synth"))
        .unwrap();
    let at = v.sc_menu_row_center(i).unwrap();
    let a = click(&mut v, d.doc(), at);
    let c = commands(&a);
    assert_eq!(c.len(), 1, "{a:?}");
    let batch = match &c[0] {
        Command::Batch(b) => b.clone(),
        c => panic!("{c:?}"),
    };
    for c in &batch {
        match c {
            Command::Connect(cb) => {
                assert!(
                    cb.from.device.0.starts_with("layer/") && cb.to.device == key("layer/comp")
                );
            }
            Command::SetParam { key: k, .. } => assert_eq!(k, &key("layer/comp")),
            c => panic!("{c:?}"),
        }
    }
    // The model accepts the batch.
    d.apply(c[0].clone(), &reg).unwrap();
}

// ------------------------------------------------------------------ the property test

mod property {
    use super::*;
    use atomik_model::ModelError;
    use atomik_ui::{Modifiers, UiEvent};
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum G {
        Click(Point, u8, u8),
        Drag(Point, Point),
        Key(u8),
        Wheel(Point, i8),
        Flip,
        Browser,
        Move(Point),
    }

    fn arb_point() -> impl Strategy<Value = Point> {
        (0.0..1000.0f64, 0.0..1200.0f64).prop_map(|(x, y)| Point::new(x, y))
    }

    fn arb_gesture() -> impl Strategy<Value = G> {
        prop_oneof![
            4 => (arb_point(), 1u8..=2, 0u8..3).prop_map(|(p, c, b)| G::Click(p, c, b)),
            3 => (arb_point(), arb_point()).prop_map(|(a, b)| G::Drag(a, b)),
            2 => (0u8..9).prop_map(G::Key),
            1 => (arb_point(), -2i8..=2).prop_map(|(p, n)| G::Wheel(p, n)),
            1 => Just(G::Flip),
            1 => Just(G::Browser),
            1 => arb_point().prop_map(G::Move),
        ]
    }

    fn key_for(n: u8) -> Key {
        match n {
            0 => Key::Escape,
            1 => Key::Enter,
            2 => Key::Backspace,
            3 => Key::Delete,
            4 => Key::Tab,
            5 => Key::Up,
            6 => Key::Space,
            7 => Key::Char('m'),
            _ => Key::Down,
        }
    }

    fn button(b: u8) -> PointerButton {
        match b {
            0 => PointerButton::Primary,
            1 => PointerButton::Secondary,
            _ => PointerButton::Middle,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn no_gesture_inside_a_container_makes_the_model_refuse_a_command(
            gestures in proptest::collection::vec(arb_gesture(), 1..24)
        ) {
            let mut d = nested();
            let reg = registry();
            let mut v = scoped_view(d.doc(), &["layer"]);
            for g in gestures {
                let actions = match g {
                    G::Click(p, clicks, b) => press_with(
                        &mut v, d.doc(), p, button(b), clicks, Modifiers::NONE),
                    G::Drag(a, b) => {
                        let mut out = v.handle(&down(a), d.doc(), &reg);
                        out.extend(v.handle(&mv(b), d.doc(), &reg));
                        out.extend(v.handle(&up(b), d.doc(), &reg));
                        out
                    }
                    G::Key(n) => send_key(&mut v, d.doc(), key_for(n)),
                    G::Wheel(p, n) => v.handle(
                        &UiEvent::Wheel {
                            delta: atomik_ui::kurbo::Vec2::new(0.0, f64::from(n)),
                            pos: p,
                            modifiers: Modifiers::NONE,
                        },
                        d.doc(),
                        &reg,
                    ),
                    G::Flip => {
                        let f = if v.flip() == Flip::Front { Flip::Back } else { Flip::Front };
                        v.set_flip(f);
                        Vec::new()
                    }
                    G::Browser => {
                        v.set_browser_visible(!v.browser_visible());
                        Vec::new()
                    }
                    G::Move(p) => v.handle(&mv(p), d.doc(), &reg),
                };
                for a in actions {
                    match a {
                        UiAction::Command(c) => {
                            let is_cable = matches!(c, Command::Connect(_) | Command::Disconnect { .. });
                            let r = d.apply(c.clone(), &reg);
                            // A cable may close a loop: the model says so, as at the top level.
                            let loop_refusal = matches!(&r, Err(ModelError::BadCable(_)));
                            prop_assert!(
                                r.is_ok() || (is_cable && loop_refusal),
                                "{c:?} refused: {r:?}"
                            );
                        }
                        UiAction::EndMerge => d.end_merge(),
                        UiAction::Undo => { d.undo(&reg); }
                        UiAction::Redo => { d.redo(&reg); }
                        _ => {}
                    }
                }
                v.sync(d.doc(), &reg);
            }
        }
    }
}

// ------------------------------------------------------------------ duplicate

/// QA: asks for "duplicate a container", and `DuplicateContainer` had no way in from the
/// UI. Ctrl+D on the selected container emits it (a path key inside a scope); on any other
/// device it duplicates the device.
#[test]
fn ctrl_d_duplicates_the_selected_container_at_every_level() {
    let d = nested();
    let reg = registry();
    // Ctrl+D is the `edit.duplicate` command (the shell resolves the chord and offers it to
    // the rack view).
    let ctrl_d = |v: &mut atomik_ui::RackView, doc: &atomik_model::RackDoc| {
        v.command("edit.duplicate", None, doc, &reg)
            .actions()
            .to_vec()
    };
    // Top level: the container "layer".
    let mut v = view(d.doc(), &reg);
    let p = v.plate_center(&key("layer")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    let a = ctrl_d(&mut v, d.doc());
    assert_eq!(
        commands(&a),
        vec![Command::Container(ContainerCommand::DuplicateContainer {
            container: key("layer")
        })]
    );
    // A plain device: the workflow copy, not a container duplicate.
    let p = v.plate_center(&key("lead")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    assert_eq!(
        commands(&ctrl_d(&mut v, d.doc())),
        vec![Command::Workflow(
            atomik_model::WorkflowCommand::DuplicateDevices {
                keys: vec![key("lead")]
            }
        )]
    );
    // Inside a scope the command carries the path key, and the model accepts it.
    let mut v = scoped_view(d.doc(), &["layer"]);
    let p = v.plate_center(&key("fx")).unwrap();
    let _ = click(&mut v, d.doc(), p);
    let a = ctrl_d(&mut v, d.doc());
    let cmds = commands(&a);
    assert_eq!(
        cmds,
        vec![Command::Container(ContainerCommand::DuplicateContainer {
            container: key("layer/fx")
        })]
    );
    let mut doc = d.clone();
    let before = doc.doc().inner(&key("layer")).unwrap().devices.len();
    doc.apply(cmds[0].clone(), &reg).unwrap();
    assert_eq!(
        doc.doc().inner(&key("layer")).unwrap().devices.len(),
        before + 1
    );
    // Without the modifier the key does nothing.
    let a = send_key(&mut v, d.doc(), Key::Char('d'));
    assert!(commands(&a).is_empty());
}
