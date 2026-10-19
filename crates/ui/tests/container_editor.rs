//! The container editor: the SPLITS tab (bar edges and body, velocity
//! edges, the keyboard strip, the list, ADD, retarget, transpose, delete), the MACROS tab (rows,
//! rename, knob, target fields, delete), merged drags, Esc, one command per gesture, display-list
//! goldens `container_editor_splits_*` and `container_editor_macros_*`, and a property test that
//! no gesture sequence makes the model refuse a command.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test container_editor`.

mod support;

use atomik_model::{Command, ContainerCommand, Document, NoteSplit, RackDoc};
use atomik_ui::container_edit::{Bound, Edge};
use atomik_ui::container_editor::{Hit, STRIP_VELOCITY};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    ContainerEditorView, ContainerTab, DisplayList, Key, Modifiers, PointerButton, Theme, UiAction,
    UiEvent,
};
use proptest::prelude::*;
use support::container::*;
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);

fn editor(doc: &RackDoc) -> ContainerEditorView {
    let reg = registry();
    let mut v = ContainerEditorView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(key("layer"));
    v.sync(doc, &reg);
    v
}

fn feed(v: &mut ContainerEditorView, doc: &RackDoc, e: UiEvent) -> Vec<UiAction> {
    v.handle(&e, doc, &registry())
}

fn click_at(v: &mut ContainerEditorView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let mut out = feed(v, doc, down(p));
    out.extend(feed(v, doc, up(p)));
    out
}

fn click_with(
    v: &mut ContainerEditorView,
    doc: &RackDoc,
    p: Point,
    clicks: u8,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let mut out = feed(
        v,
        doc,
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
            clicks,
        },
    );
    out.extend(feed(v, doc, up(p)));
    out
}

fn drag(v: &mut ContainerEditorView, doc: &RackDoc, from: Point, to: Point) -> Vec<UiAction> {
    let mut out = feed(v, doc, down(from));
    out.extend(feed(v, doc, mv(to)));
    out.extend(feed(v, doc, up(to)));
    out
}

fn press(v: &mut ContainerEditorView, doc: &RackDoc, k: Key) -> Vec<UiAction> {
    feed(
        v,
        doc,
        UiEvent::Key {
            key: k,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
    )
}

fn splits_of(doc: &RackDoc) -> Vec<NoteSplit> {
    doc.inner(&key("layer")).unwrap().splits.clone()
}

/// The `SetSplits` lists among `actions`, with their merge ids.
fn set_splits_of(actions: &[UiAction]) -> Vec<(Vec<NoteSplit>, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Container(ContainerCommand::SetSplits {
                container,
                splits,
                merge,
            }) => {
                assert_eq!(container, key("layer"));
                Some((splits, merge))
            }
            _ => None,
        })
        .collect()
}

fn apply_all(d: &mut Document, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), &registry())
                    .unwrap_or_else(|e| panic!("{c:?}: {e}"));
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
}

/// `layered` with the velocity of split 0 narrowed so the map has room, and a third synth
/// `lead2` without a split.
fn roomy() -> Document {
    let mut d = layered();
    add_inner(&mut d, "layer", 4, SYNTH, "lead2");
    let mut s = splits_of(d.doc());
    s[0].vel_lo = 20;
    s[0].vel_hi = 100;
    s[1].vel_lo = 30;
    s[1].vel_hi = 120;
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetSplits {
            container: key("layer"),
            splits: s,
            merge: None,
        }),
    );
    d
}

// ------------------------------------------------------------------ open and geometry

