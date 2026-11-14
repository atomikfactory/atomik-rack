//! `Shell` film export tests: File > Export > Film with Mix (its menu item and the states in which
//! it is greyed with a message), the file dialog request and its proposed name, the options
//! panel, the progress panel with CANCEL, the choices remembered in `export.json`, and the guard
//! against replacing a file the dialog did not ask about (also for the audio export, whose format
//! is chosen after the dialog).

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, Note, SongCommand, VideoClip,
    VideoCommand, VideoFacts, WorkflowCommand,
};
use atomik_ui::ExportFile;
use atomik_video::fake::FakeTool;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-shell-film-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell whose settings (and so `export.json`) live in `t`, whose FFmpeg is a fake that "makes"
/// files by writing a few bytes, and whose synth plays two bars.
fn shell(t: &Tmp) -> (Shell<NullBackend>, Arc<FakeTool>) {
    let programs = t.file("ffmpeg-folder");
    std::fs::create_dir_all(&programs).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        std::fs::write(
            programs.join(atomik_video::Platform::current().exe_name(name)),
            b"x",
        )
        .unwrap();
    }
    let tool = Arc::new(FakeTool::new().audio(b"NEWFILM".to_vec()));
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(t.file("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    let core = s.core_mut();
    core.set_video_tool(Arc::clone(&tool) as Arc<dyn atomik_video::VideoTool>);
    core.set_video_folder(Some(programs));
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(atomik_model::DeviceKey("lead".into())),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: atomik_model::DeviceKey("lead".into()),
        lane: 0,
        clip: Clip {
            id: atomik_model::song::ClipId(0),
            start: 0,
            length: 2 * BAR,
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
            content: ClipContent::Notes(vec![Note::new(0, PPQ, 64, 0.8)].into()),
        },
    }))
    .unwrap();
    s.frame();
    (s, tool)
}

/// Puts the film `scene.mp4` in the song.
fn put_film(t: &Tmp, s: &mut Shell<NullBackend>) {
    let file = t.file("scene.mp4");
    std::fs::write(&file, b"film").unwrap();
    let facts = VideoFacts {
        duration_us: 12_021_333,
        fps_num: 25,
        fps_den: 1,
        width: 1920,
        height: 1080,
        timecode: None,
        has_audio: true,
    };
    s.core_mut()
        .apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
            file.to_string_lossy().into_owned(),
            facts,
        ))))
        .unwrap();
    s.frame();
}

fn item(s: &Shell<NullBackend>) -> crate::commands::MenuItem {
    s.menu_model()[0]
        .items()
        .find(|i| i.command == AppCommand::ExportFilm)
        .cloned()
        .expect("a File menu item")
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

fn press_film(s: &mut Shell<NullBackend>, label: &str) {
    let p = s
        .film_view()
        .and_then(|v| v.button_rect(label))
        .unwrap_or_else(|| panic!("no button {label}"))
        .center();
    click(s, p);
}

fn press_export(s: &mut Shell<NullBackend>, label: &str) {
    let p = s
        .export_view()
        .and_then(|v| v.button_rect(label))
        .unwrap_or_else(|| panic!("no button {label}"))
        .center();
    click(s, p);
}

fn escape(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

/// Lets the worker finish and the shell notice.
fn settle(s: &mut Shell<NullBackend>) {
    s.core_mut().wait_background();
    s.frame();
}

fn toast(s: &Shell<NullBackend>) -> String {
    s.core().last_rejection().unwrap_or_default().to_owned()
}

// ------------------------------------------------------------------ the menu

#[test]
fn the_export_submenu_has_the_film_item_with_a_remappable_command() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    let film = item(&s);
    assert_eq!(film.label, "Film with Mix\u{2026}");
    assert!(film.enabled);
    assert_eq!(film.command.command_id(), Some(("file.export_film", None)));
    assert_eq!(
        AppCommand::from_command("file.export_film", None),
        Some(AppCommand::ExportFilm)
    );
    // It sits in the Export submenu, after the stems.
    let file_menu = &s.menu_model()[0];
    let export = file_menu
        .entries
        .iter()
        .find_map(|e| match e {
            crate::commands::MenuEntry::Submenu(sub) if sub.label == "Export" => Some(sub),
            _ => None,
        })
        .unwrap();
    let labels: Vec<String> = export
        .entries
        .iter()
        .filter_map(|e| match e {
            crate::commands::MenuEntry::Item(i) => Some(i.label.to_string()),
            _ => None,
        })
        .collect();
    let at = labels.iter().position(|l| l.starts_with("Film")).unwrap();
    assert!(labels[at - 1].starts_with("Export Stems"), "{labels:?}");
}

#[test]
fn the_item_is_greyed_without_a_film_and_the_command_says_why() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    assert!(!item(&s).enabled, "the song has no video");
    assert!(s.execute(AppCommand::ExportFilm).is_empty());
    let message = toast(&s);
    assert!(message.contains("no video"), "{message}");
}

