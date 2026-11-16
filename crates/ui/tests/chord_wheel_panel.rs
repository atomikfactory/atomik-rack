//! The Chord Wheel rack panel: layout, wedge geometry, hit tests, the actions of the wheel, the
//! cards, the padlocks and the buttons, the length drag, the menu, display-list goldens and pixel
//! goldens (dark and light). Every emitted command is applied to a real model `Document`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test chord_wheel_panel`.

mod support;

use atomik_core::progression::{Chord, Key, KeyMode, Mood};
use atomik_core::theory::ChordKind;
use atomik_model::{Command, Document, RackDoc};
use atomik_ui::chord_wheel_panel::{
    self as cw, CHORD_WHEEL_TYPE, Ring, SLOTS, Wedge, WheelButton, chord_wheel_layout,
};
use atomik_ui::context_menu::Entry;
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::{DisplayList, Modifiers, PointerButton, RackView, Theme, UiAction, UiEvent};
use std::path::PathBuf;
use support::pixel;
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

fn wheel_doc() -> RackDoc {
    let mut doc = reference_doc();
    doc.devices = vec![device("w", CHORD_WHEEL_TYPE, "Chord Wheel")];
    doc.cables.clear();
    doc
}

fn setup() -> (RackDoc, RackView) {
    let doc = wheel_doc();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn set(doc: &mut RackDoc, param: &str, value: f32) {
    doc.devices[0].params.insert(param.into(), value);
}

fn panel() -> cw::ChordWheelPanel {
    let reg = registry();
    let desc = reg.get(CHORD_WHEEL_TYPE).unwrap().descriptor;
    chord_wheel_layout(desc)
        .chord_wheel
        .expect("chord wheel panel")
}

fn click_at(v: &mut RackView, doc: &RackDoc, p: Point, mods: Modifiers) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: mods,
            clicks: 1,
        },
        doc,
        &reg,
    );
    out.extend(v.handle(
        &UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: mods,
        },
        doc,
        &reg,
    ));
    out
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    click_at(v, doc, p, Modifiers::NONE)
}

fn click_wedge(v: &mut RackView, doc: &RackDoc, ring: Ring, pos: u8) -> Vec<UiAction> {
    let at = wedge_point(v, ring, pos);
    click(v, doc, at)
}

fn click_wedge_with(
    v: &mut RackView,
    doc: &RackDoc,
    ring: Ring,
    pos: u8,
    mods: Modifiers,
) -> Vec<UiAction> {
    let at = wedge_point(v, ring, pos);
    click_at(v, doc, at, mods)
}

fn click_btn(v: &mut RackView, doc: &RackDoc, b: WheelButton) -> Vec<UiAction> {
    let at = button_point(v, b);
    click(v, doc, at)
}

fn wedge_point(v: &RackView, ring: Ring, pos: u8) -> Point {
    v.wheel_wedge_center(&key("w"), Wedge::new(ring, pos))
        .expect("wedge in view")
}

fn button_point(v: &RackView, b: WheelButton) -> Point {
    v.wheel_button_center(&key("w"), b).expect("button in view")
}