#[test]
fn opens_on_the_splits_tab_with_bars_placed_on_the_key_and_velocity_map() {
    let d = layered();
    let v = editor(d.doc());
    assert!(v.is_present());
    assert_eq!(v.tab(), ContainerTab::Splits);
    assert_eq!(v.container(), Some(&key("layer")));
    let m = v.map_rect();
    let b0 = v.bar_rect(0).unwrap();
    let b1 = v.bar_rect(1).unwrap();
    assert!((b0.x0 - m.x0).abs() < 1e-9 && (b1.x1 - m.x1).abs() < 1e-9);
    assert!(
        (b0.x1 - b1.x0).abs() < 1e-9,
        "keys 0..=59 and 60..=127 meet"
    );
    assert!((b0.width() - m.width() * 60.0 / 128.0).abs() < 1e-6);
    assert!(
        (b0.y0 - m.y0).abs() < 1e-6 && (b0.y1 - m.y1).abs() < 1e-6,
        "full velocity"
    );
    assert!(v.bar_rect(2).is_none());
    // The hit test sees edges and body.
    assert_eq!(
        v.hit(Point::new(b0.x0 + 1.0, b0.center().y)),
        Hit::Bar(0, Edge::Left)
    );
    assert_eq!(
        v.hit(Point::new(b0.x1 - 1.0, b0.center().y)),
        Hit::Bar(0, Edge::Right)
    );
    assert_eq!(
        v.hit(Point::new(b0.center().x, b0.y0 + 1.0)),
        Hit::Bar(0, Edge::Top)
    );
    assert_eq!(
        v.hit(Point::new(b0.center().x, b0.y1 - 1.0)),
        Hit::Bar(0, Edge::Bottom)
    );
    assert_eq!(v.hit(b0.center()), Hit::Bar(0, Edge::Body));
    assert_eq!(v.hit(v.key_center(70)), Hit::Keys(70));
    assert_eq!(v.hit(v.close_rect().center()), Hit::Close);
    assert_eq!(v.hit(Point::new(-5.0, 3.0)), Hit::None);
}

#[test]
fn a_container_that_vanishes_makes_the_editor_not_present() {
    let mut d = layered();
    let reg = registry();
    let mut v = editor(d.doc());
    apply(&mut d, Command::RemoveDevice { key: key("layer") });
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
    // Undo brings it back.
    d.undo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    assert!(v.is_present());
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_splits_tab_matches_golden() {
    let d = roomy();
    let mut v = editor(d.doc());
    let r = v.row_rect(1);
    let _ = click_at(&mut v, d.doc(), Point::new(r.x1 - 20.0, r.center().y));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("container_editor_splits_1x.json", &dl.to_json());
    let texts = dl.texts();
    for t in [
        "CONTAINER EDITOR",
        "SPLITS",
        "MACROS",
        "ADD",
        "TARGET",
        "+12",
        "vel 20-100",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
}

fn macros_doc() -> Document {
    let mut d = layered();
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: target("pad", "filter.cutoff", 8000.0, 200.0),
            merge: None,
        }),
    );
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 0,
            target: target("delay", "mix", 0.0, 0.5),
            merge: None,
        }),
    );
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 2,
            target: target("synth", "filter.resonance", 0.0, 1.0),
            merge: None,
        }),
    );
    apply(
        &mut d,
        Command::SetParam {
            key: key("layer"),
            param: "macro1".into(),
            value: 0.6,
            merge: None,
        },
    );
    d
}

#[test]
fn the_macros_tab_matches_golden() {
    let d = macros_doc();
    let mut v = editor(d.doc());
    v.set_tab(ContainerTab::Macros);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("container_editor_macros_1x.json", &dl.to_json());
    let texts = dl.texts();
    for t in [
        "Cutoff",
        "Macro 2",
        "Macro 8",
        "MACRO 1  ·  Cutoff",
        "MIN",
        "MAX",
        "60 %",
    ] {
        assert!(texts.contains(&t), "missing {t:?}: {texts:?}");
    }
}

#[test]
fn the_empty_macro_says_how_to_map() {
    let d = layered();
    let mut v = editor(d.doc());
    v.set_tab(ContainerTab::Macros);
    v.select_macro(4);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    let texts = dl.texts().join("|");
    assert!(texts.contains("MACRO 5  ·  Macro 5") && texts.contains("Map to macro 5"));
}

#[test]
fn the_empty_split_list_says_so() {
    let mut d = layered();
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetSplits {
            container: key("layer"),
            splits: Vec::new(),
            merge: None,
        }),
    );
    let mut v = editor(d.doc());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    let texts = dl.texts().join("|");
    assert!(texts.contains("No splits: no notes reach the inside"));
}

// ------------------------------------------------------------------ splits: the map

