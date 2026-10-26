//! Help tooltips in the shell: the hover delay, the shortcut read from the keymap (so a rebound
//! key shows), hiding on a press, the wheel or a key, repainting only when the tooltip changes,
//! and the "Show tooltips" preference (saved in `workflow.json`, suppresses them).

use super::*;
use crate::keymap_store::{KEYMAP_FILE, WORKFLOW_FILE};
use crate::session::NullBackend;
use crate::settings_dialog::Tab;
use atomik_ui::kurbo::Vec2;
use atomik_ui::{DisplayList, HELP_DELAY, TransportButton};
use std::path::PathBuf;

const VIEW: Size = Size::new(1000.0, 800.0);

fn config(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-tooltip-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_in(dir: &Path) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: Some(atomik_ui::keymap::Profile::Standard),
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(VIEW));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_in(&config("default"))
}

fn at(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    });
}

fn button(s: &Shell<NullBackend>, b: TransportButton) -> Point {
    s.core().transport_bar().button_center(b)
}

fn press_at(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
}

fn release_at(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn paint(s: &mut Shell<NullBackend>) -> DisplayList {
    let mut list = DisplayList::new();
    s.paint(&mut list);
    list
}

/// Moves to `p` at time `t0`, lets the delay pass and returns the time it is now.
fn rest_on(s: &mut Shell<NullBackend>, p: Point, t0: f64) -> f64 {
    at(s, p);
    s.frame_at(t0);
    let later = t0 + HELP_DELAY + 0.05;
    s.frame_at(later);
    later
}

#[test]
fn a_tooltip_shows_after_the_delay_and_names_the_shortcut() {
    let mut s = shell();
    let play = button(&s, TransportButton::Play);
    at(&mut s, play);
    assert!(s.tooltip_pending());
    s.frame_at(1.0);
    s.frame_at(1.0 + HELP_DELAY - 0.1);
    assert_eq!(s.tooltip(), None, "not before the delay");
    s.frame_at(1.0 + HELP_DELAY + 0.05);
    let tip = s.tooltip().expect("shown after the delay");
    assert_eq!(tip.window, WindowKind::Main);
    assert!(tip.text.starts_with("Play"), "{}", tip.text);
    assert!(tip.text.ends_with("(Space)"), "{}", tip.text);
    assert!(tip.rect.contains(play));
    assert!(!s.tooltip_pending(), "nothing left to wait for");
}

#[test]
fn the_tooltip_shows_the_rebound_shortcut() {
    let dir = config("rebound");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.play_stop", "context": "global", "chords": ["Cmd+Alt+P"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&dir);
    let play = button(&s, TransportButton::Play);
    rest_on(&mut s, play, 1.0);
    let tip = s.tooltip().expect("shown");
    assert!(tip.text.ends_with("(Ctrl+Alt+P)"), "{}", tip.text);
    // A command the user unbound shows no shortcut at all.
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.play_stop", "context": "global", "chords": []}]}"#,
    )
    .unwrap();
    assert!(s.reload_keymap_file());
    let stop = button(&s, TransportButton::Stop);
    at(&mut s, stop);
    rest_on(&mut s, play, 10.0);
    let tip = s.tooltip().expect("shown again");
    assert!(!tip.text.contains('('), "no chord: {}", tip.text);
}

