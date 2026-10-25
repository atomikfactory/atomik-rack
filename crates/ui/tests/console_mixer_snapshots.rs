//! The snapshot bar of the console view: the chips, the `+` button, the A/B compare toggle, the
//! right-click menu, renaming in place, the status text and the indicator that the mix moved,
//! with a display-list golden of the bar. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test console_mixer_snapshots`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ConsoleCommand, Document, RackDoc, SnapshotScope, SyncOp};
use atomik_ui::console::{ConsoleButton, SNAPSHOT_BAR_H, TOOLBAR_H};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{ConsoleView, DisplayList, Key, Modifiers, PointerButton, UiAction, UiEvent};
use std::path::PathBuf;
use support::console::*;
use support::*;

type Reg = Registry;

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn set(k: &str, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    }
}

/// The bus document as a `Document` with snapshots A and B stored (B after a fader move).
fn project(snapshots: usize) -> (Document, Reg) {
    let reg = console_registry();
    let mut d = Document::new(bus_doc());
    for n in 0..snapshots {
        if n > 0 {
            d.apply(set("ch-bass", "fader", 0.2 + 0.1 * n as f32), &reg)
                .unwrap();
        }
        d.apply(
            con(ConsoleCommand::StoreSnapshot {
                name: d.doc().console.default_snapshot_name(),
            }),
            &reg,
        )
        .unwrap();
    }
    (d, reg)
}

fn wide(doc: &RackDoc, reg: &Reg) -> ConsoleView {
    let mut v = console_view(doc, reg);
    v.set_bounds(Rect::new(0.0, 0.0, 1400.0, 784.0));
    v.sync(doc, reg);
    v
}

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

fn key_event(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
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

fn chip(v: &ConsoleView, id: u32) -> Point {
    v.snapshot_center(id)
        .unwrap_or_else(|| panic!("no chip for snapshot {id}"))
}

fn button(v: &ConsoleView, b: ConsoleButton) -> Point {
    v.button_center(b)
        .unwrap_or_else(|| panic!("no {b:?} button"))
}

fn texts(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.texts().into_iter().map(str::to_owned).collect()
}

fn pick(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, label: &str) -> Vec<UiAction> {
    let i = v
        .menu_items()
        .iter()
        .position(|t| t == label)
        .unwrap_or_else(|| panic!("no {label:?} in {:?}", v.menu_items()));
    let p = v.menu_row_center(i).unwrap();
    click(v, doc, reg, p)
}

// ---- The bar --------------------------------------------------------------------------------

#[test]
fn the_bar_sits_under_the_toolbar_and_pushes_the_strips_down() {
    let (d, reg) = project(0);
    let v = wide(d.doc(), &reg);
    let top = v.strip_rect(&key("ch-bass")).unwrap().y0;
    assert!(top >= TOOLBAR_H + SNAPSHOT_BAR_H - 1e-9, "{top}");
    let store = button(&v, ConsoleButton::StoreSnapshot);
    assert!(store.y > TOOLBAR_H && store.y < TOOLBAR_H + SNAPSHOT_BAR_H);
    assert!(v.snapshot_ids().is_empty());
    assert!(v.button_center(ConsoleButton::EndCompare).is_none());
}

#[test]
fn the_plus_button_stores_the_mix_under_the_next_default_name() {
    let (d, reg) = project(0);
    let mut v = wide(d.doc(), &reg);
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::StoreSnapshot);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::StoreSnapshot { name: "A".into() }]
    );
    let (d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::StoreSnapshot);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::StoreSnapshot { name: "1".into() }]
    );
}

#[test]
fn chips_follow_the_list_and_a_click_recalls_the_snapshot() {
    let (d, reg) = project(3);
    let mut v = wide(d.doc(), &reg);
    assert_eq!(v.snapshot_ids(), [1, 2, 3]);
    let xs: Vec<f64> = [1, 2, 3].map(|i| chip(&v, i).x).to_vec();
    assert!(xs[0] < xs[1] && xs[1] < xs[2]);
    assert!(xs[2] < button(&v, ConsoleButton::StoreSnapshot).x);
    // Snapshot 3 matches the mix; 1 and 2 do not.
    assert_eq!(v.current_snapshot(), Some(3));
    let a = click_chip(&mut v, d.doc(), &reg, 1);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RecallSnapshot {
            id: 1,
            scope: SnapshotScope::ALL
        }]
    );
    // Clicking the one the mix already is sends nothing.
    assert!(click_chip(&mut v, d.doc(), &reg, 3).is_empty());
    assert!(texts(&mut v, d.doc(), &reg).iter().any(|t| t == "A"));
}

