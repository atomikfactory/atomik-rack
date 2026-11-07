//! The insert area of a mix channel in the rack: the insert effects stacked under the channel in
//! a frame, folded to one header line, and the gestures on it (each one command).

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, Document, InsertEffect, OutputDest, RackSlot,
    WorkflowCommand, insert_chain,
};
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::{RackView, Theme, UiAction, UiEvent};
use support::starter::*;
use support::*;

const DROP_ROW: f64 = 24.0;
const HEADER: f64 = 18.0;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

/// Two synths, the first channel unfolded with a reverb and a delay stacked under it, a loose
/// effect (`verb2`) at the end and a bus.
fn stacked(reg: &Registry) -> Document {
    let mut d = two_instruments(reg);
    run(
        &mut d,
        reg,
        [Command::SetFolded {
            key: key("channel"),
            folded: false,
        }],
    );
    for (slot, t) in ["atomik.reverb", "atomik.stereo-delay"]
        .into_iter()
        .enumerate()
    {
        run(
            &mut d,
            reg,
            [cc(ConsoleCommand::AddInsert {
                channel: key("channel"),
                slot,
                effect: InsertEffect::New { type_id: t.into() },
            })],
        );
    }
    run(&mut d, reg, [add_end("atomik.eq", "loose")]);
    run(
        &mut d,
        reg,
        [cc(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        })],
    );
    d
}

fn tall(doc: &atomik_model::RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 3000.0)), doc, reg);
    v.sync(doc, reg);
    v
}

fn chain(d: &Document, reg: &Registry, k: &str) -> Vec<DeviceKey> {
    insert_chain(d.doc(), reg, &key(k))
}

fn drag(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
    a: Point,
    b: Point,
) -> Vec<UiAction> {
    let mut out = v.handle(&down(a), doc, reg);
    out.extend(v.handle(&mv(Point::new(a.x, a.y + 12.0)), doc, reg));
    out.extend(v.handle(&mv(b), doc, reg));
    out.extend(v.handle(&up(b), doc, reg));
    out
}

fn click(
    v: &mut RackView,
    at: Point,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
) -> Vec<UiAction> {
    let mut out = v.handle(&down(at), doc, reg);
    out.extend(v.handle(&up(at), doc, reg));
    out
}

fn rect(v: &RackView, k: &DeviceKey) -> Rect {
    v.device_rect(k).unwrap_or_else(|| panic!("{k} is drawn"))
}

fn area<'a>(areas: &'a [atomik_ui::InsertAreaInfo], k: &str) -> &'a atomik_ui::InsertAreaInfo {
    areas.iter().find(|a| a.owner == key(k)).expect("an area")
}

fn at(v: &RackView, column_x: f64, y: f64) -> Point {
    v.rack_to_view(Point::new(column_x, y))
}

// ------------------------------------------------------------------ layout

#[test]
fn the_inserts_are_stacked_under_their_channel_in_chain_order() {
    let reg = registry();
    let d = stacked(&reg);
    let v = tall(d.doc(), &reg);
    let members = chain(&d, &reg, "channel");
    assert_eq!(members.len(), 2);
    let channel = rect(&v, &key("channel"));
    let (a, b) = (rect(&v, &members[0]), rect(&v, &members[1]));
    assert_eq!(
        a.y0, channel.y1,
        "the first insert is right under the panel"
    );
    assert_eq!(b.y0, a.y1, "the second right under the first");
    assert_eq!(a.x0, channel.x0);
    let areas = v.insert_areas();
    assert_eq!(area(&areas, "channel").members, members);
    assert!(area(&areas, "channel").shown);
}

#[test]
fn the_frame_is_the_panels_plus_the_drop_row_and_nothing_overlaps() {
    let reg = registry();
    let d = stacked(&reg);
    let v = tall(d.doc(), &reg);
    let members = chain(&d, &reg, "channel");
    let areas = v.insert_areas();
    let a = area(&areas, "channel");
    let panels: f64 = members.iter().map(|m| rect(&v, m).height()).sum();
    // The frame grows two units above and below its content.
    assert!((a.frame.height() - 4.0 - (panels + DROP_ROW)).abs() < 1e-6);
    assert!((a.row.height() - DROP_ROW).abs() < 1e-6);
    assert!((a.row.y1 - (rect(&v, &members[1]).y1 + DROP_ROW)).abs() < 1e-6);
    // Every device and every area row of a column stack without overlap.
    let mut boxes: Vec<(String, Rect)> = d
        .doc()
        .devices
        .iter()
        .filter_map(|x| v.device_rect(&x.key).map(|r| (x.key.0.clone(), r)))
        .collect();
    for ar in &areas {
        boxes.push((format!("row of {}", ar.owner), ar.row));
    }
    for (i, (na, ra)) in boxes.iter().enumerate() {
        for (nb, rb) in &boxes[i + 1..] {
            let apart =
                ra.x1 <= rb.x0 || rb.x1 <= ra.x0 || ra.y1 <= rb.y0 + 1e-6 || rb.y1 <= ra.y0 + 1e-6;
            assert!(apart, "{na} {ra:?} overlaps {nb} {rb:?}");
        }
    }
}

