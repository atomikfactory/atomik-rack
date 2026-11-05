//! VCA strips in the console view: where they sit, their mute, solo and fader, rename and the
//! context menus, assigning by menu and by dragging a name plate, and what a member's strip shows.

mod support;

use atomik_model::{Command, ConsoleCommand, RackDoc, VcaGroup};
use atomik_ui::kurbo::Point;
use atomik_ui::{
    ConsoleView, DisplayList, Key, Modifiers, PointerButton, UiAction, UiEvent, VcaPart,
};
use support::console::*;
use support::*;

/// The bus document with two groups: `Drums` (id 1, `ch-bass` and `ch-lead`, -6 dB, red) and
/// `Keys` (id 2, `ch-keys`, no colour, muted).
fn vca_doc() -> RackDoc {
    let mut doc = bus_doc();
    doc.console.vcas = vec![
        VcaGroup {
            id: 1,
            name: "Drums".into(),
            color: Some([0xe0, 0x5a, 0x4f]),
            level_db: -6.0,
            mute: false,
            solo: false,
            members: vec![key("ch-bass"), key("ch-lead")],
            follows: None,
        },
        VcaGroup {
            id: 2,
            name: "Keys".into(),
            color: None,
            level_db: 0.0,
            mute: true,
            solo: false,
            members: vec![key("ch-keys")],
            follows: None,
        },
    ];
    doc
}

/// A view wide enough to show every strip of the document, the VCA strips included.
fn wide_view(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> ConsoleView {
    let mut v = console_view(doc, reg);
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1600.0, 760.0));
    v.sync(doc, reg);
    v
}

fn setup() -> (RackDoc, atomik_device_sdk::Registry, ConsoleView) {
    let (doc, reg) = (vca_doc(), console_registry());
    let v = wide_view(&doc, &reg);
    (doc, reg, v)
}

type Reg = atomik_device_sdk::Registry;

fn click(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, p: Point) -> Vec<UiAction> {
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn double_click(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, p: Point) -> Vec<UiAction> {
    let mut a = v.handle(&down_with(p, PointerButton::Primary, 2), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn right_click(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, p: Point) -> Vec<UiAction> {
    v.handle(&down_with(p, PointerButton::Secondary, 1), doc, reg)
}

fn click_part(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, id: u32, p: VcaPart) -> Vec<UiAction> {
    let at = part(v, id, p);
    click(v, doc, reg, at)
}

fn double_click_part(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &Reg,
    id: u32,
    p: VcaPart,
) -> Vec<UiAction> {
    let at = part(v, id, p);
    double_click(v, doc, reg, at)
}

fn click_name(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, strip: &str) -> Vec<UiAction> {
    let at = v.name_center(&key(strip)).unwrap();
    click(v, doc, reg, at)
}

fn right_click_name(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, strip: &str) -> Vec<UiAction> {
    let at = v.name_center(&key(strip)).unwrap();
    right_click(v, doc, reg, at)
}

/// Shift-clicks a strip's name plate (extends the selection).
fn shift_click_name(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, strip: &str) {
    let at = v.name_center(&key(strip)).unwrap();
    v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
            clicks: 1,
        },
        doc,
        reg,
    );
    v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
        },
        doc,
        reg,
    );
}

