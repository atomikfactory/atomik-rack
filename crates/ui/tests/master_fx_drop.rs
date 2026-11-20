//! Browser rows dropped on the mixer strips in the rack: on the master section's panel an
//! effect becomes the next free FX send, on the insert area of the master, a channel or a bus
//! it becomes an insert effect after the effect above the pointer (first in an empty area).

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateTarget, DeviceKey, Document, InsertEffect, OutputDest, RackDoc,
    WorkflowCommand, insert_chain, send_chain,
};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{RackView, Theme, UiAction, UiEvent};
use support::starter::*;
use support::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn insert(d: &mut Document, reg: &Registry, strip: &DeviceKey, slot: usize, t: &str) {
    run(
        d,
        reg,
        [cc(ConsoleCommand::AddInsert {
            channel: strip.clone(),
            slot,
            effect: InsertEffect::New { type_id: t.into() },
        })],
    );
}

fn with_bus(reg: &Registry) -> (Document, DeviceKey, DeviceKey) {
    let mut d = two_instruments(reg);
    run(
        &mut d,
        reg,
        [
            Command::SetFolded {
                key: key("channel"),
                folded: false,
            },
            cc(ConsoleCommand::CreateBus {
                name: "Drums".into(),
                dest: OutputDest::Master,
            }),
        ],
    );
    let bus = d.doc().console.buses[0].bus.clone();
    let master = d.doc().console.master.clone().expect("a master");
    (d, bus, master)
}

fn tall(doc: &RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 7000.0)), doc, reg);
    v.sync(doc, reg);
    v.set_browser_visible(true);
    v
}

fn row_of(v: &RackView, type_id: &str) -> Point {
    let i = v
        .browser_rows(&registry())
        .iter()
        .position(|r| *r == type_id)
        .unwrap_or_else(|| panic!("no row {type_id}"));
    v.browser_row_center(i).unwrap()
}

fn drop_row(
    v: &mut RackView,
    doc: &RackDoc,
    reg: &Registry,
    type_id: &str,
    to: Point,
) -> Vec<UiAction> {
    v.set_browser_visible(true);
    let from = row_of(v, type_id);
    let mut out = v.handle(&down(from), doc, reg);
    out.extend(v.handle(&mv(to), doc, reg));
    out.extend(v.handle(&up(to), doc, reg));
    out
}

/// Applies the one creation command of a drop and checks it is one undo step.
fn apply(d: &mut Document, reg: &Registry, out: &[UiAction]) {
    let cmds = commands(out);
    assert_eq!(cmds.len(), 1, "{out:?}");
    assert!(
        matches!(cmds[0], Command::Workflow(_)),
        "creation goes through the workflow: {cmds:?}"
    );
    let before = d.doc().devices.len();
    d.apply(cmds[0].clone(), reg).expect("the model takes it");
    assert_eq!(d.doc().devices.len(), before + 1);
    assert!(d.undo().is_some());
    assert_eq!(d.doc().devices.len(), before, "one undo step");
    d.apply(cmds[0].clone(), reg).expect("again");
}

fn kinds(doc: &RackDoc, reg: &Registry, strip: &DeviceKey) -> Vec<String> {
    insert_chain(doc, reg, strip)
        .iter()
        .map(|k| doc.device(k).unwrap().type_id.clone())
        .collect()
}

#[test]
fn an_effect_on_the_master_panel_is_the_next_free_send() {
    let reg = registry();
    let (mut d, _, master) = with_bus(&reg);
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let r = v.device_rect(&master).unwrap();
    // Both halves of the panel: the lower half used to be a master insert.
    for frac in [0.25, 0.85] {
        let to = v.rack_to_view(Point::new(r.x0 + 600.0, r.y0 + r.height() * frac));
        let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
        match &commands(&out)[..] {
            [Command::Workflow(WorkflowCommand::CreateEffect { target, .. })] => {
                assert_eq!(*target, CreateTarget::Master, "frac {frac}");
            }
            other => panic!("{other:?}"),
        }
    }
    let to = v.rack_to_view(Point::new(r.x0 + 600.0, r.y0 + r.height() * 0.85));
    let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
    apply(&mut d, &reg, &out);
    assert_eq!(send_chain(d.doc(), &reg, 0).len(), 1);
    assert!(insert_chain(d.doc(), &reg, &master).is_empty());
}

