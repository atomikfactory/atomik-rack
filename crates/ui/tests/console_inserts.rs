//! Insert chains in the console strip: the INSERTS slots (0, 1 and 3
//! effects, and a long chain that scrolls), the add menu, select and remove, and display-list
//! goldens. Regenerate goldens
//! with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_inserts`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ConsoleCommand, InsertEffect, RackDoc};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::{ConsoleView, DisplayList, Key, Modifiers, UiAction, UiEvent};
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

// ------------------------------------------------------------------ bypassing the inserts

fn set_param_cmds(actions: &[UiAction]) -> Vec<(String, String, f32)> {
    commands(actions)
        .into_iter()
        .flat_map(|c| match c {
            Command::Batch(all) => all,
            c => vec![c],
        })
        .filter_map(|c| match c {
            Command::SetParam {
                key, param, value, ..
            } => Some((key.0, param, value)),
            _ => None,
        })
        .collect()
}

fn with_bypass(mut doc: RackDoc, bypassed: &[&str]) -> RackDoc {
    for d in &mut doc.devices {
        if bypassed.contains(&d.key.0.as_str()) {
            d.params.insert("bypass".into(), 1.0);
        }
    }
    doc
}

#[test]
fn the_b_of_a_filled_slot_toggles_the_bypass_of_its_effect() {
    let (doc, reg) = (doc_with(2), console_registry());
    let mut v = view(&doc, &reg);
    let ch = key("ch-bass");
    let b = v.insert_bypass_center(&ch, 1).expect("B on a filled slot");
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, b)),
        [("fx2".to_string(), "bypass".to_string(), 1.0)]
    );
    // Bypassed it is switched back in by the same click.
    let doc = with_bypass(doc, &["fx2"]);
    let mut v = view(&doc, &reg);
    let b = v.insert_bypass_center(&ch, 1).unwrap();
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, b)),
        [("fx2".to_string(), "bypass".to_string(), 0.0)]
    );
    // An empty slot has no B, and neither does the "+ add" row.
    let v = view(&doc, &reg);
    assert!(v.insert_bypass_center(&ch, 2).is_none());
    assert!(v.insert_bypass_center(&ch, 3).is_none());
}

#[test]
fn the_bypass_all_button_bypasses_every_effect_or_restores_them_in_one_command() {
    let (doc, reg) = (doc_with(3), console_registry());
    let ch = key("ch-bass");
    let mut v = view(&doc, &reg);
    v.scroll_to_control(&ch, "insert.bypass");
    let p = v.control_center(&ch, "insert.bypass").unwrap();
    let cmds = commands(&click(&mut v, &doc, &reg, p));
    assert_eq!(cmds.len(), 1, "one command, so one undo step");
    assert!(matches!(&cmds[0], Command::Batch(b) if b.len() == 3));
    assert_eq!(
        set_param_cmds(&[UiAction::Command(cmds[0].clone())]),
        [
            ("echo".to_string(), "bypass".to_string(), 1.0),
            ("fx2".to_string(), "bypass".to_string(), 1.0),
            ("fx3".to_string(), "bypass".to_string(), 1.0),
        ]
    );
    // With one still playing, the click bypasses the rest; all bypassed, it restores all.
    let doc = with_bypass(doc, &["echo", "fx3"]);
    let mut v = view(&doc, &reg);
    v.scroll_to_control(&ch, "insert.bypass");
    let p = v.control_center(&ch, "insert.bypass").unwrap();
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, p)),
        [("fx2".to_string(), "bypass".to_string(), 1.0)]
    );
    let doc = with_bypass(doc, &["fx2"]);
    let mut v = view(&doc, &reg);
    v.scroll_to_control(&ch, "insert.bypass");
    let p = v.control_center(&ch, "insert.bypass").unwrap();
    assert_eq!(set_param_cmds(&click(&mut v, &doc, &reg, p)).len(), 3);
    assert!(
        set_param_cmds(&click(&mut v, &doc, &reg, p))
            .iter()
            .all(|(_, param, value)| param == "bypass" && *value == 0.0)
    );
}

