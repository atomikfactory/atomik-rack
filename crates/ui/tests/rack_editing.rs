//! Rack editing gestures: dropping a browser row on a device of its kind replaces it, a device
//! move re-routes by the setting (Shift inverts it), Alt on the fold button folds the whole
//! column, and the cable display modes and roles.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, CreateSource, DeviceKey, RackDoc, RackSlot, WorkflowCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::{CableVisibility, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::*;

fn row_of(v: &RackView, reg: &Registry, type_id: &str) -> Point {
    let i = v
        .browser_rows(reg)
        .iter()
        .position(|t| *t == type_id)
        .expect("row");
    v.browser_row_center(i).expect("browser open")
}

fn middle_of(v: &RackView, device: &str) -> Point {
    v.rack_to_view(v.device_rect(&key(device)).unwrap().center())
}

fn drag_row(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    type_id: &str,
    to: Point,
) -> Vec<UiAction> {
    v.set_browser_visible(true);
    let from = row_of(v, reg, type_id);
    let mut out = v.handle(&down(from), doc, reg);
    out.extend(v.handle(&mv(to), doc, reg));
    out.extend(v.handle(&up(to), doc, reg));
    out
}

fn replaced(actions: &[UiAction]) -> Option<(DeviceKey, CreateSource)> {
    match commands(actions).as_slice() {
        [Command::Workflow(WorkflowCommand::ReplaceDevice { target, source })] => {
            Some((target.clone(), source.clone()))
        }
        _ => None,
    }
}

#[test]
fn a_row_dropped_on_the_middle_of_a_device_of_its_kind_replaces_it() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let to = middle_of(&v, "delay");
    let a = drag_row(&mut v, &doc, &reg, "atomik.reverb", to);
    assert_eq!(
        replaced(&a),
        Some((key("delay"), CreateSource::Type("atomik.reverb".into())))
    );
}

#[test]
fn a_row_dropped_on_a_device_of_another_kind_or_between_devices_creates() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    // An instrument row on an effect: no replacement.
    let to = middle_of(&v, "delay");
    let a = drag_row(&mut v, &doc, &reg, "atomik.poly-synth", to);
    assert!(replaced(&a).is_none());
    assert!(!commands(&a).is_empty(), "the drop creates the instrument");
    // An effect row near the top edge of the effect: the gap between devices.
    let mut v = view(&doc, &reg);
    let r = v.device_rect(&key("delay")).unwrap();
    let to = v.rack_to_view(Point::new(r.center().x, r.y0 + 2.0));
    let a = drag_row(&mut v, &doc, &reg, "atomik.reverb", to);
    assert!(replaced(&a).is_none());
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::Workflow(WorkflowCommand::CreateEffectAt { .. })]
    ));
    // A mixer is neither an instrument nor an effect: it is never replaced.
    let mut v = view(&doc, &reg);
    let to = middle_of(&v, "mixer");
    let a = drag_row(&mut v, &doc, &reg, "atomik.reverb", to);
    assert!(replaced(&a).is_none());
}

#[test]
fn a_device_move_re_routes_by_the_setting_and_shift_inverts_it() {
    let (doc, reg) = (reference_doc(), registry());
    let move_with = |auto: bool, shift: bool| {
        let mut v = view(&doc, &reg);
        v.set_auto_route(auto);
        let p = v.plate_center(&key("mixer")).unwrap();
        let synth = v.device_rect(&key("synth")).unwrap();
        let target = v.rack_to_view(Point::new(60.0, synth.y0 + 5.0));
        let _ = v.handle(&down(p), &doc, &reg);
        let _ = v.handle(&mv(target), &doc, &reg);
        let release = UiEvent::PointerUp {
            pos: target,
            button: PointerButton::Primary,
            modifiers: Modifiers {
                shift,
                ..Modifiers::NONE
            },
        };
        let a = v.handle(&release, &doc, &reg);
        match commands(&a).as_slice() {
            [Command::Workflow(WorkflowCommand::MoveDevicesToSlot { route, slot, .. })] => {
                assert_eq!(
                    *slot,
                    RackSlot {
                        column: 0,
                        index: 0
                    }
                );
                *route
            }
            other => panic!("{other:?}"),
        }
    };
    assert!(move_with(true, false));
    assert!(!move_with(true, true));
    assert!(!move_with(false, false));
    assert!(move_with(false, true));
}

#[test]
fn alt_on_the_fold_button_folds_the_whole_column_in_one_command() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let f = v.fold_button_center(&key("delay")).unwrap();
    let alt = UiEvent::PointerDown {
        pos: f,
        button: PointerButton::Primary,
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::NONE
        },
        clicks: 1,
    };
    let a = v.handle(&alt, &doc, &reg);
    match commands(&a).as_slice() {
        [Command::Workflow(WorkflowCommand::SetFoldedMany { keys, folded })] => {
            assert!(*folded);
            assert_eq!(keys, &[key("synth"), key("delay"), key("mixer")]);
        }
        other => panic!("{other:?}"),
    }
    // Without Alt it is the single device.
    let a = v.handle(&down(f), &doc, &reg);
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::SetFolded { .. }]
    ));
}

#[test]
fn cable_display_modes_show_all_selected_or_the_hand_made_cables() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_flip(atomik_ui::Flip::Back);
    let all = v.visible_cables(&doc).len();
    assert_eq!(all, doc.cables.len());
    // Selected only: nothing is selected, then the synth is.
    v.set_cable_visibility(CableVisibility::SelectedOnly);
    assert!(v.visible_cables(&doc).is_empty());
    let c = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let _ = v.handle(&up(c), &doc, &reg);
    let shown = v.visible_cables(&doc);
    assert_eq!(shown.len(), 2, "the synth's two cables: {shown:?}");
    assert!(
        shown
            .iter()
            .all(|c| c.from.device == key("synth") || c.to.device == key("synth"))
    );
    // Hide auto-routed: the reference rack has no console, so every cable is hand-made.
    v.set_cable_visibility(CableVisibility::HideAutoRouted);
    assert_eq!(v.visible_cables(&doc).len(), all);
    // The K order walks the five modes and comes back.
    let mut m = CableVisibility::Show;
    let mut seen = Vec::new();
    for _ in 0..5 {
        m = m.next();
        seen.push(m);
    }
    assert_eq!(
        seen,
        [
            CableVisibility::HoverOnly,
            CableVisibility::SelectedOnly,
            CableVisibility::HideAutoRouted,
            CableVisibility::Hide,
            CableVisibility::Show
        ]
    );
}

#[test]
fn the_create_player_browser_lists_note_players_only() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.open_browser_create(atomik_ui::browser::CreateKind::Player, &reg);
    let rows = v.browser_rows(&reg);
    assert!(rows.contains(&"atomik.arpeggiator"));
    assert!(!rows.contains(&"atomik.poly-synth") && !rows.contains(&"atomik.stereo-delay"));
    assert_eq!(
        v.browser_create(),
        Some(atomik_ui::browser::CreateKind::Player)
    );
}
