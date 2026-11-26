//! The key and chord picker and the lanes in the shell: Transport > Song Key opens the picker on the
//! song key, a double click on a chord opens it on the chord and a typed symbol changes it, Escape
//! closes without a change, and the lanes are switched by View > Lanes and Markers and kept in the
//! interface state. The shell is built as Windows, so Ctrl is the command key on every host.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::harmony::Key as SongKey;
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{Clip, ClipContent, ClipId, CreateOptions, CreateSource, Note, WorkflowCommand};
use atomik_ui::{HarmonyTarget, Key, Modifiers, PointerButton};

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
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s
}

fn click_n(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point, clicks: u8) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        s.handle(&UiEvent::Key {
            key: Key::Char(c),
            modifiers: Modifiers::NONE,
            pressed: true,
        });
    }
}

fn press(s: &mut Shell<NullBackend>, key: Key) {
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

/// Shows the chord lane and clicks a chord into it; returns the shell and the chord's start.
fn with_chord() -> (Shell<NullBackend>, i64) {
    let mut s = shell();
    s.execute(AppCommand::ToggleChordLane);
    assert!(s.core().arrangement().chord_lane_visible());
    let lane = s.core().arrangement().chord_lane_rect();
    let p = atomik_ui::kurbo::Point::new(lane.x0 + 100.0, lane.center().y);
    click_n(&mut s, p, 1);
    assert_eq!(s.core().harmony().chords.len(), 1, "a click adds a chord");
    let tick = s.core().harmony().chords[0].tick;
    (s, tick)
}

#[test]
fn the_song_key_command_opens_the_picker_and_apply_sets_the_key_in_one_undo_step() {
    let mut s = shell();
    assert!(s.harmony_picker_view().is_none());
    assert_eq!(s.execute(AppCommand::SetSongKey), vec![]);
    assert!(s.modal_open());
    assert!(!s.wants_text_input(), "a key picker has no text field");
    let picker = s.harmony_picker_view().unwrap();
    assert_eq!(picker.target(), HarmonyTarget::SongKey);
    let minor = ScaleKind::ALL
        .iter()
        .filter(|k| **k != ScaleKind::Chromatic)
        .position(|k| *k == ScaleKind::NaturalMinor)
        .unwrap();
    let (a, minor, apply) = (
        picker.root_rect(9).center(),
        picker.scale_rect(minor).center(),
        picker.apply_rect().center(),
    );
    click_n(&mut s, a, 1);
    click_n(&mut s, minor, 1);
    assert_eq!(s.core().harmony().key, None, "nothing is set before APPLY");
    click_n(&mut s, apply, 1);
    assert!(s.harmony_picker_view().is_none());
    assert_eq!(
        s.core().harmony().key,
        Some(SongKey::new(9, ScaleKind::NaturalMinor))
    );
    assert!(s.core_mut().undo());
    assert_eq!(s.core().harmony().key, None);
}

#[test]
fn escape_closes_the_picker_and_remove_clears_the_key() {
    let mut s = shell();
    s.execute(AppCommand::SetSongKey);
    press(&mut s, Key::Escape);
    assert!(s.harmony_picker_view().is_none());
    assert!(s.core().harmony().is_empty());
    s.core_mut()
        .set_song_key(Some(SongKey::new(2, ScaleKind::Dorian)))
        .unwrap();
    s.execute(AppCommand::SetSongKey);
    let remove = s.harmony_picker_view().unwrap().remove_rect().center();
    click_n(&mut s, remove, 1);
    assert_eq!(s.core().harmony().key, None);
    assert!(s.harmony_picker_view().is_none());
}

#[test]
fn a_double_click_on_a_chord_opens_the_picker_and_a_typed_symbol_changes_the_chord() {
    let (mut s, tick) = with_chord();
    let inside = s.core().arrangement().chord_center(tick).unwrap();
    click_n(&mut s, inside, 2);
    let picker = s.harmony_picker_view().expect("the picker opened");
    assert!(matches!(picker.target(), HarmonyTarget::Chord { .. }));
    assert!(s.wants_text_input(), "a chord picker has a symbol field");
    // The field starts selected: typing replaces the chord.
    type_text(&mut s, "Am7/G");
    press(&mut s, Key::Enter);
    assert!(s.harmony_picker_view().is_none());
    let chord = s.core().harmony().chords[0];
    assert_eq!(chord.symbol(), "Am7/G");
    assert_eq!((chord.root, chord.quality), (9, ChordKind::MinorSeventh));
    assert_eq!(s.core().harmony().chords.len(), 1);
}

#[test]
fn a_symbol_that_cannot_be_read_keeps_the_picker_open_and_the_song_unchanged() {
    let (mut s, tick) = with_chord();
    let before = s.core().harmony().clone();
    let inside = s.core().arrangement().chord_center(tick).unwrap();
    click_n(&mut s, inside, 2);
    type_text(&mut s, "Hmm");
    press(&mut s, Key::Enter);
    assert!(s.harmony_picker_view().is_some(), "Enter waits");
    assert!(s.harmony_picker_view().unwrap().error().is_some());
    press(&mut s, Key::Escape);
    assert_eq!(*s.core().harmony(), before);
}

#[test]
fn the_lane_commands_toggle_the_lanes_and_keep_the_choice() {
    let mut s = shell();
    assert!(!s.core().arrangement().key_lane_visible());
    s.execute(AppCommand::ToggleKeyLane);
    s.execute(AppCommand::ToggleChordLane);
    assert!(s.core().arrangement().key_lane_visible());
    assert!(s.core().arrangement().chord_lane_visible());
    assert!(s.ui.show_key_lane && s.ui.show_chord_lane);
    s.execute(AppCommand::ToggleKeyLane);
    assert!(!s.core().arrangement().key_lane_visible());
    assert!(!s.ui.show_key_lane);
}

#[test]
fn detect_song_key_sets_the_key_of_the_notes_in_one_undo_step() {
    let mut s = shell();
    let lead = atomik_model::DeviceKey("lead".into());
    s.core_mut()
        .apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(lead.clone()),
            options: CreateOptions::default(),
        }))
        .unwrap();
    let notes: Vec<Note> = [0u8, 2, 4, 5, 7, 9, 11]
        .iter()
        .enumerate()
        .map(|(i, k)| Note::new(i as i64 * PPQ, PPQ, 60 + k, 0.8))
        .collect();
    s.core_mut()
        .apply_command(Command::Song(atomik_model::SongCommand::AddClip {
            device: lead,
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 8 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                follow_song_key: true,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes.into()),
            },
        }))
        .unwrap();
    assert_eq!(s.execute(AppCommand::DetectKey), vec![]);
    assert_eq!(
        s.core().harmony().key,
        Some(SongKey::new(0, ScaleKind::Major))
    );
    assert!(s.core_mut().undo());
    assert_eq!(s.core().harmony().key, None);
}

