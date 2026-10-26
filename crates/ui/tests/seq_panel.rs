//! The step sequencer's rack panel: layout, display-list goldens of
//! both faces, the pattern slots (select, play, rename, add, duplicate, remove, copy, paste,
//! clear), the Edit button, the live step and pattern markers from the published meter values,
//! and the "Players" grouping of the browser.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test seq_panel`.

mod support;

use atomik_model::{Command, Document, PatternDoc, RackDoc, StepDoc};
use atomik_ui::autolayout::{CONTENT_X, PanelSpec};
use atomik_ui::kurbo::Point;
use atomik_ui::seq_panel::{SeqButton, seq_layout};
use atomik_ui::{DisplayList, Flip, Key, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::seq::*;
use support::song::press;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
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
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn setup() -> (Document, RackView) {
    let doc = seq_only();
    let reg = registry();
    let v = view(&doc, &reg);
    (Document::new(doc), v)
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

macro_rules! click {
    ($v:ident, $d:expr, $p:expr) => {{
        let p = $p;
        click_fn(&mut $v, $d, p)
    }};
}

macro_rules! click_with {
    ($v:ident, $d:expr, $p:expr, $n:expr, $m:expr) => {{
        let p = $p;
        click_with_fn(&mut $v, $d, p, $n, $m)
    }};
}

fn click_with_fn(
    v: &mut RackView,
    d: &Document,
    p: Point,
    clicks: u8,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
            clicks,
        },
        d.doc(),
        &reg,
    );
    out.extend(v.handle(&up(p), d.doc(), &reg));
    out
}

fn click_fn(v: &mut RackView, d: &Document, p: Point) -> Vec<UiAction> {
    click_with_fn(v, d, p, 1, Modifiers::NONE)
}

fn btn(v: &RackView, b: SeqButton) -> Point {
    v.seq_button_center(&key("seq"), b)
        .unwrap_or_else(|| panic!("no button {b:?}"))
}

