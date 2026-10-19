//! Headless smoke test: everything the app builds except the window and the GPU. A real
//! `LiveSession` (system backend, audio disabled, so no device is opened), the shell, the demo
//! patch, input events, and painting into both a display list and a vello scene.

use atomik_app::AppCore;
use atomik_app::commands::AppCommand;
use atomik_app::session::{LiveSession, SystemBackend};
use atomik_app::settings::AudioSettings;
use atomik_app::shell::{Shell, ShellOptions};
use atomik_ui::kurbo::{Point, Size};
use atomik_ui::{
    DisplayList, Key, Modifiers, PointerButton, SceneCache, SceneCanvas, TextEngine, UiEvent,
};

#[test]
fn the_app_runs_headless_without_audio() {
    let session = LiveSession::new(SystemBackend, AudioSettings::default(), false);
    let core = AppCore::new(session);
    let mut shell = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
        },
    );
    shell.handle(&UiEvent::ScaleChanged(1.5));
    shell.handle(&UiEvent::Resize(Size::new(1280.0, 860.0)));
    shell.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    )));
    assert_eq!(shell.core().doc().devices.len(), 4);
    assert!(!shell.core().sink().is_running(), "audio stays off");

    // Some input: hover, a click in the rack, a wheel, a flip, an undoable edit.
    for event in [
        UiEvent::PointerMove {
            pos: Point::new(400.0, 300.0),
            modifiers: Modifiers::NONE,
        },
        UiEvent::PointerDown {
            pos: Point::new(400.0, 300.0),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: Point::new(400.0, 300.0),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
        UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: Point::new(400.0, 300.0),
            modifiers: Modifiers::NONE,
        },
        UiEvent::Key {
            key: Key::Tab,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
    ] {
        assert!(shell.handle(&event).is_empty());
    }
    for request in shell.execute(AppCommand::StressRack) {
        // The click may have edited the demo: confirm discarding it.
        assert_eq!(
            request,
            atomik_app::shell::ShellRequest::ConfirmDiscard(AppCommand::StressRack)
        );
        assert!(shell.confirmed(AppCommand::StressRack).is_empty());
    }
    assert_eq!(shell.core().doc().devices.len(), 100);
    shell.frame();
    shell.poll_status();
    assert!(shell.status_text().contains("audio off"));

    let mut list = DisplayList::new();
    shell.paint(&mut list);
    assert!(!list.is_empty());

    let mut scene = vello::Scene::new();
    let mut text = TextEngine::new();
    let mut cache = SceneCache::new();
    {
        let mut canvas = SceneCanvas::new(&mut scene, &mut text).with_cache(&mut cache);
        shell.paint(&mut canvas);
    }
    cache.trim();
    assert!(!cache.is_empty(), "panels are cached as encoded scenes");
    assert_eq!(shell.physical_size(), (1920, 1290));
}

/// The app as `atomik` with no file starts it: the starter project, Audio I/O and the
/// locked master, painted; File, New makes the same project again; nothing opens a device.
#[test]
fn the_app_starts_with_the_starter_project_and_paints_it() {
    use clap::Parser;
    let cli = atomik_app::cli::Cli::try_parse_from(["atomik", "--no-audio"]).unwrap();
    let doc = cli.start_document(&atomik_app::app_core::default_registry());
    let session = LiveSession::new(SystemBackend, AudioSettings::default(), false);
    let core = AppCore::with_doc(session, doc, None);
    let mut shell = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
        },
    );
    shell.handle(&UiEvent::Resize(Size::new(1280.0, 860.0)));
    assert_eq!(shell.core().doc().devices.len(), 1, "the master");
    assert!(shell.core().doc().console.master.is_some());
    assert_eq!(shell.core().doc().cables.len(), 2, "main to out 1 and 2");
    assert!(!shell.core().is_dirty());
    shell.frame();
    let mut list = DisplayList::new();
    shell.paint(&mut list);
    assert!(!list.is_empty());
    assert!(shell.confirmed(AppCommand::New).is_empty());
    assert_eq!(shell.core().doc().devices.len(), 1);
}
