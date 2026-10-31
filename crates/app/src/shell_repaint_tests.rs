//! What a pointer move repaints: nothing when it changes nothing the user can see, one paint
//! per change of hover, and only the window it happened in. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use crate::windows::WindowKind;
use atomik_model::{ClipContent, ClipId, Command, DeviceKey, SongCommand};
use atomik_ui::{DisplayList, Modifiers};

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
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    let core = s.core_mut();
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(DeviceKey("a".into())),
    })
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddTrack {
        device: DeviceKey("a".into()),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: DeviceKey("a".into()),
        lane: 0,
        clip: atomik_model::Clip {
            id: ClipId(0),
            start: 0,
            length: 4 * atomik_core::PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(Vec::new().into()),
        },
    }))
    .unwrap();
    s
}

/// Paints every open window, as the platform does, so none is left marked.
fn settle(s: &mut Shell<NullBackend>) {
    for kind in [WindowKind::Main, WindowKind::Rack, WindowKind::Console] {
        s.paint_in(kind, &mut DisplayList::new());
    }
}

fn moved(x: f64, y: f64) -> UiEvent {
    UiEvent::PointerMove {
        pos: Point::new(x, y),
        modifiers: Modifiers::NONE,
    }
}

/// Moves to `(x, y)` in `window` and says which windows now have something to paint.
fn move_to(s: &mut Shell<NullBackend>, window: WindowKind, x: f64, y: f64) -> Vec<WindowKind> {
    s.handle_in(window, &moved(x, y));
    s.windows_to_paint()
}

/// A grid of pointer positions over a window of `w` by `h` logical units.
fn grid(w: f64, h: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut y = 3.0;
    while y < h {
        let mut x = 3.0;
        while x < w {
            out.push((x, y));
            x += 23.0;
        }
        y += 19.0;
    }
    out
}

#[test]
fn a_move_to_where_the_pointer_already_is_paints_nothing_in_every_pane() {
    let mut s = shell();
    settle(&mut s);
    let mut paints = 0;
    for (x, y) in grid(1200.0, 800.0) {
        // The first move may change a hover and so paint once; the same move again must not.
        let first = move_to(&mut s, WindowKind::Main, x, y);
        if !first.is_empty() {
            paints += 1;
        }
        settle(&mut s);
        let again = move_to(&mut s, WindowKind::Main, x, y);
        assert!(
            again.is_empty(),
            "a no-op move at ({x}, {y}) paints {again:?}"
        );
    }
    assert!(paints > 0, "hovering something does paint");
}

#[test]
fn a_sweep_over_nothing_paints_nothing_once_the_hover_has_settled() {
    let mut s = shell();
    settle(&mut s);
    // Far from every control: the empty corner of the song area.
    let (x, y) = (1190.0, 790.0);
    move_to(&mut s, WindowKind::Main, x, y);
    settle(&mut s);
    for i in 0..50 {
        let painted = move_to(
            &mut s,
            WindowKind::Main,
            x - f64::from(i % 3),
            y - f64::from(i % 2),
        );
        assert!(painted.is_empty(), "move {i} paints {painted:?}");
    }
}

#[test]
fn a_move_in_one_window_does_not_mark_the_others() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    settle(&mut s);
    // A sweep of the main window never marks the rack or the console windows.
    for (x, y) in grid(1200.0, 800.0) {
        let painted = move_to(&mut s, WindowKind::Main, x, y);
        assert!(
            !painted.contains(&WindowKind::Rack) && !painted.contains(&WindowKind::Console),
            "a move over the main window marks {painted:?} at ({x}, {y})"
        );
        settle(&mut s);
    }
}

#[test]
fn a_move_in_the_detached_rack_window_marks_only_that_window() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    settle(&mut s);
    let size = s.window_size(WindowKind::Rack).unwrap();
    let mut rack_paints = 0;
    for (x, y) in grid(size.width, size.height) {
        let painted = move_to(&mut s, WindowKind::Rack, x, y);
        assert!(
            !painted.contains(&WindowKind::Main),
            "a move over the rack window marks {painted:?} at ({x}, {y})"
        );
        rack_paints += usize::from(painted.contains(&WindowKind::Rack));
        settle(&mut s);
        let again = move_to(&mut s, WindowKind::Rack, x, y);
        assert!(
            again.is_empty(),
            "a no-op move at ({x}, {y}) paints {again:?}"
        );
    }
    assert!(rack_paints > 0, "hovering a control in the rack paints it");
}