fn texts_of(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

// ------------------------------------------------------------------ wedges

#[test]
fn the_wedges_follow_the_circle_of_fifths_with_c_at_the_top() {
    let majors: Vec<u8> = (0..12).map(|p| Wedge::new(Ring::Major, p).root()).collect();
    // C G D A E B F# C# G# D# A# F
    assert_eq!(majors, [0, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10, 5]);
    // Under each major its relative minor and the diminished chord on its leading tone.
    assert_eq!(
        Wedge::new(Ring::Minor, 0).chord(),
        Chord::new(9, ChordKind::Minor)
    );
    assert_eq!(
        Wedge::new(Ring::Minor, 1).chord(),
        Chord::new(4, ChordKind::Minor)
    );
    assert_eq!(
        Wedge::new(Ring::Dim, 0).chord(),
        Chord::new(11, ChordKind::Dim)
    );
    assert_eq!(
        Wedge::new(Ring::Dim, 11).chord(),
        Chord::new(4, ChordKind::Dim)
    );
}

#[test]
fn a_chord_belongs_to_the_wedge_of_its_root_and_family() {
    for ring in Ring::ALL {
        for pos in 0..12 {
            let w = Wedge::new(ring, pos);
            assert_eq!(Wedge::of(w.chord()), w);
        }
    }
    let on = |root, kind| Wedge::of(Chord::new(root, kind));
    // Sevenths, ninths and sus chords sit on their triad's wedge.
    assert_eq!(on(9, ChordKind::MinorSeventh), Wedge::new(Ring::Minor, 0));
    assert_eq!(on(7, ChordKind::Seventh), Wedge::new(Ring::Major, 1));
    assert_eq!(on(0, ChordKind::MajorSeventh), Wedge::new(Ring::Major, 0));
    assert_eq!(on(0, ChordKind::Sus4), Wedge::new(Ring::Major, 0));
    assert_eq!(on(11, ChordKind::HalfDim), Wedge::new(Ring::Dim, 0));
    assert_eq!(on(2, ChordKind::MinorNinth), Wedge::new(Ring::Minor, 11));
    // A slash bass does not move it.
    assert_eq!(
        Wedge::of(Chord::new(0, ChordKind::Major).over(4)),
        Wedge::new(Ring::Major, 0)
    );
}

#[test]
fn the_home_key_tints_its_seven_triads() {
    let all = |key| -> Vec<Wedge> {
        Ring::ALL
            .into_iter()
            .flat_map(|r| (0..12).map(move |p| Wedge::new(r, p)))
            .filter(|w| cw::is_diatonic(key, *w))
            .collect()
    };
    let c = all(Key::new(0, KeyMode::Major));
    assert_eq!(c.len(), 7);
    let names: Vec<Chord> = c.iter().map(|w| w.chord()).collect();
    for expect in [
        Chord::new(0, ChordKind::Major),
        Chord::new(5, ChordKind::Major),
        Chord::new(7, ChordKind::Major),
        Chord::new(2, ChordKind::Minor),
        Chord::new(4, ChordKind::Minor),
        Chord::new(9, ChordKind::Minor),
        Chord::new(11, ChordKind::Dim),
    ] {
        assert!(names.contains(&expect), "{expect:?}");
    }
    // A minor shares the triads of C major.
    let mut a: Vec<Wedge> = all(Key::new(9, KeyMode::Minor));
    let mut c = c;
    a.sort_by_key(|w| (w.ring.index(), w.pos));
    c.sort_by_key(|w| (w.ring.index(), w.pos));
    assert_eq!(a, c);
}

#[test]
fn a_point_finds_its_wedge_including_the_ring_boundaries() {
    let p = panel();
    let at = |r: f64, deg: f64| {
        let a = (deg - 90.0).to_radians();
        Point::new(p.centre.x + r * a.cos(), p.centre.y + r * a.sin())
    };
    // The top of each ring is C, A minor and B diminished.
    assert_eq!(p.wedge_at(at(120.0, 0.0)), Some(Wedge::new(Ring::Major, 0)));
    assert_eq!(p.wedge_at(at(85.0, 0.0)), Some(Wedge::new(Ring::Minor, 0)));
    assert_eq!(p.wedge_at(at(52.0, 0.0)), Some(Wedge::new(Ring::Dim, 0)));
    // Clockwise: 30 degrees is G.
    assert_eq!(
        p.wedge_at(at(120.0, 30.0)),
        Some(Wedge::new(Ring::Major, 1))
    );
    // Counter-clockwise: F is at 330.
    assert_eq!(
        p.wedge_at(at(120.0, 330.0)),
        Some(Wedge::new(Ring::Major, 11))
    );
    // The boundary between two wedges is half way: 14.9 degrees is still C, 15.1 is G.
    assert_eq!(
        p.wedge_at(at(120.0, 14.9)),
        Some(Wedge::new(Ring::Major, 0))
    );
    assert_eq!(
        p.wedge_at(at(120.0, 15.1)),
        Some(Wedge::new(Ring::Major, 1))
    );
    assert_eq!(
        p.wedge_at(at(120.0, -14.9)),
        Some(Wedge::new(Ring::Major, 0))
    );
    // The rings change half way through the gap between them.
    assert_eq!(p.wedge_at(at(68.9, 0.0)), Some(Wedge::new(Ring::Dim, 0)));
    assert_eq!(p.wedge_at(at(69.1, 0.0)), Some(Wedge::new(Ring::Minor, 0)));
    assert_eq!(p.wedge_at(at(102.9, 0.0)), Some(Wedge::new(Ring::Minor, 0)));
    assert_eq!(p.wedge_at(at(103.1, 0.0)), Some(Wedge::new(Ring::Major, 0)));
    // The hub and the outside have no wedge.
    assert_eq!(p.wedge_at(at(35.0, 0.0)), None);
    assert_eq!(p.wedge_at(at(36.5, 0.0)), Some(Wedge::new(Ring::Dim, 0)));
    assert_eq!(p.wedge_at(at(138.5, 0.0)), None);
    assert_eq!(p.wedge_at(p.centre), None);
}

#[test]
fn every_wedge_centre_finds_its_own_wedge() {
    let p = panel();
    for ring in Ring::ALL {
        for pos in 0..12 {
            let w = Wedge::new(ring, pos);
            assert_eq!(p.wedge_at(p.wedge_centre(w)), Some(w), "{w:?}");
        }
    }
}

#[test]
fn the_wedge_makes_a_key_only_on_the_major_and_minor_rings() {
    assert_eq!(
        cw::key_of(Wedge::new(Ring::Major, 1)),
        Some(Key::new(7, KeyMode::Major))
    );
    assert_eq!(
        cw::key_of(Wedge::new(Ring::Minor, 0)),
        Some(Key::new(9, KeyMode::Minor))
    );
    assert_eq!(cw::key_of(Wedge::new(Ring::Dim, 0)), None);
}

// ------------------------------------------------------------------ layout

#[test]
fn the_panel_is_five_units_and_keeps_the_strip_bypass_out_of_the_body() {
    let reg = registry();
    let desc = reg.get(CHORD_WHEEL_TYPE).unwrap().descriptor;
    let spec = chord_wheel_layout(desc);
    assert_eq!(spec.height_units, cw::CHORD_WHEEL_UNITS);
    assert!(atomik_ui::autolayout::has_strip_bypass(desc));
    assert!(
        spec.front
            .iter()
            .all(|c| desc.params[c.param].key != "bypass")
    );
    // The slot params have no control of their own: the cards edit them.
    assert!(
        spec.front
            .iter()
            .all(|c| !desc.params[c.param].key.starts_with("slot."))
    );
    // Every other param except the seed has one.
    let placed: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    for p in desc.params {
        let expect =
            !p.key.starts_with("slot.") && !matches!(p.key, "bypass" | "slots" | "gen.seed");
        assert_eq!(placed.contains(&p.key), expect, "{}", p.key);
    }
}

#[test]
fn nothing_on_the_panel_overlaps_a_sibling() {
    let reg = registry();
    let desc = reg.get(CHORD_WHEEL_TYPE).unwrap().descriptor;
    let spec = chord_wheel_layout(desc);
    let p = spec.chord_wheel.unwrap();
    let height = spec.height();
    let mut rects: Vec<(String, Rect)> = vec![
        ("wheel".into(), p.wheel),
        ("generate".into(), p.generate),
        ("suggest".into(), p.suggest),
        ("readout".into(), p.readout),
        ("hint".into(), p.hint),
        ("slots_dec".into(), p.slots_dec),
        ("slots_text".into(), p.slots_text),
        ("slots_inc".into(), p.slots_inc),
        ("to_lane".into(), p.to_lane),
        ("to_clip".into(), p.to_clip),
    ];
    for n in 0..SLOTS {
        rects.push((format!("card{n}"), p.cards[n]));
    }
    for c in &spec.front {
        rects.push((desc.params[c.param].key.to_string(), c.rect));
    }
    for (name, r) in &rects {
        assert!(
            r.x0 >= atomik_ui::autolayout::CONTENT_X.0 - 0.5,
            "{name} {r:?}"
        );
        assert!(
            r.x1 <= atomik_ui::autolayout::CONTENT_X.1 + 0.5,
            "{name} {r:?}"
        );
        assert!(
            r.y0 >= 0.0 && r.y1 <= height - 2.0,
            "{name} {r:?} in {height}"
        );
    }
    for (i, (an, a)) in rects.iter().enumerate() {
        for (bn, b) in rects.iter().skip(i + 1) {
            let hit = a.intersect(*b);
            assert!(
                hit.width() <= 0.01 || hit.height() <= 0.01,
                "{an} {a:?} overlaps {bn} {b:?}"
            );
        }
    }
    // The padlock and the length lie inside their card and apart.
    for n in 0..SLOTS {
        let (c, l, b) = (p.cards[n], p.locks[n], p.beats[n]);
        assert!(c.contains_rect(l) && c.contains_rect(b));
        let both = l.intersect(b);
        assert!(both.width() <= 0.01 || both.height() <= 0.01);
    }
    // The wheel's disc stays inside its square, and its text columns do not meet the right column.
    assert!(p.wheel.x1 < p.cards[0].x0);
}

// ------------------------------------------------------------------ actions

fn action_commands(actions: &[UiAction]) -> Vec<Command> {
    commands(actions)
}

#[test]
fn a_click_on_a_wedge_sets_the_selected_slot_and_selects_the_next() {
    let (doc, mut v) = setup();
    assert_eq!(v.wheel_slot(&key("w")), Some(0));
    // The click on G minor (the middle ring, position 11 holds D minor ... use E minor).
    let at = wedge_point(&v, Ring::Minor, 1);
    let out = click(&mut v, &doc, at);
    let set: Vec<&UiAction> = out
        .iter()
        .filter(|a| matches!(a, UiAction::WheelSetChord { .. }))
        .collect();
    assert_eq!(
        set,
        [&UiAction::WheelSetChord {
            device: key("w"),
            slot: 0,
            chord: Chord::new(4, ChordKind::Minor),
        }]
    );
    // The next slot is selected, and a second click sets that one.
    assert_eq!(v.wheel_slot(&key("w")), Some(1));
    let out = click_wedge(&mut v, &doc, Ring::Major, 0);
    assert!(out.contains(&UiAction::WheelSetChord {
        device: key("w"),
        slot: 1,
        chord: Chord::new(0, ChordKind::Major),
    }));
}

#[test]
fn the_selection_wraps_past_the_last_active_slot() {
    let (doc, mut v) = setup();
    // Four slots by default.
    v.set_wheel_slot(&key("w"), 3);
    click_wedge(&mut v, &doc, Ring::Major, 0);
    assert_eq!(v.wheel_slot(&key("w")), Some(0));
    // The selection stays inside the active slots.
    v.set_wheel_slot(&key("w"), 7);
    assert_eq!(v.wheel_slot(&key("w")), Some(3));
}

#[test]
fn a_press_in_the_hub_or_between_wedges_sets_nothing() {
    let (doc, mut v) = setup();
    let p = panel();
    let origin = button_point(&v, WheelButton::Wheel);
    let hub = Point::new(origin.x, origin.y);
    let out = click(&mut v, &doc, hub);
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelSetChord { .. })),
        "{out:?}"
    );
    let _ = p;
}