fn with_quantizer() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut()
        .apply_command(atomik_model::Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.cv-quantizer".into(),
            key: Some(atomik_model::DeviceKey("q".into())),
        })
        .unwrap();
    s
}

fn picker_action(own: u16, id: Option<u8>) -> UiAction {
    UiAction::SongScalePicker {
        device: atomik_model::DeviceKey("q".into()),
        id,
        own,
    }
}

#[test]
fn a_new_song_scale_from_a_quantizer_is_saved_in_the_song_and_the_quantizer_names_it() {
    let mut s = with_quantizer();
    s.pane_actions(&[picker_action(0b1_0001_0101, None)]);
    let v = s.harmony_picker_view().expect("the scale picker");
    assert_eq!(v.target(), HarmonyTarget::Scale { id: None });
    assert_eq!(v.scale_name(), "Scale", "a name nobody has");
    assert!(s.wants_text_input(), "the name field takes typed text");
    let apply = v.apply_rect().center();
    click_n(&mut s, apply, 1);
    assert!(s.harmony_picker_view().is_none());
    let scales = s.core().harmony().scales.clone();
    assert_eq!(scales.len(), 1);
    assert_eq!(
        (scales[0].name.as_str(), scales[0].mask),
        ("Scale", 0b1_0001_0101)
    );
    // The quantizer follows the new scale (source named, its id).
    let dev = s
        .core()
        .doc()
        .devices
        .iter()
        .find(|d| d.key.0 == "q")
        .unwrap();
    assert_eq!(dev.params.get("scale.source"), Some(&2.0));
    assert_eq!(dev.params.get("scale.id"), Some(&f32::from(scales[0].id)));
}

#[test]
fn editing_a_song_scale_is_one_undo_step_and_remove_deletes_it() {
    let mut s = with_quantizer();
    s.pane_actions(&[picker_action(0b101, None)]);
    let apply = s.harmony_picker_view().unwrap().apply_rect().center();
    click_n(&mut s, apply, 1);
    let id = s.core().harmony().scales[0].id;
    // Edit: turn the note D (pitch class 2) off.
    s.pane_actions(&[picker_action(0, Some(id))]);
    let v = s.harmony_picker_view().unwrap();
    assert_eq!(v.target(), HarmonyTarget::Scale { id: Some(id) });
    let (d, apply) = (v.root_rect(1).center(), v.apply_rect().center());
    click_n(&mut s, d, 1);
    click_n(&mut s, apply, 1);
    assert_eq!(s.core().harmony().scales[0].mask, 0b111);
    assert!(s.core_mut().undo());
    assert_eq!(s.core().harmony().scales[0].mask, 0b101);
    // REMOVE.
    s.pane_actions(&[picker_action(0, Some(id))]);
    let remove = s.harmony_picker_view().unwrap().remove_rect().center();
    click_n(&mut s, remove, 1);
    assert!(s.core().harmony().scales.is_empty());
}
