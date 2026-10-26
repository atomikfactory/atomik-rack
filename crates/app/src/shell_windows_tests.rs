//! The rack and the console (mixer) in windows of their own: detaching and attaching, the
//! layout of what stays in the main window, the size and scale of each window, and the saved
//! flags. Headless (null audio backend); the platform opens the windows (see `windows`).

use super::*;
use crate::session::NullBackend;
use crate::ui_state::UiState;
use crate::windows::{WindowGeom, WindowHit, WindowKind};
use atomik_model::{Command, DeviceKey};
use atomik_ui::keymap::Context;
use atomik_ui::{Key, Modifiers, PointerButton};

fn shell_at(ui_state_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_at(None)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-windows-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn add_synth(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(k.into())),
        })
        .unwrap();
}

#[test]
fn detach_rack_main_layout_has_no_rack() {
    let mut s = shell();
    assert!(s.layout().rack.height() > 0.0);
    s.execute(AppCommand::DetachRack);
    let l = s.layout();
    assert_eq!(l.rack.height(), 0.0, "the rack leaves the main window");
    assert!(l.divider.is_none());
    assert!(l.song.is_some(), "the song area takes the room");
    assert_eq!(s.windows_wanted(), [WindowKind::Main, WindowKind::Rack]);
    assert_eq!(s.window_of(Pane::Rack), WindowKind::Rack);
    assert_eq!(s.window_of(Pane::Arrangement), WindowKind::Main);
    let size = s.window_size(WindowKind::Rack).unwrap();
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((
            WindowKind::Rack,
            Rect::new(0.0, 0.0, size.width, size.height)
        ))
    );
    assert_eq!(s.pane_rect(Pane::Console), None, "the console is not shown");
}

#[test]
fn detach_rack_keeps_rack_visible_pref() {
    let mut s = shell();
    s.execute(AppCommand::ToggleRack);
    assert!(!s.menu_state().rack);
    s.execute(AppCommand::DetachRack);
    assert!(
        !s.menu_state().rack,
        "detaching does not touch the F6 choice"
    );
    assert!(s.menu_state().rack_detached);
    assert!(s.detached(WindowKind::Rack));
    s.execute(AppCommand::DetachRack);
    assert!(!s.detached(WindowKind::Rack));
    assert!(s.menu_state().rack, "attached again, the rack shows");
    assert!(s.layout().rack.height() > 0.0);
}

#[test]
fn detach_console_lower_pane_never_console() {
    let mut s = shell();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(s.layout().rack.height() > 0.0);
    assert_eq!(s.window_of(Pane::Console), WindowKind::Console);
    let size = s.window_size(WindowKind::Console).unwrap();
    assert_eq!(
        s.core().console().bounds(),
        Rect::new(0.0, 0.0, size.width, size.height)
    );
    assert!(s.menu_state().console, "a detached console counts as shown");
    assert!(s.menu_state().console_detached);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(
        s.lower_pane(),
        Pane::Console,
        "attached, it is back in the lower pane"
    );
    assert_eq!(Some(s.core().console().bounds()), s.layout().song);
}

#[test]
fn detach_console_meters_still_fed() {
    let mut s = shell();
    add_synth(&mut s, "a");
    let ch = s.core().doc().console.channels[0].channel.clone();
    s.execute(AppCommand::DetachConsole);
    s.frame_at(0.1);
    s.frame_at(0.2);
    assert!(s.core().console().meter(&ch).is_some());
}

#[test]
fn window_closed_attaches_pane() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(
        s.windows_wanted(),
        [WindowKind::Main, WindowKind::Rack, WindowKind::Console]
    );
    s.window_closed(WindowKind::Rack);
    assert_eq!(s.windows_wanted(), [WindowKind::Main, WindowKind::Console]);
    assert!(s.layout().rack.height() > 0.0);
    s.window_closed(WindowKind::Console);
    assert_eq!(s.windows_wanted(), [WindowKind::Main]);
    assert_eq!(
        s.lower_pane(),
        Pane::Console,
        "the mixer comes back to the lower pane"
    );
    // The main window is not a pane: closing it changes nothing here (the platform quits).
    s.window_closed(WindowKind::Main);
    assert_eq!(s.windows_wanted(), [WindowKind::Main]);
}