#[test]
fn the_range_select_role_on_a_wedge_makes_the_home_key_in_one_command() {
    let (doc, mut v) = setup();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let out = click_wedge_with(&mut v, &doc, Ring::Major, 1, shift);
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelSetChord { .. }))
    );
    let cmds = action_commands(&out);
    let [Command::Batch(batch)] = &cmds[..] else {
        panic!("{cmds:?}");
    };
    let values: Vec<(&str, f32)> = batch
        .iter()
        .map(|c| match c {
            Command::SetParam { param, value, .. } => (param.as_str(), *value),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(values, [("key.root", 7.0), ("key.mode", 0.0)]);
    // The minor ring makes a minor key.
    let out = click_wedge_with(&mut v, &doc, Ring::Minor, 0, shift);
    let cmds = action_commands(&out);
    let [Command::Batch(batch)] = &cmds[..] else {
        panic!("{cmds:?}");
    };
    assert!(matches!(
        &batch[..],
        [
            Command::SetParam { value: r, .. },
            Command::SetParam { value: m, .. }
        ] if (*r - 9.0).abs() < 1e-6 && (*m - 1.0).abs() < 1e-6
    ));
    // The diminished ring makes none, and the selected slot stays.
    let out = click_wedge_with(&mut v, &doc, Ring::Dim, 0, shift);
    assert!(action_commands(&out).is_empty());
    assert_eq!(v.wheel_slot(&key("w")), Some(0));
}

#[test]
fn the_key_commands_apply_as_one_undo_step() {
    let (doc, mut v) = setup();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let out = click_wedge_with(&mut v, &doc, Ring::Major, 1, shift);
    let reg = registry();
    let mut d = Document::new(doc.clone());
    for c in action_commands(&out) {
        d.apply(c, &reg).unwrap();
    }
    let value = |d: &Document, p: &str| d.doc().param_value(&key("w"), p, &registry()).unwrap();
    assert_eq!((value(&d, "key.root"), value(&d, "key.mode")), (7.0, 0.0));
    d.undo().unwrap();
    assert_eq!((value(&d, "key.root"), value(&d, "key.mode")), (0.0, 0.0));
}

#[test]
fn a_right_click_on_a_wedge_asks_for_its_menu() {
    let (doc, mut v) = setup();
    let reg = registry();
    let at = wedge_point(&v, Ring::Major, 1);
    let mut out = v.handle(&down_with(at, PointerButton::Secondary, 1), &doc, &reg);
    out.extend(v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Secondary,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    ));
    let menu = out.iter().find_map(|a| match a {
        UiAction::WheelMenu {
            device, entries, ..
        } => Some((device.clone(), entries.clone())),
        _ => None,
    });
    let (device, entries) = menu.expect("a wheel menu");
    assert_eq!(device, key("w"));
    // The qualities of G, then the Slash Bass submenu.
    let labels: Vec<String> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some(i.label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        labels,
        [
            "G", "G7", "Gmaj7", "G6", "Gadd9", "G9", "Gmaj9", "Gsus2", "Gsus4", "Gaug"
        ]
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Submenu(s) if s.label == "Slash Bass"))
    );
    // A right click does not set a chord.
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelSetChord { .. }))
    );
}