#[test]
fn dragging_the_right_edge_changes_key_hi_in_one_merged_step() {
    let mut d = layered();
    let mut v = editor(d.doc());
    let b = v.bar_rect(0).unwrap();
    let cw = v.map_rect().width() / 128.0;
    let from = Point::new(b.x1 - 1.0, b.center().y);
    let a = drag(
        &mut v,
        d.doc(),
        from,
        Point::new(from.x - 10.0 * cw, from.y),
    );
    let sets = set_splits_of(&a);
    assert!(!sets.is_empty());
    let (last, merge) = sets.last().unwrap();
    assert_eq!(last[0].key_hi, 49);
    assert_eq!(
        (last[0].key_lo, last[1].clone()),
        (0, splits_of(d.doc())[1].clone())
    );
    assert!(sets.iter().all(|(_, m)| m == merge && m.is_some()));
    assert_eq!(a.iter().filter(|x| **x == UiAction::EndMerge).count(), 1);
    // One undo step.
    let before = splits_of(d.doc());
    apply_all(&mut d, &a);
    assert_eq!(splits_of(d.doc())[0].key_hi, 49);
    d.undo(&registry()).unwrap();
    assert_eq!(splits_of(d.doc()), before);
}

#[test]
fn the_left_edge_and_the_velocity_edges_move_their_own_end() {
    let d = roomy();
    let mut v = editor(d.doc());
    let s0 = splits_of(d.doc())[0].clone();
    let b = v.bar_rect(0).unwrap();
    let cw = v.map_rect().width() / 128.0;
    let u = v.map_rect().height() / 127.0;
    // Left edge: +5 keys.
    let p = Point::new(b.x0 + 1.0, b.center().y);
    let a = drag(&mut v, d.doc(), p, Point::new(p.x + 5.0 * cw, p.y));
    assert_eq!(set_splits_of(&a).last().unwrap().0[0].key_lo, s0.key_lo + 5);
    // Top edge: down 10 velocity steps.
    let p = Point::new(b.center().x, b.y0 + 1.0);
    let a = drag(&mut v, d.doc(), p, Point::new(p.x, p.y + 10.0 * u));
    let s = set_splits_of(&a).last().unwrap().0[0].clone();
    assert_eq!((s.vel_hi, s.vel_lo), (s0.vel_hi - 10, s0.vel_lo));
    // Bottom edge: up 10 velocity steps.
    let p = Point::new(b.center().x, b.y1 - 1.0);
    let a = drag(&mut v, d.doc(), p, Point::new(p.x, p.y - 10.0 * u));
    let s = set_splits_of(&a).last().unwrap().0[0].clone();
    assert_eq!((s.vel_lo, s.vel_hi), (s0.vel_lo + 10, s0.vel_hi));
}

#[test]
fn the_body_moves_the_whole_zone() {
    let d = roomy();
    let mut v = editor(d.doc());
    let s1 = splits_of(d.doc())[1].clone();
    let b = v.bar_rect(1).unwrap();
    let cw = v.map_rect().width() / 128.0;
    let u = v.map_rect().height() / 127.0;
    let p = b.center();
    let a = drag(
        &mut v,
        d.doc(),
        p,
        Point::new(p.x - 8.0 * cw, p.y + 5.0 * u),
    );
    let s = set_splits_of(&a).last().unwrap().0[1].clone();
    assert_eq!(
        (s.key_lo, s.key_hi, s.vel_lo, s.vel_hi),
        (s1.key_lo - 8, s1.key_hi - 8, s1.vel_lo - 5, s1.vel_hi - 5)
    );
    assert_eq!(v.selected_split(), Some(1));
}

#[test]
fn a_press_without_a_move_emits_nothing_and_selects() {
    let d = layered();
    let mut v = editor(d.doc());
    let b = v.bar_rect(1).unwrap();
    let a = click_at(&mut v, d.doc(), b.center());
    assert!(a.is_empty(), "{a:?}");
    assert_eq!(v.selected_split(), Some(1));
    assert!(!v.is_dragging());
}