#[test]
fn scale_changed_in_rack_window_leaves_main_views() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    let main_physical = s.physical_size();
    s.handle_in(WindowKind::Rack, &UiEvent::ScaleChanged(2.0));
    s.handle_in(WindowKind::Rack, &UiEvent::Resize(Size::new(900.0, 500.0)));
    assert_eq!(s.window_scale(WindowKind::Rack), Some(2.0));
    assert_eq!(
        s.window_size(WindowKind::Rack),
        Some(Size::new(900.0, 500.0))
    );
    assert_eq!(s.window_scale(WindowKind::Main), Some(1.0));
    assert_eq!(
        s.window_size(WindowKind::Main),
        Some(Size::new(1200.0, 800.0))
    );
    assert_eq!(s.physical_size(), main_physical);
    assert_eq!(s.physical_size_in(WindowKind::Rack), Some((1800, 1000)));
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((WindowKind::Rack, Rect::new(0.0, 0.0, 900.0, 500.0)))
    );
    // The other way round: the main window's scale does not reach the rack window.
    s.handle(&UiEvent::ScaleChanged(1.5));
    assert_eq!(s.window_scale(WindowKind::Rack), Some(2.0));
    assert_eq!(s.window_scale(WindowKind::Main), Some(1.5));
    // An event for a window that is not open is ignored.
    s.handle_in(WindowKind::Console, &UiEvent::Resize(Size::new(10.0, 10.0)));
    assert_eq!(s.window_size(WindowKind::Console), None);
}

#[test]
fn detach_restored_from_ui_state() {
    let path = temp("restore.json");
    let _ = std::fs::remove_file(&path);
    {
        let mut s = shell_at(Some(path.clone()));
        s.execute(AppCommand::DetachRack);
        s.execute(AppCommand::DetachConsole);
    }
    let s = shell_at(Some(path.clone()));
    assert_eq!(
        s.windows_wanted(),
        [WindowKind::Main, WindowKind::Rack, WindowKind::Console]
    );
    assert_eq!(s.layout().rack.height(), 0.0);
    assert_ne!(s.lower_pane(), Pane::Console);
    assert!(
        s.window_size(WindowKind::Rack).is_some(),
        "sized before the platform reports"
    );
    // A fresh state file starts attached.
    let fresh = shell_at(Some(temp("restore-none.json")));
    assert_eq!(fresh.windows_wanted(), [WindowKind::Main]);
}

#[test]
fn menu_detach_items_checked_when_detached() {
    let mut s = shell();
    let checked = |s: &Shell<NullBackend>, label: &str| {
        let model = s.menu_model();
        model
            .iter()
            .flat_map(|m| m.items())
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no menu item {label}"))
            .checked
    };
    assert_eq!(checked(&s, "Detach Rack"), Some(false));
    assert_eq!(checked(&s, "Detach Mixer"), Some(false));
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(checked(&s, "Detach Rack"), Some(true));
    assert_eq!(checked(&s, "Detach Mixer"), Some(true));
    assert!(s.menu_state().rack_detached && s.menu_state().console_detached);
}

// ------------------------------------------------------------------ input, focus, painting

fn down_at(p: Point, button: PointerButton) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn up_at(p: Point, button: PointerButton) -> UiEvent {
    UiEvent::PointerUp {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
    }
}