#[test]
fn a_folded_area_is_one_header_line_and_hides_the_stack() {
    let reg = registry();
    let d = stacked(&reg);
    let mut v = tall(d.doc(), &reg);
    let members = chain(&d, &reg, "channel");
    let before = area(&v.insert_areas(), "channel").frame.height();
    assert!(v.set_inserts_shown(&key("channel"), false));
    v.sync(d.doc(), &reg);
    let areas = v.insert_areas();
    let a = area(&areas, "channel");
    assert!(!a.shown);
    assert!((a.frame.height() - 4.0 - HEADER).abs() < 1e-6, "{a:?}");
    assert!(a.frame.height() < before);
    for m in &members {
        assert!(v.device_rect(m).is_none(), "{m} is hidden");
    }
    assert_eq!(a.members, members, "the header still knows the effects");
    // Shown again, the stack is back where it was.
    v.set_inserts_shown(&key("channel"), true);
    v.sync(d.doc(), &reg);
    assert!((area(&v.insert_areas(), "channel").frame.height() - before).abs() < 1e-6);
}

#[test]
fn a_channel_without_inserts_still_has_a_drop_row_and_a_bus_has_the_same_area() {
    let reg = registry();
    let d = stacked(&reg);
    let v = tall(d.doc(), &reg);
    let areas = v.insert_areas();
    let second = area(&areas, "channel-2");
    assert!(second.members.is_empty() && second.shown);
    let bus = d.doc().console.buses[0].bus.0.clone();
    assert!(
        areas.iter().any(|a| a.owner.0 == bus),
        "the bus has an area"
    );
}

#[test]
fn the_stack_keeps_its_place_when_the_document_order_is_scattered() {
    // A hand-made project: the insert sits at the end of the rack, the chain is cabled.
    let reg = registry();
    let mut d = stacked(&reg);
    let members = chain(&d, &reg, "channel");
    let mut doc = d.doc().clone();
    let at = doc
        .devices
        .iter()
        .position(|x| x.key == members[0])
        .unwrap();
    let moved = doc.devices.remove(at);
    let col = doc.devices.last().unwrap().column;
    doc.devices.push(moved);
    doc.devices.last_mut().unwrap().column = col;
    let _ = &mut d;
    let v = tall(&doc, &reg);
    let areas = v.insert_areas();
    assert_eq!(
        area(&areas, "channel").members,
        insert_chain(&doc, &reg, &key("channel"))
    );
}

// ------------------------------------------------------------------ gestures

#[test]
fn dragging_an_insert_down_inside_the_area_moves_it_in_the_chain() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    let members = chain(&d, &reg, "channel");
    let from = v.plate_center(&members[0]).unwrap();
    let r = rect(&v, &members[1]);
    let to = at(&v, r.x0 + 300.0, r.y1 - 2.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        commands(&a),
        vec![cc(ConsoleCommand::MoveInsert {
            channel: key("channel"),
            from: 0,
            to: 1
        })]
    );
    // One command, one undo step: the chain reads back reordered.
    let mut d = d;
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    assert_eq!(
        chain(&d, &reg, "channel"),
        vec![members[1].clone(), members[0].clone()]
    );
}

#[test]
fn dragging_a_rack_effect_into_the_area_adds_it_at_that_place() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    let members = chain(&d, &reg, "channel");
    let from = v.plate_center(&key("loose")).unwrap();
    let first = rect(&v, &members[0]);
    // Just under the first insert, above the middle of the second.
    let to = at(&v, first.x0 + 300.0, first.y1 + 1.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    let want = cc(ConsoleCommand::AddInsert {
        channel: key("channel"),
        slot: 1,
        effect: InsertEffect::Existing(key("loose")),
    });
    assert_eq!(commands(&a), vec![want]);
    let mut d = d;
    for c in commands(&a) {
        d.apply(c, &reg).unwrap();
    }
    let now = chain(&d, &reg, "channel");
    assert_eq!(now.len(), 3);
    assert_eq!(now[1], key("loose"));
}