#[test]
fn escape_during_a_drag_restores_the_press_state() {
    let mut d = layered();
    let mut v = editor(d.doc());
    let before = splits_of(d.doc());
    let b = v.bar_rect(0).unwrap();
    let cw = v.map_rect().width() / 128.0;
    let from = Point::new(b.x1 - 1.0, b.center().y);
    let mut a = feed(&mut v, d.doc(), down(from));
    a.extend(feed(
        &mut v,
        d.doc(),
        mv(Point::new(from.x - 20.0 * cw, from.y)),
    ));
    assert!(v.is_dragging());
    a.extend(press(&mut v, d.doc(), Key::Escape));
    assert!(!v.is_dragging());
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    let sets = set_splits_of(&a);
    assert!(sets.len() >= 2 && sets.last().unwrap().0 == before);
    apply_all(&mut d, &a);
    assert_eq!(splits_of(d.doc()), before);
    // The release afterwards does nothing.
    assert!(feed(&mut v, d.doc(), up(from)).is_empty());
}

#[test]
fn a_press_on_empty_map_space_auditions_the_key_at_the_clicked_velocity() {
    let d = roomy();
    let mut v = editor(d.doc());
    let m = v.map_rect();
    // Above split 0's velocity range (it ends at 100), over key 20.
    let p = Point::new(
        m.x0 + 20.5 * m.width() / 128.0,
        m.y1 - 115.0 * m.height() / 127.0,
    );
    assert_eq!(v.hit(p), Hit::Map);
    let a = feed(&mut v, d.doc(), down(p));
    match &a[..] {
        [
            UiAction::AuditionVelocity {
                device,
                key: k,
                velocity,
                on: true,
            },
        ] => {
            assert_eq!(device, &key("layer"));
            assert_eq!(*k, 20);
            assert!((113..=117).contains(velocity), "{velocity}");
        }
        a => panic!("{a:?}"),
    }
    assert!(v.is_dragging());
    let a = feed(&mut v, d.doc(), up(p));
    assert!(matches!(
        &a[..],
        [UiAction::AuditionVelocity {
            on: false,
            key: 20,
            ..
        }]
    ));
    assert_eq!(v.selected_split(), None);
}

#[test]
fn the_keyboard_strip_auditions_through_the_splits_and_escape_releases() {
    let d = layered();
    let mut v = editor(d.doc());
    let p = v.key_center(64);
    let a = feed(&mut v, d.doc(), down(p));
    assert_eq!(
        a,
        vec![UiAction::AuditionVelocity {
            device: key("layer"),
            key: 64,
            velocity: STRIP_VELOCITY,
            on: true
        }]
    );
    let a = press(&mut v, d.doc(), Key::Escape);
    assert_eq!(
        a,
        vec![UiAction::AuditionVelocity {
            device: key("layer"),
            key: 64,
            velocity: STRIP_VELOCITY,
            on: false
        }],
        "Esc ends the note, it does not close the editor"
    );
    assert!(!v.is_dragging());
}

// ------------------------------------------------------------------ splits: the list

#[test]
fn the_list_selects_steps_the_transpose_and_deletes() {
    let mut d = layered();
    let mut v = editor(d.doc());
    // Transpose plus: +1, Shift: +12 (split 1 is at +12, so +13 and +24).
    let inc = v.inc_rect(1).center();
    let a = click_at(&mut v, d.doc(), inc);
    assert_eq!(set_splits_of(&a)[0].0[1].transpose, 13);
    assert_eq!(set_splits_of(&a)[0].1, None, "a click is a standalone step");
    let a = click_with(
        &mut v,
        d.doc(),
        inc,
        1,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(set_splits_of(&a)[0].0[1].transpose, 24);
    let dec = v.dec_rect(0).center();
    let a = click_at(&mut v, d.doc(), dec);
    assert_eq!(set_splits_of(&a)[0].0[0].transpose, -1);
    assert_eq!(v.selected_split(), Some(0));
    // The wheel over the value steps too.
    let w = v.transpose_rect(1).center();
    let a = feed(
        &mut v,
        d.doc(),
        UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: w,
            modifiers: Modifiers::NONE,
        },
    );
    assert_eq!(set_splits_of(&a)[0].0[1].transpose, 13);
    // Delete.
    let x = v.delete_rect(0).center();
    let a = click_at(&mut v, d.doc(), x);
    let list = set_splits_of(&a)[0].0.clone();
    assert_eq!(list, vec![splits_of(d.doc())[1].clone()]);
    apply_all(&mut d, &a);
    assert_eq!(splits_of(d.doc()).len(), 1);
}

