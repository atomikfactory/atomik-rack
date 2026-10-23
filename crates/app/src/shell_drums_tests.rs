//! `Shell` tests for the drum machine: the step editor in the lower pane,
//! pad and background drops on the rack panel and the editor, the Load button on a drum
//! machine, and a double-click on a drum clip in the arrangement.

use super::*;
use crate::session::NullBackend;
use atomik_audio_file::AudioData;
use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipId, Note, SongCommand, pad_key};
use atomik_ui::{Key, Modifiers, PointerButton};

const BAR: i64 = 4 * PPQ;

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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(s: &mut Shell<NullBackend>, type_id: &str, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        })
        .unwrap();
}

fn shell_with_drums() -> Shell<NullBackend> {
    let mut s = shell();
    add(&mut s, "atomik.drum-machine", "drm");
    s
}

fn wav(name: &str, frames: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-drums-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn zone_keys(s: &Shell<NullBackend>) -> Vec<u8> {
    s.core()
        .doc()
        .device(&key("drm"))
        .unwrap()
        .zones
        .iter()
        .map(|z| z.key_lo)
        .collect()
}

fn down(pos: Point, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    }
}

fn up(pos: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

/// A new pattern on the drum machine through the actions, with the editor showing.
fn open_pattern(s: &mut Shell<NullBackend>) -> ClipId {
    let create = UiAction::CreateStepClip { device: key("drm") };
    s.core_mut().handle_action(create.clone());
    s.pane_actions(&[create]);
    s.core().step_editor().clip().unwrap()
}

/// A window point on the drum panel: a pad, or the grid background.
fn panel_point(s: &Shell<NullBackend>, pad: Option<u8>) -> Point {
    let view = s.core().view();
    let p = match pad {
        Some(n) => view.pad_center(&key("drm"), n),
        None => view.pad_grid_background(&key("drm")),
    }
    .unwrap();
    Point::new(p.x, p.y + s.layout().rack.y0)
}

fn clip_notes(s: &Shell<NullBackend>, id: ClipId) -> Vec<Note> {
    match &s.core().doc().song.clip(id).unwrap().2.content {
        ClipContent::Notes(n) => n.clone(),
        _ => unreachable!(),
    }
}

#[test]
fn view_step_editor_opens_on_the_selected_else_the_first_drum_machine_and_closes_again() {
    let mut s = shell();
    s.execute(AppCommand::ToggleStepEditor);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no drum machine")
    );
    assert_eq!(s.lower_pane(), Pane::Arrangement);

    let mut s = shell_with_drums();
    add(&mut s, "atomik.drum-machine", "drm2");
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(
        s.core().step_editor_device(),
        Some(&key("drm")),
        "the first"
    );
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    assert_eq!(s.focus(), Pane::StepEditor);
    assert!(s.menu_state().step_editor);
    let song = s.layout().song.unwrap();
    assert_eq!(
        s.core().step_editor().bounds(),
        song,
        "same bounds as the arrangement"
    );
    assert_eq!(s.hit_pane(song.center()), Some(Pane::StepEditor));
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().step_editor);

    s.core_mut().select(Some(key("drm2")));
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(
        s.core().step_editor_device(),
        Some(&key("drm2")),
        "the selected"
    );
}

#[test]
fn the_view_menu_lists_the_step_editor_after_the_sample_editor() {
    let s = shell_with_drums();
    let model = s.menu_model();
    let view = model.iter().find(|m| m.title == "View").unwrap();
    let labels: Vec<&str> = view
        .entries
        .iter()
        .filter_map(|e| match e {
            crate::commands::MenuEntry::Item(i) => Some(i.label),
            _ => None,
        })
        .collect();
    let at = labels.iter().position(|l| *l == "Sample Editor").unwrap();
    assert_eq!(labels[at + 1], "Step Editor");
}