#[test]
fn the_item_is_greyed_when_the_films_file_is_missing() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    // The song names a film whose file is not there.
    let facts = VideoFacts {
        duration_us: 12_021_333,
        fps_num: 25,
        fps_den: 1,
        width: 1920,
        height: 1080,
        timecode: None,
        has_audio: true,
    };
    s.core_mut()
        .apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
            t.file("gone.mp4").to_string_lossy().into_owned(),
            facts,
        ))))
        .unwrap();
    s.frame();
    assert!(!item(&s).enabled);
    assert!(s.execute(AppCommand::ExportFilm).is_empty());
    assert!(toast(&s).contains("Relink Video"), "{}", toast(&s));
}

#[test]
fn the_item_is_greyed_without_ffmpeg() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    let empty = t.file("empty");
    std::fs::create_dir_all(&empty).unwrap();
    s.core_mut().set_video_folder(Some(empty));
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    assert!(!item(&s).enabled);
    assert!(s.execute(AppCommand::ExportFilm).is_empty());
    assert!(toast(&s).contains("needs FFmpeg"), "{}", toast(&s));
}

#[test]
fn the_command_asks_for_a_file_and_proposes_an_mp4_name() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    assert_eq!(
        s.execute(AppCommand::ExportFilm),
        vec![ShellRequest::PickExportFilm]
    );
    assert_eq!(s.export_film_name(), "project.mp4");
}

// ------------------------------------------------------------------ the panel

#[test]
fn the_panel_makes_the_film_and_remembers_its_choices() {
    let t = Tmp::new();
    let (mut s, tool) = shell(&t);
    put_film(&t, &mut s);
    s.export_film_to(&t.file("out.mp4"));
    assert!(s.film_open());
    // FFmpeg is being looked for: the choices can be made but EXPORT waits for the answer.
    press_film(&mut s, "RE-ENCODE");
    press_film(&mut s, "SMALL");
    press_film(&mut s, "256 kbps");
    press_film(&mut s, "KEEP AS SECOND TRACK");
    press_film(&mut s, "EXPORT");
    assert!(!s.core().audio_export_busy());
    settle(&mut s);
    press_film(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    assert!(s.film_view().unwrap().in_progress());
    settle(&mut s);
    assert!(!s.film_open(), "the panel closes when the job is over");
    assert_eq!(std::fs::read(t.file("out.mp4")).unwrap(), b"NEWFILM");
    assert!(
        toast(&s).contains("Exported out.mp4") && toast(&s).contains("re-encoded"),
        "{}",
        toast(&s)
    );
    let calls: Vec<_> = tool
        .calls()
        .into_iter()
        .filter(|c| c.has("-filter_complex"))
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].after("-crf").unwrap(), "28");
    assert_eq!(calls[0].after("-b:a").unwrap(), "256k");
    // The choices are remembered, in the file and for the next panel.
    let saved = std::fs::read_to_string(t.file("export.json")).unwrap();
    assert!(saved.contains("\"film_picture\": \"encode\""), "{saved}");
    assert!(saved.contains("\"film_quality\": \"small\""), "{saved}");
    assert!(saved.contains("\"film_aac_kbps\": 256"), "{saved}");
    assert!(saved.contains("\"film_own_audio\": true"), "{saved}");
    s.export_film_to(&t.file("again.mp4"));
    let v = s.film_view().unwrap().values();
    assert_eq!(v.aac, 256);
    assert!(v.own_audio);
    let s2 = shell(&t).0;
    assert_eq!(s2.film_values().aac, 256);
}

#[test]
fn pcm_makes_a_mov_and_the_next_dialog_proposes_it() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    s.export_film_to(&t.file("out.mp4"));
    press_film(&mut s, "PCM (MOV)");
    press_film(&mut s, "EXPORT");
    settle(&mut s);
    assert!(t.file("out.mov").is_file());
    assert!(!t.file("out.mp4").exists());
    assert_eq!(s.export_film_name(), "project.mov");
}

#[test]
fn escape_closes_the_options_and_cancel_stops_the_job() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    s.export_film_to(&t.file("out.mp4"));
    escape(&mut s);
    assert!(!s.film_open());
    assert!(!s.core().audio_export_busy());
    // While the job runs, Esc stops it, and the panel closes once the worker has stopped.
    let release = crate::export_worker::hold::next_export();
    s.export_film_to(&t.file("out.mp4"));
    press_film(&mut s, "EXPORT");
    assert!(s.film_view().unwrap().in_progress());
    escape(&mut s);
    drop(release);
    settle(&mut s);
    assert!(!s.film_open());
    assert!(!t.file("out.mp4").exists());
    assert!(toast(&s).contains("cancelled"), "{}", toast(&s));
}