fn type_text(v: &mut RackView, d: &Document, s: &str) {
    let reg = registry();
    for ch in s.chars() {
        let _ = v.handle(&press(Key::Char(ch), Modifiers::NONE), d.doc(), &reg);
    }
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup();
    v.set_edited_pattern(&key("seq"), 1);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    check("seq_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_with_a_running_step_and_a_queued_pattern_matches_golden() {
    let mut doc = seq_only();
    doc.devices[0].params.insert("pattern".into(), 2.0);
    let reg = registry();
    let mut v = view(&doc, &reg);
    // Playing pattern 1 (0-based 0), step 4, running; the param asks for pattern 2.
    v.set_device_meter_values(&key("seq"), &[0.0, 4.0, 1.0]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("seq_panel_playing_1x.json", &dl.to_json());
}

#[test]
fn the_front_panel_while_renaming_matches_golden() {
    let (doc, mut v) = setup();
    let p = btn(&v, SeqButton::Slot(1));
    let _ = click_with!(v, &doc, p, 2, Modifiers::NONE);
    type_text(&mut v, &doc, "Ab");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    check("seq_panel_rename_1x.json", &dl.to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let (doc, mut v) = setup();
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc.doc(), &registry());
    check("seq_panel_back_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_places_every_param_once_and_every_extra_inside_the_content_area() {
    let reg = registry();
    let desc = reg.get(SEQ_TYPE).unwrap().descriptor;
    let spec: PanelSpec = seq_layout(desc);
    assert_eq!(spec.height_units, 2);
    let mut params: Vec<usize> = spec.front.iter().map(|c| c.param).collect();
    params.sort_unstable();
    // The `bypass` param (last) belongs to the name strip's switch.
    assert_eq!(params, (0..desc.params.len() - 1).collect::<Vec<_>>());
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    let panel = spec.seq.expect("seq panel");
    let content = atomik_ui::kurbo::Rect::new(CONTENT_X.0, 0.0, CONTENT_X.1, spec.height());
    let mut rects: Vec<atomik_ui::kurbo::Rect> = panel.buttons().iter().map(|(_, r)| *r).collect();
    rects.push(panel.preview);
    rects.push(panel.name);
    rects.extend(spec.front.iter().map(|c| c.rect));
    for (i, a) in rects.iter().enumerate() {
        assert!(
            a.x0 >= content.x0 && a.x1 <= content.x1 && a.y0 >= 0.0 && a.y1 <= content.y1,
            "{a:?} outside {content:?}"
        );
        for b in &rects[i + 1..] {
            let overlap = a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;
            assert!(!overlap, "{a:?} overlaps {b:?}");
        }
    }
    assert!(
        spec.drum.is_none() && spec.slicer.is_none() && spec.fx.is_none() && spec.arp.is_none()
    );
}

#[test]
fn the_panel_shows_the_slots_the_name_the_knobs_and_the_buttons() {
    let (doc, mut v) = setup();
    let t = texts_of(&mut v, doc.doc());
    for s in [
        "PATTERNS",
        "PLAYER",
        "1",
        "2",
        "3",
        "8",
        "ADD",
        "DUP",
        "DEL",
        "COPY",
        "PASTE",
        "CLEAR",
        "EDIT",
        "Pattern",
        "Transpose",
        "Mute",
    ] {
        assert!(t.iter().any(|x| x == s), "missing {s:?}: {t:?}");
    }
    assert!(
        t.iter().any(|x| x.contains("Bass") && x.contains("16")),
        "the edited pattern's name and length: {t:?}"
    );
    let r = v.device_rect(&key("seq")).unwrap();
    assert!((r.height() - 2.0 * 76.0).abs() < 1e-9, "{r:?}");
}

// ------------------------------------------------------------------ slots

#[test]
fn a_click_on_a_slot_selects_the_edited_pattern_without_a_command() {
    let (doc, mut v) = setup();
    assert_eq!(v.edited_pattern(&key("seq")), Some(0));
    let before = v.panel_revisions();
    let a = click!(v, &doc, btn(&v, SeqButton::Slot(2)));
    assert_eq!(v.edited_pattern(&key("seq")), Some(2));
    assert!(commands(&a).is_empty(), "{a:?}");
    assert!(
        a.contains(&UiAction::SelectPattern {
            device: key("seq"),
            index: 2
        }),
        "{a:?}"
    );
    assert_ne!(v.panel_revisions(), before, "the panel is re-recorded");
    // The same slot again is no change and no action.
    let a = click!(v, &doc, btn(&v, SeqButton::Slot(2)));
    assert!(
        !a.iter()
            .any(|x| matches!(x, UiAction::SelectPattern { .. })),
        "{a:?}"
    );
    // The panel shows the edited pattern's name.
    let t = texts_of(&mut v, doc.doc());
    assert!(t.iter().any(|x| x.contains("Pattern 3")), "{t:?}");
}

#[test]
fn a_click_on_a_slot_without_a_pattern_does_nothing() {
    let (doc, mut v) = setup();
    let a = click!(v, &doc, btn(&v, SeqButton::Slot(6)));
    assert_eq!(v.edited_pattern(&key("seq")), Some(0));
    assert!(
        !a.iter()
            .any(|x| matches!(x, UiAction::SelectPattern { .. })),
        "{a:?}"
    );
    assert!(commands(&a).is_empty());
}

#[test]
fn a_ctrl_click_on_a_slot_also_plays_it_through_the_pattern_param() {
    let (doc, mut v) = setup();
    let p = btn(&v, SeqButton::Slot(1));
    let a = click_with_fn(
        &mut v,
        &doc,
        p,
        1,
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: key("seq"),
            param: "pattern".into(),
            value: 2.0,
            merge: None
        }]
    );
    assert_eq!(v.edited_pattern(&key("seq")), Some(1));
}

#[test]
fn the_playing_and_the_queued_pattern_come_from_the_param_and_the_meters() {
    let mut doc = seq_only();
    let reg = registry();
    let mut v = view(&doc, &reg);
    let d = key("seq");
    // No meters yet: the param is the playing pattern, nothing is queued.
    assert_eq!(v.playing_pattern(&d), Some(0));
    assert_eq!(v.queued_pattern(&d), None);
    doc.devices[0].params.insert("pattern".into(), 3.0);
    v.sync(&doc, &reg);
    assert_eq!(v.playing_pattern(&d), Some(2));
    // The device still plays pattern 1 until the end of the pattern: 3 is queued.
    v.set_device_meter_values(&d, &[0.0, 7.0, 1.0]);
    assert_eq!(v.playing_pattern(&d), Some(0));
    assert_eq!(v.queued_pattern(&d), Some(2));
    assert_eq!(v.step_cursor(&d), Some(7));
    // Stopped: no cursor.
    v.set_device_meter_values(&d, &[0.0, 7.0, 0.0]);
    assert_eq!(v.step_cursor(&d), None);
    // Arrived.
    v.set_device_meter_values(&d, &[2.0, 0.0, 1.0]);
    assert_eq!(v.playing_pattern(&d), Some(2));
    assert_eq!(v.queued_pattern(&d), None);
}

#[test]
fn a_meter_feed_re_records_the_panel_only_when_a_displayed_value_changed() {
    let (doc, mut v) = setup();
    let d = key("seq");
    let _ = texts_of(&mut v, doc.doc());
    assert!(!v.needs_repaint());
    v.set_device_meter_values(&d, &[0.0, 3.0, 1.0]);
    assert!(v.needs_repaint());
    let _ = texts_of(&mut v, doc.doc());
    let revs = v.panel_revisions();
    v.set_device_meter_values(&d, &[0.0, 3.0, 1.0]);
    assert_eq!(v.panel_revisions(), revs, "same values, no re-record");
    assert!(!v.needs_repaint());
    v.set_device_meter_values(&d, &[0.0, 4.0, 1.0]);
    assert_ne!(v.panel_revisions(), revs);
    // Other devices and short slices are ignored.
    v.set_device_meter_values(&key("nope"), &[1.0, 2.0, 3.0]);
    v.set_device_meter_values(&d, &[]);
}

// ------------------------------------------------------------------ pattern commands

#[test]
fn add_duplicate_and_remove_emit_the_pattern_commands_and_follow_the_selection() {
    let reg = registry();
    let (mut document, mut v) = setup();
    let d = key("seq");
    let a = click!(v, &document, btn(&v, SeqButton::Add));
    assert_eq!(
        commands(&a),
        vec![Command::AddPattern { device: d.clone() }]
    );
    assert!(
        a.contains(&UiAction::SelectPattern {
            device: d.clone(),
            index: 3
        }),
        "the new pattern becomes the edited one: {a:?}"
    );
    let (ops, _) = apply_all(&mut document, &reg, &a);
    assert!(
        ops.iter()
            .any(|o| matches!(o, atomik_model::SyncOp::PatternsChanged { .. }))
    );
    v.sync(document.doc(), &reg);
    assert_eq!(pattern_count(document.doc()), 4);
    assert_eq!(v.edited_pattern(&d), Some(3));

    let a = click!(v, &document, btn(&v, SeqButton::Duplicate));
    assert_eq!(
        commands(&a),
        vec![Command::DuplicatePattern {
            device: d.clone(),
            index: 3
        }]
    );
    let _ = apply_all(&mut document, &reg, &a);
    v.sync(document.doc(), &reg);
    assert_eq!(pattern_count(document.doc()), 5);
    assert_eq!(v.edited_pattern(&d), Some(4));

    let a = click!(v, &document, btn(&v, SeqButton::Remove));
    assert_eq!(
        commands(&a),
        vec![Command::RemovePattern {
            device: d.clone(),
            index: 4
        }]
    );
    let _ = apply_all(&mut document, &reg, &a);
    v.sync(document.doc(), &reg);
    assert_eq!(pattern_count(document.doc()), 4);
    assert_eq!(v.edited_pattern(&d), Some(3), "clamped to the last pattern");
}

#[test]
fn add_and_duplicate_are_dim_at_eight_patterns_and_remove_at_one() {
    let reg = registry();
    let mut doc = seq_only();
    let steps = doc.devices[0].steps.as_mut().unwrap();
    while steps.patterns.len() < 8 {
        steps.patterns.push(PatternDoc::new("x"));
    }
    let mut v = view(&doc, &reg);
    let document = Document::new(doc);
    for b in [SeqButton::Add, SeqButton::Duplicate] {
        let a = click!(v, &document, btn(&v, b));
        assert!(commands(&a).is_empty(), "{b:?}: {a:?}");
    }
    let mut doc = seq_only();
    doc.devices[0].steps.as_mut().unwrap().patterns.truncate(1);
    let mut v = view(&doc, &reg);
    let document = Document::new(doc);
    let a = click!(v, &document, btn(&v, SeqButton::Remove));
    assert!(commands(&a).is_empty(), "the last pattern stays: {a:?}");
}

#[test]
fn copy_paste_and_clear_edit_the_selected_pattern_in_one_command_each() {
    let reg = registry();
    let (mut document, mut v) = setup();
    let d = key("seq");
    // Paste with an empty clipboard does nothing.
    assert!(v.pattern_clipboard().is_none());
    let a = click!(v, &document, btn(&v, SeqButton::Paste));
    assert!(commands(&a).is_empty());
    // Copy pattern 1 ("Bass"), select pattern 3, paste.
    let a = click!(v, &document, btn(&v, SeqButton::Copy));
    assert!(commands(&a).is_empty(), "copy edits nothing");
    assert_eq!(v.pattern_clipboard(), Some(&bass()));
    let _ = click!(v, &document, btn(&v, SeqButton::Slot(2)));
    let a = click!(v, &document, btn(&v, SeqButton::Paste));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    let (index, pasted, merge) = &sets[0];
    assert_eq!(*index, 2);
    assert_eq!(*merge, None);
    let mut expect = bass();
    expect.name = "Pattern 3".into();
    assert_eq!(
        pasted, &expect,
        "the steps come over, the slot keeps its name"
    );
    let _ = apply_all(&mut document, &reg, &a);
    v.sync(document.doc(), &reg);
    assert_eq!(pattern_of(document.doc(), 2), expect);
    // Clear: steps default, the rest stays.
    let a = click!(v, &document, btn(&v, SeqButton::Clear));
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1);
    let mut cleared = expect.clone();
    cleared.steps = vec![StepDoc::default(); 64];
    assert_eq!(sets[0].1, cleared);
    let _ = apply_all(&mut document, &reg, &a);
    // Undo restores the pasted pattern in one step.
    document.undo(&reg);
    assert_eq!(pattern_of(document.doc(), 2), expect);
    let _ = d;
}