#[test]
fn dragging_an_insert_out_of_the_area_takes_it_out_of_the_chain() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    let members = chain(&d, &reg, "channel");
    let from = v.plate_center(&members[0]).unwrap();
    // Below the last device of the rack.
    let bottom = d
        .doc()
        .devices
        .iter()
        .filter_map(|x| v.device_rect(&x.key))
        .map(|r| r.y1)
        .fold(0.0, f64::max);
    let to = at(&v, 300.0, bottom + 60.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    let cmds = commands(&a);
    assert!(
        matches!(
            cmds.as_slice(),
            [Command::Console(ConsoleCommand::ExtractInsert { channel, slot: 0, to: Some(_) })]
                if *channel == key("channel")
        ),
        "{cmds:?}"
    );
    let mut d = d;
    for c in cmds {
        d.apply(c, &reg).unwrap();
    }
    assert_eq!(chain(&d, &reg, "channel"), vec![members[1].clone()]);
    assert!(
        d.doc().device(&members[0]).is_some(),
        "the effect stays in the rack"
    );
}

#[test]
fn dragging_the_channel_takes_its_inserts_along() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    let members = chain(&d, &reg, "channel");
    let from = v.plate_center(&key("channel")).unwrap();
    let bottom = d
        .doc()
        .devices
        .iter()
        .filter_map(|x| v.device_rect(&x.key))
        .map(|r| r.y1)
        .fold(0.0, f64::max);
    let to = at(&v, 300.0, bottom + 60.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    let cmds = commands(&a);
    let [Command::Workflow(WorkflowCommand::MoveDevicesToSlot { keys, .. })] = cmds.as_slice()
    else {
        panic!("one move: {cmds:?}");
    };
    let mut want = vec![key("channel")];
    want.extend(members);
    assert_eq!(*keys, want);
}

#[test]
fn a_device_that_cannot_be_an_insert_is_refused_with_the_models_reason() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    let members = chain(&d, &reg, "channel");
    // The second channel dragged into the first one's area.
    let from = v.plate_center(&key("channel-2")).unwrap();
    let first = rect(&v, &members[0]);
    let to = at(&v, first.x0 + 300.0, first.y1 + 1.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert!(
        commands(&a).is_empty(),
        "nothing the model refuses is emitted"
    );
    let mut list = atomik_ui::DisplayList::new();
    v.paint(&mut list, &doc, &reg);
    assert!(
        list.texts().iter().any(|t| t.contains("console device")),
        "the reason shows: {:?}",
        list.texts()
    );
    // The model says the same.
    let mut d = d;
    let command = cc(ConsoleCommand::AddInsert {
        channel: key("channel"),
        slot: 1,
        effect: InsertEffect::Existing(key("channel-2")),
    });
    let e = d.apply(command, &reg).expect_err("refused");
    assert!(e.to_string().contains("console device"), "{e}");
}

#[test]
fn the_header_shows_the_stack_and_the_drop_row_selects_the_channel() {
    let reg = registry();
    let d = stacked(&reg);
    let (doc, mut v) = (d.doc().clone(), tall(d.doc(), &reg));
    // Folded by the panel's button state, the header line click unfolds.
    v.set_inserts_shown(&key("channel"), false);
    v.sync(&doc, &reg);
    let p = v.insert_row_center(&key("channel")).unwrap();
    let _ = click(&mut v, p, &doc, &reg);
    assert!(v.inserts_shown(&key("channel")));
    v.sync(&doc, &reg);
    v.set_selected(None);
    // The drop row selects the strip (a created effect then appends to its chain).
    let p = v.insert_row_center(&key("channel")).unwrap();
    let a = click(&mut v, p, &doc, &reg);
    assert!(
        a.iter()
            .any(|x| matches!(x, UiAction::Select(k) if *k == key("channel"))),
        "{a:?}"
    );
    assert_eq!(v.selected(), Some(&key("channel")));
}

#[test]
fn the_drop_position_follows_the_pointer_among_the_stacked_effects() {
    let reg = registry();
    let d = stacked(&reg);
    let v = tall(d.doc(), &reg);
    let members = chain(&d, &reg, "channel");
    let (a, b) = (rect(&v, &members[0]), rect(&v, &members[1]));
    let pos = |y: f64| v.insert_drop_position(&key("channel"), at(&v, a.x0 + 300.0, y));
    assert_eq!(pos(a.y0 + 2.0), Some(0));
    assert_eq!(pos(a.y1 + 1.0), Some(1));
    assert_eq!(pos(b.y1 - 1.0), Some(2));
    let _ = RackSlot::default();
}