fn moved_to(p: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

fn press(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

fn tap_in(s: &mut Shell<NullBackend>, window: WindowKind, key: Key, modifiers: Modifiers) {
    s.handle_in(window, &press(key.clone(), modifiers));
    s.handle_in(
        window,
        &UiEvent::Key {
            key,
            modifiers,
            pressed: false,
        },
    );
}

/// A key press and its release in `window`; the requests of the press.
fn tap_requests(
    s: &mut Shell<NullBackend>,
    window: WindowKind,
    key: Key,
    modifiers: Modifiers,
) -> Vec<ShellRequest> {
    let requests = s.handle_in(window, &press(key.clone(), modifiers));
    s.handle_in(
        window,
        &UiEvent::Key {
            key,
            modifiers,
            pressed: false,
        },
    );
    requests
}

fn hit(window: WindowKind, p: Point) -> Option<WindowHit> {
    Some(WindowHit {
        kind: window,
        logical: (p.x, p.y),
    })
}

fn add_device(s: &mut Shell<NullBackend>, type_id: &str, k: &str) -> DeviceKey {
    let key = DeviceKey(k.into());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key.clone()),
        })
        .unwrap();
    key
}

/// A shell with the rack detached into a 1000x700 window and a synth in it.
fn rack_window() -> (Shell<NullBackend>, DeviceKey) {
    let mut s = shell();
    let key = add_device(&mut s, "atomik.poly-synth", "syn");
    s.execute(AppCommand::DetachRack);
    s.handle_in(WindowKind::Rack, &UiEvent::Resize(Size::new(1000.0, 700.0)));
    (s, key)
}

/// A point of the rack window over the left edge of a device panel.
fn device_point(s: &Shell<NullBackend>, key: &DeviceKey) -> Point {
    let r = s.core().view().device_rect(key).unwrap();
    s.core()
        .view()
        .rack_to_view(Point::new(r.x0 + 6.0, r.center().y))
}

#[test]
fn handle_in_rack_window_pointer_reaches_rack_at_window_origin() {
    let (mut s, key) = rack_window();
    assert_eq!(s.core().view().primary(), None);
    let at = device_point(&s, &key);
    s.handle_in(WindowKind::Rack, &down_at(at, PointerButton::Primary));
    s.handle_in(WindowKind::Rack, &up_at(at, PointerButton::Primary));
    assert_eq!(
        s.core().view().primary(),
        Some(key),
        "window coordinates are the rack view's own"
    );
    assert_eq!(s.focus(), Pane::Rack);
    assert_eq!(s.focused_window(), WindowKind::Rack);
    // The same point in the main window is the arrangement or the transport, not the rack.
    assert_ne!(s.hit_pane(at), Some(Pane::Rack));
}

#[test]
fn window_focused_rack_context_is_rack() {
    let (mut s, _) = rack_window();
    s.window_focused(WindowKind::Main);
    assert_ne!(s.focus(), Pane::Rack, "the main window has its own pane");
    assert_eq!(s.focused_window(), WindowKind::Main);
    s.window_focused(WindowKind::Rack);
    assert_eq!(s.focus(), Pane::Rack);
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Rack, Context::Global]
    );
}

#[test]
fn window_focused_console_context_is_mixer() {
    let mut s = shell();
    s.execute(AppCommand::DetachConsole);
    s.window_focused(WindowKind::Console);
    assert_eq!(s.focus(), Pane::Console);
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Mixer, Context::Global]
    );
    // A window that is not open cannot take the focus.
    s.window_focused(WindowKind::Rack);
    assert_eq!(s.focus(), Pane::Console);
}

#[test]
fn transport_key_from_console_window_plays() {
    let mut s = shell();
    s.execute(AppCommand::DetachConsole);
    s.window_focused(WindowKind::Console);
    assert!(!s.core().is_playing());
    tap_in(&mut s, WindowKind::Console, Key::Space, Modifiers::NONE);
    assert!(s.core().is_playing(), "Space plays from the mixer window");
    tap_in(&mut s, WindowKind::Console, Key::Space, Modifiers::NONE);
    assert!(!s.core().is_playing());
    // Undo and the other global chords work there too.
    s.execute(AppCommand::CreateAudioTrack);
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    tap_in(&mut s, WindowKind::Console, Key::Char('z'), Modifiers::CTRL);
    assert!(s.core().doc().song.tracks.is_empty(), "Ctrl+Z undid it");
}