#[test]
fn the_edit_button_and_a_double_click_on_the_preview_open_the_grid_editor() {
    let (doc, mut v) = setup();
    let open = UiAction::OpenSeqEditor { device: key("seq") };
    let a = click!(v, &doc, btn(&v, SeqButton::Edit));
    assert!(a.contains(&open), "{a:?}");
    assert!(commands(&a).is_empty());
    let a = click!(v, &doc, btn(&v, SeqButton::Preview));
    assert!(!a.contains(&open), "single click: {a:?}");
    let a = click_with!(v, &doc, btn(&v, SeqButton::Preview), 2, Modifiers::NONE);
    assert!(a.contains(&open), "{a:?}");
    v.set_flip(Flip::Back);
    assert!(
        v.seq_button_center(&key("seq"), SeqButton::Edit).is_none(),
        "front only"
    );
}

// ------------------------------------------------------------------ rename

#[test]
fn a_double_click_on_a_slot_renames_its_pattern_inline() {
    let reg = registry();
    let (mut document, mut v) = setup();
    let d = key("seq");
    let a = click_with!(
        v,
        &document,
        btn(&v, SeqButton::Slot(1)),
        2,
        Modifiers::NONE
    );
    assert_eq!(v.renaming(&d), Some("Fill"));
    assert_eq!(v.edited_pattern(&d), Some(1));
    assert!(commands(&a).is_empty());
    // Backspace x4, then the new name; nothing is written until Enter.
    let _ = reg;
    for _ in 0..4 {
        let _ = v.handle(
            &press(Key::Backspace, Modifiers::NONE),
            document.doc(),
            &registry(),
        );
    }
    type_text(&mut v, &document, "Ride");
    let _ = v.handle(
        &press(Key::Space, Modifiers::NONE),
        document.doc(),
        &registry(),
    );
    type_text(&mut v, &document, "1");
    assert_eq!(v.renaming(&d), Some("Ride 1"));
    let a = v.handle(
        &press(Key::Enter, Modifiers::NONE),
        document.doc(),
        &registry(),
    );
    let sets = set_patterns(&a);
    assert_eq!(sets.len(), 1, "{a:?}");
    let mut expect = fill();
    expect.name = "Ride 1".into();
    assert_eq!((sets[0].0, &sets[0].1, sets[0].2), (1, &expect, None));
    assert_eq!(v.renaming(&d), None);
    let reg = registry();
    let _ = apply_all(&mut document, &reg, &a);
    assert_eq!(pattern_of(document.doc(), 1).name, "Ride 1");
}

