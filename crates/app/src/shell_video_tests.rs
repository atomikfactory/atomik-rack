//! Video in the shell: the File > Video menu, what each command asks the platform for, the
//! status line while a film is prepared, and the folder preference that reaches the core. The
//! shell is built as Windows, so Ctrl is the command key on every host; the tool is the fake.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_video::fake::{FakeTool, tiny_jpeg};
use atomik_video::{FfmpegStatus, Platform};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-shell-video-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shell(t: &Tmp, tool: FakeTool) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.set_video_tool(Arc::new(tool));
    core.set_video_cache(t.path("cache"));
    let mut s = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(t.path("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(t.path("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

/// Stand-ins for the two programs in `<tmp>/ffmpeg`, which the shell is told to use.
fn programs(t: &Tmp) -> PathBuf {
    let dir = t.path("ffmpeg");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        std::fs::write(dir.join(Platform::current().exe_name(name)), b"x").unwrap();
    }
    dir
}

#[test]
fn the_file_menu_has_a_video_submenu_with_its_commands() {
    let t = Tmp::new("menu");
    let s = shell(&t, FakeTool::new());
    let video = s.menu_model()[0]
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu(sub) if sub.label == "Video" => Some(sub.clone()),
            _ => None,
        })
        .expect("File > Video");
    let rows: Vec<(String, bool)> = video
        .entries
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item(i) => Some((i.label.to_string(), i.enabled)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Import Video\u{2026}".to_owned(), true),
            ("Import Video Audio".to_owned(), false),
            ("Import Audio with Video".to_owned(), true),
            ("Remove Video".to_owned(), false),
            ("Relink Video\u{2026}".to_owned(), false),
            ("Keep Audio with Video".to_owned(), false),
            ("Move Video to Playhead".to_owned(), false),
            ("Trim Video Start to Playhead".to_owned(), false),
            ("Trim Video End to Playhead".to_owned(), false),
            ("Video Overlay".to_owned(), true),
            ("Prepare Video\u{2026}".to_owned(), true),
            ("Check FFmpeg".to_owned(), true),
            ("Cancel Video Job".to_owned(), false),
        ]
    );
}

#[test]
fn the_import_relink_and_edit_commands_ask_the_platform_or_the_core() {
    let t = Tmp::new("clip");
    let mut s = shell(&t, FakeTool::new());
    assert_eq!(
        s.execute(AppCommand::ImportVideo),
        vec![ShellRequest::PickVideoFile(VideoPick::Import)]
    );
    // Without a film in the song there is nothing to relink, remove, move or trim.
    assert!(s.execute(AppCommand::RelinkVideo).is_empty());
    assert_eq!(s.core().last_rejection(), Some("The song has no video"));
    for c in [
        AppCommand::RemoveVideo,
        AppCommand::MoveVideoToPlayhead,
        AppCommand::TrimVideoStart,
        AppCommand::TrimVideoEnd,
    ] {
        assert!(s.execute(c).is_empty());
        assert_eq!(s.core().last_rejection(), Some("The song has no video"));
    }
    // The picked film is imported: the song has it and the menu enables the rest.
    let film = t.path("scene.mp4");
    std::fs::write(&film, b"pretend film").unwrap();
    s.workflow.ffmpeg_folder = Some(programs(&t));
    s.apply_workflow_to_views();
    s.video_picked(VideoPick::Import, &film);
    s.core_mut().wait_background();
    assert!(s.core().video_clip().is_some());
    let menu = s.menu_state();
    assert!(menu.video_clip && !menu.video_missing);
    assert_eq!(
        s.execute(AppCommand::RelinkVideo),
        vec![ShellRequest::PickVideoFile(VideoPick::Relink)]
    );
    s.execute(AppCommand::RemoveVideo);
    assert!(s.core().video_clip().is_none());
    assert!(!s.menu_state().video_clip);
}

