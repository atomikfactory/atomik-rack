//! `Shell` audio tests: File > Import Audio, Ctrl+I, file drops and hovers.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_audio_file::AudioData;
use atomik_ui::{ImportTarget, Key, Modifiers};

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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn wav(name: &str, frames: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn audio_clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .map(|l| l.clips.len())
        .sum()
}

#[test]
fn import_audio_is_a_file_menu_item_with_ctrl_shift_i() {
    let s = shell();
    let file = &s.menu_model()[0];
    let item = file
        .items()
        .find(|i| i.command == AppCommand::ImportAudio)
        .expect("File > Import Audio");
    assert_eq!(item.label, "Import Audio\u{2026}");
    assert_eq!(item.shortcut.as_deref(), Some("Ctrl+Shift+I"));
    assert!(item.enabled);
    assert!(
        file.entries
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(i) if i.command == AppCommand::ImportAudio))
    );
}

#[test]
fn ctrl_shift_i_and_the_menu_command_ask_the_platform_for_audio_files() {
    let mut s = shell();
    // Ctrl+I is Create Instrument now; import moved to Ctrl+Shift+I.
    let ctrl_shift_i = UiEvent::Key {
        key: Key::Char('I'),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::CTRL
        },
        pressed: true,
    };
    assert_eq!(s.handle(&ctrl_shift_i), vec![ShellRequest::PickImportFiles]);
    assert_eq!(
        s.execute(AppCommand::ImportAudio),
        vec![ShellRequest::PickImportFiles]
    );
}

#[test]
fn the_view_action_for_the_dialog_becomes_a_request() {
    let mut s = shell();
    s.pane_actions(&[UiAction::ImportAudioDialog]);
    assert_eq!(
        s.handle(&UiEvent::PointerLeave),
        vec![ShellRequest::PickImportFiles]
    );
    assert!(s.handle(&UiEvent::PointerLeave).is_empty(), "once");
}

#[test]
fn picked_files_import_onto_new_tracks_at_the_playhead() {
    let mut s = shell();
    s.import_files(vec![wav("picked.wav", 48_000)]);
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert_eq!(audio_clip_count(&s), 1);
}

#[test]
fn the_status_bar_shows_the_import_progress_while_it_runs() {
    let mut s = shell();
    s.core_mut().import_audio(
        vec![wav("progress.wav", 48_000)],
        ImportTarget::NewTracks,
        0,
    );
    s.poll_status();
    assert!(
        s.status_text().contains("importing audio"),
        "{}",
        s.status_text()
    );
    s.core_mut().wait_background();
    s.poll_status();
    assert!(!s.status_text().contains("importing audio"));
}

/// A window point inside the track area over an audio track's lane, found through the view.
fn lane_point(s: &Shell<NullBackend>) -> Point {
    let song = s.layout().song.unwrap();
    let mut y = song.y0;
    while y < song.y1 {
        let p = Point::new(song.x0 + 500.0, y);
        if matches!(
            s.core().arrangement().drop_target_at(p),
            Some((ImportTarget::ExistingTrack(_), _))
        ) {
            return p;
        }
        y += 2.0;
    }
    panic!("no audio lane found");
}

#[test]
fn a_file_dropped_on_an_audio_lane_goes_onto_that_track() {
    let mut s = shell();
    s.import_files(vec![wav("first.wav", 24_000)]);
    s.core_mut().wait_background();
    assert_eq!(audio_clip_count(&s), 1);
    let at = lane_point(&s);
    s.handle(&UiEvent::FileHover {
        pos: at,
        paths: vec![wav("second.wav", 12_000)],
    });
    assert!(s.core().arrangement().drop_hover().is_some());
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![wav("second.wav", 12_000)],
    });
    assert!(s.core().arrangement().drop_hover().is_none());
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), 1, "no new track");
    assert_eq!(audio_clip_count(&s), 2);
}

#[test]
fn a_file_dropped_outside_the_arrangement_makes_new_tracks() {
    let mut s = shell();
    let in_rack = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: in_rack,
        paths: vec![wav("a.wav", 4_800), wav("b.wav", 4_800)],
    });
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), 2);
    assert_eq!(audio_clip_count(&s), 2);
}

#[test]
fn hovering_away_from_the_arrangement_or_cancelling_clears_the_drop_indicator() {
    let mut s = shell();
    s.import_files(vec![wav("h.wav", 24_000)]);
    s.core_mut().wait_background();
    let at = lane_point(&s);
    let hover = |pos| UiEvent::FileHover {
        pos,
        paths: vec![wav("x.wav", 100)],
    };
    s.handle(&hover(at));
    assert!(s.core().arrangement().drop_hover().is_some());
    s.handle(&hover(s.layout().rack.center()));
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle(&hover(at));
    s.handle(&UiEvent::FileHoverCancel);
    assert!(s.core().arrangement().drop_hover().is_none());
}
