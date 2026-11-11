//! The kick and bass check through the shell: the Mixer menu item, the panel opening and closing,
//! running the check from its button and applying a fix from its row. Headless (null audio
//! backend).

use super::*;
use crate::kick_bass::tests::scene;
use crate::session::NullBackend;
use atomik_model::{RackDoc, save_project};
use atomik_render::default_registry;
use atomik_ui::{Key, PointerButton};
use std::time::Duration;

fn shell_with(doc: RackDoc) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::with_registry(session, default_registry(), doc, None),
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

fn click(s: &mut Shell<NullBackend>, at: Point) {
    s.handle(&UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Default::default(),
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Default::default(),
    });
}

fn wait(s: &mut Shell<NullBackend>) {
    let mut waited = 0;
    let mut t = 0.0;
    while s.core().kick_bass_running() {
        waited += 1;
        assert!(waited < 6_000, "the check did not finish");
        std::thread::sleep(Duration::from_millis(20));
        t += 0.02;
        s.frame_at(t);
    }
    s.frame_at(t + 1.0);
}

#[test]
fn the_mixer_menu_has_the_check_and_it_opens_the_panel() {
    let mut s = shell_with(scene(180.0));
    let item = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Analyze")
        .and_then(|m| {
            m.items()
                .find(|i| i.command == AppCommand::KickBassCheck)
                .cloned()
        })
        .expect("Kick and Bass Check in the Mixer menu");
    assert!(item.enabled);
    assert!(!s.kick_bass_open());
    s.execute(AppCommand::KickBassCheck);
    assert!(s.kick_bass_open());
    // The names pick the two tracks.
    let info = s.kick_bass_view().unwrap().info().clone();
    assert_eq!(info.tracks, ["Kick Drum", "Sub Bass"]);
    assert_eq!((info.kick, info.bass), (Some(0), Some(1)));
    assert!(s.modal_open());
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Default::default(),
        pressed: true,
    });
    assert!(!s.kick_bass_open());
    // CLOSE closes it too.
    s.execute(AppCommand::KickBassCheck);
    let close = s.kick_bass_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.kick_bass_open());
}

#[test]
fn the_check_button_runs_the_check_and_an_apply_button_fixes_the_song_in_one_step() {
    let mut s = shell_with(scene(180.0));
    s.execute(AppCommand::KickBassCheck);
    let run = s.kick_bass_view().unwrap().run_rect().center();
    click(&mut s, run);
    assert!(s.core().kick_bass_running());
    // While it runs the button cancels, and the song is untouched.
    assert!(matches!(
        s.kick_bass_view().unwrap().info().run,
        atomik_ui::RunView::Running(_)
    ));
    wait(&mut s);
    let info = s.kick_bass_view().unwrap().info().clone();
    assert_eq!(info.run, atomik_ui::RunView::Done);
    assert!(!info.fixes.is_empty(), "{info:?}");
    assert!(info.summary.contains("cancel"), "{}", info.summary);
    let before = save_project(s.core().doc());
    let apply = s.kick_bass_view().unwrap().apply_rect(0).center();
    click(&mut s, apply);
    assert_ne!(save_project(s.core().doc()), before);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("Kick and Bass: ")),
        "{:?}",
        s.core().last_rejection()
    );
    wait(&mut s);
    assert!(s.core_mut().undo());
    assert_eq!(save_project(s.core().doc()), before);
}

#[test]
fn a_song_without_tracks_opens_an_empty_panel_that_explains_what_to_pick() {
    let mut s = shell_with(atomik_model::new_project_doc());
    s.execute(AppCommand::KickBassCheck);
    let run = s.kick_bass_view().unwrap().run_rect().center();
    click(&mut s, run);
    assert!(!s.core().kick_bass_running());
    let info = s.kick_bass_view().unwrap().info();
    assert!(matches!(&info.run, atomik_ui::RunView::Failed(w) if w.contains("Pick")));
}

#[test]
fn the_panel_shows_the_result_as_soon_as_the_check_ends_without_another_input() {
    let mut s = shell_with(scene(180.0));
    s.execute(AppCommand::KickBassCheck);
    let run = s.kick_bass_view().unwrap().run_rect().center();
    click(&mut s, run);
    // The platform loop ticks while the app is busy and then paints what the last frame left:
    // that last frame must already hold the result, not the 100 % of the render before it.
    let mut waited = 0;
    let mut t = 0.0;
    while s.is_busy() {
        waited += 1;
        assert!(waited < 6_000, "the check did not finish");
        std::thread::sleep(Duration::from_millis(20));
        t += 0.02;
        s.frame_at(t);
    }
    assert_eq!(
        s.kick_bass_view().unwrap().info().run,
        atomik_ui::RunView::Done
    );
}