#[test]
fn the_menu_rows_carry_the_slot_and_the_chord_and_check_the_current_one() {
    let key_c = Key::new(0, KeyMode::Major);
    let w = Wedge::new(Ring::Major, 0);
    let current = Chord::new(0, ChordKind::MajorSeventh);
    let entries = cw::menu_entries(2, w, key_c, current);
    let args: Vec<(String, Option<bool>)> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some((i.arg.clone().unwrap(), i.checked)),
            _ => None,
        })
        .collect();
    let checked: Vec<&str> = args
        .iter()
        .filter(|(_, c)| *c == Some(true))
        .map(|(a, _)| a.as_str())
        .collect();
    assert_eq!(checked, [cw::pick_arg(2, current).as_str()]);
    assert!(args.iter().all(|(_, c)| c.is_some()));
    // Every argument parses back to its slot.
    for (arg, _) in &args {
        let (slot, chord) = cw::parse_pick(arg).expect(arg);
        assert_eq!(slot, 2);
        assert_eq!(chord.root, 0);
    }
    // The slash rows: the chord tones first (E, G, B), then the other notes of the key.
    let Some(Entry::Submenu(sub)) = entries.iter().find(|e| matches!(e, Entry::Submenu(_))) else {
        panic!("no submenu");
    };
    let labels: Vec<String> = sub
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some(i.label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        labels,
        [
            "No slash", "Cmaj7/E", "Cmaj7/G", "Cmaj7/B", "Cmaj7/D", "Cmaj7/F", "Cmaj7/A"
        ]
    );
    assert!(sub.entries.iter().any(|e| matches!(e, Entry::Separator)));
}

