//! The wavetable editor through the shell: a click on a table display opens it, a stroke stores
//! the table in one undo step, Escape cancels a stroke and then closes the panel, CLOSE closes
//! it, and undo while it is open shows the table before. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_model::empty_doc;
use atomik_ui::{Key, PointerButton};

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::with_registry(
        session,
        atomik_render::default_registry(),
        empty_doc(),
        None,
    );
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.wavetable-synth".into(),
        key: Some(key("wt")),
    })
    .unwrap();
    let mut s = Shell::new(
        core,
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
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    s
}

fn press(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Default::default(),
        clicks: 1,
    });
}

fn release(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Default::default(),
    });
}

fn escape(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Default::default(),
        pressed: true,
    });
}

fn zones(s: &Shell<NullBackend>) -> usize {
    s.core().doc().device(&key("wt")).unwrap().zones.len()
}

#[test]
fn a_table_display_action_opens_the_editor_on_that_device() {
    let mut s = shell();
    assert!(!s.wavetable_open());
    s.pane_actions(&[UiAction::EditWavetable(key("wt"))]);
    assert!(s.wavetable_open());
    assert!(s.modal_open());
    let view = s.wavetable_view().unwrap();
    assert_eq!(
        view.keys().len(),
        4,
        "the table oscillator 1 shows, as a start"
    );
    assert!(!view.data().stored);
    // A device that is no wavetable synth opens nothing.
    s.wavetable = None;
    s.pane_actions(&[UiAction::EditWavetable(key("nothing"))]);
    assert!(!s.wavetable_open());
}

#[test]
fn a_stroke_stores_the_table_in_one_undo_step_and_escape_cancels_a_stroke_first() {
    let mut s = shell();
    s.open_wavetable_editor(key("wt"));
    let depth = s.core().document().undo_depth();
    let canvas = s.wavetable_view().unwrap().canvas_rect();
    let a = Point::new(canvas.x0 + 40.0, canvas.y0 + 20.0);
    let b = Point::new(canvas.x0 + 300.0, canvas.y0 + 140.0);
    // A stroke Escape cancels: nothing is stored and the panel stays.
    press(&mut s, a);
    s.handle(&UiEvent::PointerMove {
        pos: b,
        modifiers: Default::default(),
    });
    assert!(s.wavetable_view().unwrap().dragging());
    escape(&mut s);
    assert!(s.wavetable_open());
    assert!(!s.wavetable_view().unwrap().dragging());
    release(&mut s, b);
    assert_eq!(zones(&s), 0);
    assert_eq!(s.core().document().undo_depth(), depth);
    // A stroke that is released stores the table.
    press(&mut s, a);
    s.handle(&UiEvent::PointerMove {
        pos: b,
        modifiers: Default::default(),
    });
    release(&mut s, b);
    assert_eq!(zones(&s), 1);
    assert_eq!(s.core().document().undo_depth(), depth + 1);
    let view = s.wavetable_view().unwrap();
    assert!(view.data().stored);
    assert_eq!(view.keys().len(), 4);
    // Undo while the editor is open shows the table before.
    assert!(s.core_mut().undo());
    s.frame_at(0.1);
    assert_eq!(zones(&s), 0);
    assert!(!s.wavetable_view().unwrap().data().stored);
    // Escape closes the panel.
    escape(&mut s);
    assert!(!s.wavetable_open());
}

#[test]
fn close_and_removing_the_device_close_the_editor() {
    let mut s = shell();
    s.open_wavetable_editor(key("wt"));
    let close = s.wavetable_view().unwrap().close_rect().center();
    press(&mut s, close);
    release(&mut s, close);
    assert!(!s.wavetable_open());
    s.open_wavetable_editor(key("wt"));
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("wt") })
        .unwrap();
    s.frame_at(0.2);
    assert!(!s.wavetable_open());
}

#[test]
fn the_panel_has_tooltips_and_follows_the_scale() {
    let mut s = shell();
    s.open_wavetable_editor(key("wt"));
    let at = s.wavetable_view().unwrap().safe_rect().center();
    let help = s.wavetable_view().unwrap().help_at(at);
    assert!(help.is_some_and(|h| h.text.contains("Align")));
    s.handle(&UiEvent::ScaleChanged(2.0));
    assert!(s.wavetable_open());
}
