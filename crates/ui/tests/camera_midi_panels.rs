//! The camera as a MIDI controller, drawn: the preview box with six bars of live values under the
//! hand and its channel (or the note that the camera sends for MIDI Learn), and the MIDI
//! Mappings list reading a camera source as "Camera, right hand twist" with its message under
//! it. The views are pure: they draw a state they are given. Goldens in English here and in
//! French in `french_goldens.rs`.

mod support;

use atomik_model::{
    CAMERA_PORT, CameraControl, CameraHand, DeviceKey, MapMode, MapOptions, MidiMapping,
    MidiSource, MidiTarget, RelativeKind, SourceKind,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    CAMERA_PREVIEW_HEIGHT, CameraBar, CameraControlsState, CameraLamp, CameraPicture,
    CameraPreviewState, CameraPreviewView, DisplayList, MidiMapRow, MidiMapsView, Theme,
};
use support::*;

fn controls(learning: bool) -> CameraControlsState {
    let bar = |label, value, centred| CameraBar {
        label,
        value,
        centred,
    };
    CameraControlsState {
        hand: "Right hand, channel 1",
        bars: vec![
            bar("Twist", 0.4, true),
            bar("Height", 0.8, false),
            bar("Left/right", 0.5, false),
            bar("Closeness", 0.65, false),
            bar("Pinch", 1.0, false),
            bar("Palm", 0.0, false),
        ],
        learning,
    }
}

/// A pinching hand with the camera's MIDI controls shown.
fn midi_preview(learning: bool) -> CameraPreviewState {
    CameraPreviewState {
        lamp: CameraLamp::Pinch,
        picture: Some(CameraPicture {
            width: 2,
            height: 2,
            rgb: vec![10; 12],
        }),
        hands: vec![
            (0..21)
                .map(|i| (0.2 + 0.03 * i as f32, 0.9 - 0.035 * i as f32))
                .collect(),
        ],
        target: None,
        value: None,
        message: None,
        fps: 29.6,
        infer_ms: 21.4,
        controls: Some(controls(learning)),
    }
}

fn preview(state: CameraPreviewState) -> CameraPreviewView {
    let mut v = CameraPreviewView::new();
    v.set_origin(Point::new(660.0, 330.0));
    v.set_state(state);
    v
}

fn texts_of(v: &mut CameraPreviewView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &Theme::dark());
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_midi_preview_shows_six_bars_under_the_hand_and_its_channel() {
    let mut v = preview(midi_preview(false));
    let t = texts_of(&mut v);
    for word in [
        "Right hand, channel 1",
        "Twist",
        "Height",
        "Left/right",
        "Closeness",
        "Pinch",
        "Palm",
    ] {
        assert!(t.iter().any(|s| s == word), "{word} in {t:?}");
    }
    let b = v.bounds();
    assert!(
        b.height() > CAMERA_PREVIEW_HEIGHT,
        "the box grows to hold them"
    );
    assert_eq!(
        b.height(),
        CameraPreviewView::height_for(&midi_preview(false))
    );
    let plain = CameraPreviewState {
        controls: None,
        ..midi_preview(false)
    };
    assert_eq!(
        CameraPreviewView::height_for(&plain),
        CAMERA_PREVIEW_HEIGHT,
        "the other mode keeps the size it had"
    );
}

#[test]
fn learning_without_the_midi_mode_says_the_camera_is_sending_for_learn() {
    let mut v = preview(midi_preview(true));
    let t = texts_of(&mut v);
    assert!(t.iter().any(|s| s == "Sending for MIDI Learn"), "{t:?}");
    assert!(!t.iter().any(|s| s == "Right hand, channel 1"));
}

#[test]
fn the_midi_preview_in_each_state_matches_its_golden() {
    for (name, learning) in [("midi", false), ("midi_learn", true)] {
        let mut v = preview(midi_preview(learning));
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &Theme::dark());
        starter::check_golden(&format!("camera_preview_{name}_1x.json"), &dl.to_json());
    }
}

/// The MIDI Mappings list: two camera sources, a hardware source and a camera source edited to
/// any channel (which the camera does not send, so it reads as a plain one).
fn mappings_view() -> MidiMapsView {
    let row = |id: u32, source: MidiSource, mode: MapMode, name: &str| MidiMapRow {
        mapping: MidiMapping {
            id,
            source,
            target: MidiTarget::Param {
                device: DeviceKey("ch".into()),
                param: "fader".into(),
            },
            options: MapOptions {
                mode,
                ..MapOptions::default()
            },
        },
        target: name.to_owned(),
        missing: false,
    };
    let mut odd = MidiSource::camera(CameraHand::Right, CameraControl::Height);
    odd.channel = None;
    let mut v = MidiMapsView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 400.0));
    v.set_rows(vec![
        row(
            1,
            MidiSource::camera(CameraHand::Right, CameraControl::Twist),
            MapMode::Relative(RelativeKind::TwosComplement),
            "Lead Cutoff",
        ),
        row(
            2,
            MidiSource::camera(CameraHand::Left, CameraControl::OpenPalm),
            MapMode::Toggle,
            "Lead Mute",
        ),
        row(
            3,
            MidiSource::any_port(Some(0), SourceKind::Cc(21)),
            MapMode::Absolute,
            "Lead Pan",
        ),
        row(4, odd, MapMode::Absolute, "Lead Send"),
    ]);
    v.set_ports(vec!["Pads".into(), CAMERA_PORT.into()]);
    v
}

#[test]
fn a_camera_source_reads_as_the_camera_with_its_message_under_it() {
    let v = mappings_view();
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let t: Vec<&str> = dl.texts();
    assert!(t.contains(&"Camera, right hand twist"), "{t:?}");
    assert!(
        t.contains(&"CC 20 ch 1"),
        "the message stays visible: {t:?}"
    );
    assert!(t.contains(&"Camera, left hand open palm"), "{t:?}");
    assert!(t.contains(&"Note C#3 ch 2"), "{t:?}");
    assert!(t.contains(&"Camera"), "the port cell says Camera: {t:?}");
    // A hardware source reads as it always did.
    assert!(t.contains(&"CC 21"), "{t:?}");
    assert!(!t.iter().any(|s| s.contains("atomik:camera")), "{t:?}");
}

#[test]
fn the_mappings_list_with_camera_sources_matches_its_golden() {
    let v = mappings_view();
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    starter::check_golden("midi_maps_camera_1x.json", &dl.to_json());
}