// ------------------------------------------------------------------ property

mod random {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Gesture {
        Add(usize, usize, usize),
        Take(usize, usize, bool),
        Reorder(usize, usize, usize),
        Delete(usize, usize),
        Fold(usize),
        MoveChannel(usize, usize),
    }

    fn gestures() -> impl Strategy<Value = Vec<Gesture>> {
        let g = prop_oneof![
            (0usize..2, 0usize..5, 0usize..3).prop_map(|(c, s, t)| Gesture::Add(c, s, t)),
            (0usize..2, 0usize..5, any::<bool>()).prop_map(|(c, s, o)| Gesture::Take(c, s, o)),
            (0usize..2, 0usize..5, 0usize..5).prop_map(|(c, a, b)| Gesture::Reorder(c, a, b)),
            (0usize..2, 0usize..5).prop_map(|(c, s)| Gesture::Delete(c, s)),
            (0usize..2).prop_map(Gesture::Fold),
            (0usize..2, 0usize..12).prop_map(|(c, i)| Gesture::MoveChannel(c, i)),
        ];
        proptest::collection::vec(g, 1..20)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        /// Whatever the gestures, the stack drawn under each channel is the chain read back from
        /// the cables: same effects, same order, right under the panel, no overlap.
        #[test]
        fn the_drawn_stack_is_always_the_chain_of_the_cables(plan in gestures()) {
            let reg = registry();
            let mut d = two_instruments(&reg);
            let channels = [key("channel"), key("channel-2")];
            let mut v = RackView::new(Theme::dark());
            let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 3000.0)), d.doc(), &reg);
            for g in plan {
                let (ci, command) = match g {
                    Gesture::Add(c, s, t) => {
                        let n = chain(&d, &reg, &channels[c].0).len();
                        let types = ["atomik.reverb", "atomik.eq", "atomik.compressor"];
                        (c, Some(cc(ConsoleCommand::AddInsert {
                            channel: channels[c].clone(),
                            slot: s % (n + 1),
                            effect: InsertEffect::New { type_id: types[t].into() },
                        })))
                    }
                    Gesture::Take(c, s, out) => {
                        let n = chain(&d, &reg, &channels[c].0).len();
                        (c, (n > 0).then(|| cc(ConsoleCommand::ExtractInsert {
                            channel: channels[c].clone(),
                            slot: s % n.max(1),
                            to: out.then_some(RackSlot { column: 0, index: usize::MAX }),
                        })))
                    }
                    Gesture::Reorder(c, a, b) => {
                        let n = chain(&d, &reg, &channels[c].0).len();
                        (c, (n > 1).then(|| cc(ConsoleCommand::MoveInsert {
                            channel: channels[c].clone(),
                            from: a % n.max(1),
                            to: b % n.max(1),
                        })))
                    }
                    Gesture::Delete(c, s) => {
                        let members = chain(&d, &reg, &channels[c].0);
                        (c, (!members.is_empty()).then(|| {
                            Command::Workflow(WorkflowCommand::DeleteDevices {
                                keys: vec![members[s % members.len()].clone()],
                            })
                        }))
                    }
                    Gesture::Fold(c) => {
                        let shown = v.inserts_shown(&channels[c]);
                        v.set_inserts_shown(&channels[c], !shown);
                        (c, None)
                    }
                    Gesture::MoveChannel(c, i) => (c, Some(Command::Workflow(
                        WorkflowCommand::MoveDevicesToSlot {
                            keys: vec![channels[c].clone()],
                            slot: RackSlot { column: 0, index: i },
                            route: i % 2 == 0,
                        },
                    ))),
                };
                let _ = ci;
                if let Some(command) = command {
                    let _ = d.apply(command, &reg);
                }
                v.sync(d.doc(), &reg);
                for k in &channels {
                    let members = chain(&d, &reg, &k.0);
                    let areas = v.insert_areas();
                    let a = area(&areas, &k.0);
                    prop_assert_eq!(&a.members, &members);
                    let mut y = rect(&v, k).y1;
                    for m in &members {
                        if a.shown {
                            let r = rect(&v, m);
                            prop_assert!((r.y0 - y).abs() < 1e-6, "{} is stacked", m);
                            y = r.y1;
                        } else {
                            prop_assert!(v.device_rect(m).is_none());
                        }
                    }
                }
            }
        }
    }
}