#[test]
fn the_pick_argument_round_trips() {
    for chord in [
        Chord::new(0, ChordKind::Major),
        Chord::new(11, ChordKind::HalfDim),
        Chord::new(2, ChordKind::MinorNinth).over(9),
        Chord::new(7, ChordKind::Octave),
    ] {
        for slot in [0, 3, 7] {
            assert_eq!(
                cw::parse_pick(&cw::pick_arg(slot, chord)),
                Some((slot, chord))
            );
        }
    }
    for bad in [
        "",
        "1",
        "1:2:3",
        "8:0:0:-",
        "0:12:0:-",
        "0:0:21:-",
        "0:0:0:12",
        "0:0:0:-:1",
        "a:0:0:-",
    ] {
        assert_eq!(cw::parse_pick(bad), None, "{bad}");
    }
}

#[test]
fn the_wheel_menu_is_not_built_outside_the_wheel() {
    let (doc, mut v) = setup();
    let reg = registry();
    let hub = button_point(&v, WheelButton::Wheel);
    let out = v.handle(&down_with(hub, PointerButton::Secondary, 1), &doc, &reg);
    assert!(!out.iter().any(|a| matches!(a, UiAction::WheelMenu { .. })));
}

// ------------------------------------------------------------------ cards and buttons

#[test]
fn a_click_on_a_card_selects_the_slot_and_an_inactive_card_does_nothing() {
    let (doc, mut v) = setup();
    click_btn(&mut v, &doc, WheelButton::Card(2));
    assert_eq!(v.wheel_slot(&key("w")), Some(2));
    click_btn(&mut v, &doc, WheelButton::Card(6));
    assert_eq!(v.wheel_slot(&key("w")), Some(2), "slot 7 is not active");
}

#[test]
fn the_padlock_toggles_the_lock_of_its_slot() {
    let (doc, mut v) = setup();
    let out = click_btn(&mut v, &doc, WheelButton::Lock(1));
    assert!(out.contains(&UiAction::WheelToggleLock {
        device: key("w"),
        slot: 1,
    }));
    // The padlock does not select the card, and a dim one does nothing.
    assert_eq!(v.wheel_slot(&key("w")), Some(0));
    let out = click_btn(&mut v, &doc, WheelButton::Lock(5));
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelToggleLock { .. }))
    );
}