fn part(v: &ConsoleView, id: u32, part: VcaPart) -> Point {
    v.vca_center(id, part)
        .unwrap_or_else(|| panic!("no {part:?} on VCA {id}"))
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

fn pick(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, text: &str) -> Vec<UiAction> {
    let i = v
        .menu_items()
        .iter()
        .position(|t| t == text)
        .unwrap_or_else(|| panic!("no menu row {text:?} in {:?}", v.menu_items()));
    let p = v.menu_row_center(i).unwrap();
    click(v, doc, reg, p)
}

fn key_event(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn texts(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.texts().into_iter().map(str::to_owned).collect()
}

// ---------------------------------------------------------------- strips

#[test]
fn vca_strips_follow_the_buses_one_slot_each_and_none_without_groups() {
    let (doc, reg, v) = setup();
    assert_eq!(v.vca_ids(), [1, 2]);
    let last_bus = v.strip_rect(&key("bus-2")).unwrap();
    let a = v.vca_rect(1).unwrap();
    let b = v.vca_rect(2).unwrap();
    assert!(
        a.x0 > last_bus.x1 && b.x0 > a.x1,
        "{last_bus:?} {a:?} {b:?}"
    );
    assert!(
        (a.width() - last_bus.width()).abs() < 1e-9,
        "a strip slot wide"
    );
    assert_eq!(a.y0, last_bus.y0);
    // A project without groups has none, and the same strips as before.
    let plain = bus_doc();
    let v = wide_view(&plain, &reg);
    assert!(v.vca_ids().is_empty() && v.vca_rect(1).is_none());
    assert_eq!(
        v.strip_rect(&key("bus-2")),
        wide_view(&doc, &reg).strip_rect(&key("bus-2"))
    );
}

#[test]
fn a_vca_strip_shows_its_name_member_count_and_offset() {
    let (doc, reg, mut v) = setup();
    let t = texts(&mut v, &doc, &reg);
    for want in [
        "Drums", "Keys", "VCA 1", "VCA 2", "2 ch", "1 ch", "-6.0 dB", "0.0 dB",
    ] {
        assert!(t.iter().any(|s| s == want), "{want} missing from {t:?}");
    }
}

#[test]
fn mute_and_solo_buttons_send_the_linked_commands_and_follow_the_state() {
    let (doc, reg, mut v) = setup();
    let a = click_part(&mut v, &doc, &reg, 1, VcaPart::Mute);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaMute { vca: 1, on: true }]
    );
    let a = click_part(&mut v, &doc, &reg, 1, VcaPart::Solo);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaSolo { vca: 1, on: true }]
    );
    // Group 2 is muted: the click turns it off.
    let a = click_part(&mut v, &doc, &reg, 2, VcaPart::Mute);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaMute { vca: 2, on: false }]
    );
}