#[test]
fn escape_cancels_a_rename_and_an_unchanged_or_empty_name_writes_nothing() {
    let (doc, mut v) = setup();
    let d = key("seq");
    let reg = registry();
    let _ = click_with!(v, &doc, btn(&v, SeqButton::Slot(0)), 2, Modifiers::NONE);
    type_text(&mut v, &doc, "zzz");
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), doc.doc(), &reg);
    assert!(commands(&a).is_empty());
    assert_eq!(v.renaming(&d), None);
    // Enter with the name unchanged.
    let _ = click_with!(v, &doc, btn(&v, SeqButton::Slot(0)), 2, Modifiers::NONE);
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), doc.doc(), &reg);
    assert!(commands(&a).is_empty(), "{a:?}");
    // Enter with an empty name.
    let _ = click_with!(v, &doc, btn(&v, SeqButton::Slot(0)), 2, Modifiers::NONE);
    for _ in 0..8 {
        let _ = v.handle(&press(Key::Backspace, Modifiers::NONE), doc.doc(), &reg);
    }
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), doc.doc(), &reg);
    assert!(commands(&a).is_empty(), "{a:?}");
    // Clicking elsewhere ends the rename without a command.
    let _ = click_with!(v, &doc, btn(&v, SeqButton::Slot(0)), 2, Modifiers::NONE);
    type_text(&mut v, &doc, "q");
    let _ = click!(v, &doc, btn(&v, SeqButton::Slot(2)));
    assert_eq!(v.renaming(&d), None);
}