#[test]
fn without_a_chain_the_bypass_all_button_is_the_strips_own_switch() {
    let (doc, reg) = (doc_with(0), console_registry());
    let ch = key("ch-bass");
    let mut v = view(&doc, &reg);
    v.scroll_to_control(&ch, "insert.bypass");
    let p = v.control_center(&ch, "insert.bypass").unwrap();
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, p)),
        [("ch-bass".to_string(), "insert.bypass".to_string(), 1.0)]
    );
}

#[test]
fn the_chain_draws_a_b_per_effect_and_the_button_reads_bypass_all() {
    let (doc, reg) = (doc_with(2), console_registry());
    let t = texts(&doc, &reg);
    assert_eq!(t.iter().filter(|s| *s == "B").count(), 2, "{t:?}");
    assert!(t.iter().any(|s| s == "BYP ALL"));
}

// ------------------------------------------------------------------ a long chain scrolls

/// The console document with `n` stereo delays cabled in a chain on ch-bass's insert loop.
fn doc_with_many(n: usize) -> RackDoc {
    let mut doc = console_doc();
    let mut from = ("ch-bass".to_owned(), ["insert_send_l", "insert_send_r"]);
    for i in 0..n {
        let k = format!("long{i}");
        doc.devices
            .push(device(&k, "atomik.stereo-delay", &format!("Echo {i}")));
        for (side, s) in ["in_l", "in_r"].into_iter().enumerate() {
            doc.cables.push(cable((&from.0, from.1[side]), (&k, s)));
        }
        from = (k, ["out_l", "out_r"]);
    }
    for (side, s) in ["insert_return_l", "insert_return_r"]
        .into_iter()
        .enumerate()
    {
        doc.cables
            .push(cable((&from.0, from.1[side]), ("ch-bass", s)));
    }
    doc
}

/// The strip scrolled far enough to show four insert rows whole.
fn long_view(doc: &RackDoc, reg: &Registry) -> ConsoleView {
    let mut v = console_view(doc, reg);
    v.set_scroll_y(400.0);
    v
}

fn wheel(v: &mut ConsoleView, doc: &RackDoc, reg: &Registry, at: Point, dy: f64) {
    v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, dy),
            pos: at,
            modifiers: Modifiers::NONE,
        },
        doc,
        reg,
    );
}

#[test]
fn a_chain_longer_than_four_scrolls_with_the_wheel_over_its_rows() {
    let (doc, reg) = (doc_with_many(6), console_registry());
    let mut v = long_view(&doc, &reg);
    let ch = key("ch-bass");
    assert_eq!(v.insert_scroll_of(&ch), 0);
    assert!(v.insert_slot_center(&ch, 3).is_some());
    assert!(v.insert_slot_center(&ch, 4).is_none(), "below the rows");
    let at = v.insert_slot_center(&ch, 1).unwrap();
    let y = v.scroll_y();
    wheel(&mut v, &doc, &reg, at, -1.0);
    wheel(&mut v, &doc, &reg, at, -1.0);
    assert_eq!(v.insert_scroll_of(&ch), 2);
    assert_eq!(v.scroll_y(), y, "the strip itself stays");
    assert!(v.insert_slot_center(&ch, 1).is_none());
    assert!(v.insert_slot_center(&ch, 5).is_some());
    // The row hits name the slot, not the row.
    let x = v.insert_remove_center(&ch, 4).unwrap();
    assert_eq!(
        console_cmds(&click(&mut v, &doc, &reg, x)),
        [ConsoleCommand::SetInsert {
            channel: ch.clone(),
            slot: 4,
            effect: None
        }]
    );
    // The end of the chain: the add row is the last one.
    wheel(&mut v, &doc, &reg, at, -1.0);
    wheel(&mut v, &doc, &reg, at, -1.0);
    assert_eq!(v.insert_scroll_of(&ch), 3, "stops at the end");
    let add = v.insert_slot_center(&ch, 6).expect("the add row");
    click(&mut v, &doc, &reg, add);
    assert!(!v.menu_items().is_empty(), "the add menu of slot 6");
    // And back.
    let escape = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    v.handle(&escape, &doc, &reg);
    let up_at = v.insert_slot_center(&ch, 5).unwrap();
    for _ in 0..5 {
        wheel(&mut v, &doc, &reg, up_at, 1.0);
    }
    assert_eq!(v.insert_scroll_of(&ch), 0);
}