#[test]
fn the_fader_drag_is_one_merge_group_and_ends_it() {
    let (doc, reg, mut v) = setup();
    let p = part(&v, 1, VcaPart::Fader);
    let mut all = v.handle(&down(p), &doc, &reg);
    assert!(all.is_empty(), "a press alone changes nothing: {all:?}");
    for dy in [10.0, 20.0, 40.0] {
        all.extend(v.handle(&mv(Point::new(p.x, p.y - dy)), &doc, &reg));
    }
    all.extend(v.handle(&up(Point::new(p.x, p.y - 40.0)), &doc, &reg));
    let cmds = console_cmds(&all);
    assert_eq!(cmds.len(), 3, "{cmds:?}");
    let mut last = -6.0f32;
    let mut merge = None;
    for c in &cmds {
        let ConsoleCommand::SetVcaLevel {
            vca: 1,
            db,
            merge: m,
        } = c
        else {
            panic!("{c:?}")
        };
        assert!(
            *db > last,
            "dragging up raises the level: {db} after {last}"
        );
        last = *db;
        assert!(m.is_some() && (merge.is_none() || merge == Some(*m)));
        merge = Some(*m);
    }
    assert!(all.iter().any(|a| matches!(a, UiAction::EndMerge)));
    // Dragging down far clamps at the bottom, which is off.
    let mut all = v.handle(&down(p), &doc, &reg);
    all.extend(v.handle(&mv(Point::new(p.x, p.y + 600.0)), &doc, &reg));
    match console_cmds(&all).last() {
        Some(ConsoleCommand::SetVcaLevel { db, .. }) => assert_eq!(*db, atomik_core::VCA_MIN_DB),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_double_click_resets_the_offset_and_the_wheel_steps_it() {
    let (doc, reg, mut v) = setup();
    let a = double_click_part(&mut v, &doc, &reg, 1, VcaPart::Fader);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaLevel {
            vca: 1,
            db: 0.0,
            merge: None
        }]
    );
    // Already at 0 dB: nothing to reset.
    let a = double_click_part(&mut v, &doc, &reg, 2, VcaPart::Readout);
    assert!(a.is_empty(), "{a:?}");
    let p = part(&v, 1, VcaPart::Fader);
    let a = v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
            pos: p,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    match console_cmds(&a).as_slice() {
        [ConsoleCommand::SetVcaLevel { vca: 1, db, .. }] => assert!(*db > -6.0, "{db}"),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- plate, rename, menu

#[test]
fn a_click_on_the_plate_selects_the_members() {
    let (doc, reg, mut v) = setup();
    let a = click_part(&mut v, &doc, &reg, 1, VcaPart::Plate);
    assert!(a.is_empty(), "{a:?}");
    assert_eq!(v.selected_strips(), [key("ch-bass"), key("ch-lead")]);
    click_part(&mut v, &doc, &reg, 2, VcaPart::Plate);
    assert_eq!(v.selected_strips(), [key("ch-keys")]);
}

#[test]
fn a_double_click_renames_in_place_and_enter_sends_rename_vca() {
    let (doc, reg, mut v) = setup();
    let a = double_click_part(&mut v, &doc, &reg, 1, VcaPart::Plate);
    assert!(a.is_empty());
    assert!(v.is_text_owner());
    assert_eq!(v.rename_text(), Some("Drums"));
    for _ in 0..5 {
        v.handle(&key_event(Key::Backspace), &doc, &reg);
    }
    for c in "Kit".chars() {
        v.handle(&key_event(Key::Char(c)), &doc, &reg);
    }
    assert!(texts(&mut v, &doc, &reg).iter().any(|s| s == "Kit|"));
    let a = v.handle(&key_event(Key::Enter), &doc, &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RenameVca {
            vca: 1,
            name: "Kit".into()
        }]
    );
    assert!(!v.is_text_owner());
    // Escape and an unchanged name send nothing.
    double_click_part(&mut v, &doc, &reg, 1, VcaPart::Plate);
    assert!(v.handle(&key_event(Key::Escape), &doc, &reg).is_empty());
    double_click_part(&mut v, &doc, &reg, 2, VcaPart::Plate);
    assert!(v.handle(&key_event(Key::Enter), &doc, &reg).is_empty());
}

#[test]
fn the_vca_menu_renames_selects_colours_and_deletes() {
    let (doc, reg, mut v) = setup();
    let p = part(&v, 1, VcaPart::Plate);
    assert!(right_click(&mut v, &doc, &reg, p).is_empty());
    let items = v.menu_items();
    assert_eq!(items[0], "Rename");
    assert_eq!(items[1], "Select members");
    assert_eq!(items.last().unwrap(), "Delete VCA");
    assert!(items.iter().any(|s| s == "Colour: blue") && items.iter().any(|s| s == "Colour: none"));
    let a = pick(&mut v, &doc, &reg, "Colour: blue");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaColor {
            vca: 1,
            color: Some([0x3f, 0x8f, 0xd2])
        }]
    );
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "Colour: none");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaColor {
            vca: 1,
            color: None
        }]
    );
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "Select members");
    assert!(a.is_empty());
    assert_eq!(v.selected_strips(), [key("ch-bass"), key("ch-lead")]);
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "Rename");
    assert!(a.is_empty() && v.is_text_owner());
    v.handle(&key_event(Key::Escape), &doc, &reg);
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "Delete VCA");
    assert_eq!(console_cmds(&a), [ConsoleCommand::DeleteVca { vca: 1 }]);
}

// ---------------------------------------------------------------- assigning