#[test]
fn transpose_stops_at_48() {
    let mut d = layered();
    let mut s = splits_of(d.doc());
    s[0].transpose = 48;
    apply(
        &mut d,
        Command::Container(ContainerCommand::SetSplits {
            container: key("layer"),
            splits: s,
            merge: None,
        }),
    );
    let mut v = editor(d.doc());
    let at = v.inc_rect(0).center();
    let a = click_at(&mut v, d.doc(), at);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn add_puts_a_full_range_split_on_the_next_free_device_and_refuses_without_one() {
    let mut d = roomy();
    let mut v = editor(d.doc());
    let at = v.add_rect().center();
    let a = click_at(&mut v, d.doc(), at);
    let list = set_splits_of(&a)[0].0.clone();
    assert_eq!(list.len(), 3);
    assert_eq!(list[2], NoteSplit::full(key("lead2")));
    apply_all(&mut d, &a);
    v.sync(d.doc(), &registry());
    // No free device with a Notes input is left: the button does nothing.
    let at = v.add_rect().center();
    let a = click_at(&mut v, d.doc(), at);
    assert!(a.is_empty(), "{a:?}");
}

#[test]
fn the_target_selector_steps_to_the_next_free_device() {
    let mut d = roomy();
    let mut v = editor(d.doc());
    let at = v.target_rect(0).center();
    let a = click_at(&mut v, d.doc(), at);
    let list = set_splits_of(&a)[0].0.clone();
    assert_eq!(list[0].target, key("lead2"), "synth -> the only free one");
    apply_all(&mut d, &a);
    v.sync(d.doc(), &registry());
    // Now synth is free: split 0 (lead2) steps to synth again.
    let at = v.target_rect(0).center();
    let a = click_at(&mut v, d.doc(), at);
    assert_eq!(set_splits_of(&a)[0].0[0].target, key("synth"));
    // Without a free device nothing happens.
    let d2 = layered();
    let mut v2 = editor(d2.doc());
    let at = v2.target_rect(0).center();
    assert!(click_at(&mut v2, d2.doc(), at).is_empty());
}

#[test]
fn escape_closes_and_so_does_the_button() {
    let d = layered();
    let mut v = editor(d.doc());
    assert_eq!(
        press(&mut v, d.doc(), Key::Escape),
        vec![UiAction::CloseContainerEditor]
    );
    let at = v.close_rect().center();
    let a = click_at(&mut v, d.doc(), at);
    assert_eq!(a, vec![UiAction::CloseContainerEditor]);
}

#[test]
fn undo_and_redo_keys_are_forwarded() {
    let d = layered();
    let mut v = editor(d.doc());
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    let z = |m| UiEvent::Key {
        key: Key::Char('z'),
        modifiers: m,
        pressed: true,
    };
    assert_eq!(feed(&mut v, d.doc(), z(ctrl)), vec![UiAction::Undo]);
    assert_eq!(
        feed(
            &mut v,
            d.doc(),
            z(Modifiers {
                shift: true,
                ..ctrl
            })
        ),
        vec![UiAction::Redo]
    );
}

// ------------------------------------------------------------------ the MACROS tab

fn macros_view(d: &Document) -> ContainerEditorView {
    let mut v = editor(d.doc());
    let t = v.tab_rect(ContainerTab::Macros).center();
    let _ = click_at(&mut v, d.doc(), t);
    assert_eq!(v.tab(), ContainerTab::Macros);
    v
}

#[test]
fn the_tabs_switch_and_a_row_selects_its_macro() {
    let d = macros_doc();
    let mut v = macros_view(&d);
    assert_eq!(v.selected_macro(), 0);
    let r = v.macro_row_rect(2);
    let _ = click_at(&mut v, d.doc(), Point::new(r.x1 - 3.0, r.center().y));
    assert_eq!(v.selected_macro(), 2);
    let t = v.tab_rect(ContainerTab::Splits).center();
    let _ = click_at(&mut v, d.doc(), t);
    assert_eq!(v.tab(), ContainerTab::Splits);
}

#[test]
fn the_name_renames_in_place() {
    let d = layered();
    let mut v = macros_view(&d);
    let p = v.macro_name_rect(1).center();
    let _ = click_with(&mut v, d.doc(), p, 2, Modifiers::NONE);
    assert_eq!(v.selected_macro(), 1);
    assert_eq!(v.renaming(), Some(""));
    for ch in "Drive".chars() {
        let _ = press(&mut v, d.doc(), Key::Char(ch));
    }
    assert_eq!(v.renaming(), Some("Drive"));
    let a = press(&mut v, d.doc(), Key::Enter);
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::RenameMacro {
                container: key("layer"),
                index: 1,
                name: "Drive".into()
            }
        ))]
    );
    assert_eq!(v.renaming(), None);
    // Esc cancels, an unchanged name emits nothing.
    let p = v.macro_name_rect(0).center();
    let _ = click_with(&mut v, d.doc(), p, 2, Modifiers::NONE);
    assert_eq!(v.renaming(), Some("Cutoff"));
    assert!(press(&mut v, d.doc(), Key::Enter).is_empty());
    let _ = click_with(&mut v, d.doc(), p, 2, Modifiers::NONE);
    assert!(press(&mut v, d.doc(), Key::Escape).is_empty());
    assert_eq!(v.renaming(), None);
    // A click elsewhere cancels without a command, and Esc then closes nothing.
    let _ = click_with(&mut v, d.doc(), p, 2, Modifiers::NONE);
    assert!(click_at(&mut v, d.doc(), Point::new(900.0, 520.0)).is_empty());
    assert_eq!(v.renaming(), None);
}

