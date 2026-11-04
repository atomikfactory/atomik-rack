//! Rack editing follow-ups in the view: the "+" handles between devices, a cable dragged over a
//! folded device unfolding it after a delay, and a library device dropped on a device or a
//! track header replacing it in place.

mod support;

use atomik_device_sdk::Registry;
use atomik_library::device_uri;
use atomik_model::{Command, RackDoc, RackSlot, WorkflowCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::{Flip, Key, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::song::*;
use support::*;

fn click_at(v: &mut RackView, doc: &RackDoc, reg: &Registry, at: Point) -> Vec<UiAction> {
    let mut out = v.handle(&mv(at), doc, reg);
    out.extend(v.handle(&down(at), doc, reg));
    out.extend(v.handle(&up(at), doc, reg));
    out
}

fn key_down(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

/// Opens the browser with a handle and double-clicks the row of `type_id`.
fn insert_through(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    handle: usize,
    type_id: &str,
) -> Vec<UiAction> {
    let at = v.handle_centers()[handle];
    let opened = click_at(v, doc, reg, at);
    assert!(opened.contains(&UiAction::OpenBrowser));
    let i = v
        .browser_rows(reg)
        .iter()
        .position(|t| *t == type_id)
        .expect("row");
    let row = v.browser_row_center(i).expect("browser open");
    v.handle(&down_with(row, PointerButton::Primary, 2), doc, reg)
}

fn created_at(actions: &[UiAction]) -> RackSlot {
    match commands(actions).as_slice() {
        [Command::Workflow(WorkflowCommand::CreateEffectAt { slot, .. })] => *slot,
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------------- the "+" handles

#[test]
fn a_handle_sits_on_every_seam_and_shows_only_under_the_pointer() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    // Above the first device, then below each of the three.
    assert_eq!(v.handle_centers().len(), 4);
    assert!(!v.plus_visible());
    let at = v.handle_centers()[1];
    let _ = v.handle(&mv(at), &doc, &reg);
    assert!(v.plus_visible());
    assert!(v.needs_repaint(), "the + is drawn on hover");
    // The middle of a panel is not a seam.
    let mid = v.rack_to_view(v.device_rect(&key("delay")).unwrap().center());
    let _ = v.handle(&mv(mid), &doc, &reg);
    assert!(!v.plus_visible());
    // Nor is a point on the seam far from its middle.
    let off = Point::new(at.x - 120.0, at.y);
    let _ = v.handle(&mv(off), &doc, &reg);
    assert!(!v.plus_visible());
}

#[test]
fn pressing_a_handle_opens_the_browser_and_a_row_inserts_at_that_slot() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    // The handle between the synth and the delay: index 1 of column 0.
    let a = insert_through(&mut v, &doc, &reg, 1, "atomik.reverb");
    assert_eq!(
        created_at(&a),
        RackSlot {
            column: 0,
            index: 1
        }
    );
    assert!(!v.browser_visible(), "the browser closes once it inserted");
    assert_eq!(v.pending_insert(), None);
    // The handle above the first device: index 0. Below the last: index 3.
    let mut v = view(&doc, &reg);
    let a = insert_through(&mut v, &doc, &reg, 0, "atomik.reverb");
    assert_eq!(created_at(&a).index, 0);
    let mut v = view(&doc, &reg);
    let a = insert_through(&mut v, &doc, &reg, 3, "atomik.reverb");
    assert_eq!(created_at(&a).index, 3);
}

#[test]
fn an_instrument_row_inserted_from_a_handle_is_created_at_the_slot() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let a = insert_through(&mut v, &doc, &reg, 2, "atomik.poly-synth");
    assert!(matches!(
        commands(&a).as_slice(),
        [Command::Workflow(WorkflowCommand::CreateInstrumentAt {
            slot: RackSlot {
                column: 0,
                index: 2
            },
            ..
        })]
    ));
}

#[test]
fn escape_cancels_the_insert_and_the_browser_forgets_the_slot() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let at = v.handle_centers()[1];
    let _ = click_at(&mut v, &doc, &reg, at);
    assert!(v.browser_visible());
    assert!(v.pending_insert().is_some());
    let a = v.handle(&key_down(Key::Escape), &doc, &reg);
    assert!(commands(&a).is_empty());
    assert!(!v.browser_visible());
    assert_eq!(v.pending_insert(), None);
    // The next browser opened by hand adds the usual way (no slot).
    v.set_browser_visible(true);
    assert_eq!(v.pending_insert(), None);
}

#[test]
fn a_handle_is_a_front_side_top_level_aid_only() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let at = v.handle_centers()[1];
    v.set_flip(Flip::Back);
    assert!(v.handle_centers().is_empty(), "the back side has none");
    let a = click_at(&mut v, &doc, &reg, at);
    assert!(!a.contains(&UiAction::OpenBrowser));
}