#[test]
fn a_press_the_wheel_a_key_or_leaving_hides_it() {
    let mut s = shell();
    let play = button(&s, TransportButton::Play);
    let stop = button(&s, TransportButton::Stop);
    let t = rest_on(&mut s, play, 1.0);
    assert!(s.tooltip().is_some());
    // The wheel.
    s.handle(&UiEvent::Wheel {
        delta: Vec2::new(0.0, -1.0),
        pos: play,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.tooltip(), None);
    s.frame_at(t + 5.0);
    assert_eq!(
        s.tooltip(),
        None,
        "hidden while the pointer stays on the button"
    );
    // Another button shows its own.
    let t = rest_on(&mut s, stop, t + 10.0);
    assert!(s.tooltip().is_some_and(|t| t.text.starts_with("Stop")));
    // A key.
    s.handle(&UiEvent::Key {
        key: Key::Char('x'),
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!(s.tooltip(), None);
    // A press.
    let t = rest_on(&mut s, play, t + 20.0);
    assert!(s.tooltip().is_some());
    press_at(&mut s, play);
    assert_eq!(s.tooltip(), None);
    release_at(&mut s, play);
    // Moving about on the button it was pressed on does not bring it back.
    at(&mut s, play);
    at(&mut s, play + Vec2::new(1.0, 0.0));
    s.frame_at(t + 5.0);
    s.frame_at(t + 10.0);
    assert_eq!(s.tooltip(), None);
    // Leaving the window drops it.
    let _ = rest_on(&mut s, stop, t + 10.0);
    assert!(s.tooltip().is_some());
    s.handle(&UiEvent::PointerLeave);
    assert_eq!(s.tooltip(), None);
    assert!(!s.tooltip_pending());
}

#[test]
fn moving_off_the_button_hides_it_and_a_gap_waits_for_nothing() {
    let mut s = shell();
    let play = button(&s, TransportButton::Play);
    rest_on(&mut s, play, 1.0);
    assert!(s.tooltip().is_some());
    at(&mut s, Point::new(990.0, 790.0));
    assert_eq!(s.tooltip(), None);
    assert!(!s.tooltip_pending(), "no button there: nothing to wait for");
}

#[test]
fn it_repaints_only_when_the_tooltip_appears_or_goes_away() {
    let mut s = shell();
    for t in [0.0, 0.5, 1.0] {
        s.frame_at(t);
        paint(&mut s);
    }
    s.frame_at(1.5);
    assert!(
        !s.needs_repaint(),
        "an idle shell does not repaint on ticks"
    );
    let play = button(&s, TransportButton::Play);
    at(&mut s, play);
    paint(&mut s);
    // The delay runs: no repaint on the ticks.
    s.frame_at(2.0);
    s.frame_at(2.0 + HELP_DELAY * 0.5);
    assert!(!s.needs_repaint(), "waiting repaints nothing");
    // It appears: one repaint, and the canvas has the text.
    s.frame_at(2.0 + HELP_DELAY + 0.05);
    assert!(s.needs_repaint());
    let list = paint(&mut s);
    assert!(
        list.texts().iter().any(|t| t.starts_with("Play, or stop")),
        "{:?}",
        list.texts()
    );
    assert!(!s.needs_repaint());
    // Ticking on does nothing.
    s.frame_at(9.0);
    s.frame_at(10.0);
    assert!(!s.needs_repaint());
    // It goes away: one repaint without the text.
    s.handle(&UiEvent::PointerLeave);
    assert!(s.needs_repaint());
    let list = paint(&mut s);
    assert!(!list.texts().iter().any(|t| t.starts_with("Play, or stop")));
}

#[test]
fn the_tooltip_box_stays_inside_the_window() {
    let mut s = shell();
    let view = Rect::new(0.0, 0.0, VIEW.width, VIEW.height);
    for b in [
        TransportButton::ReturnToZero,
        TransportButton::Metronome,
        TransportButton::Loop,
    ] {
        let p = button(&s, b);
        rest_on(&mut s, p, 1.0);
        let tip = s.tooltip().expect("shown");
        let boxed = atomik_ui::tooltip::box_for(tip.rect, &tip.text, view);
        assert!(boxed.x0 >= 0.0 && boxed.y0 >= 0.0, "{boxed:?}");
        assert!(boxed.x1 <= view.x1 && boxed.y1 <= view.y1, "{boxed:?}");
        at(&mut s, Point::new(990.0, 790.0));
    }
}

#[test]
fn off_in_workflow_json_shows_no_tooltip() {
    let dir = config("off");
    std::fs::write(dir.join(WORKFLOW_FILE), r#"{"show_tooltips": false}"#).unwrap();
    let mut s = shell_in(&dir);
    assert!(!s.tooltips_enabled());
    let play = button(&s, TransportButton::Play);
    at(&mut s, play);
    assert!(!s.tooltip_pending());
    s.frame_at(1.0);
    s.frame_at(5.0);
    assert_eq!(s.tooltip(), None);
}

#[test]
fn the_setting_is_on_by_default_saved_by_apply_and_switches_a_shown_tooltip_off() {
    let dir = config("setting");
    let mut s = shell_in(&dir);
    assert!(s.tooltips_enabled(), "on by default");
    let play = button(&s, TransportButton::Play);
    rest_on(&mut s, play, 1.0);
    assert!(s.tooltip().is_some());
    // Settings > Workflow > Show tooltips.
    s.execute(AppCommand::AudioSettings);
    let tab = s
        .dialog()
        .unwrap()
        .layout(VIEW)
        .tabs
        .iter()
        .find(|(_, t)| *t == Tab::Workflow)
        .unwrap()
        .0;
    let click = |s: &mut Shell<NullBackend>, p: Point| {
        press_at(s, p);
        release_at(s, p);
    };
    click(&mut s, tab.center());
    let layout = s.dialog().unwrap().workflow_layout(VIEW).unwrap();
    let toggle = layout
        .toggles
        .iter()
        .find(|t| t.label == "Show tooltips")
        .expect("the toggle is in the Workflow tab");
    assert!(toggle.on);
    click(&mut s, toggle.rect.center());
    let apply = s.dialog().unwrap().buttons(VIEW).0;
    click(&mut s, apply.center());
    assert!(!s.dialog_open());
    assert!(!s.tooltips_enabled());
    assert_eq!(s.tooltip(), None, "a shown tooltip goes with the setting");
    let text = std::fs::read_to_string(dir.join(WORKFLOW_FILE)).unwrap();
    assert!(text.contains("\"show_tooltips\": false"), "{text}");
    // It stays off while hovering, and across a restart.
    let t = rest_on(&mut s, play, 5.0);
    assert_eq!(s.tooltip(), None);
    s.frame_at(t + 5.0);
    assert_eq!(s.tooltip(), None);
    let mut again = shell_in(&dir);
    assert!(!again.tooltips_enabled());
    rest_on(&mut again, play, 1.0);
    assert_eq!(again.tooltip(), None);
}

#[test]
fn the_value_readout_of_a_drag_stays_when_tooltips_are_off() {
    let dir = config("drag");
    std::fs::write(dir.join(WORKFLOW_FILE), r#"{"show_tooltips": false}"#).unwrap();
    let mut s = shell_in(&dir);
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    s.handle(&UiEvent::Resize(VIEW));
    let origin = s.layout().rack.origin().to_vec2();
    let doc = s.core().doc().clone();
    let (key, p) = doc
        .devices
        .iter()
        .find_map(|d| {
            d.params.keys().find_map(|k| {
                let p = s.core().view().control_center(&d.key, k)?;
                Some((d.key.clone(), p + origin))
            })
        })
        .expect("a device with a control on the rack in the demo");
    let _ = key;
    press_at(&mut s, p);
    at(&mut s, p - Vec2::new(0.0, 20.0));
    assert!(
        s.core().view().tooltip().is_some(),
        "the value readout shows while dragging"
    );
    release_at(&mut s, p);
}