#[test]
fn a_click_on_the_bar_selects_no_strip() {
    let (d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    click_chip(&mut v, d.doc(), &reg, 1);
    click(
        &mut v,
        d.doc(),
        &reg,
        Point::new(900.0, TOOLBAR_H + SNAPSHOT_BAR_H / 2.0),
    );
    assert!(v.selected_strips().is_empty());
}

#[test]
fn the_chip_stays_lit_with_a_dot_once_the_mix_has_moved() {
    let (mut d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    v.set_snapshot_hint(Some(1));
    // Snapshot 2 matches the mix, so it is the current one whatever the hint says.
    assert_eq!(v.current_snapshot(), Some(2));
    assert!(!v.snapshot_modified(2));
    d.apply(set("ch-bass", "fader", 0.91), &reg).unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(
        v.current_snapshot(),
        Some(1),
        "the hint is the last recalled"
    );
    assert!(v.snapshot_modified(1));
    assert!(!v.snapshot_modified(2));
    // A deleted hint is dropped.
    v.set_snapshot_hint(Some(9));
    assert_eq!(v.current_snapshot(), None);
}

// ---- Compare --------------------------------------------------------------------------------

#[test]
fn the_ab_button_compares_with_the_current_snapshot_and_flips() {
    let (mut d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    d.apply(set("ch-bass", "fader", 0.91), &reg).unwrap();
    v.set_snapshot_hint(Some(1));
    v.sync(d.doc(), &reg);
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::Compare);
    assert_eq!(console_cmds(&a), [ConsoleCommand::Compare { id: 1 }]);
    assert!(v.snapshot_status().is_none());
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &reg)
        .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.compare_state(), Some((1, true)));
    assert_eq!(v.snapshot_status().as_deref(), Some("B: A"));
    // While comparing it flips the compared snapshot, whatever else is current.
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::Compare);
    assert_eq!(console_cmds(&a), [ConsoleCommand::Compare { id: 1 }]);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &reg)
        .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.compare_state(), Some((1, false)));
    assert_eq!(v.snapshot_status().as_deref(), Some("A: working mix"));
    // END keeps the live mix.
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::EndCompare);
    assert_eq!(console_cmds(&a), [ConsoleCommand::EndCompare]);
    d.apply(con(ConsoleCommand::EndCompare), &reg).unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.compare_state(), None);
    assert!(v.button_center(ConsoleButton::EndCompare).is_none());
}

#[test]
fn without_a_current_snapshot_the_ab_button_takes_the_first() {
    let (mut d, reg) = project(2);
    d.apply(set("ch-bass", "fader", 0.91), &reg).unwrap();
    let mut v = wide(d.doc(), &reg);
    let a = click_button(&mut v, d.doc(), &reg, ConsoleButton::Compare);
    assert_eq!(console_cmds(&a), [ConsoleCommand::Compare { id: 1 }]);
    // No snapshot at all: nothing to compare with.
    let (d, reg) = project(0);
    let mut v = wide(d.doc(), &reg);
    assert!(click_button(&mut v, d.doc(), &reg, ConsoleButton::Compare).is_empty());
}

#[test]
fn a_click_on_a_chip_while_comparing_recalls_even_when_it_matches() {
    let (mut d, reg) = project(2);
    d.apply(con(ConsoleCommand::Compare { id: 2 }), &reg)
        .unwrap();
    let mut v = wide(d.doc(), &reg);
    let a = click_chip(&mut v, d.doc(), &reg, 2);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RecallSnapshot {
            id: 2,
            scope: SnapshotScope::ALL
        }]
    );
}

// ---- Menu and rename ------------------------------------------------------------------------