#[test]
fn a_rename_is_limited_to_64_characters_and_the_model_accepts_it() {
    let reg = registry();
    let (mut document, mut v) = setup();
    let _ = click_with!(
        v,
        &document,
        btn(&v, SeqButton::Slot(0)),
        2,
        Modifiers::NONE
    );
    for _ in 0..8 {
        let _ = v.handle(
            &press(Key::Backspace, Modifiers::NONE),
            document.doc(),
            &reg,
        );
    }
    type_text(&mut v, &document, &"x".repeat(80));
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), document.doc(), &reg);
    let sets = set_patterns(&a);
    assert_eq!(sets[0].1.name.chars().count(), 64);
    let _ = apply_all(&mut document, &reg, &a);
}

// ------------------------------------------------------------------ knobs, selection, undo

#[test]
fn the_global_knobs_are_the_device_params() {
    let (doc, v) = setup();
    let d = key("seq");
    for p in [
        "pattern",
        "pattern.switch",
        "transpose",
        "follow",
        "probability.amount",
        "random.seed",
        "mute",
        "velocity.scale",
        "gate.scale",
    ] {
        assert!(v.control_center(&d, p).is_some(), "{p}");
    }
    let _ = doc;
}

#[test]
fn a_toggle_and_a_knob_drag_emit_set_param() {
    let (doc, mut v) = setup();
    let reg = registry();
    let d = key("seq");
    let p = v.control_center(&d, "mute").unwrap();
    let a = click!(v, &doc, p);
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: d.clone(),
            param: "mute".into(),
            value: 1.0,
            merge: None
        }]
    );
    let p = v.control_center(&d, "transpose").unwrap();
    let mut cmds = Vec::new();
    for e in [
        down(p),
        mv(p + atomik_ui::kurbo::Vec2::new(0.0, -20.0)),
        up(p),
    ] {
        cmds.extend(commands(&v.handle(&e, doc.doc(), &reg)));
    }
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::SetParam { param, .. } if param == "transpose")),
        "{cmds:?}"
    );
}

#[test]
fn the_edited_pattern_is_view_state_and_survives_a_resync() {
    let reg = registry();
    let (mut document, mut v) = setup();
    let d = key("seq");
    v.set_edited_pattern(&d, 2);
    assert_eq!(v.edited_pattern(&d), Some(2));
    // Another edit of the document leaves it alone.
    let _ = document.apply(
        Command::SetParam {
            key: d.clone(),
            param: "transpose".into(),
            value: 5.0,
            merge: None,
        },
        &reg,
    );
    v.sync(document.doc(), &reg);
    assert_eq!(v.edited_pattern(&d), Some(2));
    // Out of range values are clamped; other devices have none.
    v.set_edited_pattern(&d, 99);
    assert_eq!(v.edited_pattern(&d), Some(2));
    assert_eq!(v.edited_pattern(&key("nope")), None);
    // Removing the pattern that was edited clamps it.
    let _ = document.apply(
        Command::RemovePattern {
            device: d.clone(),
            index: 2,
        },
        &reg,
    );
    v.sync(document.doc(), &reg);
    assert_eq!(v.edited_pattern(&d), Some(1));
}

// ------------------------------------------------------------------ browser

#[test]
fn the_browser_groups_the_players_after_the_other_devices() {
    let reg = registry();
    let doc = reference_doc();
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    let everything = v.browser_rows(&reg);
    assert_eq!(
        everything.last(),
        Some(&"atomik.container"),
        "containers last"
    );
    let rows: Vec<&str> = everything
        .into_iter()
        .filter(|t| *t != "atomik.container")
        .collect();
    let first_player = rows
        .iter()
        .position(|t| atomik_devices::is_player(t))
        .expect("players listed");
    assert!(
        rows[first_player..]
            .iter()
            .all(|t| atomik_devices::is_player(t)),
        "players are contiguous before the containers: {rows:?}"
    );
    assert_eq!(
        rows.len() - first_player,
        atomik_devices::PLAYER_TYPE_IDS.len()
    );
    assert!(rows[..first_player].contains(&"atomik.poly-synth"));
    // The group has a label and a search for it finds both.
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"PLAYER"), "{:?}", dl.texts());
}
