use super::*;
use crate::event::Modifiers;

fn view() -> PianoKeysView {
    let mut v = PianoKeysView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 800.0, PIANO_KEYS_HEIGHT));
    v.set_focused(true);
    v
}

fn on(key: u8, velocity: u8) -> UiAction {
    UiAction::KeysNote {
        key,
        velocity,
        on: true,
    }
}

fn off(key: u8) -> UiAction {
    UiAction::KeysNote {
        key,
        velocity: 0,
        on: false,
    }
}

fn ch(c: char) -> Key {
    Key::Char(c)
}

fn down(pos: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn up(pos: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn centre(v: &PianoKeysView, i: u8) -> Point {
    let (r, black) = v.key_rect(i);
    Point::new(r.center().x, if black { r.y0 + 10.0 } else { r.y1 - 6.0 })
}

fn velocity_of(actions: &[UiAction]) -> u8 {
    match actions[0] {
        UiAction::KeysNote { velocity, .. } => velocity,
        _ => 0,
    }
}

#[test]
fn a_typed_key_plays_the_note_above_the_strips_c_and_ends_on_release() {
    let mut v = view();
    assert_eq!(v.low(), 60);
    assert_eq!(v.type_note(&ch('a'), 0), vec![on(60, 98)]);
    assert_eq!(v.type_note(&ch('a'), 0), vec![], "auto-repeat is ignored");
    assert_eq!(v.type_note(&ch('o'), 13), vec![on(73, 98)]);
    assert_eq!(v.type_release(&ch('a')), vec![off(60)]);
    assert_eq!(v.type_release(&ch('A')), vec![], "already released");
    assert_eq!(v.type_release(&ch('o')), vec![off(73)]);
    assert!(v.sounding().is_empty());
}

#[test]
fn octave_and_velocity_steps_stay_in_range() {
    let mut v = view();
    for _ in 0..20 {
        v.shift_octave(1);
    }
    assert_eq!(v.octave(), 9);
    assert_eq!(v.octave_name(), "C8");
    // The top octave still plays its notes (up to key 125).
    assert_eq!(v.type_note(&ch('a'), 0), vec![on(108, 98)]);
    for _ in 0..20 {
        v.shift_octave(-1);
    }
    assert_eq!((v.octave(), v.low()), (0, 0));
    v.set_velocity_step(1);
    assert_eq!(v.velocity(), 1);
    v.set_velocity_step(2);
    assert_eq!(v.velocity(), 14);
    v.set_velocity_step(10);
    assert_eq!(v.velocity(), 127);
    v.set_velocity_step(11);
    assert_eq!(v.velocity(), 127);
    v.set_velocity(98);
    v.step_velocity(true);
    assert_eq!(v.velocity(), 112);
    v.step_velocity(false);
    v.step_velocity(false);
    assert_eq!(v.velocity(), 84);
    v.set_velocity(100);
    v.step_velocity(false);
    assert_eq!(v.velocity(), 98);
}

#[test]
fn the_variation_stays_near_the_velocity_and_in_range() {
    let mut v = view();
    v.set_variation(true);
    v.set_velocity(127);
    let mut seen = BTreeSet::new();
    for i in 0..60u8 {
        let a = v.type_note(&ch('a'), i % 18);
        let velocity = velocity_of(&a);
        assert!((115..=127).contains(&velocity), "{velocity}");
        seen.insert(velocity);
        v.release_all();
    }
    assert!(seen.len() > 3, "it varies: {seen:?}");
}

#[test]
fn shift_is_the_sustain_pedal_of_the_typed_keys() {
    let mut v = view();
    assert_eq!(v.type_note(&ch('a'), 0), vec![on(60, 98)]);
    assert_eq!(v.set_shift(true), vec![]);
    assert!(v.sustaining());
    assert_eq!(v.type_release(&ch('a')), vec![], "sustained");
    assert_eq!(v.sounding(), vec![60]);
    assert_eq!(v.set_shift(false), vec![off(60)]);
    assert!(v.sounding().is_empty());
}

#[test]
fn a_key_struck_again_while_sustained_is_retriggered() {
    let mut v = view();
    v.set_shift(true);
    v.type_note(&ch('a'), 0);
    v.type_release(&ch('a'));
    assert_eq!(v.type_note(&ch('a'), 0), vec![off(60), on(60, 98)]);
    assert_eq!(v.set_shift(false), vec![], "still held");
    assert_eq!(v.type_release(&ch('a')), vec![off(60)]);
}

#[test]
fn losing_the_focus_releases_everything() {
    let mut v = view();
    v.type_note(&ch('a'), 0);
    v.type_note(&ch('s'), 2);
    v.set_shift(true);
    v.type_release(&ch('a'));
    let mut released = v.set_focused(false);
    released.sort_by_key(|a| match a {
        UiAction::KeysNote { key, .. } => *key,
        _ => 0,
    });
    assert_eq!(released, vec![off(60), off(62)]);
    assert!(v.sounding().is_empty());
    assert!(!v.sustaining());
    // The release of the held key arrives somewhere else: nothing is left to end.
    assert_eq!(v.type_release(&ch('s')), vec![]);
    assert_eq!(v.set_focused(false), vec![]);
}

#[test]
fn releasing_all_ends_latched_notes_too() {
    let mut v = view();
    let hold = v.button_rect(KeysButton::Hold).center();
    v.handle(&down(hold));
    assert!(v.holding());
    v.handle(&down(centre(&v, 2)));
    v.handle(&up(centre(&v, 2)));
    assert_eq!(v.sounding(), vec![62]);
    assert_eq!(v.release_all(), vec![off(62)]);
}

#[test]
fn the_pointer_plays_with_a_velocity_from_the_click_height_and_glides() {
    let mut v = view();
    let area = v.keys_area();
    let (r, _) = v.key_rect(0);
    let top = Point::new(r.center().x, area.y0 + 1.0);
    let low = Point::new(r.center().x, area.y1 - 1.0);
    let soft = velocity_of(&v.handle(&down(top)));
    v.handle(&up(top));
    let loud = velocity_of(&v.handle(&down(low)));
    assert!(soft < 40 && loud > 110, "{soft} {loud}");
    // Glide to the next white key: the first ends, the next starts.
    let next = centre(&v, 2);
    let moved = v.handle(&UiEvent::PointerMove {
        pos: next,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[0], off(60));
    assert!(matches!(
        moved[1],
        UiAction::KeysNote {
            key: 62,
            on: true,
            ..
        }
    ));
    assert_eq!(v.handle(&up(next)), vec![off(62)]);
}

#[test]
fn black_keys_win_over_the_white_keys_under_them() {
    let v = view();
    let (r, black) = v.key_rect(1);
    assert!(black);
    let (key, _) = v.key_at(Point::new(r.center().x, r.y0 + 5.0)).unwrap();
    assert_eq!(key, 61);
    let (key, _) = v.key_at(Point::new(r.center().x, r.y1 + 10.0)).unwrap();
    assert!(key == 60 || key == 62);
}

#[test]
fn hold_latches_keys_and_a_second_press_releases_them() {
    let mut v = view();
    let hold = v.button_rect(KeysButton::Hold).center();
    v.handle(&down(hold));
    let p = centre(&v, 4);
    assert!(matches!(
        v.handle(&down(p))[0],
        UiAction::KeysNote {
            key: 64,
            on: true,
            ..
        }
    ));
    assert_eq!(v.handle(&up(p)), vec![]);
    assert_eq!(v.sounding(), vec![64]);
    assert_eq!(v.handle(&down(p)), vec![off(64)]);
    v.handle(&up(p));
    assert!(v.sounding().is_empty());
}

#[test]
fn the_sustain_button_keeps_released_keys_until_it_is_switched_off() {
    let mut v = view();
    let sustain = v.button_rect(KeysButton::Sustain).center();
    v.handle(&down(sustain));
    let p = centre(&v, 0);
    v.handle(&down(p));
    assert_eq!(v.handle(&up(p)), vec![]);
    assert_eq!(v.sounding(), vec![60]);
    assert_eq!(v.handle(&down(sustain)), vec![off(60)]);
}

#[test]
fn switching_the_mode_releases_the_notes() {
    let mut v = view();
    v.type_note(&ch('a'), 0);
    assert_eq!(v.set_mode(KeysMode::Mouse), vec![off(60)]);
    assert!(!v.is_typing());
    assert_eq!(v.set_mode(KeysMode::Mouse), vec![]);
}

#[test]
fn the_buttons_move_the_octave_and_the_velocity() {
    let mut v = view();
    v.handle(&down(v.button_rect(KeysButton::OctaveUp).center()));
    assert_eq!(v.octave(), DEFAULT_OCTAVE + 1);
    v.handle(&down(v.button_rect(KeysButton::VelocityUp).center()));
    assert_eq!(v.velocity(), 112);
    v.handle(&down(v.button_rect(KeysButton::Variation).center()));
    assert!(v.variation());
}