#[test]
fn a_chain_that_fits_leaves_the_wheel_to_the_strip() {
    let (doc, reg) = (doc_with(3), console_registry());
    let mut v = long_view(&doc, &reg);
    let ch = key("ch-bass");
    let at = v.insert_slot_center(&ch, 1).unwrap();
    wheel(&mut v, &doc, &reg, at, -1.0);
    assert_eq!(v.insert_scroll_of(&ch), 0);
    assert!(v.scroll_y() > 300.0, "the strip scrolled");
}

#[test]
fn a_chain_that_grows_past_its_rows_scrolls_to_the_add_row_and_a_short_one_resets() {
    let reg = console_registry();
    let (four, five) = (doc_with_many(4), doc_with_many(5));
    let mut v = long_view(&four, &reg);
    let ch = key("ch-bass");
    assert_eq!(v.insert_scroll_of(&ch), 0);
    v.sync(&five, &reg);
    assert_eq!(v.insert_scroll_of(&ch), 2, "six rows, four shown");
    assert!(v.insert_slot_center(&ch, 5).is_some());
    v.sync(&four, &reg);
    assert_eq!(v.insert_scroll_of(&ch), 1, "back in range");
    v.sync(&doc_with(1), &reg);
    assert_eq!(v.insert_scroll_of(&ch), 0);
}

#[test]
fn scrolled_strip_with_six_inserts_matches_golden() {
    let (doc, reg) = (doc_with_many(6), console_registry());
    let mut v = long_view(&doc, &reg);
    v.scroll_inserts_to(&key("ch-bass"), 2);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("console_inserts6_scrolled_1x.json", &dl.to_json());
}

#[test]
fn the_b_of_a_container_slot_toggles_the_bypass_of_the_container() {
    let (mut doc, mut reg) = (doc_with(2), console_registry());
    reg.register(atomik_devices::container::factory()).unwrap();
    for d in &mut doc.devices {
        if d.key.0 == "fx2" {
            d.type_id = atomik_model::CONTAINER_TYPE.into();
        }
    }
    let ch = key("ch-bass");
    let mut v = view(&doc, &reg);
    let b = v
        .insert_bypass_center(&ch, 1)
        .expect("B on a container slot");
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, b)),
        [("fx2".to_string(), "bypass".to_string(), 1.0)]
    );
    let doc = with_bypass(doc, &["fx2"]);
    let mut v = view(&doc, &reg);
    let b = v.insert_bypass_center(&ch, 1).unwrap();
    assert_eq!(
        set_param_cmds(&click(&mut v, &doc, &reg, b)),
        [("fx2".to_string(), "bypass".to_string(), 0.0)]
    );
}

fn drag(v: &mut ConsoleView, doc: &RackDoc, reg: &Registry, a: Point, b: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(a), doc, reg);
    out.extend(v.handle(&mv(Point::new(a.x, a.y + 3.0)), doc, reg));
    out.extend(v.handle(&mv(b), doc, reg));
    out.extend(v.handle(&up(b), doc, reg));
    out
}

