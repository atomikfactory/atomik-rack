//! `Shell` test for the BYPASS switch of a built-in effect's name strip: a click in the
//! window sets the reverb's `bypass` param, one undo step restores it.

use super::*;
use crate::session::NullBackend;
use atomik_ui::{Modifiers, PointerButton};

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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn bypass(s: &Shell<NullBackend>) -> f32 {
    s.core()
        .doc()
        .device(&key("rev"))
        .unwrap()
        .params
        .get("bypass")
        .copied()
        .unwrap_or(0.0)
}

fn click_switch(s: &mut Shell<NullBackend>) {
    let r = s
        .core()
        .view()
        .bypass_button_rect(&key("rev"))
        .expect("the reverb has a BYPASS switch");
    let at = Point::new(r.center().x, r.center().y + s.layout().rack.y0);
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn clicking_the_strip_switch_of_a_reverb_bypasses_it_and_undo_restores_it() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.reverb".into(),
            key: Some(key("rev")),
        })
        .unwrap();
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    assert_eq!(bypass(&s), 0.0);
    click_switch(&mut s);
    assert_eq!(bypass(&s), 1.0);
    // A second click switches the effect back in.
    click_switch(&mut s);
    assert_eq!(bypass(&s), 0.0);
    click_switch(&mut s);
    assert_eq!(bypass(&s), 1.0);
    assert!(s.core_mut().undo());
    assert_eq!(bypass(&s), 0.0, "one undo step per click");
    assert!(s.core_mut().redo());
    assert_eq!(bypass(&s), 1.0);
}