#[test]
fn prepare_video_asks_the_platform_for_a_film() {
    let t = Tmp::new("pick");
    let mut s = shell(&t, FakeTool::new());
    assert_eq!(
        s.execute(AppCommand::PrepareVideo),
        vec![ShellRequest::PickVideoFile(VideoPick::Prepare)]
    );
    // Cancel with nothing running says so.
    s.execute(AppCommand::CancelVideo);
    assert_eq!(s.core().last_rejection(), Some("No video job is running"));
}

#[test]
fn the_ffmpeg_folder_preference_reaches_the_core_and_check_ffmpeg_reports() {
    let t = Tmp::new("folder");
    let mut s = shell(&t, FakeTool::new());
    s.workflow.ffmpeg_folder = Some(programs(&t));
    s.apply_workflow_to_views();
    assert_eq!(s.core().video_folder(), Some(programs(&t).as_path()));
    s.execute(AppCommand::CheckFfmpeg);
    assert!(s.menu_state().video_busy);
    s.core_mut().wait_background();
    assert!(s.core().video_status().is_usable());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("found at"))
    );
    // Back to the system path: what was found is forgotten.
    s.workflow.ffmpeg_folder = None;
    s.apply_workflow_to_views();
    assert_eq!(*s.core().video_status(), FfmpegStatus::Unchecked);
}

