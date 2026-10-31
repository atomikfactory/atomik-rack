//! The Key & Chord panel's FROM CLIP button through the shell: it copies the scale of the clip
//! open in the piano roll into `scale.root` and `scale.kind` as one undo step, and does nothing
//! while no clip with a scale is open. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::theory::{Scale, ScaleKind};
use atomik_model::{Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand};
use atomik_ui::{Modifiers, PointerButton};

const KEY_CHORD: &str = "atomik.key-chord";

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
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    // Without the console devices the panel is near the top of the rack, in view.
    s.core_mut().view_mut().set_show_console_devices(false);
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.into())
}

/// A synth `a` with one note clip (id 1), and a Key & Chord `kc`.
fn setup() -> Shell<NullBackend> {
    let mut s = shell();
    // The Key & Chord is created first, so its panel is in view at the top of the rack.
    for (type_id, k) in [(KEY_CHORD, "kc"), ("atomik.poly-synth", "a")] {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.into(),
                key: Some(key(k)),
            })
            .unwrap();
    }
    let song = |s: &mut Shell<NullBackend>, c: SongCommand| {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    };
    song(&mut s, SongCommand::AddTrack { device: key("a") });
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(vec![Note::new(0, PPQ, 60, 0.8)].into()),
    };
    song(
        &mut s,
        SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip,
        },
    );
    s
}

fn value(s: &Shell<NullBackend>, p: &str) -> f32 {
    s.core()
        .doc()
        .param_value(&key("kc"), p, s.core().registry())
        .unwrap()
}

fn open_roll(s: &mut Shell<NullBackend>) {
    s.core_mut().open_piano_roll(ClipId(1));
    s.core_mut().after_view_change();
}

fn set_clip_scale(s: &mut Shell<NullBackend>, scale: Option<Scale>) {
    let c = Command::Song(SongCommand::SetClipScale {
        clip: ClipId(1),
        scale,
    });
    s.core_mut().apply_command(c).unwrap();
}

fn click_from_clip(s: &mut Shell<NullBackend>) {
    let origin = s.layout().rack.origin();
    let at = s
        .core()
        .view()
        .from_clip_button_center(&key("kc"))
        .expect("the button is in view");
    let p = Point::new(at.x + origin.x, at.y + origin.y);
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn from_clip_sets_root_and_kind_in_one_undo_step() {
    let mut s = setup();
    set_clip_scale(&mut s, Some(Scale::new(2, ScaleKind::Dorian)));
    open_roll(&mut s);
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (0.0, 1.0)
    );
    click_from_clip(&mut s);
    // D dorian: root 2, kind index 5.
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (2.0, 5.0)
    );
    assert!(s.core_mut().undo());
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (0.0, 1.0),
        "both params come back with one undo"
    );
}

#[test]
fn from_clip_does_nothing_without_an_open_clip_or_a_scale() {
    let mut s = setup();
    // No clip open.
    click_from_clip(&mut s);
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (0.0, 1.0)
    );
    // A clip without a scale.
    open_roll(&mut s);
    click_from_clip(&mut s);
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (0.0, 1.0)
    );
    // Giving the open clip a scale enables the button; closing the roll disables it again.
    set_clip_scale(&mut s, Some(Scale::new(9, ScaleKind::MinorPentatonic)));
    click_from_clip(&mut s);
    assert_eq!(
        (value(&s, "scale.root"), value(&s, "scale.kind")),
        (9.0, 11.0)
    );
    s.core_mut()
        .apply_command(Command::SetParam {
            key: key("kc"),
            param: "scale.root".into(),
            value: 4.0,
            merge: None,
        })
        .unwrap();
    s.core_mut().close_piano_roll();
    s.core_mut().after_view_change();
    click_from_clip(&mut s);
    assert_eq!(value(&s, "scale.root"), 4.0);
}
