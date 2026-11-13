//! The webcam controller's views: the consent dialog of the model download (the question, the
//! progress, a failure) and the camera preview box (no hand, a hand, a pinch with its target, a
//! message). The views are pure: they draw a state they are given, so the pictures here are fake
//! ones. Each state's display list matches its golden, in English here and in French in
//! `french_goldens.rs`.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    CAMERA_PREVIEW_HEIGHT, CAMERA_PREVIEW_WIDTH, CameraDialogChoice, CameraDialogState,
    CameraDialogView, CameraLamp, CameraPicture, CameraPreviewState, CameraPreviewView,
    DisplayList, Key, Modifiers, Theme, UiAction, UiEvent,
};
use support::*;

const WINDOW: (f64, f64) = (900.0, 600.0);

/// A fake 8 by 6 picture: a gradient.
fn picture() -> CameraPicture {
    let (width, height) = (8u32, 6u32);
    let mut rgb = Vec::new();
    for y in 0..height {
        for x in 0..width {
            rgb.extend_from_slice(&[(x * 30) as u8, (y * 40) as u8, 120]);
        }
    }
    CameraPicture { width, height, rgb }
}

/// One hand: 21 points along a diagonal.
fn hand() -> Vec<(f32, f32)> {
    (0..21)
        .map(|i| (0.2 + 0.03 * i as f32, 0.9 - 0.035 * i as f32))
        .collect()
}

fn preview_state(
    lamp: CameraLamp,
    hands: usize,
    target: Option<(&str, &str)>,
    message: Option<&str>,
) -> CameraPreviewState {
    CameraPreviewState {
        lamp,
        picture: Some(picture()),
        hands: (0..hands).map(|_| hand()).collect(),
        target: target.map(|t| t.0.to_owned()),
        value: target.map(|t| t.1.to_owned()),
        message: message.map(str::to_owned),
        fps: 29.6,
        infer_ms: 21.4,
        controls: None,
        pad: None,
    }
}

/// The four states the goldens show.
fn previews() -> Vec<(&'static str, CameraPreviewState)> {
    vec![
        ("no_hand", preview_state(CameraLamp::NoHand, 0, None, None)),
        ("hand", preview_state(CameraLamp::Hand, 1, None, None)),
        (
            "pinch",
            preview_state(CameraLamp::Pinch, 1, Some(("Lead Cutoff", "2.4 kHz")), None),
        ),
        (
            "no_target",
            preview_state(CameraLamp::Pinch, 1, None, Some("Click a knob first")),
        ),
        ("pad", pad_preview()),
    ]
}

/// Both hands pinched: the crosshair of the two-hand pad, a little right of the middle of the
/// preview and as wide as the hands are far apart.
fn pad_preview() -> CameraPreviewState {
    CameraPreviewState {
        pad: Some((0.62, 0.4, 0.5)),
        ..preview_state(CameraLamp::Pinch, 2, Some(("Lead Cutoff", "2.4 kHz")), None)
    }
}

fn preview(state: CameraPreviewState) -> CameraPreviewView {
    let mut v = CameraPreviewView::new();
    v.set_origin(Point::new(660.0, 380.0));
    v.set_state(state);
    v
}

fn dialogs() -> Vec<(&'static str, CameraDialogState)> {
    vec![
        (
            "consent",
            CameraDialogState::Consent {
                source: "storage.googleapis.com".into(),
                bytes: 7_818_763,
            },
        ),
        (
            "downloading",
            CameraDialogState::Downloading {
                received: 3_000_000,
                total: 7_818_763,
            },
        ),
        (
            "failed",
            CameraDialogState::Failed {
                message: "The server answered 500".into(),
            },
        ),
    ]
}

fn dialog(state: CameraDialogState) -> CameraDialogView {
    CameraDialogView::new(WINDOW, state)
}