#[test]
fn a_channel_menu_offers_new_vca_add_to_and_remove_from() {
    let (doc, reg, mut v) = setup();
    // ch-keys is in `Keys` only: it can be added to `Drums`, removed, or start a new group.
    let p = v.name_center(&key("ch-keys")).unwrap();
    assert!(right_click(&mut v, &doc, &reg, p).is_empty());
    assert_eq!(
        vca_items(&v),
        ["New VCA...", "Add to VCA Drums", "Remove from VCA"]
    );
    let a = pick(&mut v, &doc, &reg, "Add to VCA Drums");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::AssignToVca {
            vca: Some(1),
            strips: vec![key("ch-keys")]
        }]
    );
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "Remove from VCA");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::AssignToVca {
            vca: None,
            strips: vec![key("ch-keys")]
        }]
    );
    right_click(&mut v, &doc, &reg, p);
    let a = pick(&mut v, &doc, &reg, "New VCA...");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::CreateVca {
            name: "VCA 1".into(),
            members: vec![key("ch-keys")]
        }]
    );
}

#[test]
fn the_menu_acts_on_the_whole_selection_when_the_strip_is_in_it() {
    let (mut doc, reg, _) = setup();
    doc.console.vcas.clear();
    let mut v = wide_view(&doc, &reg);
    click_name(&mut v, &doc, &reg, "ch-bass");
    shift_click_name(&mut v, &doc, &reg, "ch-lead");
    assert_eq!(v.selected_strips().len(), 2);
    right_click_name(&mut v, &doc, &reg, "ch-lead");
    assert_eq!(vca_items(&v), ["New VCA..."], "no group to add to yet");
    let a = pick(&mut v, &doc, &reg, "New VCA...");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::CreateVca {
            name: "VCA 1".into(),
            members: vec![key("ch-bass"), key("ch-lead")]
        }]
    );
    // A strip outside the selection is alone.
    right_click_name(&mut v, &doc, &reg, "ch-keys");
    let a = pick(&mut v, &doc, &reg, "New VCA...");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::CreateVca {
            name: "VCA 1".into(),
            members: vec![key("ch-keys")]
        }]
    );
    // A bus plate has its own entries first.
    right_click_name(&mut v, &doc, &reg, "bus");
    assert_eq!(vca_items(&v), ["Rename", "Delete bus", "New VCA..."]);
}

fn drag_name_to(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &Reg,
    strip: &str,
    to: Point,
) -> Vec<UiAction> {
    let p = v.name_center(&key(strip)).unwrap();
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&mv(Point::new(to.x, p.y)), doc, reg));
    a.extend(v.handle(&up(Point::new(to.x, p.y)), doc, reg));
    a
}