#[test]
fn f5_with_console_detached_requests_focus_window() {
    let mut s = shell();
    s.execute(AppCommand::DetachConsole);
    let lower = s.lower_pane();
    let requests = tap_requests(&mut s, WindowKind::Main, Key::F(5), Modifiers::NONE);
    assert_eq!(
        requests,
        [ShellRequest::FocusWindow(WindowKind::Console)],
        "F5 raises the mixer window"
    );
    assert_eq!(s.focus(), Pane::Console);
    assert_eq!(s.lower_pane(), lower, "the main layout does not change");
    let requests = tap_requests(&mut s, WindowKind::Main, Key::F(5), Modifiers::SHIFT);
    assert_eq!(
        requests,
        [ShellRequest::ToggleMaximise(WindowKind::Console)],
        "Shift+F5 maximises the window"
    );
    assert!(!s.menu_state().console_maximised);
    // The console toolbar buttons and the menu ask the same.
    let requests = s.execute(AppCommand::ToggleConsole);
    assert_eq!(requests, [ShellRequest::FocusWindow(WindowKind::Console)]);
    let requests = s.execute(AppCommand::MaximiseConsole);
    assert_eq!(
        requests,
        [ShellRequest::ToggleMaximise(WindowKind::Console)]
    );
}

#[test]
fn f6_with_rack_detached_requests_focus_window() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    let requests = tap_requests(&mut s, WindowKind::Main, Key::F(6), Modifiers::NONE);
    assert_eq!(requests, [ShellRequest::FocusWindow(WindowKind::Rack)]);
    assert_eq!(s.focus(), Pane::Rack);
    assert!(s.menu_state().rack, "the F6 choice is untouched");
}

#[test]
fn f7_with_rack_detached_focuses_main_arrangement() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.window_focused(WindowKind::Rack);
    let before = s.layout();
    let requests = tap_requests(&mut s, WindowKind::Rack, Key::F(7), Modifiers::NONE);
    assert_eq!(requests, [ShellRequest::FocusWindow(WindowKind::Main)]);
    assert_eq!(s.focus(), Pane::Arrangement);
    assert_eq!(s.layout(), before, "no layout change");
    assert!(!s.menu_state().sequencer_only);
}

#[test]
fn context_menu_opens_in_window_of_pane() {
    let (mut s, key) = rack_window();
    let at = device_point(&s, &key);
    s.handle_in(WindowKind::Rack, &down_at(at, PointerButton::Secondary));
    s.handle_in(WindowKind::Rack, &up_at(at, PointerButton::Secondary));
    assert!(s.context_menu().is_some());
    assert_eq!(s.context_menu_window(), Some(WindowKind::Rack));
    let item = s
        .context_menu()
        .unwrap()
        .items()
        .next()
        .unwrap()
        .label
        .clone();
    let mut rack = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Rack, &mut rack);
    let mut main = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut main);
    assert!(rack.texts().contains(&item.as_str()), "{item}");
    assert!(!main.texts().contains(&item.as_str()));
}

#[test]
fn press_in_other_window_closes_context_menu() {
    let (mut s, key) = rack_window();
    let at = device_point(&s, &key);
    s.handle_in(WindowKind::Rack, &down_at(at, PointerButton::Secondary));
    assert!(s.context_menu().is_some());
    // A move in the main window is not the menu's business.
    s.handle_in(WindowKind::Main, &moved_to(Point::new(600.0, 400.0)));
    assert!(s.context_menu().is_some());
    s.handle_in(
        WindowKind::Main,
        &down_at(Point::new(600.0, 400.0), PointerButton::Primary),
    );
    assert!(s.context_menu().is_none());
}