#[test]
fn the_edit_action_shows_the_step_editor_even_over_the_console() {
    let mut s = shell_with_drums();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    let open = UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    };
    s.core_mut().handle_action(open.clone());
    s.pane_actions(&[open]);
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    assert_eq!(s.focus(), Pane::StepEditor);
    assert!(!s.menu_state().console);
}

#[test]
fn the_step_editor_and_the_other_lower_panes_exclude_each_other() {
    let mut s = shell_with_drums();
    add(&mut s, "atomik.sampler", "smp");
    s.execute(AppCommand::ToggleStepEditor);
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    s.execute(AppCommand::ToggleSampleEditor);
    assert!(s.core().step_editor_device().is_none());
    assert_eq!(s.lower_pane(), Pane::SampleEditor);
    s.execute(AppCommand::ToggleStepEditor);
    assert!(s.core().sample_editor_device().is_none());
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console, "the console shows over it");
}

#[test]
fn esc_closes_the_step_editor_and_a_removed_drum_machine_closes_it_too() {
    let mut s = shell_with_drums();
    open_pattern(&mut s);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    s.execute(AppCommand::ToggleStepEditor);
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("drm") })
        .unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn creating_a_pattern_shows_the_editor_and_a_click_on_a_cell_edits_the_document() {
    let mut s = shell_with_drums();
    let id = open_pattern(&mut s);
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    let at = s.core().step_editor().cell_center(1, 2);
    s.handle(&down(at, 1));
    s.handle(&up(at));
    let notes = clip_notes(&s, id);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].key, notes[0].tick), (36, 2 * 3840));
}

#[test]
fn a_drag_that_leaves_the_pane_keeps_going_to_the_editor_until_the_release() {
    let mut s = shell_with_drums();
    let id = open_pattern(&mut s);
    // Paint across the row, then leave the pane over the transport bar and release there.
    let from = s.core().step_editor().cell_center(2, 0);
    let to = s.core().step_editor().cell_center(2, 3);
    s.handle(&down(from, 1));
    s.handle(&UiEvent::PointerMove {
        pos: to,
        modifiers: Modifiers::NONE,
    });
    assert!(s.core().step_editor().is_dragging());
    // Above the pane, over the transport bar, in the column of step 3.
    let outside = Point::new(to.x, s.layout().transport.center().y);
    s.handle(&UiEvent::PointerMove {
        pos: outside,
        modifiers: Modifiers::NONE,
    });
    s.handle(&up(outside));
    assert!(
        !s.core().step_editor().is_dragging(),
        "the release reached the editor"
    );
    assert_eq!(clip_notes(&s, id).len(), 4, "steps 0 to 3 of pad 2");
}

// ------------------------------------------------------------------ drops

#[test]
fn a_drop_on_a_pad_of_the_panel_loads_that_pad_and_on_its_background_fills_empty_pads() {
    let mut s = shell_with_drums();
    let at = panel_point(&s, Some(5));
    let paths = vec![wav("kick.wav", 2_400)];
    s.handle(&UiEvent::FileHover {
        pos: at,
        paths: paths.clone(),
    });
    assert_eq!(s.core().view().drop_pad(), Some((&key("drm"), 5)));
    s.handle(&UiEvent::FileDrop { pos: at, paths });
    s.core_mut().wait_background();
    assert_eq!(zone_keys(&s), [pad_key(5)]);
    assert!(
        s.core().doc().song.tracks.is_empty(),
        "no audio tracks made"
    );

    let bg = panel_point(&s, None);
    s.handle(&UiEvent::FileDrop {
        pos: bg,
        paths: vec![wav("b_snare.wav", 1_200), wav("a_clap.wav", 1_200)],
    });
    s.core_mut().wait_background();
    // Pad 5 is taken: the files, in file-name order, go to pads 1 and 2.
    assert_eq!(zone_keys(&s), [pad_key(5), pad_key(1), pad_key(2)]);
    assert!(s.core().doc().song.tracks.is_empty());
}