#[test]
fn dragging_a_name_plate_onto_a_vca_strip_assigns_the_strip() {
    let (doc, reg, mut v) = setup();
    let onto_drums = v.vca_rect(1).unwrap().center();
    let a = drag_name_to(&mut v, &doc, &reg, "ch-keys", onto_drums);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::AssignToVca {
            vca: Some(1),
            strips: vec![key("ch-keys")]
        }]
    );
    // A member dropped on its own group changes nothing, and nothing reorders.
    let a = drag_name_to(&mut v, &doc, &reg, "ch-bass", onto_drums);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn dragging_a_selected_strip_assigns_the_whole_selection() {
    let (mut doc, reg, _) = setup();
    doc.console.vcas.truncate(1);
    let mut v = wide_view(&doc, &reg);
    click_name(&mut v, &doc, &reg, "ch-keys");
    shift_click_name(&mut v, &doc, &reg, "bus");
    let to = v.vca_rect(1).unwrap().center();
    let a = drag_name_to(&mut v, &doc, &reg, "ch-keys", to);
    match console_cmds(&a).as_slice() {
        [
            ConsoleCommand::AssignToVca {
                vca: Some(1),
                strips,
            },
        ] => {
            assert!(
                strips.contains(&key("ch-keys")) && strips.contains(&key("bus")),
                "{strips:?}"
            )
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- members

/// The readout of a member at fader position `pos` under `offset` dB, as the strip writes it.
fn readout(pos: f32, offset: f32) -> String {
    let db = 60.0 * (pos / 0.75).log10() + offset;
    format!("{:.1} dB", db + 0.0)
}

#[test]
fn a_member_shows_the_level_it_plays_at_not_its_own_fader() {
    let (mut doc, reg, _) = setup();
    let pos = doc
        .param_value(&key("ch-bass"), "fader", &reg)
        .expect("a fader");
    let own = readout(pos, 0.0);
    let under = readout(pos, -6.0);
    assert_ne!(own, under);
    let mut v = wide_view(&doc, &reg);
    let t = texts(&mut v, &doc, &reg);
    assert!(t.contains(&under), "{under} missing from {t:?}");
    // Out of the group, the member reads its own level again.
    doc.console.vcas[0].members.retain(|m| m != &key("ch-bass"));
    let mut v = wide_view(&doc, &reg);
    let t = texts(&mut v, &doc, &reg);
    assert!(t.contains(&own), "{own} missing from {t:?}");
    assert!(!t.contains(&under));
}

#[test]
fn removing_a_group_drops_its_strip_and_a_vanished_rename_is_cancelled() {
    let (mut doc, reg, mut v) = setup();
    double_click_part(&mut v, &doc, &reg, 2, VcaPart::Plate);
    assert!(v.is_text_owner());
    doc.console.vcas.remove(1);
    v.sync(&doc, &reg);
    assert_eq!(v.vca_ids(), [1]);
    assert!(!v.is_text_owner());
}

// ---------------------------------------------------------------- the arrangement

#[test]
fn a_track_whose_channel_is_in_a_group_gets_the_groups_colour_bar() {
    use atomik_model::ConsoleChannel;
    let reg = registry();
    let mut doc = support::song::song_doc();
    doc.console.channels.push(ConsoleChannel {
        channel: key("ch-bass"),
        source: Some(key("bass")),
    });
    let paint = |doc: &RackDoc| {
        let mut v = support::song::arrangement(doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, doc, &reg);
        dl.to_json()
    };
    let chip = "#e05a4fff";
    assert!(!paint(&doc).contains(chip), "no group, no bar");
    doc.console.vcas.push(VcaGroup {
        id: 1,
        name: "Drums".into(),
        color: Some([0xe0, 0x5a, 0x4f]),
        level_db: 0.0,
        mute: false,
        solo: false,
        members: vec![key("ch-bass")],
        follows: None,
    });
    assert!(paint(&doc).contains(chip), "the bar shows the colour");
    // A group without a colour still marks its tracks (neutral), and leaving it removes the bar.
    doc.console.vcas[0].color = None;
    assert!(!paint(&doc).contains(chip));
    doc.console.vcas[0].members.clear();
    let plain = paint(&doc);
    doc.console.vcas.clear();
    assert_eq!(paint(&doc), plain, "an empty group changes nothing");
}

/// What every strip menu ends with: the channel settings commands.
const SETTINGS_ITEMS: [&str; 13] = [
    "Cut strip",
    "Copy strip",
    "Paste strip",
    "Duplicate strip",
    "Copy settings",
    "Copy levels",
    "Copy sends",
    "Copy inserts",
    "Paste settings",
    "Reset channel",
    "Clear inserts",
    "Create parallel channel",
    "Delete channel",
];

/// The menu entries before the channel settings ones.
fn vca_items(v: &ConsoleView) -> Vec<String> {
    let items = v.menu_items();
    assert!(
        items.ends_with(&SETTINGS_ITEMS.map(str::to_owned)),
        "{items:?}"
    );
    items[..items.len() - SETTINGS_ITEMS.len()].to_vec()
}

// ---------------------------------------------------------------- link faders

/// The fader batches a drag of `from`'s fader by a tenth of its travel sends.
fn fader_drag(
    v: &mut ConsoleView,
    doc: &RackDoc,
    reg: &Reg,
    from: &str,
) -> Vec<Vec<(String, String, f32)>> {
    let l = v.strip_layout().unwrap().clone();
    let fader = l.controls.iter().find(|c| c.param == "fader").unwrap();
    let travel = fader.rect.height() - 16.0;
    let p = v.control_center(&key(from), "fader").unwrap();
    v.handle(&down(p), doc, reg);
    let a = v.handle(
        &mv(p + atomik_ui::kurbo::Vec2::new(0.0, travel * 0.1)),
        doc,
        reg,
    );
    v.handle(&up(p), doc, reg);
    commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            Command::Batch(list) => Some(
                list.into_iter()
                    .filter_map(|c| match c {
                        Command::SetParam {
                            key, param, value, ..
                        } => Some((key.0, param, value)),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn link_faders_moves_the_levels_of_the_whole_group_without_a_selection() {
    let (doc, reg, mut v) = setup();
    assert!(!v.link_faders());
    // Off: a lone fader moves alone.
    assert!(fader_drag(&mut v, &doc, &reg, "ch-bass").is_empty());
    v.set_link_faders(true);
    assert!(v.link_faders());
    let steps = fader_drag(&mut v, &doc, &reg, "ch-bass");
    assert_eq!(steps.len(), 1, "one batch for the step");
    let names: Vec<&str> = steps[0].iter().map(|s| s.0.as_str()).collect();
    assert_eq!(
        names,
        ["ch-bass", "ch-lead"],
        "the members of Drums, not Keys"
    );
    assert!(steps[0].iter().all(|s| s.1 == "fader"));
    // The strips keep their offsets: the same dB for both.
    let db = |pos: f32| 60.0 * (pos / 0.75).log10();
    let (a, b) = (db(steps[0][0].2) - db(0.62), db(steps[0][1].2) - db(0.75));
    assert!((a - b).abs() < 1e-3 && a < -1.0, "{a} {b}");
}

#[test]
fn link_faders_leaves_a_strip_outside_any_group_alone_and_a_selection_wins() {
    let (mut doc, reg, mut v) = setup();
    v.set_link_faders(true);
    // `ch-keys` is in Keys alone: nothing else to move.
    assert!(fader_drag(&mut v, &doc, &reg, "ch-keys").is_empty());
    doc.console.vcas[1].members.clear();
    v.sync(&doc, &reg);
    assert!(fader_drag(&mut v, &doc, &reg, "ch-keys").is_empty());
    // A multiple selection takes its own strips along, group or not.
    v.set_selected_strips(&[key("ch-bass"), key("ch-keys")]);
    v.sync(&doc, &reg);
    let steps = fader_drag(&mut v, &doc, &reg, "ch-bass");
    let names: Vec<&str> = steps[0].iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, ["ch-bass", "ch-keys"]);
}

// ---------------------------------------------------------------- dragging a track header

/// The song of the arrangement tests with a console channel for each of `bass`, `lead` and
/// `keys`, and a group `Drums` (id 1) holding the channel of `bass`.
fn grouped_song() -> (
    RackDoc,
    atomik_device_sdk::Registry,
    atomik_ui::ArrangementView,
) {
    use atomik_model::ConsoleChannel;
    let reg = registry();
    let mut doc = support::song::song_doc();
    for (channel, source) in [
        ("ch-bass", "bass"),
        ("ch-lead", "lead"),
        ("ch-keys", "keys"),
    ] {
        doc.console.channels.push(ConsoleChannel {
            channel: key(channel),
            source: Some(key(source)),
        });
    }
    doc.console.vcas.push(VcaGroup {
        id: 1,
        name: "Drums".into(),
        color: Some([0xe0, 0x5a, 0x4f]),
        level_db: 0.0,
        mute: false,
        solo: false,
        members: vec![key("ch-bass")],
        follows: None,
    });
    let v = support::song::arrangement(&doc, &reg);
    (doc, reg, v)
}

/// The point on the group bar at the right edge of `device`'s header.
fn bar_of(v: &atomik_ui::ArrangementView, device: &str) -> Point {
    let lane = v.lane_rect(&key(device), 0).unwrap();
    Point::new(lane.x0 - 6.0, lane.y0 + 12.0)
}

#[test]
fn a_header_dropped_on_the_bar_of_a_group_joins_that_group() {
    let (doc, reg, mut v) = grouped_song();
    let start = v.track_header_center(&key("lead")).unwrap();
    let on_bar = bar_of(&v, "bass");
    v.handle(&down(start), &doc, &reg);
    v.handle(
        &mv(Point::new(start.x, (start.y + on_bar.y) / 2.0)),
        &doc,
        &reg,
    );
    v.handle(&mv(on_bar), &doc, &reg);
    assert_eq!(v.vca_drop(), Some(1), "the bar is the target");
    assert!(v.track_drop().is_none(), "no insertion line over the bar");
    let actions = v.handle(&up(on_bar), &doc, &reg);
    assert_eq!(
        console_cmds(&actions),
        [ConsoleCommand::AssignToVca {
            vca: Some(1),
            strips: vec![key("ch-lead")],
        }]
    );
    assert!(commands(&actions).len() == 1, "nothing else is sent");
    assert_eq!(v.vca_drop(), None, "the drag is over");
}

#[test]
fn a_header_dragged_elsewhere_still_moves_and_several_tracks_join_together() {
    let (doc, reg, mut v) = grouped_song();
    // Over the lanes (not the bar) the drag is the usual move, with a line.
    let start = v.track_header_center(&key("lead")).unwrap();
    let below = v.track_header_center(&key("keys")).unwrap();
    v.handle(&down(start), &doc, &reg);
    v.handle(
        &mv(Point::new(start.x, (start.y + below.y) / 2.0)),
        &doc,
        &reg,
    );
    v.handle(&mv(Point::new(below.x, below.y + 30.0)), &doc, &reg);
    assert_eq!(v.vca_drop(), None);
    v.handle(&up(below), &doc, &reg);

    // Two selected tracks dropped on the bar join as one command.
    let (doc, reg, mut v) = grouped_song();
    v.set_selected_track(Some(key("lead")));
    v.toggle_selected_track(&key("keys"));
    let start = v.track_header_center(&key("lead")).unwrap();
    let on_bar = bar_of(&v, "bass");
    v.handle(&down(start), &doc, &reg);
    v.handle(
        &mv(Point::new(start.x, (start.y + on_bar.y) / 2.0)),
        &doc,
        &reg,
    );
    v.handle(&mv(on_bar), &doc, &reg);
    let actions = v.handle(&up(on_bar), &doc, &reg);
    assert_eq!(
        console_cmds(&actions),
        [ConsoleCommand::AssignToVca {
            vca: Some(1),
            strips: vec![key("ch-lead"), key("ch-keys")],
        }]
    );
}

#[test]
fn the_bar_of_a_track_in_no_group_is_not_a_target() {
    let (doc, reg, mut v) = grouped_song();
    let start = v.track_header_center(&key("keys")).unwrap();
    let lead_bar = bar_of(&v, "lead");
    v.handle(&down(start), &doc, &reg);
    v.handle(
        &mv(Point::new(start.x, (start.y + lead_bar.y) / 2.0)),
        &doc,
        &reg,
    );
    v.handle(&mv(lead_bar), &doc, &reg);
    assert_eq!(v.vca_drop(), None, "lead has no group");
    v.handle(&up(lead_bar), &doc, &reg);
}

#[test]
fn a_group_that_follows_a_folder_offers_to_stop() {
    let (mut doc, reg, mut v) = setup();
    let p = part(&v, 1, VcaPart::Plate);
    right_click(&mut v, &doc, &reg, p);
    assert!(!v.menu_items().iter().any(|s| s == "Stop following folder"));
    v.handle(&key_event(Key::Escape), &doc, &reg);
    doc.console.vcas[0].follows = Some(1);
    v.sync(&doc, &reg);
    right_click(&mut v, &doc, &reg, p);
    let items = v.menu_items();
    assert_eq!(items[items.len() - 2], "Stop following folder");
    let a = pick(&mut v, &doc, &reg, "Stop following folder");
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::SetVcaFollow {
            vca: 1,
            folder: None
        }]
    );
}