#[test]
fn modal_open_ignores_detached_window_events_and_raises_main() {
    let (mut s, key) = rack_window();
    let requests = s.execute(AppCommand::AudioSettings);
    assert!(s.dialog_open());
    assert!(
        requests.contains(&ShellRequest::FocusWindow(WindowKind::Main)),
        "{requests:?}"
    );
    let at = device_point(&s, &key);
    assert!(s.handle_in(WindowKind::Rack, &moved_to(at)).is_empty());
    let requests = s.handle_in(WindowKind::Rack, &down_at(at, PointerButton::Primary));
    assert_eq!(requests, [ShellRequest::FocusWindow(WindowKind::Main)]);
    s.handle_in(WindowKind::Rack, &up_at(at, PointerButton::Primary));
    assert_eq!(s.core().view().primary(), None, "the rack never saw it");
    assert!(s.dialog_open());
    // Keys from the detached window do not reach the dialog behind it either.
    assert!(
        s.handle_in(WindowKind::Rack, &press(Key::Char('x'), Modifiers::NONE))
            .is_empty()
    );
}

fn wav(name: &str, frames: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-windows-wav-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(
        &path,
        &atomik_audio_file::AudioData::new(48_000, vec![data]).unwrap(),
    )
    .unwrap();
    path
}

fn clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .map(|l| l.clips.len())
        .sum()
}

/// Starts a drag the way the file browser panel does: a press in it, then its `DragFiles`
/// action. The panel must show.
fn start_drag(s: &mut Shell<NullBackend>, paths: Vec<PathBuf>) {
    let fb = s.layout().file_browser.expect("the file browser shows");
    s.handle(&down_at(fb.center(), PointerButton::Primary));
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::DragFiles(paths))]);
    assert!(s.cross_window_drag());
}

/// A shell with the file browser shown, a sampler in the detached rack and its window point.
fn sampler_rack() -> (Shell<NullBackend>, DeviceKey, Point) {
    let mut s = shell();
    s.execute(AppCommand::ToggleFileBrowser);
    let key = add_device(&mut s, "atomik.sampler", "smp");
    s.execute(AppCommand::DetachRack);
    s.handle_in(WindowKind::Rack, &UiEvent::Resize(Size::new(1000.0, 700.0)));
    let at = device_point(&s, &key);
    (s, key, at)
}

fn zone_count(s: &Shell<NullBackend>, key: &DeviceKey) -> usize {
    s.core().doc().device(key).unwrap().zones.len()
}

#[test]
fn file_drag_from_browser_drops_on_detached_rack() {
    let (mut s, key, at) = sampler_rack();
    let paths = vec![wav("tone_C3.wav", 2_400), wav("tone_E3.wav", 2_400)];
    start_drag(&mut s, paths);
    // The OS keeps the events in the main window, with positions outside it.
    let outside = Point::new(-300.0, 200.0);
    s.handle_routed(
        WindowKind::Main,
        &moved_to(outside),
        hit(WindowKind::Rack, at),
    );
    assert_eq!(s.core().view().drop_hover(), Some(&key));
    s.handle_routed(
        WindowKind::Main,
        &up_at(outside, PointerButton::Primary),
        hit(WindowKind::Rack, at),
    );
    s.core_mut().wait_background();
    assert!(s.core().view().drop_hover().is_none());
    assert_eq!(zone_count(&s, &key), 2, "both files became zones");
    assert!(!s.cross_window_drag());
}

#[test]
fn library_item_drag_inserts_into_detached_rack() {
    let (mut s, key, _) = sampler_rack();
    // Below the sampler, in its column: a slot of the rack.
    let r = s.core().view().device_rect(&key).unwrap();
    let over = s
        .core()
        .view()
        .rack_to_view(Point::new(r.x0 + 300.0, r.y1 + 20.0));
    let synths = |s: &Shell<NullBackend>| {
        s.core()
            .doc()
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.poly-synth")
            .count()
    };
    let before = synths(&s);
    start_drag(
        &mut s,
        vec![atomik_library::device_uri("atomik.poly-synth")],
    );
    let outside = Point::new(-300.0, 200.0);
    s.handle_routed(
        WindowKind::Main,
        &moved_to(outside),
        hit(WindowKind::Rack, over),
    );
    s.handle_routed(
        WindowKind::Main,
        &up_at(outside, PointerButton::Primary),
        hit(WindowKind::Rack, over),
    );
    assert_eq!(synths(&s), before + 1, "the dropped synth was added");
}

