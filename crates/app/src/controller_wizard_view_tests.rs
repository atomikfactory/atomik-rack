use super::*;
use crate::surface::LoopbackSurface;
use atomik_ui::Modifiers;

const VIEW: Size = Size::new(1000.0, 700.0);

fn ports() -> ControllerWizardView {
    ControllerWizardView::new(
        vec!["Desk In".to_owned(), "Other".to_owned()],
        vec!["Desk Out".to_owned()],
    )
}

fn learning() -> (ControllerWizardView, LoopbackSurface) {
    let mut v = ports();
    let pad = LoopbackSurface::new();
    v.begin(Box::new(pad.clone()), "Desk In");
    (v, pad)
}

fn click(v: &mut ControllerWizardView, p: Point) -> WizardOutcome {
    v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        VIEW,
    )
}

fn key(v: &mut ControllerWizardView, k: Key) -> WizardOutcome {
    v.handle(
        &UiEvent::Key {
            key: k,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        VIEW,
    )
}

fn button(v: &ControllerWizardView, t: Target) -> Point {
    v.buttons(VIEW)
        .into_iter()
        .find(|(x, ..)| *x == t)
        .map(|(_, r, _)| r.center())
        .unwrap_or_else(|| panic!("no {t:?} button"))
}

fn turn_knob(pad: &LoopbackSurface, v: &mut ControllerWizardView, number: u8) {
    for value in (0..=127u8).step_by(9) {
        pad.feed(&[0xB0, number, value]);
    }
    assert!(v.poll());
}

#[test]
fn the_port_list_picks_a_port_by_click_or_keys() {
    let mut v = ports();
    assert!(!v.learning());
    let (_, row) = v.port_rows(VIEW)[1];
    assert_eq!(
        click(&mut v, row.center()),
        WizardOutcome::PickPort("Other".to_owned())
    );
    assert_eq!(key(&mut v, Key::Down), WizardOutcome::Open);
    assert_eq!(
        key(&mut v, Key::Enter),
        WizardOutcome::PickPort("Other".to_owned())
    );
    key(&mut v, Key::Up);
    assert_eq!(
        key(&mut v, Key::Enter),
        WizardOutcome::PickPort("Desk In".to_owned())
    );
    assert_eq!(key(&mut v, Key::Escape), WizardOutcome::Close);
}

#[test]
fn a_click_outside_the_box_or_on_cancel_closes_it() {
    let mut v = ports();
    assert_eq!(click(&mut v, Point::new(2.0, 2.0)), WizardOutcome::Close);
    let cancel = button(&v, Target::Cancel);
    assert_eq!(click(&mut v, cancel), WizardOutcome::Close);
}

#[test]
fn no_ports_says_so_and_a_failed_port_stays_on_the_list() {
    let v = ControllerWizardView::new(Vec::new(), Vec::new());
    assert!(
        v.lines()
            .iter()
            .any(|l| l.starts_with("No MIDI input ports"))
    );
    let mut v = ports();
    v.fail("Desk In: busy".to_owned());
    assert!(!v.learning());
    assert!(v.lines().contains(&"Desk In: busy".to_owned()));
}

#[test]
fn the_output_port_of_a_controller_is_found_by_name() {
    let outs = vec!["Desk Out".to_owned(), "Synth".to_owned()];
    assert_eq!(matching_output("Synth", &outs).as_deref(), Some("Synth"));
    assert_eq!(matching_output("Synth 2", &outs).as_deref(), Some("Synth"));
    assert_eq!(matching_output("Desk", &outs).as_deref(), Some("Desk Out"));
    assert_eq!(matching_output("Pad", &outs), None);
    let both = vec!["Desk A".to_owned(), "Desk B".to_owned()];
    assert_eq!(
        matching_output("Desk", &both),
        None,
        "two fit: none is guessed"
    );
}

#[test]
fn the_wizard_learns_from_the_port_and_the_screen_follows() {
    let (mut v, pad) = learning();
    assert!(v.learning());
    assert_eq!(v.wizard().unwrap().output(), Some("Desk Out"));
    let lines = v.lines();
    assert!(lines.contains(&"Move knob 1".to_owned()), "{lines:?}");
    turn_knob(&pad, &mut v, 21);
    let lines = v.lines();
    assert!(lines.contains(&"Move knob 2".to_owned()), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("Heard: CC 21")));
    assert!(lines.contains(&"1 controls set up".to_owned()));
    // Messages that are not channel messages are dropped.
    pad.feed(&[0xF8]);
    assert!(!v.poll());
}

#[test]
fn skip_back_and_finish_work_from_the_buttons_and_keys() {
    let (mut v, pad) = learning();
    turn_knob(&pad, &mut v, 21);
    let skip = button(&v, Target::Skip);
    click(&mut v, skip);
    assert_eq!(v.wizard().unwrap().prompt(), "Move knob 3");
    key(&mut v, Key::Backspace);
    assert_eq!(v.wizard().unwrap().prompt(), "Move knob 2");
    key(&mut v, Key::Char('s'));
    assert_eq!(v.wizard().unwrap().prompt(), "Move knob 3");
    let group = button(&v, Target::SkipGroup);
    click(&mut v, group);
    assert_eq!(v.wizard().unwrap().prompt(), "Move fader 1");
    let finish = button(&v, Target::Finish);
    click(&mut v, finish);
    // The controller has an output: the knob question comes first.
    assert_eq!(
        v.wizard().unwrap().stage(),
        Stage::Ask(crate::controller_wizard::Group::Knobs)
    );
    key(&mut v, Key::Char('y'));
    assert_eq!(v.wizard().unwrap().stage(), Stage::Done);
    assert!(v.lines().contains(&"Name: Desk In".to_owned()));
}

#[test]
fn back_at_the_first_step_returns_to_the_port_list() {
    let (mut v, _pad) = learning();
    let back = button(&v, Target::Back);
    assert_eq!(click(&mut v, back), WizardOutcome::Open);
    assert!(!v.learning());
}

#[test]
fn save_hands_over_the_profile_and_refuses_an_empty_one() {
    let (mut v, pad) = learning();
    let finish = button(&v, Target::Finish);
    click(&mut v, finish);
    assert_eq!(v.wizard().unwrap().stage(), Stage::Done);
    let save = button(&v, Target::Save);
    assert_eq!(click(&mut v, save), WizardOutcome::Open);
    assert!(v.lines().iter().any(|l| l.contains("nothing to save")));

    let (mut v, pad2) = learning();
    drop(pad);
    turn_knob(&pad2, &mut v, 21);
    let finish = button(&v, Target::Finish);
    click(&mut v, finish);
    key(&mut v, Key::Char('n'));
    let WizardOutcome::Save(p) = key(&mut v, Key::Enter) else {
        panic!("not saved");
    };
    assert_eq!(p.name, "Desk In");
    assert_eq!(p.input, "Desk In");
    assert_eq!(p.output.as_deref(), Some("Desk Out"));
    assert_eq!(p.controls.len(), 1);
}

#[test]
fn the_box_paints_in_both_phases() {
    let theme = Theme::default();
    let mut canvas = atomik_ui::DisplayList::new();
    ports().paint(&mut canvas, &theme, VIEW);
    let (v, _) = learning();
    v.paint(&mut canvas, &theme, VIEW);
}

#[test]
fn a_profile_name_comes_from_the_port() {
    assert_eq!(profile_name("  Desk In "), "Desk In");
    assert_eq!(profile_name("***"), "Controller");
    assert_eq!(profile_name(&"x".repeat(200)).chars().count(), 64);
}