#[test]
fn the_view_says_when_it_owns_the_keyboard_as_a_text_field() {
    // single-letter commands must not fire while a macro name is typed.
    let d = layered();
    let mut v = macros_view(&d);
    assert!(!v.is_text_owner());
    let p = v.macro_name_rect(1).center();
    let _ = click_with(&mut v, d.doc(), p, 2, Modifiers::NONE);
    assert!(v.is_text_owner(), "the name is being edited");
    // Every letter, digit and Space reaches the field.
    for ch in "rl1k s".chars() {
        let key = if ch == ' ' { Key::Space } else { Key::Char(ch) };
        let _ = press(&mut v, d.doc(), key);
    }
    assert_eq!(v.renaming(), Some("rl1k s"));
    let _ = press(&mut v, d.doc(), Key::Escape);
    assert!(!v.is_text_owner(), "Esc gives the field up");
}

#[test]
fn the_knob_drags_the_macro_value_merged_and_resets_on_double_click() {
    let d = macros_doc();
    let mut v = macros_view(&d);
    let k = v.macro_knob_rect(3).center();
    let a = drag(&mut v, d.doc(), k, Point::new(k.x, k.y - 30.0));
    let c = commands(&a);
    assert!(!c.is_empty());
    let merge0 = match &c[0] {
        Command::SetParam {
            key: kk,
            param,
            value,
            merge,
        } => {
            assert_eq!((kk, param.as_str()), (&key("layer"), "macro4"));
            assert!(*value > 0.0);
            assert!(merge.is_some());
            *merge
        }
        c => panic!("{c:?}"),
    };
    assert!(
        c.iter()
            .all(|c| matches!(c, Command::SetParam { merge, .. } if *merge == merge0))
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Double-click resets.
    let k = v.macro_knob_rect(0).center();
    let a = click_with(&mut v, d.doc(), k, 2, Modifiers::NONE);
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: key("layer"),
            param: "macro1".into(),
            value: 0.0,
            merge: None
        }]
    );
    // Escape restores the value of the press.
    let k = v.macro_knob_rect(0).center();
    let mut a = feed(&mut v, d.doc(), down(k));
    a.extend(feed(&mut v, d.doc(), mv(Point::new(k.x, k.y - 40.0))));
    a.extend(press(&mut v, d.doc(), Key::Escape));
    let c = commands(&a);
    match c.last().unwrap() {
        Command::SetParam { value, .. } => assert!((*value - 0.6).abs() < 1e-6),
        c => panic!("{c:?}"),
    }
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

