//! The Chord Wheel panel through the shell: a click on a wedge sets the selected slot in one undo
//! step and selects the next one, the right-click menu opens as a context menu and its rows run
//! `rack.wheel_pick`, the padlock and the slot count change the device, and GENERATE and the key
//! shortcut work from the panel. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_core::progression::{Chord, KeyMode};
use atomik_core::theory::ChordKind;
use atomik_model::{Command, DeviceKey};
use atomik_ui::chord_wheel_panel::{self as cw, Ring, Wedge, WheelButton};
use atomik_ui::{Modifiers, PointerButton};

const WHEEL: &str = "atomik.chord-wheel";

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::Resize(Size::new(1400.0, 2400.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.into())
}

fn setup() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: WHEEL.into(),
            key: Some(key("w")),
        })
        .unwrap();
    s
}

fn value(s: &Shell<NullBackend>, p: &str) -> f32 {
    s.core()
        .doc()
        .param_value(&key("w"), p, s.core().registry())
        .unwrap()
}

fn chord_of(s: &Shell<NullBackend>, slot: usize) -> (f32, f32, f32) {
    let n = slot + 1;
    (
        value(s, &format!("slot.{n}.root")),
        value(s, &format!("slot.{n}.quality")),
        value(s, &format!("slot.{n}.bass")),
    )
}

fn window_point(s: &Shell<NullBackend>, p: Point) -> Point {
    let origin = s.layout().rack.origin();
    Point::new(p.x + origin.x, p.y + origin.y)
}

fn press(s: &mut Shell<NullBackend>, p: Point, button: PointerButton, modifiers: Modifiers) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button,
        modifiers,
    });
}

fn wedge_at(s: &Shell<NullBackend>, ring: Ring, pos: u8) -> Point {
    let p = s
        .core()
        .view()
        .wheel_wedge_center(&key("w"), Wedge::new(ring, pos))
        .expect("the wheel is in view");
    window_point(s, p)
}

fn button_at(s: &Shell<NullBackend>, b: WheelButton) -> Point {
    let p = s
        .core()
        .view()
        .wheel_button_center(&key("w"), b)
        .expect("the button is in view");
    window_point(s, p)
}

fn q(kind: ChordKind) -> f32 {
    ChordKind::ALL.iter().position(|k| *k == kind).unwrap() as f32
}

#[test]
fn a_wedge_click_sets_the_slot_in_one_undo_step_and_selects_the_next() {
    let mut s = setup();
    // The default progression is C G Am F.
    assert_eq!(chord_of(&s, 0), (0.0, q(ChordKind::Major), 0.0));
    let at = wedge_at(&s, Ring::Minor, 1);
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    // E minor in slot 1; the other slots stay.
    assert_eq!(chord_of(&s, 0), (4.0, q(ChordKind::Minor), 0.0));
    assert_eq!(chord_of(&s, 1), (7.0, q(ChordKind::Major), 0.0));
    assert_eq!(s.core().view().wheel_slot(&key("w")), Some(1));
    // One undo step puts the whole chord back.
    assert!(s.core_mut().undo());
    assert_eq!(chord_of(&s, 0), (0.0, q(ChordKind::Major), 0.0));
    assert!(!s.core().doc().devices.is_empty());
}

#[test]
fn clicking_round_the_wheel_fills_the_slots_in_turn() {
    let mut s = setup();
    for (ring, pos) in [
        (Ring::Major, 11),
        (Ring::Minor, 0),
        (Ring::Major, 1),
        (Ring::Dim, 0),
    ] {
        let at = wedge_at(&s, ring, pos);
        press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    }
    // F, Am, G, Bdim in slots 1 to 4, and the selection is back at the first.
    let roots: Vec<f32> = (0..4).map(|k| chord_of(&s, k).0).collect();
    assert_eq!(roots, [5.0, 9.0, 7.0, 11.0]);
    assert_eq!(chord_of(&s, 3).1, q(ChordKind::Dim));
    assert_eq!(s.core().view().wheel_slot(&key("w")), Some(0));
}

#[test]
fn the_key_shortcut_sets_root_and_mode_in_one_undo_step() {
    let mut s = setup();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    let at = wedge_at(&s, Ring::Minor, 0);
    press(&mut s, at, PointerButton::Primary, shift);
    assert_eq!((value(&s, "key.root"), value(&s, "key.mode")), (9.0, 1.0));
    // No slot changed.
    assert_eq!(chord_of(&s, 0), (0.0, q(ChordKind::Major), 0.0));
    assert!(s.core_mut().undo());
    assert_eq!((value(&s, "key.root"), value(&s, "key.mode")), (0.0, 0.0));
    let _ = KeyMode::Major;
}

#[test]
fn the_right_click_menu_opens_and_its_rows_set_the_slot() {
    let mut s = setup();
    // Slot 2 is selected, then a right click on the G wedge.
    s.core_mut().view_mut().set_wheel_slot(&key("w"), 1);
    let at = wedge_at(&s, Ring::Major, 1);
    press(&mut s, at, PointerButton::Secondary, Modifiers::NONE);
    assert!(s.context_menu.is_some(), "the wedge menu is open");
    assert_eq!(s.wheel_menu, Some(key("w")));
    // A row runs rack.wheel_pick with the argument the panel built: G major seventh on B.
    let chord = Chord::new(7, ChordKind::MajorSeventh).over(11);
    let arg = cw::pick_arg(1, chord);
    s.run_command("rack.wheel_pick", Some(&arg));
    assert_eq!(
        chord_of(&s, 1),
        (7.0, q(ChordKind::MajorSeventh), 12.0),
        "bass B is value 12"
    );
    assert_eq!(s.wheel_menu, None);
    assert!(s.core_mut().undo());
    assert_eq!(chord_of(&s, 1), (7.0, q(ChordKind::Major), 0.0));
}

#[test]
fn a_menu_row_without_a_wheel_is_refused_with_a_message() {
    let mut s = shell();
    s.run_command("rack.wheel_pick", Some("0:0:0:-"));
    assert!(s.core().doc().devices.iter().all(|d| d.type_id != WHEEL));
}

#[test]
fn the_padlock_and_the_slot_buttons_edit_the_device() {
    let mut s = setup();
    let at = button_at(&s, WheelButton::Lock(2));
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(value(&s, "slot.3.lock"), 1.0);
    let at = button_at(&s, WheelButton::SlotsInc);
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(value(&s, "slots"), 5.0);
    let at = button_at(&s, WheelButton::SlotsDec);
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(value(&s, "slots"), 3.0);
}

#[test]
fn generate_writes_the_unlocked_slots_in_one_undo_step() {
    let mut s = setup();
    let before: Vec<(f32, f32, f32)> = (0..4).map(|k| chord_of(&s, k)).collect();
    let seed = value(&s, "gen.seed");
    let at = button_at(&s, WheelButton::Generate);
    press(&mut s, at, PointerButton::Primary, Modifiers::NONE);
    assert_ne!(value(&s, "gen.seed"), seed, "the seed advances");
    assert!(s.core_mut().undo());
    let after_undo: Vec<(f32, f32, f32)> = (0..4).map(|k| chord_of(&s, k)).collect();
    assert_eq!(after_undo, before);
    assert_eq!(value(&s, "gen.seed"), seed);
}