#[test]
fn the_panel_is_modal_and_follows_the_window() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    s.export_film_to(&t.file("out.mp4"));
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(
        dl.texts()
            .iter()
            .any(|x| x.contains("EXPORT FILM WITH MIX")),
        "{:?}",
        dl.texts()
    );
    // The panel takes every event: a key does not reach the song underneath.
    s.handle(&UiEvent::Key {
        key: Key::Space,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(s.film_open());
    s.handle(&UiEvent::Resize(Size::new(700.0, 600.0)));
    let panel = s.film_view().unwrap().panel();
    assert!(panel.x1 <= 700.0 && panel.y1 <= 600.0, "{panel:?}");
}

// ------------------------------------------------------------------ not replacing unasked

#[test]
fn a_film_that_would_replace_a_file_at_the_swapped_name_asks_first() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    // The dialog picked out.mp4 (which it confirmed was free); PCM makes it out.mov, which exists.
    std::fs::write(t.file("out.mov"), b"precious").unwrap();
    s.export_film_to(&t.file("out.mp4"));
    press_film(&mut s, "PCM (MOV)");
    press_film(&mut s, "EXPORT");
    assert!(!s.core().audio_export_busy(), "nothing started");
    assert!(s.film_open());
    let warning = s.film_view().unwrap().warning().unwrap().to_owned();
    assert!(warning.contains("out.mov already exists"), "{warning}");
    assert_eq!(std::fs::read(t.file("out.mov")).unwrap(), b"precious");
    // Another sound is another question: the line goes away and the export is free to run.
    press_film(&mut s, "AAC (MP4)");
    assert!(s.film_view().unwrap().warning().is_none());
    // Back to the sound that was warned about: EXPORT again is the confirmation.
    press_film(&mut s, "PCM (MOV)");
    press_film(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    settle(&mut s);
    assert_eq!(std::fs::read(t.file("out.mov")).unwrap(), b"NEWFILM");
}

#[test]
fn a_film_at_the_name_the_dialog_confirmed_needs_no_second_question() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    put_film(&t, &mut s);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    // The native dialog asked about replacing out.mp4 already.
    std::fs::write(t.file("out.mp4"), b"old").unwrap();
    s.export_film_to(&t.file("out.mp4"));
    press_film(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    settle(&mut s);
    assert_eq!(std::fs::read(t.file("out.mp4")).unwrap(), b"NEWFILM");
}

#[test]
fn an_audio_export_that_would_replace_a_compressed_file_asks_first() {
    let t = Tmp::new();
    let (mut s, tool) = shell(&t);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    // The dialog proposes .wav; the user picks MP3 in the panel, and mix.mp3 exists.
    std::fs::write(t.file("mix.mp3"), b"old song").unwrap();
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    press_export(&mut s, "MP3");
    press_export(&mut s, "EXPORT");
    assert!(!s.core().audio_export_busy(), "nothing started");
    let warning = s.export_view().unwrap().warning().unwrap().to_owned();
    assert!(warning.contains("mix.mp3 already exists"), "{warning}");
    assert_eq!(std::fs::read(t.file("mix.mp3")).unwrap(), b"old song");
    assert!(
        tool.calls().iter().all(|c| !c.has("-vn")),
        "nothing encoded"
    );
    // Changing the format clears the line; WAV goes to the name the dialog picked.
    press_export(&mut s, "WAV");
    assert!(s.export_view().unwrap().warning().is_none());
    press_export(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    settle(&mut s);
    assert!(t.file("mix.wav").is_file());
    assert_eq!(std::fs::read(t.file("mix.mp3")).unwrap(), b"old song");
    // MP3 again and a second EXPORT replaces the file.
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    press_export(&mut s, "MP3");
    press_export(&mut s, "EXPORT");
    assert!(!s.core().audio_export_busy());
    press_export(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    settle(&mut s);
    assert_eq!(std::fs::read(t.file("mix.mp3")).unwrap(), b"NEWFILM");
}

#[test]
fn the_audio_dialog_proposes_the_extension_of_the_format_used_last() {
    let t = Tmp::new();
    let (mut s, _) = shell(&t);
    s.core_mut().check_ffmpeg().unwrap();
    settle(&mut s);
    assert_eq!(s.export_audio_name(ExportScope::Song), "project.wav");
    assert_eq!(s.export_audio_extension(), "wav");
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    press_export(&mut s, "AAC");
    press_export(&mut s, "EXPORT");
    settle(&mut s);
    assert!(t.file("mix.m4a").is_file());
    assert_eq!(s.export_audio_extension(), "m4a");
    assert_eq!(s.export_audio_name(ExportScope::Song), "project.m4a");
    assert_eq!(s.export_audio_name(ExportScope::Loop), "project loop.m4a");
    assert_eq!(
        s.export_audio_name(ExportScope::Selection),
        "project selection.m4a"
    );
    // A folder of stems has no extension.
    assert_eq!(s.export_audio_name(ExportScope::Stems), "Bounced project");
    // The remembered format reaches the next session too.
    let s2 = shell(&t).0;
    assert_eq!(s2.export_audio_extension(), "m4a");
    assert_eq!(s2.export_values().file, ExportFile::Aac);
}
