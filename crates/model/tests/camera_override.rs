//! The project's own camera settings: `SetCameraOverride` as one undo step, the optional
//! `"camera"` field of project format 45 (ignorable, written only when set, byte-identical save,
//! load and save) and the limits with their JSON paths.

mod common;

use atomik_model::{
    CameraGesture, CameraOverride, Command, Document, GestureHand, GestureRow, MAX_GESTURE_ROWS,
    PROJECT_VERSION, SyncOp, load_project, new_project_doc, save_project,
};
use common::workflow::{one_step, rejected, text};
use common::*;

fn settings() -> CameraOverride {
    CameraOverride {
        mode: "midi".into(),
        gestures: vec![
            GestureRow {
                gesture: CameraGesture::Fist,
                hand: GestureHand::Either,
                command: "transport.stop".into(),
            },
            GestureRow {
                gesture: CameraGesture::ThumbsUp,
                hand: GestureHand::Right,
                command: "transport.record".into(),
            },
        ],
    }
}

fn doc() -> (Document, atomik_device_sdk::Registry) {
    (Document::new(new_project_doc()), console_registry())
}

#[test]
fn a_new_project_has_no_camera_settings_and_saves_no_field() {
    let (d, _) = doc();
    assert!(d.doc().camera.is_none());
    assert!(!text(&d).contains("\"camera\": {"));
    assert_eq!(PROJECT_VERSION, 45, "the format does not change");
}

#[test]
fn setting_them_is_one_undo_step_that_reports_a_view_change() {
    let (mut d, r) = doc();
    let none = text(&d);
    let ops = one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    assert!(ops.contains(&SyncOp::ViewChanged), "{ops:?}");
    assert_eq!(d.doc().camera, Some(settings()));
    assert_ne!(text(&d), none);
    assert!(d.undo().is_some());
    assert!(d.doc().camera.is_none());
    assert_eq!(text(&d), none);
    assert!(d.redo().is_some());
    assert_eq!(d.doc().camera, Some(settings()));
}

#[test]
fn clearing_them_is_one_step_and_a_repeat_is_rejected() {
    let (mut d, r) = doc();
    rejected(&mut d, &r, Command::SetCameraOverride(None));
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    rejected(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    one_step(&mut d, &r, Command::SetCameraOverride(None));
    assert!(d.doc().camera.is_none());
    assert!(d.undo().is_some());
    assert_eq!(d.doc().camera, Some(settings()));
}

#[test]
fn the_settings_save_and_load_byte_for_byte() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    let saved = text(&d);
    assert!(saved.contains("\"camera\": {"), "{saved}");
    let loaded = load_project(&saved, &r).expect("loads");
    assert_eq!(loaded.camera, Some(settings()));
    assert_eq!(save_project(&loaded), saved);
    // A table with no rows is still a project's own setting.
    let empty = CameraOverride {
        mode: "knob".into(),
        gestures: Vec::new(),
    };
    one_step(&mut d, &r, Command::SetCameraOverride(Some(empty.clone())));
    let saved = text(&d);
    let loaded = load_project(&saved, &r).expect("loads");
    assert_eq!(loaded.camera, Some(empty));
    assert_eq!(save_project(&loaded), saved);
}

#[test]
fn the_field_is_ignorable_so_an_older_reader_may_drop_it() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    assert!(
        text(&d).contains("\"camera\": 45"),
        "the ignorable header lists the field"
    );
}

#[test]
fn the_field_is_unknown_in_a_version_44_file_without_the_header() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    let old = text(&d).replacen("\"version\": 45", "\"version\": 44", 1);
    assert!(load_project(&old, &r).is_err());
}

#[test]
fn the_limits_hold_in_the_command() {
    let (mut d, r) = doc();
    let mut bad = settings();
    bad.mode = "wave".into();
    rejected(&mut d, &r, Command::SetCameraOverride(Some(bad)));
    let mut bad = settings();
    bad.gestures[0].command = String::new();
    rejected(&mut d, &r, Command::SetCameraOverride(Some(bad)));
    let mut bad = settings();
    bad.gestures[0].command = "a\u{7}b".into();
    rejected(&mut d, &r, Command::SetCameraOverride(Some(bad)));
    let mut bad = settings();
    bad.gestures = vec![bad.gestures[0].clone(); MAX_GESTURE_ROWS + 1];
    rejected(&mut d, &r, Command::SetCameraOverride(Some(bad)));
    let mut ok = settings();
    ok.gestures = vec![ok.gestures[0].clone(); MAX_GESTURE_ROWS];
    one_step(&mut d, &r, Command::SetCameraOverride(Some(ok)));
}

#[test]
fn the_loader_names_the_json_path_of_what_breaks_a_limit() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    let saved = text(&d);
    let bad_mode = saved.replacen("\"mode\": \"midi\"", "\"mode\": \"wave\"", 1);
    let e = load_project(&bad_mode, &r).unwrap_err().to_string();
    assert!(e.contains("camera") && e.contains("mode"), "{e}");
    let bad_gesture = saved.replacen("\"fist\"", "\"wink\"", 1);
    let e = load_project(&bad_gesture, &r).unwrap_err().to_string();
    assert!(e.contains("camera.gestures[0].gesture"), "{e}");
    let bad_hand = saved.replacen("\"either\"", "\"third\"", 1);
    let e = load_project(&bad_hand, &r).unwrap_err().to_string();
    assert!(e.contains("camera.gestures[0].hand"), "{e}");
    let unknown = saved.replacen("\"command\": \"transport.stop\"", "\"cmd\": \"x\"", 1);
    assert!(load_project(&unknown, &r).is_err());
}

#[test]
fn a_copy_of_the_document_keeps_the_settings() {
    let (mut d, r) = doc();
    one_step(&mut d, &r, Command::SetCameraOverride(Some(settings())));
    let copy = d.doc().clone();
    assert_eq!(copy.camera, Some(settings()));
    assert_eq!(copy, *d.doc());
}