// ------------------------------------------------------------------- unfold on hover

fn folded_delay() -> (RackDoc, Registry, RackView) {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.devices[1].folded = true;
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    (doc, reg, v)
}

/// Starts a cable drag from the synth and rests the pointer on the folded delay.
fn drag_over_delay(v: &mut RackView, doc: &RackDoc, reg: &Registry) -> Point {
    let from = v.socket_center(&sref("synth", "out_r")).expect("jack");
    let _ = v.handle(&down(from), doc, reg);
    let over = v.rack_to_view(v.device_rect(&key("delay")).unwrap().center());
    let _ = v.handle(&mv(over), doc, reg);
    over
}

#[test]
fn a_cable_resting_on_a_folded_device_unfolds_it_after_the_delay() {
    let (doc, reg, mut v) = folded_delay();
    assert!(!v.unfold_pending());
    drag_over_delay(&mut v, &doc, &reg);
    assert!(v.unfold_pending());
    // The first look only starts the clock; nothing is due before the delay has passed.
    assert_eq!(v.hover_unfold(10.0), None);
    assert_eq!(v.hover_unfold(10.3), None);
    assert_eq!(v.hover_unfold(10.8), Some(key("delay")));
    // Said once: the next look starts the wait again.
    assert_eq!(v.hover_unfold(10.9), None);
}

#[test]
fn leaving_the_folded_device_or_dropping_the_cable_stops_the_wait() {
    let (doc, reg, mut v) = folded_delay();
    let over = drag_over_delay(&mut v, &doc, &reg);
    assert_eq!(v.hover_unfold(5.0), None);
    // The pointer moves to the empty rack: the clock is forgotten.
    let _ = v.handle(&mv(Point::new(over.x + 600.0, over.y)), &doc, &reg);
    assert!(!v.unfold_pending());
    assert_eq!(v.hover_unfold(5.9), None);
    // Back on the device it must wait the whole delay again.
    let _ = v.handle(&mv(over), &doc, &reg);
    assert_eq!(v.hover_unfold(6.0), None);
    assert_eq!(v.hover_unfold(6.5), None);
    assert_eq!(v.hover_unfold(6.8), Some(key("delay")));
    // With no cable in hand nothing unfolds.
    let _ = v.handle(&up(over), &doc, &reg);
    assert!(!v.unfold_pending());
    assert_eq!(v.hover_unfold(20.0), None);
}

#[test]
fn an_unfolded_device_does_not_end_the_cable_drag() {
    let (mut doc, reg, mut v) = folded_delay();
    let over = drag_over_delay(&mut v, &doc, &reg);
    // The host unfolds the device (an edit of the project) and the drag goes on.
    doc.devices[1].folded = false;
    let _ = v.handle(&mv(over), &doc, &reg);
    let target = v.socket_center(&sref("delay", "in_r")).expect("jack");
    let _ = v.handle(&mv(target), &doc, &reg);
    let a = v.handle(&up(target), &doc, &reg);
    assert!(
        commands(&a)
            .iter()
            .any(|c| matches!(c, Command::Connect(_) | Command::Batch(_))),
        "the cable lands on the socket that was out of reach: {a:?}"
    );
}