#[test]
fn dragging_an_insert_row_reorders_the_chain_in_one_command() {
    let (doc, reg) = (doc_with(3), console_registry());
    let mut v = view(&doc, &reg);
    let ch = key("ch-bass");
    // The first row is dragged below the last one (the name part of the row, left of the E).
    let from = v.insert_slot_center(&ch, 0).unwrap() - Vec2::new(14.0, 0.0);
    let to = v.insert_slot_center(&ch, 2).unwrap() + Vec2::new(0.0, 4.0) - Vec2::new(14.0, 0.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveInsert {
            channel: ch.clone(),
            from: 0,
            to: 2
        }]
    );
    assert!(
        !a.iter().any(|x| matches!(x, UiAction::Select(_))),
        "a drag does not select"
    );
    // The last row dragged to the top moves up.
    let from = v.insert_slot_center(&ch, 2).unwrap() - Vec2::new(14.0, 0.0);
    let to = v.insert_slot_center(&ch, 0).unwrap() - Vec2::new(14.0, 4.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveInsert {
            channel: ch.clone(),
            from: 2,
            to: 0
        }]
    );
    // A drag that ends on its own row changes nothing.
    let at = v.insert_slot_center(&ch, 1).unwrap() - Vec2::new(14.0, 0.0);
    let a = drag(&mut v, &doc, &reg, at, at + Vec2::new(0.0, 5.0));
    assert!(console_cmds(&a).is_empty());
}

#[test]
fn the_e_of_a_row_asks_the_rack_to_show_its_effect() {
    let (doc, reg) = (doc_with(2), console_registry());
    let mut v = view(&doc, &reg);
    let ch = key("ch-bass");
    let e = v.insert_edit_center(&ch, 1).unwrap();
    assert_eq!(
        click(&mut v, &doc, &reg, e),
        [UiAction::EditInsert(key("fx2"))]
    );
    assert!(
        v.insert_edit_center(&ch, 2).is_none(),
        "an empty row has no E"
    );
}

/// [`doc_with`]'s effects cabled on the master's insert loop instead (echo, fx2, fx3).
fn master_doc_with(n: usize) -> RackDoc {
    let mut doc = doc_with(0);
    let chain = ["echo", "fx2", "fx3"];
    let mut from = ("master", ["insert_send_l", "insert_send_r"]);
    for fx in chain.iter().take(n) {
        // The echo is the FX return 1 effect in this document; free it first.
        doc.cables
            .retain(|c| !(c.from.device.0 == *fx || c.to.device.0 == *fx));
        for (side, s) in ["in_l", "in_r"].into_iter().enumerate() {
            doc.cables.push(cable((from.0, from.1[side]), (fx, s)));
        }
        from = (fx, ["out_l", "out_r"]);
    }
    for (side, s) in ["insert_return_l", "insert_return_r"]
        .into_iter()
        .enumerate()
    {
        doc.cables
            .push(cable((from.0, from.1[side]), ("master", s)));
    }
    doc.console.returns = Default::default();
    doc
}

#[test]
fn the_master_insert_rows_reorder_like_a_strips() {
    let (doc, reg) = (master_doc_with(3), console_registry());
    let mut v = view(&doc, &reg);
    let m = key("master");
    let from = v.insert_slot_center(&m, 0).unwrap() - Vec2::new(14.0, 0.0);
    let to = v.insert_slot_center(&m, 2).unwrap() + Vec2::new(0.0, 4.0) - Vec2::new(14.0, 0.0);
    let a = drag(&mut v, &doc, &reg, from, to);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::MoveInsert {
            channel: m.clone(),
            from: 0,
            to: 2
        }]
    );
    assert!(!a.iter().any(|x| matches!(x, UiAction::Select(_))));
}

#[test]
fn the_e_of_a_master_insert_row_asks_the_rack_to_show_its_effect() {
    let (doc, reg) = (master_doc_with(2), console_registry());
    let mut v = view(&doc, &reg);
    let e = v.insert_edit_center(&key("master"), 1).unwrap();
    assert_eq!(
        click(&mut v, &doc, &reg, e),
        [UiAction::EditInsert(key("fx2"))]
    );
}