/// A shell with the file browser shown and one audio track with a clip, and its lane point.
fn lane_shell(tag: &str) -> (Shell<NullBackend>, Point) {
    let mut s = shell();
    s.execute(AppCommand::ToggleFileBrowser);
    s.import_files(vec![wav(&format!("first-{tag}.wav"), 24_000)]);
    s.core_mut().wait_background();
    assert_eq!(clip_count(&s), 1);
    let song = s.layout().song.unwrap();
    let mut y = song.y0;
    while y < song.y1 {
        let p = Point::new(song.x0 + 500.0, y);
        if matches!(
            s.core().arrangement().drop_target_at(p),
            Some((atomik_ui::ImportTarget::ExistingTrack(_), _))
        ) {
            return (s, p);
        }
        y += 2.0;
    }
    panic!("no audio lane found");
}

#[test]
fn file_drag_leaving_main_cancels_hover_there() {
    let (mut s, lane) = lane_shell("leaving");
    s.execute(AppCommand::DetachRack);
    start_drag(&mut s, vec![wav("second-windows.wav", 12_000)]);
    s.handle_routed(
        WindowKind::Main,
        &moved_to(lane),
        hit(WindowKind::Main, lane),
    );
    assert!(s.core().arrangement().drop_hover().is_some());
    // The pointer crosses into the rack window: the arrangement lets go of its hover.
    s.handle_routed(
        WindowKind::Main,
        &moved_to(Point::new(-50.0, 100.0)),
        hit(WindowKind::Rack, Point::new(200.0, 200.0)),
    );
    assert!(s.core().arrangement().drop_hover().is_none());
}

#[test]
fn file_drag_without_target_window_cancels() {
    let (mut s, lane) = lane_shell("without");
    start_drag(&mut s, vec![wav("second-windows.wav", 12_000)]);
    s.handle_routed(
        WindowKind::Main,
        &moved_to(lane),
        hit(WindowKind::Main, lane),
    );
    assert!(s.core().arrangement().drop_hover().is_some());
    // Over no window of the app: nothing is hovered and the release drops nothing.
    s.handle_routed(WindowKind::Main, &moved_to(Point::new(-400.0, 50.0)), None);
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle_routed(
        WindowKind::Main,
        &up_at(Point::new(-400.0, 50.0), PointerButton::Primary),
        None,
    );
    s.core_mut().wait_background();
    assert_eq!(clip_count(&s), 1, "no file was imported");
    assert!(!s.cross_window_drag());
}

#[test]
fn os_file_drop_on_rack_window_reaches_rack() {
    let (mut s, key, at) = sampler_rack();
    let paths = vec![wav("tone_G3.wav", 2_400)];
    s.handle_in(
        WindowKind::Rack,
        &UiEvent::FileHover {
            pos: at,
            paths: paths.clone(),
        },
    );
    assert_eq!(s.core().view().drop_hover(), Some(&key));
    s.handle_in(WindowKind::Rack, &UiEvent::FileDrop { pos: at, paths });
    s.core_mut().wait_background();
    assert_eq!(zone_count(&s, &key), 1);
}