// ---------------------------------------------------------- library item on a device

fn drop_item(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    to: Point,
    path: &str,
) -> Vec<UiAction> {
    let paths = vec![device_uri(path)];
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: to,
            paths: paths.clone(),
        },
        doc,
        reg,
    );
    v.handle(&UiEvent::FileDrop { pos: to, paths }, doc, reg)
}

fn replaced(actions: &[UiAction]) -> Option<(atomik_model::DeviceKey, atomik_model::DeviceKey)> {
    actions.iter().find_map(|a| match a {
        UiAction::ReplaceWithLibrary { path, device } => Some((
            atomik_model::DeviceKey(path.to_string_lossy().into_owned()),
            device.clone(),
        )),
        _ => None,
    })
}

#[test]
fn a_device_row_dropped_on_the_middle_of_a_device_of_its_kind_replaces_it() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let to = v.rack_to_view(v.device_rect(&key("delay")).unwrap().center());
    let hover = UiEvent::FileHover {
        pos: to,
        paths: vec![device_uri("atomik.reverb")],
    };
    let _ = v.handle(&hover, &doc, &reg);
    assert_eq!(
        v.drop_hover(),
        Some(&key("delay")),
        "the device is outlined"
    );
    let a = drop_item(&mut v, &doc, &reg, to, "atomik.reverb");
    let (path, device) = replaced(&a).expect("a replacement");
    assert_eq!(path.0, device_uri("atomik.reverb").to_string_lossy());
    assert_eq!(device, key("delay"));
    assert!(
        !a.iter()
            .any(|a| matches!(a, UiAction::InsertLibrary { .. })),
        "{a:?}"
    );
}

#[test]
fn a_device_row_on_another_kind_or_on_a_seam_inserts_as_before() {
    let (doc, reg) = (reference_doc(), registry());
    // An instrument over an effect: no replacement, an insert.
    let mut v = view(&doc, &reg);
    let to = v.rack_to_view(v.device_rect(&key("delay")).unwrap().center());
    let a = drop_item(&mut v, &doc, &reg, to, "atomik.poly-synth");
    assert!(replaced(&a).is_none());
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::InsertLibrary { .. }))
    );
    // An effect near the edge of an effect: the gap between devices.
    let mut v = view(&doc, &reg);
    let r = v.device_rect(&key("delay")).unwrap();
    let to = v.rack_to_view(Point::new(r.center().x, r.y0 + 2.0));
    let a = drop_item(&mut v, &doc, &reg, to, "atomik.reverb");
    assert!(replaced(&a).is_none());
    assert!(
        a.iter()
            .any(|a| matches!(a, UiAction::InsertLibrary { .. }))
    );
    // The mixer is never replaced.
    let mut v = view(&doc, &reg);
    let to = v.rack_to_view(v.device_rect(&key("mixer")).unwrap().center());
    let a = drop_item(&mut v, &doc, &reg, to, "atomik.reverb");
    assert!(replaced(&a).is_none());
}

// ----------------------------------------------------------- library item on a track header

#[test]
fn a_device_row_dropped_on_a_track_header_replaces_the_track_device() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let header = v.track_header_center(&key("lead")).expect("header");
    let paths = vec![device_uri("atomik.mono-bass")];
    let _ = v.handle(
        &UiEvent::FileHover {
            pos: header,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: header,
            paths: paths.clone(),
        },
        &doc,
        &reg,
    );
    assert!(matches!(
        a.as_slice(),
        [UiAction::ReplaceWithLibrary { device, .. }] if *device == key("lead")
    ));
    // An effect cannot replace an instrument track's device: nothing is taken.
    let a = v.handle(
        &UiEvent::FileDrop {
            pos: header,
            paths: vec![device_uri("atomik.reverb")],
        },
        &doc,
        &reg,
    );
    assert!(a.is_empty(), "{a:?}");
}
