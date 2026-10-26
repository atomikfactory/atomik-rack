//! `Shell` Audio to Notes tests: the menu item and its registry id, the panel (opening, settings
//! kept between openings, Esc and CLOSE), EXTRACT starting the worker and closing the panel, and
//! the notes arriving as one undo step.

use super::*;
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{ClipContent, ClipId};
use atomik_ui::{ImportTarget, NotesMode, NotesRow};

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
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

/// A 1.2 s mono file with an A3 from 0.1 s to 0.6 s and an E4 from 0.7 s to 1.1 s.
fn imported(s: &mut Shell<NullBackend>) -> ClipId {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-notes-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..57_600)
        .map(|i| {
            let t = f64::from(i) / 48_000.0;
            let f = if (0.1..0.6).contains(&t) {
                220.0
            } else if (0.7..1.1).contains(&t) {
                329.63
            } else {
                return 0.0;
            };
            (0.5 * (std::f64::consts::TAU * f * t).sin()) as f32
        })
        .collect();
    let path = dir.join("tune.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    s.core_mut()
        .import_audio(vec![path], ImportTarget::NewTracks, 0);
    s.core_mut().wait_background();
    let _ = std::fs::remove_dir_all(&dir);
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .find(|c| c.content.is_audio())
        .map(|c| c.id)
        .expect("an audio clip")
}

fn item(s: &Shell<NullBackend>, label: &str) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .find(|m| m.title == "Audio")
        .unwrap()
        .items()
        .find(|i| i.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("Audio > {label}"))
}

fn note_keys(s: &Shell<NullBackend>) -> Vec<u8> {
    let clip = &s.core().doc().song.tracks.last().unwrap().lanes[0].clips[0];
    match &clip.content {
        ClipContent::Notes(n) => n.iter().map(|n| n.key).collect(),
        _ => vec![],
    }
}

#[test]
fn the_menu_item_needs_an_audio_clip_and_has_a_registry_id() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    let label = "Audio to Notes\u{2026}";
    assert!(!item(&s, label).enabled);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let it = item(&s, label);
    assert!(it.enabled);
    assert_eq!(it.command, AppCommand::AudioToNotes);
    assert_eq!(
        it.command.command_id(),
        Some(("sequencer.audio_to_notes", None))
    );
    assert_eq!(
        AppCommand::from_command("sequencer.audio_to_notes", None),
        Some(AppCommand::AudioToNotes)
    );
}

#[test]
fn the_panel_opens_keeps_its_settings_and_extracts_in_one_undo_step() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(!s.notes_open());
    assert!(s.execute(AppCommand::AudioToNotes).is_empty());
    assert!(s.notes_open());
    // A changed value is kept for the next opening.
    let plus = s
        .notes_view()
        .unwrap()
        .plus_rect(NotesRow::Sensitivity)
        .center();
    click(&mut s, plus);
    assert_eq!(s.notes_values().sensitivity, 55);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.notes_open());
    s.execute(AppCommand::AudioToNotes);
    assert_eq!(s.notes_view().unwrap().values().sensitivity, 55);
    // EXTRACT starts the worker and closes the panel; the notes come when it is done.
    let before = s.core().doc().clone();
    let extract = s.notes_view().unwrap().apply_rect().center();
    click(&mut s, extract);
    assert!(!s.notes_open(), "the panel closes when the work starts");
    assert!(s.core().notes_busy());
    s.core_mut().wait_background();
    assert_eq!(note_keys(&s), vec![57, 64]);
    s.core_mut().undo();
    assert_eq!(s.core().doc(), &before);
    // CLOSE closes it too.
    s.execute(AppCommand::AudioToNotes);
    let close = s.notes_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.notes_open());
}

#[test]
fn the_registry_command_opens_the_panel_and_a_refusal_keeps_it_open() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.run_command("sequencer.audio_to_notes", None);
    assert!(s.notes_open());
    // The target names the file; with nothing selected EXTRACT does nothing.
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.handle(&UiEvent::PointerLeave);
    let apply = s.notes_view().unwrap().apply_rect().center();
    let before = s.core().doc().clone();
    click(&mut s, apply);
    assert!(s.notes_open());
    assert_eq!(s.core().doc(), &before);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"AUDIO TO NOTES"));
}

#[test]
fn the_headless_entry_runs_with_given_values_and_remembers_them() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let values = NotesValues {
        mode: NotesMode::Pitched,
        min_note_ms: 200,
        ..NotesValues::default()
    };
    assert!(s.audio_to_notes(&values));
    s.core_mut().wait_background();
    // Both notes are 400 ms or longer.
    assert_eq!(note_keys(&s), vec![57, 64]);
    assert_eq!(s.notes_values().min_note_ms, 200);
}