#[test]
fn the_slot_count_buttons_change_slots_within_one_to_eight() {
    let (doc, mut v) = setup();
    let out = click_btn(&mut v, &doc, WheelButton::SlotsInc);
    assert!(out.contains(&UiAction::WheelSetSlots {
        device: key("w"),
        slots: 5,
    }));
    let out = click_btn(&mut v, &doc, WheelButton::SlotsDec);
    assert!(out.contains(&UiAction::WheelSetSlots {
        device: key("w"),
        slots: 3,
    }));
    // The ends do nothing.
    let mut one = wheel_doc();
    set(&mut one, "slots", 1.0);
    let mut v = view(&one, &registry());
    let out = click_btn(&mut v, &one, WheelButton::SlotsDec);
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelSetSlots { .. }))
    );
    let mut eight = wheel_doc();
    set(&mut eight, "slots", 8.0);
    let mut v = view(&eight, &registry());
    let out = click_btn(&mut v, &eight, WheelButton::SlotsInc);
    assert!(
        !out.iter()
            .any(|a| matches!(a, UiAction::WheelSetSlots { .. }))
    );
}

#[test]
fn removing_a_slot_moves_the_selection_inside_the_rest() {
    let (doc, mut v) = setup();
    v.set_wheel_slot(&key("w"), 3);
    click_btn(&mut v, &doc, WheelButton::SlotsDec);
    assert_eq!(v.wheel_slot(&key("w")), Some(2));
}

#[test]
fn the_buttons_emit_their_device_actions() {
    let (doc, mut v) = setup();
    for (b, expect) in [
        (WheelButton::Generate, UiAction::WheelGenerate(key("w"))),
        (WheelButton::ToLane, UiAction::WheelToChordLane(key("w"))),
        (WheelButton::ToClip, UiAction::WheelToClip(key("w"))),
    ] {
        let out = click_btn(&mut v, &doc, b);
        assert!(out.contains(&expect), "{b:?}: {out:?}");
    }
}

#[test]
fn suggest_is_view_state_that_toggles_with_its_button() {
    let (doc, mut v) = setup();
    assert!(!v.wheel_suggest(&key("w")));
    let out = click_btn(&mut v, &doc, WheelButton::Suggest);
    assert!(action_commands(&out).is_empty(), "no command: {out:?}");
    assert!(v.wheel_suggest(&key("w")));
    click_btn(&mut v, &doc, WheelButton::Suggest);
    assert!(!v.wheel_suggest(&key("w")));
}