fn field(v: &ContainerEditorView, j: usize, b: Bound) -> Point {
    v.field_rect(j, b).center()
}

#[test]
fn the_min_and_max_fields_drag_wheel_and_reset() {
    let mut d = macros_doc();
    let mut v = macros_view(&d);
    let reg = registry();
    let desc = reg.get(SYNTH).unwrap().descriptor;
    let p = desc.param_by_key("filter.cutoff").unwrap();
    // Macro 1: target 0 = synth cutoff 200..8000 (the fixture), 1 = pad cutoff, 2 = delay mix.
    let from = field(&v, 0, Bound::Min);
    let a = drag(&mut v, d.doc(), from, Point::new(from.x, from.y - 40.0));
    let sets: Vec<_> = commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            Command::Container(ContainerCommand::SetMacro {
                container,
                index,
                target,
                merge,
            }) => Some((container, index, target, merge)),
            _ => None,
        })
        .collect();
    assert!(!sets.is_empty());
    let (_, index, t, merge) = sets.last().unwrap();
    assert_eq!(*index, 0);
    assert_eq!(
        (t.device.clone(), t.param.as_str()),
        (key("synth"), "filter.cutoff")
    );
    assert!(t.min > 200.0 && (t.max - 8000.0).abs() < 1e-3, "{t:?}");
    assert!(merge.is_some() && sets.iter().all(|s| s.3 == *merge));
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    apply_all(&mut d, &a);
    v.sync(d.doc(), &reg);
    // Double-click resets the end to the descriptor's.
    let from = field(&v, 0, Bound::Min);
    let a = click_with(&mut v, d.doc(), from, 2, Modifiers::NONE);
    match &commands(&a)[0] {
        Command::Container(ContainerCommand::SetMacro { target, merge, .. }) => {
            assert_eq!(target.min, p.min);
            assert_eq!(*merge, None);
        }
        c => panic!("{c:?}"),
    }
    // The wheel over a field nudges it.
    let w = field(&v, 1, Bound::Max);
    let a = feed(
        &mut v,
        d.doc(),
        UiEvent::Wheel {
            delta: Vec2::new(0.0, -1.0),
            pos: w,
            modifiers: Modifiers::NONE,
        },
    );
    assert_eq!(commands(&a).len(), 1);
    // Esc during a drag restores the press value.
    let from = field(&v, 2, Bound::Max);
    let mut a = feed(&mut v, d.doc(), down(from));
    a.extend(feed(&mut v, d.doc(), mv(Point::new(from.x, from.y - 40.0))));
    a.extend(press(&mut v, d.doc(), Key::Escape));
    let c = commands(&a);
    match c.last().unwrap() {
        Command::Container(ContainerCommand::SetMacro { target, .. }) => {
            assert!((target.max - 0.5).abs() < 1e-6, "{target:?}");
        }
        c => panic!("{c:?}"),
    }
}

#[test]
fn deleting_a_target_unmaps_it() {
    let mut d = macros_doc();
    let mut v = macros_view(&d);
    let x = v.target_delete_rect(1).center();
    let a = click_at(&mut v, d.doc(), x);
    assert_eq!(
        a,
        vec![UiAction::Command(Command::Container(
            ContainerCommand::RemoveMacroTarget {
                container: key("layer"),
                index: 0,
                device: key("pad"),
                param: "filter.cutoff".into(),
            }
        ))]
    );
    apply_all(&mut d, &a);
    v.sync(d.doc(), &registry());
    assert_eq!(
        d.doc().inner(&key("layer")).unwrap().macros[0]
            .targets
            .len(),
        2
    );
}

#[test]
fn the_edit_range_entry_opens_on_a_macro() {
    let d = macros_doc();
    let reg = registry();
    let mut v = ContainerEditorView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(key("layer"));
    v.set_tab(ContainerTab::Macros);
    v.select_macro(2);
    v.sync(d.doc(), &reg);
    assert_eq!((v.tab(), v.selected_macro()), (ContainerTab::Macros, 2));
    assert_eq!(
        v.hit(v.target_delete_rect(0).center()),
        Hit::TargetDelete(0)
    );
}

// ------------------------------------------------------------------ the property test

