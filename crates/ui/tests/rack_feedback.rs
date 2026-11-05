//! Feedback routing in the rack: a cable dragged to close a loop is judged by the model like any
//! cable, so the rack offers it through a Feedback Route and refuses it anywhere else, and a rack
//! that holds a feedback loop draws and syncs like any other.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, RackDoc, empty_doc};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{DisplayList, Flip, RackView, Theme, UiAction, UiEvent};
use support::*;

/// A synth into a gain tool into a mixer, and a Feedback Route; no loop cabled yet.
fn rack() -> (Document, Registry) {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    for (i, (type_id, k)) in [
        ("atomik.poly-synth", "synth"),
        ("atomik.gain-tool", "gain"),
        ("atomik.line-mixer", "mixer"),
        ("atomik.feedback", "fb"),
    ]
    .into_iter()
    .enumerate()
    {
        d.apply(
            Command::AddDevice {
                index: i,
                type_id: type_id.into(),
                key: Some(key(k)),
            },
            &reg,
        )
        .unwrap();
    }
    for (from, to) in [
        (("synth", "out_l"), ("gain", "in_l")),
        (("gain", "out_l"), ("mixer", "ch1_in_l")),
    ] {
        d.apply(Command::Connect(cable(from, to)), &reg).unwrap();
    }
    (d, reg)
}

fn back_view(doc: &RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 3000.0)), doc, reg);
    v.sync(doc, reg);
    v.set_flip(Flip::Back);
    v
}

fn drag(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    from: (&str, &str),
    to: (&str, &str),
) -> Vec<UiAction> {
    let a: Point = v.socket_center(&sref(from.0, from.1)).expect("source jack");
    let b: Point = v.socket_center(&sref(to.0, to.1)).expect("target jack");
    let mut out = v.handle(&down(a), doc, reg);
    out.extend(v.handle(&mv(b), doc, reg));
    out.extend(v.handle(&up(b), doc, reg));
    out
}

#[test]
fn a_cable_dragged_to_close_a_loop_is_offered_through_the_feedback_route_only() {
    let (mut d, reg) = rack();
    // The bare loop: the gain tool's output back into its own aux input.
    let doc = d.doc().clone();
    let mut v = back_view(&doc, &reg);
    let a = drag(&mut v, &doc, &reg, ("gain", "out_l"), ("gain", "aux_l"));
    for c in commands(&a) {
        assert!(
            d.apply(c.clone(), &reg).is_err(),
            "the model accepted a bare loop: {c:?}"
        );
    }
    assert!(d.doc().validate().is_ok());
    // Through the Feedback Route both cables are accepted.
    for (from, to) in [
        (("gain", "out_l"), ("fb", "in_l")),
        (("fb", "out_l"), ("gain", "aux_l")),
    ] {
        let doc = d.doc().clone();
        let mut v = back_view(&doc, &reg);
        let a = drag(&mut v, &doc, &reg, from, to);
        let cmds = commands(&a);
        assert!(!cmds.is_empty(), "{from:?} -> {to:?} makes a cable");
        for c in cmds {
            d.apply(c, &reg)
                .unwrap_or_else(|e| panic!("{from:?} -> {to:?}: {e}"));
        }
    }
    assert!(d.doc().cables.iter().any(|c| c.from.device == key("fb")));
    assert!(d.doc().validate().is_ok());
}

#[test]
fn a_rack_with_a_feedback_loop_syncs_and_draws() {
    let (mut d, reg) = rack();
    for (f, t) in [
        (("gain", "out_l"), ("fb", "in_l")),
        (("fb", "out_l"), ("gain", "aux_l")),
    ] {
        d.apply(Command::Connect(cable(f, t)), &reg).unwrap();
    }
    let doc = d.doc().clone();
    for flip in [Flip::Front, Flip::Back] {
        let mut v = back_view(&doc, &reg);
        v.set_flip(flip);
        let mut list = DisplayList::new();
        v.paint(&mut list, &doc, &reg);
        let json = list.to_json().to_lowercase();
        assert!(json.contains("feedback route"), "{flip:?}");
    }
}
