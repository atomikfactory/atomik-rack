//! The webcam controller in the shell: the View menu rows, the consent dialog (nothing is fetched
//! before Download), the preview box, the CAM button of the transport bar, the Camera tab of the
//! settings and the camera going off when the app quits. A fake backend stands in for the camera,
//! the models and the network.

use super::*;
use crate::session::NullBackend;
use crate::vision_link::{FakeVision, ModelState};
use atomik_ui::{CameraDialogChoice, ClusterPart, Key, Modifiers, PointerButton};
use std::sync::Arc;
use std::time::Duration;

fn shell(models: ModelState) -> (Shell<NullBackend>, FakeVision) {
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
    s.handle(&UiEvent::Resize(Size::new(1600.0, 900.0)));
    let fake = FakeVision::new(models);
    s.core_mut().set_vision(Arc::new(fake.clone()));
    (s, fake)
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

fn wait(s: &mut Shell<NullBackend>, what: &str, done: impl Fn(&Shell<NullBackend>) -> bool) {
    // Up to five seconds, counted in the sleeps (the tests read no clock).
    for _ in 0..2500 {
        if done(s) {
            return;
        }
        s.frame();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(done(s), "timed out waiting for {what}");
}

/// The labels of the checked rows of View > Panels.
fn panel_rows(s: &Shell<NullBackend>) -> Vec<(String, Option<bool>)> {
    let view = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "View")
        .expect("a View menu");
    let panels = view
        .entries
        .iter()
        .find_map(|e| match e {
            crate::commands::MenuEntry::Submenu(sub) if sub.label == "Panels" => Some(sub),
            _ => None,
        })
        .expect("a Panels submenu");
    panels
        .entries
        .iter()
        .filter_map(|e| match e {
            crate::commands::MenuEntry::Item(i) => Some((i.label.to_string(), i.checked)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_view_menu_has_the_camera_and_its_preview_as_check_rows() {
    let (mut s, _fake) = shell(ModelState::Ready);
    let rows = panel_rows(&s);
    assert!(
        rows.contains(&("Camera".to_owned(), Some(false))),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("Camera Preview".to_owned(), Some(true))),
        "the preview is on by default: {rows:?}"
    );
    s.execute(AppCommand::ToggleCamera);
    assert!(panel_rows(&s).contains(&("Camera".to_owned(), Some(true))));
    s.execute(AppCommand::ToggleCameraPreview);
    assert!(panel_rows(&s).contains(&("Camera Preview".to_owned(), Some(false))));
}

#[test]
fn the_command_ids_name_the_two_commands() {
    assert_eq!(
        AppCommand::ToggleCamera.command_id(),
        Some(("camera.toggle", None))
    );
    assert_eq!(
        AppCommand::from_command("camera.preview", None),
        Some(AppCommand::ToggleCameraPreview)
    );
}

#[test]
fn turning_the_camera_on_without_models_opens_the_dialog_and_fetches_nothing_until_download() {
    let (mut s, fake) = shell(ModelState::Missing);
    s.execute(AppCommand::ToggleCamera);
    s.frame();
    assert!(s.camera_dialog_visible());
    assert!(!s.core().camera_is_on());
    assert_eq!(fake.fetches(), 0);
    // The dialog is modal: a key under it is not heard, and a click elsewhere does nothing.
    s.frame();
    assert_eq!(fake.fetches(), 0, "a frame fetches nothing");
    let download = s
        .camera_dialog_view()
        .unwrap()
        .choices()
        .into_iter()
        .find(|(c, _)| *c == CameraDialogChoice::Download)
        .unwrap()
        .1
        .center();
    click(&mut s, download);
    wait(&mut s, "the camera", |s| s.core().camera_is_on());
    assert_eq!(fake.fetches(), 2);
    assert!(!s.camera_dialog_visible());
    assert!(s.camera_preview_visible());
}

#[test]
fn cancel_closes_the_dialog_and_escape_does_too() {
    let (mut s, fake) = shell(ModelState::Missing);
    s.execute(AppCommand::ToggleCamera);
    s.frame();
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    s.frame();
    assert!(!s.camera_dialog_visible());
    assert_eq!(fake.fetches(), 0);
    assert!(!s.core().camera_is_on());
}

#[test]
fn the_cam_button_of_the_transport_bar_turns_the_camera_on_and_off_and_lights_up() {
    let (mut s, fake) = shell(ModelState::Ready);
    s.frame();
    let at = s
        .core()
        .transport_bar()
        .cluster_rect(ClusterPart::Camera)
        .expect("the bar has room for CAM at 1600 px")
        .center();
    click(&mut s, at);
    s.frame();
    assert!(s.core().camera_is_on());
    assert_eq!(fake.started(), 1);
    click(&mut s, at);
    s.frame();
    assert!(!s.core().camera_is_on());
    assert_eq!(fake.stopped(), 1);
}

#[test]
fn the_close_button_of_the_preview_hides_it_and_the_camera_stays_on() {
    let (mut s, _fake) = shell(ModelState::Ready);
    s.execute(AppCommand::ToggleCamera);
    s.frame();
    assert!(s.camera_preview_visible());
    let close = s.camera_preview_view().close_rect().center();
    click(&mut s, close);
    s.frame();
    assert!(!s.camera_preview_visible());
    assert!(s.core().camera_is_on());
}

#[test]
fn quitting_turns_the_camera_off() {
    let (mut s, fake) = shell(ModelState::Ready);
    s.execute(AppCommand::ToggleCamera);
    assert!(s.core().camera_is_on());
    let requests = s.execute(AppCommand::Quit);
    assert!(requests.contains(&ShellRequest::Quit));
    assert!(!s.core().camera_is_on());
    assert_eq!(fake.stopped(), 1);
}

#[test]
fn the_camera_tab_edits_the_settings_and_apply_saves_them_in_workflow_json() {
    let dir = std::env::temp_dir().join(format!("atomik-shell-camera-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(dir.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    let fake = FakeVision::new(ModelState::Ready);
    s.core_mut().set_vision(Arc::new(fake.clone()));
    s.open_settings(Some(crate::settings_dialog::Tab::Camera));
    let size = Size::new(1000.0, 800.0);
    let layout = |s: &Shell<NullBackend>| s.dialog.as_ref().unwrap().camera_layout(size).unwrap();
    // Refresh asks the system, then the next camera can be picked.
    let at = layout(&s).camera.refresh.center();
    click(&mut s, at);
    let at = layout(&s).camera.next.center();
    click(&mut s, at);
    assert_eq!(
        s.dialog
            .as_ref()
            .unwrap()
            .preferences()
            .unwrap()
            .1
            .camera
            .camera,
        Some("fake-0".to_owned())
    );
    // Left hand, a quarter turn less, fine control off, a less eager pinch.
    let left = layout(&s).hand.choices[1].1.center();
    click(&mut s, left);
    for _ in 0..2 {
        let at = layout(&s).turn.less.center();
        click(&mut s, at);
    }
    let at = layout(&s).fine.rect.center();
    click(&mut s, at);
    let at = layout(&s).sensitivity.less.center();
    click(&mut s, at);
    let draft = s.dialog.as_ref().unwrap().preferences().unwrap().1.camera;
    assert_eq!(
        (draft.hand, draft.full_turn, draft.fine, draft.sensitivity),
        (crate::keymap_store::HandChoice::Left, 130, false, 90)
    );
    // Apply reaches the core and the file.
    let (apply, _) = s.dialog.as_ref().unwrap().buttons(size);
    click(&mut s, apply.center());
    assert_eq!(s.core().camera_prefs().full_turn, 130);
    assert_eq!(
        s.workflow().camera.hand,
        crate::keymap_store::HandChoice::Left
    );
    let text = std::fs::read_to_string(dir.join("workflow.json")).unwrap();
    assert!(text.contains("\"full_turn\": 130"), "{text}");
    assert!(text.contains("\"camera\": \"fake-0\""), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn calibrate_in_the_camera_tab_applies_closes_and_a_finished_calibration_is_saved() {
    use atomik_vision::gesture::{HandId, HandState};
    use atomik_vision::tracker::{FrameReport, TrackerEvent};
    let dir = std::env::temp_dir().join(format!("atomik-shell-calibrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(dir.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    let fake = FakeVision::new(ModelState::Ready);
    s.core_mut().set_vision(Arc::new(fake.clone()));
    s.open_settings(Some(crate::settings_dialog::Tab::Camera));
    let size = Size::new(1000.0, 800.0);
    let at = s
        .dialog
        .as_ref()
        .unwrap()
        .camera_layout(size)
        .unwrap()
        .calibrate
        .button
        .center();
    click(&mut s, at);
    assert!(s.dialog.is_none(), "the dialog was applied and closed");
    assert!(s.core().camera_is_on());
    assert!(s.core().camera_calibration_step().is_some());
    // Pinch, hold still, twist 40 degrees right and hold, 40 left of the start and hold.
    let report = |t: f64, angle: f32| {
        TrackerEvent::Frame(FrameReport {
            t,
            hands: vec![HandState {
                id: HandId(0),
                present: true,
                handedness: atomik_vision::Handedness::Right,
                pinch: true,
                pinch_strength: 1.0,
                twist_angle: angle.to_radians(),
                palm_center: (0.5, 0.5),
                palm_height: 0.2,
                hand_size: 0.1,
                depth_proxy: 0.1,
            }],
            fps: 30.0,
            infer_ms: 20.0,
        })
    };
    let mut t = 0.0;
    while t < 7.0 {
        let angle = if t < 2.0 {
            0.0
        } else if t < 3.5 {
            40.0
        } else {
            -40.0
        };
        fake.push([report(t, angle)]);
        s.frame();
        t += 1.0 / 30.0;
    }
    assert_eq!(s.core().camera_calibration_step(), None);
    assert_eq!(s.workflow().camera.full_turn, 80);
    let text = std::fs::read_to_string(dir.join("workflow.json")).unwrap();
    assert!(text.contains("\"full_turn\": 80"), "{text}");
    assert!(text.contains("\"dead_zone\": 5"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_downloaded_models_in_the_camera_tab_turns_the_camera_off_and_deletes() {
    let (mut s, fake) = shell(ModelState::Ready);
    s.execute(AppCommand::ToggleCamera);
    s.open_settings(Some(crate::settings_dialog::Tab::Camera));
    let size = s.size();
    let at = s
        .dialog
        .as_ref()
        .unwrap()
        .camera_layout(size)
        .unwrap()
        .models
        .delete
        .center();
    click(&mut s, at);
    assert!(!s.core().camera_is_on());
    assert_eq!(
        fake.stats.deleted.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(s.core().camera_models(), ModelState::Missing);
    // With nothing to delete the button does nothing.
    let at = s
        .dialog
        .as_ref()
        .unwrap()
        .camera_layout(size)
        .unwrap()
        .models
        .delete
        .center();
    click(&mut s, at);
    assert_eq!(
        fake.stats.deleted.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