#[test]
fn the_chip_menu_lists_the_operations_and_each_sends_its_command() {
    let (mut d, reg) = project(3);
    d.apply(set("ch-bass", "fader", 0.91), &reg).unwrap();
    let mut v = wide(d.doc(), &reg);
    right_click_chip(&mut v, d.doc(), &reg, 2);
    assert_eq!(
        v.menu_items(),
        [
            "Recall",
            "Recall levels only",
            "Recall mutes and solos only (no change)",
            "Recall sends only (no change)",
            "Recall insert bypass only (no change)",
            "Compare with this",
            "Update from current mix",
            "Rename",
            "Move left",
            "Move right",
            "Delete snapshot",
        ]
    );
    let cases: [(&str, ConsoleCommand); 7] = [
        (
            "Recall",
            ConsoleCommand::RecallSnapshot {
                id: 2,
                scope: SnapshotScope::ALL,
            },
        ),
        (
            "Recall levels only",
            ConsoleCommand::RecallSnapshot {
                id: 2,
                scope: SnapshotScope::LEVELS,
            },
        ),
        ("Compare with this", ConsoleCommand::Compare { id: 2 }),
        (
            "Update from current mix",
            ConsoleCommand::UpdateSnapshot { id: 2 },
        ),
        ("Move left", ConsoleCommand::MoveSnapshot { id: 2, to: 0 }),
        ("Move right", ConsoleCommand::MoveSnapshot { id: 2, to: 2 }),
        ("Delete snapshot", ConsoleCommand::DeleteSnapshot { id: 2 }),
    ];
    for (label, want) in cases {
        right_click_chip(&mut v, d.doc(), &reg, 2);
        let a = pick(&mut v, d.doc(), &reg, label);
        assert_eq!(console_cmds(&a), [want], "{label}");
    }
    // A disabled row sends nothing.
    right_click_chip(&mut v, d.doc(), &reg, 2);
    let a = pick(&mut v, d.doc(), &reg, "Recall sends only (no change)");
    assert!(a.is_empty());
    // The ends of the list cannot move further.
    right_click_chip(&mut v, d.doc(), &reg, 1);
    assert!(!v.menu_items().iter().any(|t| t == "Move left"));
    v.handle(&key_event(Key::Escape), d.doc(), &reg);
    right_click_chip(&mut v, d.doc(), &reg, 3);
    assert!(!v.menu_items().iter().any(|t| t == "Move right"));
}

#[test]
fn a_double_click_renames_in_place_and_enter_sends_rename_snapshot() {
    let (d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    let a = double_click_chip(&mut v, d.doc(), &reg, 1);
    assert!(a.is_empty(), "{a:?}");
    assert!(v.is_text_owner());
    assert_eq!(v.rename_text(), Some("A"));
    v.handle(&key_event(Key::Backspace), d.doc(), &reg);
    for c in "Verse".chars() {
        v.handle(&key_event(Key::Char(c)), d.doc(), &reg);
    }
    assert!(texts(&mut v, d.doc(), &reg).iter().any(|s| s == "Verse"));
    let a = v.handle(&key_event(Key::Enter), d.doc(), &reg);
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RenameSnapshot {
            id: 1,
            name: "Verse".into()
        }]
    );
    assert!(!v.is_text_owner());
    // Escape, an empty name and an unchanged name send nothing.
    double_click_chip(&mut v, d.doc(), &reg, 2);
    v.handle(&key_event(Key::Char('x')), d.doc(), &reg);
    assert!(v.handle(&key_event(Key::Escape), d.doc(), &reg).is_empty());
    double_click_chip(&mut v, d.doc(), &reg, 2);
    v.handle(&key_event(Key::Backspace), d.doc(), &reg);
    assert!(v.handle(&key_event(Key::Enter), d.doc(), &reg).is_empty());
    double_click_chip(&mut v, d.doc(), &reg, 2);
    assert!(v.handle(&key_event(Key::Enter), d.doc(), &reg).is_empty());
    // The menu item edits too, and a click elsewhere commits.
    right_click_chip(&mut v, d.doc(), &reg, 2);
    pick(&mut v, d.doc(), &reg, "Rename");
    assert!(v.is_text_owner());
    v.handle(&key_event(Key::Char('!')), d.doc(), &reg);
    let a = click(&mut v, d.doc(), &reg, Point::new(900.0, 100.0));
    assert_eq!(
        console_cmds(&a),
        [ConsoleCommand::RenameSnapshot {
            id: 2,
            name: "B!".into()
        }]
    );
}