#[test]
fn files_dropped_on_the_step_editor_load_the_row_pad_or_fill_the_empty_pads() {
    let mut s = shell_with_drums();
    open_pattern(&mut s);
    let header = s.core().step_editor().header_center(3);
    let paths = vec![wav("hit.wav", 2_400)];
    s.handle(&UiEvent::FileHover {
        pos: header,
        paths: paths.clone(),
    });
    assert_eq!(
        s.core().step_editor().drop_hover(),
        Some(Some(pad_key(3))),
        "the row header is marked"
    );
    // Over the transport bar the indicator goes away.
    s.handle(&UiEvent::FileHover {
        pos: s.layout().transport.center(),
        paths: paths.clone(),
    });
    assert!(s.core().step_editor().drop_hover().is_none());
    s.handle(&UiEvent::FileDrop { pos: header, paths });
    s.core_mut().wait_background();
    assert_eq!(zone_keys(&s), [pad_key(3)]);
    // Elsewhere in the pane: the background rule.
    let cell = s.core().step_editor().cell_center(8, 5);
    s.handle(&UiEvent::FileDrop {
        pos: cell,
        paths: vec![wav("other.wav", 1_200)],
    });
    s.core_mut().wait_background();
    assert_eq!(zone_keys(&s), [pad_key(3), pad_key(1)]);
    assert_eq!(
        s.core().doc().song.tracks[0].lanes[0].clips.len(),
        1,
        "no clips were imported"
    );
}

#[test]
fn the_load_button_of_the_sample_editor_fills_the_pads_of_a_drum_machine() {
    let mut s = shell_with_drums();
    s.execute(AppCommand::ToggleSampleEditor);
    assert_eq!(s.core().sample_editor_device(), Some(&key("drm")));
    let p = s.core().sample_editor().load_button_center();
    let mut requests = s.handle(&down(p, 1));
    requests.extend(s.handle(&up(p)));
    assert_eq!(requests, vec![ShellRequest::PickLoadSamples]);
    s.load_samples(vec![wav("b_picked.wav", 2_400), wav("a_picked.wav", 2_400)]);
    s.core_mut().wait_background();
    assert_eq!(zone_keys(&s), [pad_key(1), pad_key(2)]);
}

// ------------------------------------------------------------------ the arrangement

#[test]
fn a_double_click_on_a_drum_clip_opens_the_step_editor_and_a_synth_clip_the_piano_roll() {
    let mut s = shell_with_drums();
    add(&mut s, "atomik.poly-synth", "syn");
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        grid: None,
        content: ClipContent::Notes(vec![Note::new(0, PPQ, 36, 0.8)]),
    };
    let mut ids = Vec::new();
    for k in ["drm", "syn"] {
        s.core_mut()
            .apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
            .unwrap();
        s.core_mut()
            .apply_command(Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: clip.clone(),
            }))
            .unwrap();
        ids.push(s.core().document().last_clip_id().unwrap());
    }
    let center =
        |s: &Shell<NullBackend>, id| s.core().arrangement().clip_rect(id).unwrap().center();
    // The synth clip: the piano roll, as before.
    let at = center(&s, ids[1]);
    s.handle(&down(at, 2));
    s.handle(&up(at));
    assert_eq!(s.lower_pane(), Pane::PianoRoll);
    s.core_mut().close_piano_roll();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    // The drum clip: the step editor, on that clip, with the keys.
    let at = center(&s, ids[0]);
    s.handle(&down(at, 2));
    s.handle(&up(at));
    assert_eq!(s.lower_pane(), Pane::StepEditor);
    assert_eq!(s.focus(), Pane::StepEditor);
    assert_eq!(s.core().step_editor().clip(), Some(ids[0]));
    assert!(s.core().piano_roll_clip().is_none());
    // View > Piano Roll still reaches the piano roll for that clip.
    s.core_mut().arrangement_mut().set_selected_clips(&[ids[0]]);
    s.execute(AppCommand::TogglePianoRoll);
    assert_eq!(s.lower_pane(), Pane::PianoRoll);
    assert_eq!(s.core().piano_roll_clip(), Some(ids[0]));
}
