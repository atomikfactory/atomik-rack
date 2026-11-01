//! The jack pop-up menu through the shell: a right click on a jack of the back view, the rows
//! it lists and what choosing one does (one cable, one undo step).

use super::*;
use crate::session::NullBackend;
use atomik_model::{Cable, Command, DeviceKey, SocketRef};
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
            library_root: None,
        },
    );
    s.handle(&UiEvent::Resize(Size::new(1400.0, 5000.0)));
    for (t, k) in [
        ("atomik.poly-synth", "synth"),
        ("atomik.stereo-delay", "delay"),
    ] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: t.into(),
                key: Some(DeviceKey(k.into())),
            })
            .unwrap();
    }
    s.execute(AppCommand::ToggleFlip);
    s
}

fn sock(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: DeviceKey(d.into()),
        socket: s.into(),
    }
}

fn mouse(s: &mut Shell<NullBackend>, p: Point, button: PointerButton) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
    });
}

fn window_point(s: &Shell<NullBackend>, jack: &SocketRef) -> Point {
    let o = s.layout().rack.origin();
    let p = s.core().view().socket_center(jack).unwrap();
    Point::new(p.x + o.x, p.y + o.y)
}

fn cable_count(s: &Shell<NullBackend>) -> usize {
    s.core().doc().cables.len()
}

#[test]
fn a_right_click_on_a_jack_opens_its_menu_and_a_choice_cables_in_one_undo_step() {
    let mut s = shell();
    let base = cable_count(&s);
    let at = window_point(&s, &sock("synth", "out_l"));
    mouse(&mut s, at, PointerButton::Secondary);
    let menu = s.context_menu().expect("the jack menu");
    assert!(menu.submenu_center("Connect to").is_some());
    assert!(menu.entries().iter().any(|e| matches!(
        e,
        atomik_ui::context_menu::Entry::Item(i) if i.label == "New Mixer Channel"
    )));
    // Open the submenu of the delay and choose its left input.
    let connect = menu.submenu_center("Connect to").unwrap();
    mouse(&mut s, connect, PointerButton::Primary);
    let menu = s.context_menu().unwrap();
    let delay = menu
        .submenu_center("Stereo Delay")
        .expect("a row per device");
    mouse(&mut s, delay, PointerButton::Primary);
    let arg = atomik_ui::JackOp::Connect(Cable {
        from: sock("synth", "out_l"),
        to: sock("delay", "in_l"),
    })
    .encode();
    let row = s
        .context_menu()
        .unwrap()
        .item_center_with("rack.jack", &arg)
        .expect("the delay's left input");
    mouse(&mut s, row, PointerButton::Primary);
    assert!(s.context_menu().is_none());
    assert_eq!(cable_count(&s), base + 1);
    assert!(s.core_mut().undo());
    assert_eq!(cable_count(&s), base);
}

#[test]
fn the_menu_of_a_cabled_jack_can_disconnect_it() {
    let mut s = shell();
    let base = cable_count(&s);
    s.core_mut()
        .apply_command(Command::Connect(Cable {
            from: sock("synth", "out_l"),
            to: sock("delay", "in_l"),
        }))
        .unwrap();
    let at = window_point(&s, &sock("delay", "in_l"));
    mouse(&mut s, at, PointerButton::Secondary);
    let menu = s.context_menu().expect("the jack menu");
    let arg = atomik_ui::JackOp::Disconnect(sock("delay", "in_l")).encode();
    let row = menu
        .item_center_with("rack.jack", &arg)
        .expect("Disconnect is offered");
    mouse(&mut s, row, PointerButton::Primary);
    assert_eq!(cable_count(&s), base);
    assert!(s.core_mut().undo());
    assert_eq!(cable_count(&s), base + 1);
}