#[test]
fn the_name_edit_closes_when_its_snapshot_is_deleted() {
    let (mut d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    double_click_chip(&mut v, d.doc(), &reg, 2);
    assert!(v.is_text_owner());
    d.apply(con(ConsoleCommand::DeleteSnapshot { id: 2 }), &reg)
        .unwrap();
    v.sync(d.doc(), &reg);
    assert!(!v.is_text_owner());
}

// ---- The status and the drift ----------------------------------------------------------------

#[test]
fn the_status_says_what_a_recall_would_skip() {
    let (mut d, reg) = project(2);
    d.apply(
        con(ConsoleCommand::RemoveChannel {
            channel: key("ch-keys"),
        }),
        &reg,
    )
    .unwrap();
    let mut v = wide(d.doc(), &reg);
    v.set_snapshot_hint(Some(1));
    v.sync(d.doc(), &reg);
    let status = v.snapshot_status().expect("a note");
    assert!(
        status.starts_with("B: ") && status.contains("ch-keys"),
        "{status}"
    );
    assert!(
        texts(&mut v, d.doc(), &reg)
            .iter()
            .any(|t| t.contains("ch-keys"))
    );
}

// ---- Layout -----------------------------------------------------------------------------------

#[test]
fn a_narrow_console_keeps_the_buttons_and_drops_the_chips_that_do_not_fit() {
    let (d, reg) = project(0);
    let mut d = d;
    for n in 0..16 {
        d.apply(set("ch-bass", "fader", 0.1 + 0.05 * n as f32), &reg)
            .unwrap();
        d.apply(
            con(ConsoleCommand::StoreSnapshot {
                name: format!("Snapshot number {n}"),
            }),
            &reg,
        )
        .unwrap();
    }
    let mut v = console_view(d.doc(), &reg);
    v.set_bounds(Rect::new(0.0, 0.0, 640.0, 784.0));
    v.sync(d.doc(), &reg);
    let shown = v.snapshot_ids().len();
    assert_eq!(shown, 16);
    let fits = (1..=16).filter(|i| v.snapshot_center(*i).is_some()).count();
    assert!(fits > 0 && fits < 16, "{fits}");
    let store = button(&v, ConsoleButton::StoreSnapshot);
    assert!(store.x < 640.0);
    assert!(button(&v, ConsoleButton::Compare).x < 640.0);
}

// ---- Golden -----------------------------------------------------------------------------------

fn check(name: &str, actual: &str) {
    const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn bar_json(doc: &RackDoc, reg: &Reg, hint: Option<u32>) -> String {
    let mut v = wide(doc, reg);
    v.set_snapshot_hint(hint);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

#[test]
fn the_snapshot_bar_matches_its_goldens() {
    let (mut d, reg) = project(3);
    // Plain: the mix matches snapshot C.
    check("console_snapshots_1x.json", &bar_json(d.doc(), &reg, None));
    // Moved since A was recalled: the dot.
    d.apply(set("ch-bass", "fader", 0.97), &reg).unwrap();
    check(
        "console_snapshots_modified_1x.json",
        &bar_json(d.doc(), &reg, Some(1)),
    );
    // Comparing, the snapshot side live.
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &reg)
        .unwrap();
    check(
        "console_snapshots_compare_1x.json",
        &bar_json(d.doc(), &reg, None),
    );
}

#[test]
fn the_model_ops_the_bar_sends_are_what_the_shell_applies() {
    // The commands the bar emits are accepted by the model (a recall of a differing snapshot).
    let (mut d, reg) = project(2);
    let mut v = wide(d.doc(), &reg);
    let a = click_chip(&mut v, d.doc(), &reg, 1);
    let ops: Vec<SyncOp> = commands(&a)
        .into_iter()
        .flat_map(|c| d.apply(c, &reg).unwrap())
        .collect();
    assert!(
        ops.iter()
            .any(|o| matches!(o, SyncOp::ParamChanged { param, .. } if param == "fader"))
    );
    assert!(d.doc().snapshot_matches(1, &reg, SnapshotScope::ALL));
}

fn click_chip(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, id: u32) -> Vec<UiAction> {
    let p = chip(v, id);
    click(v, doc, reg, p)
}

fn right_click_chip(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, id: u32) -> Vec<UiAction> {
    let p = chip(v, id);
    right_click(v, doc, reg, p)
}

fn double_click_chip(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, id: u32) -> Vec<UiAction> {
    let p = chip(v, id);
    double_click(v, doc, reg, p)
}

fn click_button(v: &mut ConsoleView, doc: &RackDoc, reg: &Reg, b: ConsoleButton) -> Vec<UiAction> {
    let p = button(v, b);
    click(v, doc, reg, p)
}

/// Writes `target/preview/console_snapshots_preview.png`: the bar with three snapshots while
/// comparing. Run: `cargo test -p atomik-ui --test console_mixer_snapshots -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn console_snapshots_preview_png() {
    use atomik_ui::TextEngine;
    use support::cpu::CpuCanvas;
    let (mut d, reg) = project(3);
    d.apply(con(ConsoleCommand::Compare { id: 1 }), &reg)
        .unwrap();
    let scale = 1.5;
    let bounds = Rect::new(0.0, 0.0, 1100.0, 784.0);
    let mut v = console_view(d.doc(), &reg);
    v.set_bounds(bounds);
    v.set_scale(scale);
    v.set_focused(true);
    let mut text = TextEngine::new();
    let (w, h) = (
        (bounds.width() * scale) as u16,
        (bounds.height() * scale) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join("console_snapshots_preview.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}