fn texts_of_preview(v: &mut CameraPreviewView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn texts_of_dialog(v: &mut CameraDialogView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut CameraDialogView, p: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out
}

// ---- The dialog -------------------------------------------------------------------------------

#[test]
fn the_consent_says_what_is_downloaded_from_where_and_how_big() {
    let mut v = dialog(dialogs().remove(0).1);
    let t = texts_of_dialog(&mut v);
    assert!(t.iter().any(|s| s == "Download the hand models?"), "{t:?}");
    assert!(
        t.iter()
            .any(|s| s.contains("7.8 MB") && s.contains("storage.googleapis.com")),
        "the size and the place: {t:?}"
    );
    assert!(t.iter().any(|s| s.contains("never saved")), "{t:?}");
    assert!(t.iter().any(|s| s == "Download") && t.iter().any(|s| s == "Cancel"));
}

#[test]
fn download_and_cancel_ask_the_app_and_nothing_else_does() {
    let mut v = dialog(dialogs().remove(0).1);
    let choices = v.choices();
    let at = |c: CameraDialogChoice| choices.iter().find(|(k, _)| *k == c).unwrap().1.center();
    assert_eq!(
        click(&mut v, at(CameraDialogChoice::Download)),
        vec![UiAction::CameraDialog(CameraDialogChoice::Download)]
    );
    assert_eq!(
        click(&mut v, at(CameraDialogChoice::Cancel)),
        vec![UiAction::CameraDialog(CameraDialogChoice::Cancel)]
    );
    // The text and the backdrop are not buttons.
    assert!(click(&mut v, Point::new(5.0, 5.0)).is_empty());
    let middle = v.card().center();
    assert!(click(&mut v, middle).is_empty());
    // Enter is the first button, Esc the last.
    let key = |k: Key| UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    assert_eq!(
        v.handle(&key(Key::Enter)),
        vec![UiAction::CameraDialog(CameraDialogChoice::Download)]
    );
    assert_eq!(
        v.handle(&key(Key::Escape)),
        vec![UiAction::CameraDialog(CameraDialogChoice::Cancel)]
    );
}

#[test]
fn the_progress_shows_the_megabytes_and_a_failure_offers_retry() {
    let mut v = dialog(dialogs().remove(1).1);
    let t = texts_of_dialog(&mut v);
    assert!(t.iter().any(|s| s == "3.0 MB of 7.8 MB"), "{t:?}");
    assert_eq!(
        v.choices().iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![CameraDialogChoice::Cancel]
    );
    v.set_state(dialogs().remove(2).1);
    let t = texts_of_dialog(&mut v);
    assert!(t.iter().any(|s| s == "The server answered 500"), "{t:?}");
    assert_eq!(
        v.choices().iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![CameraDialogChoice::Retry, CameraDialogChoice::Close]
    );
    assert!(v.help_at(v.choices()[0].1.center()).is_some());
}

#[test]
fn every_dialog_button_fits_the_card_and_none_overlap() {
    for (_, state) in dialogs() {
        let v = dialog(state);
        let card = v.card();
        let rects: Vec<Rect> = v.choices().into_iter().map(|c| c.1).collect();
        for r in &rects {
            assert!(
                r.x0 >= card.x0 && r.x1 <= card.x1 && r.y1 <= card.y1,
                "{r:?}"
            );
        }
        for pair in rects.windows(2) {
            assert!(pair[0].x0 >= pair[1].x1, "{pair:?}");
        }
    }
}

#[test]
fn the_dialog_in_each_state_matches_its_golden() {
    for (name, state) in dialogs() {
        let mut v = dialog(state);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &Theme::dark());
        starter::check_golden(&format!("camera_dialog_{name}_1x.json"), &dl.to_json());
    }
}

// ---- The preview ------------------------------------------------------------------------------

#[test]
fn the_preview_shows_the_state_of_the_hand_and_the_target_while_pinched() {
    let mut s = previews();
    let t = texts_of_preview(&mut preview(s.remove(0).1));
    assert!(t.iter().any(|s| s == "NO HAND"), "{t:?}");
    let t = texts_of_preview(&mut preview(s.remove(0).1));
    assert!(t.iter().any(|s| s == "HAND"), "{t:?}");
    let t = texts_of_preview(&mut preview(s.remove(0).1));
    assert!(t.iter().any(|s| s == "PINCH"), "{t:?}");
    assert!(t.iter().any(|s| s == "Lead Cutoff") && t.iter().any(|s| s == "2.4 kHz"));
    let t = texts_of_preview(&mut preview(s.remove(0).1));
    assert!(t.iter().any(|s| s == "Click a knob first"), "{t:?}");
}

#[test]
fn the_fps_and_the_inference_time_are_the_tooltip_of_the_box() {
    let v = preview(preview_state(CameraLamp::Hand, 1, None, None));
    let inside = v.picture_rect().center();
    let help = v.help_at(inside).expect("a tooltip");
    assert_eq!(help.text, "30 fps, models 21 ms per frame");
    assert!(v.help_at(Point::new(5.0, 5.0)).is_none());
    // The close button has its own, with the command.
    let close = v.help_at(v.close_rect().center()).unwrap();
    assert_eq!(close.command, Some("camera.preview"));
}

#[test]
fn the_box_takes_its_own_presses_and_the_close_runs_the_preview_command() {
    let mut v = preview(preview_state(CameraLamp::Hand, 1, None, None));
    let (taken, out) = v.handle(&down(v.picture_rect().center()));
    assert!(taken && out.is_empty(), "a press on the box is taken");
    let (taken, out) = v.handle(&down(Point::new(5.0, 5.0)));
    assert!(!taken && out.is_empty(), "elsewhere is not the box's");
    let (taken, out) = v.handle(&down(v.close_rect().center()));
    assert!(taken);
    assert_eq!(
        out,
        vec![UiAction::RunCommand {
            id: "camera.preview".into(),
            arg: None
        }]
    );
}

#[test]
fn the_box_is_the_size_it_says_and_a_new_state_repaints_but_the_same_one_does_not() {
    let mut v = preview(preview_state(CameraLamp::Hand, 1, None, None));
    let b = v.bounds();
    assert_eq!(
        (b.width(), b.height()),
        (CAMERA_PREVIEW_WIDTH, CAMERA_PREVIEW_HEIGHT)
    );
    assert!(v.picture_rect().y1 < b.y1 && v.picture_rect().x0 > b.x0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    assert!(!v.needs_repaint());
    v.set_state(preview_state(CameraLamp::Hand, 1, None, None));
    assert!(!v.needs_repaint(), "the same state");
    v.set_state(preview_state(CameraLamp::Pinch, 1, None, None));
    assert!(v.needs_repaint());
}

#[test]
fn the_picture_is_mirrored_like_a_selfie() {
    // The hand's wrist is at the left of the camera picture: in the mirror it is at the right.
    let mut v = preview(preview_state(CameraLamp::Hand, 1, None, None));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    let json = dl.to_json();
    assert!(
        json.contains("-1"),
        "a horizontal flip is in the display list"
    );
}

#[test]
fn the_preview_in_each_state_matches_its_golden() {
    for (name, state) in previews() {
        let mut v = preview(state);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &Theme::dark());
        starter::check_golden(&format!("camera_preview_{name}_1x.json"), &dl.to_json());
    }
}