#[test]
fn a_film_that_is_prepared_shows_progress_in_the_status_line_until_it_is_done() {
    let t = Tmp::new("status");
    let tool = FakeTool::new().frames((0..6).map(|i| tiny_jpeg(640, 360, i)).collect());
    let mut s = shell(&t, tool);
    s.workflow.ffmpeg_folder = Some(programs(&t));
    s.apply_workflow_to_views();
    let film = t.path("film.mp4");
    std::fs::write(&film, b"pretend film").unwrap();
    s.video_picked(VideoPick::Prepare, &film);
    s.poll_status();
    assert!(
        s.status_text().contains("Preparing video film.mp4"),
        "{}",
        s.status_text()
    );
    s.core_mut().wait_background();
    s.poll_status();
    assert!(
        !s.status_text().contains("Preparing video"),
        "{}",
        s.status_text()
    );
    assert!(s.core().prepared_video(&film).is_some());
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    use atomik_ui::{Modifiers, PointerButton};
    let r = s.handle(&UiEvent::PointerDown {
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
    r
}

#[test]
fn the_video_tab_shows_the_check_and_applies_a_chosen_folder() {
    use crate::settings_dialog::Tab;
    let size = Size::new(1000.0, 800.0);
    let t = Tmp::new("tab");
    let mut s = shell(&t, FakeTool::new());
    let before = s.core().last_rejection().map(str::to_owned);
    s.execute(AppCommand::AudioSettings);
    let tab = s
        .dialog()
        .unwrap()
        .tab_strip(size)
        .iter()
        .find(|(_, tab)| *tab == Tab::Video)
        .map(|(r, _)| r.center())
        .unwrap();
    click(&mut s, tab);
    // Nothing was asked of the tool yet and no toast was made by opening the tab.
    assert_eq!(s.core().last_rejection().map(str::to_owned), before);
    // The first frame starts the quiet check; its answer reaches the tab.
    s.poll_status();
    let line = |s: &Shell<NullBackend>| s.dialog().unwrap().video_layout(size).unwrap().status.1;
    assert_eq!(line(&s), ["Looking for FFmpeg..."]);
    s.core_mut().wait_background();
    s.poll_status();
    // No folder is set and the fake finds nothing on the real path of this machine unless
    // FFmpeg is installed; either way the line is a sentence about FFmpeg, not a toast.
    let shown = line(&s).join(" ");
    assert!(shown.contains("FFmpeg"), "{shown}");
    assert_eq!(s.core().last_rejection().map(str::to_owned), before);
    // Choose asks the platform; the picked folder is a draft until Apply.
    let layout = s.dialog().unwrap().video_layout(size).unwrap();
    assert_eq!(
        click(&mut s, layout.folder.choose.center()),
        vec![ShellRequest::PickFfmpegFolder]
    );
    s.set_ffmpeg_folder(programs(&t));
    assert_eq!(s.core().video_folder(), None, "not before Apply");
    let apply = s.dialog().unwrap().buttons(size).0.center();
    click(&mut s, apply);
    assert_eq!(s.core().video_folder(), Some(programs(&t).as_path()));
    assert_eq!(s.workflow().ffmpeg_folder, Some(programs(&t)));
    // The new folder is looked at without being asked, and it is found.
    s.core_mut().wait_background();
    assert!(s.core().video_status().is_usable());
    assert_eq!(s.core().last_rejection().map(str::to_owned), before);
    // It is saved with the other preferences.
    let saved = std::fs::read_to_string(t.path("workflow.json")).unwrap();
    assert!(saved.contains("ffmpeg_folder"), "{saved}");
    // A picked folder with no dialog open is applied at once.
    s.set_ffmpeg_folder(t.path("elsewhere"));
    assert_eq!(s.core().video_folder(), Some(t.path("elsewhere").as_path()));
}

#[test]
fn the_video_window_opens_closes_and_remembers_its_overlay() {
    let t = Tmp::new("window");
    let mut s = shell(&t, FakeTool::new());
    assert!(!s.detached(WindowKind::Video));
    assert!(!s.core.video_shown());
    s.execute(AppCommand::ToggleVideo);
    assert!(s.detached(WindowKind::Video));
    assert!(s.core.video_shown());
    assert!(s.windows_wanted().contains(&WindowKind::Video));
    assert_eq!(s.window_of(Pane::Video), WindowKind::Video);
    // The View menu's check follows the window, the File > Video check follows the overlay.
    assert!(s.menu_state().video_window && s.menu_state().video_overlay);
    s.execute(AppCommand::ToggleVideoOverlay);
    assert!(!s.menu_state().video_overlay);
    assert!(!s.ui.video_timecode, "the choice is kept for the next run");
    s.execute(AppCommand::ToggleVideo);
    assert!(!s.detached(WindowKind::Video));
    assert!(!s.core.video_shown(), "closing frees the pictures");
}

#[test]
fn the_video_row_hides_collapses_and_is_remembered() {
    let t = Tmp::new("row");
    let mut s = shell(&t, FakeTool::new());
    // On by default, open.
    assert!(s.core.arrangement().video_row_visible());
    assert!(s.menu_state().video_row);
    assert!(s.ui.show_video_row && !s.ui.video_row_collapsed);
    s.execute(AppCommand::ToggleVideoRow);
    assert!(!s.core.arrangement().video_row_visible());
    assert!(!s.menu_state().video_row);
    assert!(!s.ui.show_video_row, "the choice is kept for the next run");
    s.execute(AppCommand::ToggleVideoRow);
    assert!(s.core.arrangement().video_row_visible() && s.ui.show_video_row);
    // The chevron of the row collapses it; the next save of the zoom state keeps that.
    s.core.arrangement_mut().set_video_row_collapsed(true);
    s.save_zoom_state();
    assert!(s.ui.video_row_collapsed);
    s.core.arrangement_mut().set_video_row_collapsed(false);
    s.save_zoom_state();
    assert!(!s.ui.video_row_collapsed);
}

fn wav(frames: usize) -> Vec<u8> {
    atomik_video::fake::wav_pcm16(48_000, 2, &vec![100i16; frames * 2])
}

/// A shell whose song has the film `film.mp4` (imported through the fake) and FFmpeg set up.
fn shell_with_film(t: &Tmp, tool: FakeTool) -> Shell<NullBackend> {
    let mut s = shell(t, tool);
    s.workflow.ffmpeg_folder = Some(programs(t));
    s.apply_workflow_to_views();
    let film = t.path("film.mp4");
    std::fs::write(&film, b"pretend film").unwrap();
    s.video_picked(VideoPick::Import, &film);
    s.core_mut().wait_background();
    assert!(s.core().video_clip().is_some());
    s
}

#[test]
fn import_video_audio_runs_from_the_menu_and_keep_audio_is_a_check_that_follows_the_link() {
    let t = Tmp::new("audio");
    let tool = FakeTool::new()
        .frames((0..6).map(|i| tiny_jpeg(640, 360, i)).collect())
        .audio(wav(48_000));
    let mut s = shell_with_film(&t, tool);
    let state = s.menu_state();
    assert!(state.video_clip && !state.video_audio && !state.video_audio_keep);
    assert!(s.execute(AppCommand::ImportVideoAudio).is_empty());
    s.core_mut().wait_background();
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("Imported the audio of film.mp4")),
        "{:?}",
        s.core().last_rejection()
    );
    let state = s.menu_state();
    assert!(state.video_audio && state.video_audio_keep);
    // Keep Audio with Video is a check in the menu, enabled with the link.
    let keep = s.menu_model()[0]
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu(sub) if sub.label == "Video" => Some(sub.clone()),
            _ => None,
        })
        .unwrap()
        .items()
        .find(|i| i.command == AppCommand::KeepVideoAudio)
        .cloned()
        .unwrap();
    assert!(keep.enabled && keep.checked == Some(true));
    s.execute(AppCommand::KeepVideoAudio);
    assert!(!s.menu_state().video_audio_keep);
    s.execute(AppCommand::KeepVideoAudio);
    assert!(s.menu_state().video_audio_keep);
}