fn arb_point() -> impl Strategy<Value = Point> {
    (0.0..1000.0f64, 0.0..560.0f64).prop_map(|(x, y)| Point::new(x, y))
}

#[derive(Debug, Clone)]
enum G {
    Click(Point, u8, bool),
    Drag(Point, Point),
    Key(u8),
    Tab(bool),
    Wheel(Point, i8),
}

fn arb_gesture() -> impl Strategy<Value = G> {
    prop_oneof![
        (arb_point(), 1u8..=2, any::<bool>()).prop_map(|(p, c, s)| G::Click(p, c, s)),
        (arb_point(), arb_point()).prop_map(|(a, b)| G::Drag(a, b)),
        (0u8..8).prop_map(G::Key),
        any::<bool>().prop_map(G::Tab),
        (arb_point(), -2i8..=2).prop_map(|(p, n)| G::Wheel(p, n)),
    ]
}

fn keys_for(n: u8) -> Key {
    match n {
        0 => Key::Escape,
        1 => Key::Enter,
        2 => Key::Backspace,
        3 => Key::Char('a'),
        4 => Key::Char('Z'),
        5 => Key::Space,
        6 => Key::Delete,
        _ => Key::Tab,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn no_gesture_sequence_makes_the_model_refuse_a_command(
        gestures in proptest::collection::vec(arb_gesture(), 1..30)
    ) {
        let mut d = macros_doc();
        add_inner(&mut d, "layer", 4, SYNTH, "lead2");
        let reg = registry();
        let mut v = editor(d.doc());
        for g in gestures {
            let actions = match g {
                G::Click(p, clicks, shift) => click_with(
                    &mut v,
                    d.doc(),
                    p,
                    clicks,
                    Modifiers { shift, ..Modifiers::NONE },
                ),
                G::Drag(a, b) => drag(&mut v, d.doc(), a, b),
                G::Key(n) => press(&mut v, d.doc(), keys_for(n)),
                G::Tab(macros) => {
                    let t = if macros { ContainerTab::Macros } else { ContainerTab::Splits };
                    v.set_tab(t);
                    Vec::new()
                }
                G::Wheel(p, n) => feed(
                    &mut v,
                    d.doc(),
                    UiEvent::Wheel {
                        delta: Vec2::new(0.0, f64::from(n)),
                        pos: p,
                        modifiers: Modifiers::NONE,
                    },
                ),
            };
            for a in actions {
                match a {
                    UiAction::Command(c) => {
                        let r = d.apply(c.clone(), &reg);
                        prop_assert!(r.is_ok(), "{c:?} refused: {r:?}");
                    }
                    UiAction::EndMerge => d.end_merge(),
                    UiAction::Undo => { d.undo(&reg); }
                    UiAction::Redo => { d.redo(&reg); }
                    _ => {}
                }
            }
            v.sync(d.doc(), &reg);
            prop_assert!(v.is_present());
        }
    }
}

// ------------------------------------------------------------------ the other editors

#[test]
fn the_sample_slice_and_sequencer_editors_open_for_inner_devices_by_path_key() {
    use atomik_ui::{SampleEditorView, SeqEditorView, SliceEditorView};
    let mut d = layered();
    add_inner(&mut d, "layer", 4, "atomik.sampler", "samp");
    add_inner(&mut d, "layer", 5, "atomik.loop-slicer", "slicer");
    add_inner(&mut d, "layer", 6, "atomik.step-sequencer", "seq");
    let reg = registry();
    let mut sample = SampleEditorView::new(Theme::dark());
    sample.open(key("layer/samp"));
    sample.sync(d.doc(), &reg);
    assert!(sample.is_present(), "sample editor");
    let mut slice = SliceEditorView::new(Theme::dark());
    slice.open(key("layer/slicer"));
    slice.sync(d.doc(), &reg);
    assert!(slice.is_present(), "slice editor");
    let mut seq = SeqEditorView::new(Theme::dark());
    seq.open(key("layer/seq"));
    seq.sync(d.doc(), &reg);
    assert!(seq.is_present(), "sequencer editor");
    // A wrong path is not present.
    seq.open(key("layer/nope"));
    seq.sync(d.doc(), &reg);
    assert!(!seq.is_present());
}
