//! The rack shows the console devices by default and the Master Section is visibly locked
//! (UI half). Display-list goldens regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_master_visible`.

mod support;

use atomik_model::{Command, WorkflowCommand};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{Flip, RackView, Theme, UiEvent};
use support::starter::*;
use support::*;

fn shown(v: &RackView) -> Vec<String> {
    v.panel_revisions().into_iter().map(|(k, _)| k.0).collect()
}

fn fresh(doc: &atomik_model::RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(VIEW), doc, reg);
    v.sync(doc, reg);
    v
}

#[test]
fn console_devices_are_shown_by_default_and_hidden_with_the_toggle() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    assert!(v.show_console_devices(), "shown by default");
    assert_eq!(
        shown(&v),
        [
            "interface",
            "master",
            "synth",
            "channel",
            "bass",
            "channel-2"
        ]
    );
    v.set_show_console_devices(false);
    v.sync(doc, &reg);
    assert_eq!(shown(&v), ["interface", "synth", "bass"]);
    v.set_show_console_devices(true);
    v.sync(doc, &reg);
    assert_eq!(shown(&v).len(), 6);
}

#[test]
fn a_new_channel_strip_is_folded_and_the_master_is_not() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let v = fresh(doc, &reg);
    let h = |k: &str| v.device_rect(&key(k)).unwrap().height();
    assert!(
        h("channel") < h("synth").min(h("master")),
        "the strip is slim"
    );
    assert!((h("channel") - h("channel-2")).abs() < 1e-9);
    assert!(h("master") > 100.0, "the master panel is open");
}

#[test]
fn only_a_first_master_is_locked() {
    let reg = registry();
    let d = two_instruments(&reg);
    let v = fresh(d.doc(), &reg);
    assert!(v.is_locked(&key("master")));
    assert!(!v.is_locked(&key("synth")) && !v.is_locked(&key("channel")));
    assert!(!v.is_locked(&key("interface")));

    // An old-style document with the master last has no position lock.
    let mut doc = d.doc().clone();
    let master = doc.devices.remove(0);
    doc.devices.push(master);
    let v = fresh(&doc, &reg);
    assert!(!v.is_locked(&key("master")));

    // No master, nothing locked.
    let doc = reference_doc();
    let v = fresh(&doc, &reg);
    assert!(!v.is_locked(&key("synth")));
}

#[test]
fn the_locked_master_explains_itself_in_a_tooltip() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let at = v.plate_center(&key("master")).unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    assert_eq!(v.tooltip().as_deref(), Some(RackView::MASTER_LOCKED_TEXT));
    let other = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(&mv(other), doc, &reg);
    assert_eq!(v.tooltip(), None);
}

#[test]
fn the_master_plate_cannot_be_dragged_and_nothing_goes_above_it() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    // Dragging the master down emits nothing, but the click still selects it.
    let at = v.plate_center(&key("master")).unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&mv(at + Vec2::new(0.0, 300.0)), doc, &reg);
    let out = v.handle(&up(at + Vec2::new(0.0, 300.0)), doc, &reg);
    assert!(commands(&out).iter().all(|c| !matches!(
        c,
        Command::MoveDevice { .. }
            | Command::Workflow(
                WorkflowCommand::MoveDevices { .. } | WorkflowCommand::MoveDevicesToSlot { .. }
            )
    )));
    assert_eq!(v.selected(), Some(&key("master")));

    // Dragging the bass above the master lands at index 1, below the master.
    let synth = v.plate_center(&key("bass")).unwrap();
    let top = v.device_rect(&key("master")).unwrap().y0 + 2.0;
    let _ = v.handle(&down(synth), doc, &reg);
    let _ = v.handle(&mv(Point::new(synth.x, top)), doc, &reg);
    let out = v.handle(&up(Point::new(synth.x, top)), doc, &reg);
    assert_eq!(
        commands(&out),
        [Command::Workflow(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![key("bass")],
            slot: atomik_model::RackSlot {
                column: 0,
                index: 1
            }
        })]
    );
}

#[test]
fn delete_on_the_master_goes_to_the_model_which_says_why() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    let doc = d.doc().clone();
    let mut v = fresh(&doc, &reg);
    v.set_selected(Some(key("master")));
    let outcome = v.command("edit.delete", None, &doc, &reg);
    let actions = match outcome {
        atomik_ui::keymap::CommandOutcome::Handled(a) => a,
        other => panic!("{other:?}"),
    };
    let cmds = commands(&actions);
    assert_eq!(cmds.len(), 1);
    let err = d.apply(cmds[0].clone(), &reg).unwrap_err();
    assert_eq!(err.to_string(), "The Master Section cannot be deleted");
    assert_eq!(d.doc(), &doc, "nothing changed");
}

#[test]
fn rack_master_visible_matches_golden() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    check_golden(
        "rack_master_visible_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
}

#[test]
fn the_back_of_a_new_project_shows_two_cables_and_no_console_links() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    v.set_flip(Flip::Back);
    let cables = v.visible_cables(doc);
    assert_eq!(
        cables.len(),
        2,
        "main_l and main_r to Audio I/O outputs 1 and 2"
    );
    assert!(cables.iter().all(|c| c.from.device == key("master")));
}
