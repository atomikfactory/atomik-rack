//! Insert chains in the console strip: the INSERTS slots (0, 1 and 3
//! effects), the add menu, select and remove, and display-list goldens. Regenerate goldens
//! with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_inserts`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ConsoleCommand, InsertEffect, RackDoc};
use atomik_ui::kurbo::Point;
use atomik_ui::{ConsoleView, DisplayList, UiAction};
use std::path::PathBuf;
use support::console::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from golden (regenerate with {UPDATE_ENV}=1)"
    );
}

/// The console document with `n` (0..=3) effects cabled on ch-bass's insert loop, in the
/// order echo, fx2, fx3 (all stereo delays; `delay` stays on FX return 1).
fn doc_with(n: usize) -> RackDoc {
    let mut doc = console_doc();
    for (k, name) in [("fx2", "Echo Two"), ("fx3", "Echo Three")] {
        doc.devices.push(device(k, "atomik.stereo-delay", name));
    }
    let chain = ["echo", "fx2", "fx3"];
    let mut from = ("ch-bass", ["insert_send_l", "insert_send_r"]);
    for fx in chain.iter().take(n) {
        for (side, s) in ["in_l", "in_r"].into_iter().enumerate() {
            doc.cables.push(cable((from.0, from.1[side]), (fx, s)));
        }
        from = (fx, ["out_l", "out_r"]);
    }
    if n > 0 {
        for (side, s) in ["insert_return_l", "insert_return_r"]
            .into_iter()
            .enumerate()
        {
            doc.cables
                .push(cable((from.0, from.1[side]), ("ch-bass", s)));
        }
    }
    doc
}

fn view(doc: &RackDoc, reg: &Registry) -> ConsoleView {
    let mut v = console_view(doc, reg);
    // INSERTS sits below the first screenful of the upper sections: scroll it into view.
    v.set_scroll_y(300.0);
    v
}

fn click(v: &mut ConsoleView, doc: &RackDoc, reg: &Registry, p: Point) -> Vec<UiAction> {
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn console_cmds(actions: &[UiAction]) -> Vec<ConsoleCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Console(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn texts(doc: &RackDoc, reg: &Registry) -> Vec<String> {
    let mut v = view(doc, reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.texts().into_iter().map(str::to_owned).collect()
}

#[test]
fn an_empty_chain_shows_add_on_the_first_slot_only() {
    let (doc, reg) = (doc_with(0), console_registry());
    let v = view(&doc, &reg);
    let ch = key("ch-bass");
    assert!(v.insert_slot_center(&ch, 0).is_some());
    assert!(
        v.insert_slot_center(&ch, 1).is_none(),
        "later empty slots are inert"
    );
    assert!(v.insert_remove_center(&ch, 0).is_none());
    let t = texts(&doc, &reg);
    assert!(t.iter().any(|s| s == "+ add"));
    assert!(t.iter().any(|s| s == "INSERTS"));
}

#[test]
fn the_add_menu_lists_new_effect_types_then_free_rack_effects() {
    let (doc, reg) = (doc_with(0), console_registry());
    let mut v = view(&doc, &reg);
    let p = v.insert_slot_center(&key("ch-bass"), 0).unwrap();
    assert!(click(&mut v, &doc, &reg, p).is_empty());
    // `delay` is a return effect and is not offered; echo, fx2 and fx3 are free.
    assert_eq!(
        v.menu_items(),
        ["New Stereo Delay", "Echo", "Echo Two", "Echo Three"]
    );
    let row = v.menu_row_center(0).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetInsert {
            channel: key("ch-bass"),
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: "atomik.stereo-delay".into()
            }),
        }]
    );
    let p = v.insert_slot_center(&key("ch-bass"), 0).unwrap();
    click(&mut v, &doc, &reg, p);
    let row = v.menu_row_center(2).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetInsert {
            channel: key("ch-bass"),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("fx2"))),
        }]
    );
}

#[test]
fn a_filled_slot_selects_its_effect_and_the_x_removes_it() {
    let (doc, reg) = (doc_with(1), console_registry());
    let mut v = view(&doc, &reg);
    let ch = key("ch-bass");
    let p = v.insert_slot_center(&ch, 0).unwrap();
    assert!(
        texts(&doc, &reg).iter().any(|s| s == "Echo"),
        "the chain shows the effect name"
    );
    let a = click(
        &mut v,
        &doc,
        &reg,
        p - atomik_ui::kurbo::Vec2::new(10.0, 0.0),
    );
    assert_eq!(a, [UiAction::Select(key("echo"))]);
    let x = v.insert_remove_center(&ch, 0).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, x)),
        [ConsoleCommand::SetInsert {
            channel: ch.clone(),
            slot: 0,
            effect: None
        }]
    );
    // Slot 1 is the next "+ add"; slot 2 is inert.
    assert!(v.insert_slot_center(&ch, 1).is_some());
    assert!(v.insert_slot_center(&ch, 2).is_none());
}

#[test]
fn the_menu_omits_effects_already_in_a_chain() {
    let (doc, reg) = (doc_with(1), console_registry());
    let mut v = view(&doc, &reg);
    let p = v.insert_slot_center(&key("ch-bass"), 1).unwrap();
    click(&mut v, &doc, &reg, p);
    assert_eq!(
        v.menu_items(),
        ["New Stereo Delay", "Echo Two", "Echo Three"]
    );
    let row = v.menu_row_center(1).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetInsert {
            channel: key("ch-bass"),
            slot: 1,
            effect: Some(InsertEffect::Existing(key("fx2"))),
        }]
    );
}

#[test]
fn the_master_has_an_insert_chain_area_too() {
    let (doc, reg) = (doc_with(0), console_registry());
    let mut v = view(&doc, &reg);
    let p = v
        .insert_slot_center(&key("master"), 0)
        .expect("master slot");
    click(&mut v, &doc, &reg, p);
    let row = v.menu_row_center(0).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, row)),
        [ConsoleCommand::SetInsert {
            channel: key("master"),
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: "atomik.stereo-delay".into()
            }),
        }]
    );
}

fn golden(n: usize) -> String {
    let (doc, reg) = (doc_with(n), console_registry());
    let mut v = view(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.to_json()
}

#[test]
fn strip_with_no_inserts_matches_golden() {
    check("console_inserts0_1x.json", &golden(0));
}

#[test]
fn strip_with_one_insert_matches_golden() {
    check("console_inserts1_1x.json", &golden(1));
}

#[test]
fn strip_with_three_inserts_matches_golden() {
    check("console_inserts3_1x.json", &golden(3));
}