#[test]
fn import_video_audio_without_a_film_says_so_and_the_option_is_remembered() {
    let t = Tmp::new("audio-none");
    let mut s = shell(&t, FakeTool::new());
    assert!(s.execute(AppCommand::ImportVideoAudio).is_empty());
    assert_eq!(s.core().last_rejection(), Some("The song has no video"));
    s.execute(AppCommand::KeepVideoAudio);
    assert_eq!(
        s.core().last_rejection(),
        Some("The song has no imported video audio")
    );
    // Import Audio with Video is a check, kept in the window state and handed to the core.
    assert!(!s.ui.video_import_audio && !s.core().import_audio_with_video());
    s.execute(AppCommand::ImportVideoWithAudio);
    assert!(s.ui.video_import_audio && s.core().import_audio_with_video());
    assert!(s.menu_state().video_import_audio);
    s.execute(AppCommand::ImportVideoWithAudio);
    assert!(!s.ui.video_import_audio && !s.core().import_audio_with_video());
}

#[test]
fn a_film_with_several_audio_tracks_opens_a_pop_up_and_the_choice_imports_that_track() {
    let t = Tmp::new("audio-choose");
    let tool = FakeTool::new()
        .probe(atomik_video::samples::MKV_HEVC)
        .frames((0..6).map(|i| tiny_jpeg(640, 360, i)).collect())
        .audio(wav(48_000));
    let mut s = shell_with_film(&t, tool);
    assert!(s.context_menu().is_none());
    s.execute(AppCommand::ImportVideoAudio);
    s.core_mut().wait_background();
    // The next frame shows the tracks, and nothing was imported yet.
    s.frame();
    let menu = s.context_menu().expect("the pop-up of the audio tracks");
    let rows: Vec<&str> = menu.items().map(|i| i.label.as_str()).collect();
    assert_eq!(
        rows,
        [
            "1: 5.1 opus, eng, Surround",
            "2: stereo opus, fre, Stereo (Francais)"
        ]
    );
    assert!(s.core().doc().song.tracks.is_empty());
    // Choosing a row runs the pick command, which imports that track.
    s.run_command("video.import_audio_pick", Some("2"));
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("stereo opus")),
        "{:?}",
        s.core().last_rejection()
    );
    // A bad argument is a message.
    s.run_command("video.import_audio_pick", Some("two"));
    assert_eq!(
        s.core().last_rejection(),
        Some("That is not an audio track of the video")
    );
}