#[test]
fn repaint_bits_cleared_per_window() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    let mut dl = atomik_ui::DisplayList::new();
    for w in [WindowKind::Main, WindowKind::Rack, WindowKind::Console] {
        s.paint_in(w, &mut dl);
        assert!(!s.needs_repaint_in(w), "{w:?} painted");
    }
    // A change marks every open window...
    s.execute(AppCommand::ToggleFileBrowser);
    for w in [WindowKind::Main, WindowKind::Rack, WindowKind::Console] {
        assert!(s.needs_repaint_in(w), "{w:?} marked");
    }
    // ...and painting one clears only that one.
    s.paint_in(WindowKind::Rack, &mut dl);
    assert!(!s.needs_repaint_in(WindowKind::Rack));
    assert!(s.needs_repaint_in(WindowKind::Main));
    assert!(s.needs_repaint_in(WindowKind::Console));
    s.paint_in(WindowKind::Main, &mut dl);
    s.paint_in(WindowKind::Console, &mut dl);
    for w in [WindowKind::Main, WindowKind::Rack, WindowKind::Console] {
        assert!(!s.needs_repaint_in(w), "{w:?} painted again");
    }
    // A window that is not open never needs a paint.
    s.window_closed(WindowKind::Rack);
    assert!(!s.needs_repaint_in(WindowKind::Rack));
}

#[test]
fn paint_in_console_window_displays_console_only() {
    let mut s = shell();
    add_device(&mut s, "atomik.poly-synth", "syn");
    // Attached: the console shares the main window with the menu bar and the transport.
    s.execute(AppCommand::ToggleConsole);
    let mut attached = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut attached);
    let console_text = "ADD CHANNEL";
    assert!(attached.texts().iter().any(|t| t.contains(console_text)));
    s.execute(AppCommand::DetachConsole);
    let mut main = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut main);
    let mut console = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Console, &mut console);
    assert!(!main.texts().iter().any(|t| t.contains(console_text)));
    assert!(console.texts().iter().any(|t| t.contains(console_text)));
    // No menu bar and no status bar in the console window.
    assert!(main.texts().contains(&"File"));
    assert!(!console.texts().contains(&"File"));
    assert!(main.texts().iter().any(|t| t.contains("MIDI off")));
    assert!(!console.texts().iter().any(|t| t.contains("MIDI off")));
    // The rack window paints the rack and nothing of the main window.
    s.execute(AppCommand::DetachRack);
    let mut rack = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Rack, &mut rack);
    assert!(!rack.texts().contains(&"File"));
    assert!(!rack.is_empty());
}

#[test]
fn window_moved_is_saved_at_most_every_two_seconds() {
    let path = temp("geometry.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    s.execute(AppCommand::DetachRack);
    let saved = |p: &PathBuf| UiState::load(p).windows.rack.geom;
    let geom = |x: i32| WindowGeom {
        x,
        y: 40,
        width: 900.0,
        height: 600.0,
        maximised: false,
        monitor: Some("DP-1".into()),
    };
    s.window_moved(WindowKind::Rack, geom(10));
    s.frame_at(10.0);
    assert_eq!(saved(&path), Some(geom(10)), "the first move is written");
    s.window_moved(WindowKind::Rack, geom(20));
    s.frame_at(11.0);
    assert_eq!(
        saved(&path),
        Some(geom(10)),
        "within two seconds: memory only"
    );
    s.frame_at(12.5);
    assert_eq!(saved(&path), Some(geom(20)));
    // Maximising keeps the restored bounds and records the flag.
    s.window_moved(
        WindowKind::Rack,
        WindowGeom {
            maximised: true,
            width: 1920.0,
            height: 1080.0,
            ..geom(0)
        },
    );
    s.shutdown();
    let g = saved(&path).unwrap();
    assert!(g.maximised);
    assert_eq!((g.x, g.width, g.height), (20, 900.0, 600.0));
    assert_eq!(s.saved_geom(WindowKind::Rack), Some(g));
    // The geometry of a window that is not open is ignored; the saved one survives an attach.
    s.window_closed(WindowKind::Rack);
    s.window_moved(WindowKind::Rack, geom(99));
    assert_eq!(s.saved_geom(WindowKind::Rack).map(|g| g.x), Some(20));
}

#[test]
fn title_in_names_each_window() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(s.title_in(WindowKind::Main), s.title());
    assert_eq!(s.title_in(WindowKind::Rack), "Rack \u{2014} Untitled");
    assert_eq!(s.title_in(WindowKind::Console), "Mixer \u{2014} Untitled");
}