#[test]
fn a_drag_on_the_length_steps_it_as_one_merged_edit_and_a_press_alone_only_selects() {
    let (doc, mut v) = setup();
    let reg = registry();
    let at = button_point(&v, WheelButton::Beats(2));
    // A press and release in place selects the slot and changes nothing.
    let out = click(&mut v, &doc, at);
    assert!(action_commands(&out).is_empty(), "{out:?}");
    assert_eq!(v.wheel_slot(&key("w")), Some(2));
    // Dragging up 17 px: from the default (4 beats = index 3) two steps up.
    let mut out = v.handle(&down(at), &doc, &reg);
    out.extend(v.handle(&mv(Point::new(at.x, at.y - 17.0)), &doc, &reg));
    out.extend(v.handle(&up(Point::new(at.x, at.y - 17.0)), &doc, &reg));
    let set: Vec<(String, f32, Option<u64>)> = action_commands(&out)
        .into_iter()
        .map(|c| match c {
            Command::SetParam {
                param,
                value,
                merge,
                ..
            } => (param, value, merge),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(set.len(), 1, "{set:?}");
    assert_eq!((set[0].0.as_str(), set[0].1), ("slot.3.beats", 5.0));
    assert!(set[0].2.is_some());
    assert!(out.contains(&UiAction::EndMerge));
    // Dragging down past the first length stops there.
    let mut out = v.handle(&down(at), &doc, &reg);
    out.extend(v.handle(&mv(Point::new(at.x, at.y + 80.0)), &doc, &reg));
    out.extend(v.handle(&up(Point::new(at.x, at.y + 80.0)), &doc, &reg));
    let values: Vec<f32> = action_commands(&out)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { value, .. } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(values, [0.0]);
}

#[test]
fn the_wheel_over_a_length_steps_it_and_stops_at_the_ends() {
    let (doc, mut v) = setup();
    let reg = registry();
    let at = button_point(&v, WheelButton::Beats(0));
    let notch = |v: &mut RackView, dy: f64| {
        v.handle(
            &UiEvent::Wheel {
                delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
                pos: at,
                modifiers: Modifiers::NONE,
            },
            &doc,
            &reg,
        )
    };
    let out = notch(&mut v, 1.0);
    let c = action_commands(&out);
    assert!(matches!(
        &c[..],
        [Command::SetParam { param, value, .. }] if param == "slot.1.beats" && (*value - 4.0).abs() < 1e-6
    ));
    let out = notch(&mut v, -1.0);
    assert!(matches!(
        &action_commands(&out)[..],
        [Command::SetParam { value, .. }] if (*value - 2.0).abs() < 1e-6
    ));
    // At the shortest length a notch down does nothing.
    let mut doc = wheel_doc();
    set(&mut doc, "slot.1.beats", 0.0);
    let mut v = view(&doc, &reg);
    let out = v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: at,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!(action_commands(&out).is_empty());
}

#[test]
fn the_meter_values_name_the_playing_slot() {
    let (_, mut v) = setup();
    assert_eq!(v.wheel_playing(&key("w")), None);
    v.set_device_meter_values(&key("w"), &[3.0, 60.0, 64.0, 67.0]);
    assert_eq!(v.wheel_playing(&key("w")), Some(2));
    v.set_device_meter_values(&key("w"), &[0.0]);
    assert_eq!(v.wheel_playing(&key("w")), None);
    assert_eq!(cw::playing_slot(&[9.0]), None);
    assert_eq!(cw::playing_slot(&[]), None);
}

// ------------------------------------------------------------------ suggestions

#[test]
fn suggestions_after_g_put_the_tonic_first_and_number_three_wedges() {
    let key_c = Key::new(0, KeyMode::Major);
    let s = cw::suggestions(key_c, Mood::Pop, Chord::new(7, ChordKind::Major), 0.3);
    assert!(!s.is_empty());
    assert_eq!(s[0].wedge, Wedge::new(Ring::Major, 0), "I first after V");
    let ranked: Vec<u8> = s.iter().map(|x| x.rank).filter(|r| *r > 0).collect();
    assert_eq!(ranked, [1, 2, 3]);
    // Wedges are folded: no wedge twice.
    for (i, a) in s.iter().enumerate() {
        assert!(s.iter().skip(i + 1).all(|b| b.wedge != a.wedge));
    }
}

#[test]
fn the_helpers_pick_the_neighbouring_slots() {
    assert_eq!(cw::next_slot(3, 4), 0);
    assert_eq!(cw::next_slot(0, 4), 1);
    assert_eq!(cw::previous_slot(0, 4), 3);
    assert_eq!(cw::previous_slot(2, 4), 1);
    assert_eq!(cw::selected_slot(7, 4), 3);
    assert_eq!(cw::dragged_beats(2, 16.0), 4);
    assert_eq!(cw::dragged_beats(2, -100.0), 0);
    assert_eq!(cw::dragged_beats(6, 100.0), 7);
}

// ------------------------------------------------------------------ painting

fn texts_for(doc: &RackDoc, mut v: RackView) -> Vec<String> {
    texts_of(&mut v, doc)
}

#[test]
fn the_panel_names_the_chords_the_numerals_the_lengths_and_the_key() {
    let (doc, v) = setup();
    let t = texts_for(&doc, v);
    // The default progression C G Am F in C: every card shows a symbol and a numeral.
    for want in ["C", "G", "Am", "F", "I", "V", "vi", "IV", "4 BEATS"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    // The wheel names the three rings and the hub names the key.
    for want in ["F#", "Dm", "B\u{b0}", "MAJOR", "4 SLOTS"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    // The slots past `slots` are dimmed but still drawn.
    assert!(t.iter().filter(|x| x.as_str() == "4 BEATS").count() >= 8);
}

#[test]
fn the_wheel_respells_for_a_flat_key() {
    let mut doc = wheel_doc();
    set(&mut doc, "key.root", 5.0);
    let v = view(&doc, &registry());
    let t = texts_for(&doc, v);
    assert!(t.iter().any(|x| x == "Bb"), "{t:?}");
    assert!(!t.iter().any(|x| x == "A#"), "{t:?}");
}

#[test]
fn the_song_tag_shows_only_while_the_device_follows_the_song() {
    let mut doc = wheel_doc();
    let mut v = view(&doc, &registry());
    v.set_song_harmony(Some("D".into()), Some("Em".into()));
    let t = texts_of(&mut v, &doc);
    assert!(!t.iter().any(|x| x == "SONG"), "{t:?}");
    set(&mut doc, "key.source", 1.0);
    let mut v = view(&doc, &registry());
    v.set_song_harmony(Some("D".into()), Some("Em".into()));
    let t = texts_of(&mut v, &doc);
    assert!(t.iter().any(|x| x == "SONG"), "{t:?}");
}

#[test]
fn suggest_draws_the_three_badges() {
    let (doc, mut v) = setup();
    let plain = texts_of(&mut v, &doc);
    click_btn(&mut v, &doc, WheelButton::Suggest);
    let t = texts_of(&mut v, &doc);
    assert!(t.len() > plain.len(), "badges add text");
    for badge in ["1", "2", "3"] {
        assert!(
            t.iter().filter(|x| x.as_str() == badge).count()
                > plain.iter().filter(|x| x.as_str() == badge).count(),
            "badge {badge}"
        );
    }
}

fn dl_json(doc: &RackDoc, v: &mut RackView) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

/// A rack with a lit, locked, extended progression: Am7 / Dm7 / G7 / Cmaj7 / E7/G# in A minor.
fn busy_doc() -> RackDoc {
    let mut doc = wheel_doc();
    set(&mut doc, "key.root", 0.0);
    set(&mut doc, "slots", 5.0);
    let slots: [(f32, ChordKind, f32, f32); 5] = [
        (9.0, ChordKind::MinorSeventh, 0.0, 3.0),
        (2.0, ChordKind::MinorSeventh, 0.0, 2.0),
        (7.0, ChordKind::Seventh, 0.0, 3.0),
        (0.0, ChordKind::MajorSeventh, 0.0, 5.0),
        (4.0, ChordKind::Seventh, 9.0, 1.0),
    ];
    for (n, (root, kind, bass, beats)) in slots.into_iter().enumerate() {
        let q = ChordKind::ALL.iter().position(|k| *k == kind).unwrap();
        set(&mut doc, &format!("slot.{}.root", n + 1), root);
        set(&mut doc, &format!("slot.{}.quality", n + 1), q as f32);
        set(&mut doc, &format!("slot.{}.bass", n + 1), bass);
        set(&mut doc, &format!("slot.{}.beats", n + 1), beats);
    }
    set(&mut doc, "slot.2.lock", 1.0);
    doc
}

#[test]
fn the_front_panel_matches_golden() {
    let (doc, mut v) = setup();
    check("chord_wheel_panel_front_1x.json", &dl_json(&doc, &mut v));
}

#[test]
fn the_busy_panel_with_a_playing_slot_and_suggestions_matches_golden() {
    let doc = busy_doc();
    let mut v = view(&doc, &registry());
    v.set_device_meter_values(&key("w"), &[3.0, 55.0, 59.0, 62.0, 65.0]);
    v.set_wheel_slot(&key("w"), 4);
    click_btn(&mut v, &doc, WheelButton::Suggest);
    check("chord_wheel_panel_busy_1x.json", &dl_json(&doc, &mut v));
}

#[test]
fn the_back_matches_golden() {
    let (doc, mut v) = setup();
    v.set_flip(atomik_ui::Flip::Back);
    check("chord_wheel_panel_back_1x.json", &dl_json(&doc, &mut v));
}

// ------------------------------------------------------------------ pixels

const PIXEL_VIEW: Size = Size::new(780.0, 392.0);

fn painted(doc: &RackDoc, theme: Theme, scale: f64, live: bool) -> pixel::Image {
    let reg = registry();
    pixel::render(PIXEL_VIEW, scale, |c| {
        let mut v = RackView::new(theme);
        let _ = v.handle(&UiEvent::Resize(PIXEL_VIEW), doc, &reg);
        v.sync(doc, &reg);
        v.reveal_key(&key("w"));
        if live {
            v.set_device_meter_values(&key("w"), &[3.0, 55.0, 59.0, 62.0, 65.0]);
            v.set_wheel_slot(&key("w"), 4);
            let p = v
                .wheel_button_center(&key("w"), WheelButton::Suggest)
                .unwrap();
            let _ = v.handle(&down(p), doc, &reg);
            let _ = v.handle(&up(p), doc, &reg);
        }
        v.paint(c, doc, &reg);
    })
}

#[test]
fn the_panel_matches_its_pixel_goldens() {
    let doc = busy_doc();
    let mut failed = Vec::new();
    for (name, theme, scale, live) in [
        ("chord_wheel_dark_1x", Theme::dark(), 1.0, true),
        ("chord_wheel_light_1x", Theme::light(), 1.0, true),
        ("chord_wheel_dark_2x", Theme::dark(), 2.0, true),
        ("chord_wheel_default_1x", Theme::dark(), 1.0, false),
    ] {
        let d = if name.contains("default") {
            wheel_doc()
        } else {
            doc.clone()
        };
        let image = painted(&d, theme, scale, live);
        if std::panic::catch_unwind(|| pixel::check(name, &image)).is_err() {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "pixel goldens differ: {failed:?}");
}