#[test]
fn an_instrument_on_the_master_panel_is_not_a_send() {
    let reg = registry();
    let (d, _, master) = with_bus(&reg);
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let r = v.device_rect(&master).unwrap();
    let to = v.rack_to_view(Point::new(r.x0 + 600.0, r.y0 + r.height() * 0.5));
    let out = drop_row(&mut v, &doc, &reg, "atomik.poly-synth", to);
    assert!(
        !commands(&out)
            .iter()
            .any(|c| matches!(c, Command::Workflow(WorkflowCommand::CreateEffect { .. }))),
        "{out:?}"
    );
}

#[test]
fn an_effect_on_an_empty_master_area_is_the_first_master_insert() {
    let reg = registry();
    let (mut d, _, master) = with_bus(&reg);
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let to = v.insert_row_center(&master).unwrap();
    let out = drop_row(&mut v, &doc, &reg, "atomik.eq", to);
    apply(&mut d, &reg, &out);
    assert_eq!(kinds(d.doc(), &reg, &master), ["atomik.eq"]);
    assert!(send_chain(d.doc(), &reg, 0).is_empty(), "no send was made");
}

#[test]
fn an_effect_dropped_between_master_inserts_goes_after_the_one_above() {
    let reg = registry();
    let (mut d, _, master) = with_bus(&reg);
    insert(&mut d, &reg, &master, 0, "atomik.eq");
    insert(&mut d, &reg, &master, 1, "atomik.compressor");
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let chain = insert_chain(&doc, &reg, &master);
    let first = v.device_rect(&chain[0]).unwrap();
    // The lower half of insert 1: the new effect is second.
    let to = v.rack_to_view(Point::new(first.x0 + 300.0, first.y1 - 2.0));
    let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
    apply(&mut d, &reg, &out);
    assert_eq!(
        kinds(d.doc(), &reg, &master),
        ["atomik.eq", "atomik.reverb", "atomik.compressor"]
    );
    assert!(send_chain(d.doc(), &reg, 0).is_empty());
}

#[test]
fn an_effect_below_the_last_master_insert_is_appended() {
    let reg = registry();
    let (mut d, _, master) = with_bus(&reg);
    insert(&mut d, &reg, &master, 0, "atomik.eq");
    insert(&mut d, &reg, &master, 1, "atomik.compressor");
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let to = v.insert_row_center(&master).unwrap();
    let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
    apply(&mut d, &reg, &out);
    assert_eq!(
        kinds(d.doc(), &reg, &master),
        ["atomik.eq", "atomik.compressor", "atomik.reverb"]
    );
}

#[test]
fn an_effect_on_the_insert_area_of_a_channel_or_a_bus_is_an_insert_there() {
    let reg = registry();
    let (mut d, bus, _) = with_bus(&reg);
    let channel = key("channel");
    insert(&mut d, &reg, &channel, 0, "atomik.eq");
    insert(&mut d, &reg, &channel, 1, "atomik.compressor");
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    // After insert 1 of 2 on the channel.
    let first = v
        .device_rect(&insert_chain(&doc, &reg, &channel)[0])
        .unwrap();
    let to = v.rack_to_view(Point::new(first.x0 + 300.0, first.y1 - 2.0));
    let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
    apply(&mut d, &reg, &out);
    assert_eq!(
        kinds(d.doc(), &reg, &channel),
        ["atomik.eq", "atomik.reverb", "atomik.compressor"]
    );
    // The empty area of the bus.
    let doc = d.doc().clone();
    let mut v = tall(&doc, &reg);
    let to = v.insert_row_center(&bus).unwrap();
    let out = drop_row(&mut v, &doc, &reg, "atomik.reverb", to);
    apply(&mut d, &reg, &out);
    assert_eq!(kinds(d.doc(), &reg, &bus), ["atomik.reverb"]);
    assert!(send_chain(d.doc(), &reg, 0).is_empty());
}
