//! `Shell` export tests: File > Export Song / Loop as Audio File, the file dialog request, the
//! options panel, the progress panel with CANCEL, and the choices remembered in `export.json`.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, Note, SongCommand, WorkflowCommand,
};
use atomik_ui::{ExportDepth, ExportDither, ExportFile, ExportValues};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-shell-export-{}-{n}",
            std::process::id()
        ));
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

/// A shell whose settings (and so `export.json`) live in `dir`, with a synth that plays two bars.
fn shell(dir: &Path) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    let core = s.core_mut();
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
    s
}

fn item(s: &Shell<NullBackend>, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()[0]
        .items()
        .find(|i| i.command == command)
        .cloned()
        .expect("a File menu item")
}

fn press(s: &mut Shell<NullBackend>, label: &str) {
    let p = s
        .export_view()
        .and_then(|v| v.button_rect(label))
        .unwrap_or_else(|| panic!("no button {label}"))
        .center();
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

fn escape(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

#[test]
fn the_file_menu_has_both_export_items_and_they_have_remappable_commands() {
    let t = Tmp::new();
    let s = shell(&t.0);
    let song = item(&s, AppCommand::ExportSongAudio);
    let looped = item(&s, AppCommand::ExportLoopAudio);
    assert_eq!(song.label, "Export Song as Audio File\u{2026}");
    assert_eq!(looped.label, "Export Loop as Audio File\u{2026}");
    assert!(song.enabled && looped.enabled);
    assert_eq!(
        song.command.command_id(),
        Some(("file.export_song_audio", None))
    );
    assert_eq!(
        looped.command.command_id(),
        Some(("file.export_loop_audio", None))
    );
}

#[test]
fn the_commands_ask_for_a_file_and_propose_a_name() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    assert_eq!(
        s.execute(AppCommand::ExportSongAudio),
        vec![ShellRequest::PickExportAudio(ExportScope::Song)]
    );
    assert_eq!(
        s.execute(AppCommand::ExportLoopAudio),
        vec![ShellRequest::PickExportAudio(ExportScope::Loop)]
    );
    assert_eq!(s.export_audio_name(ExportScope::Song), "project.wav");
    assert_eq!(s.export_audio_name(ExportScope::Loop), "project loop.wav");
    assert!(!s.export_open(), "the panel waits for the file");
}

#[test]
fn the_picked_file_opens_the_options_and_cancel_closes_them_without_exporting() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let path = t.file("mix.wav");
    s.export_audio_to(ExportScope::Song, &path);
    assert!(s.export_open());
    assert!(s.modal_open());
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    let texts = dl.texts();
    for want in [
        "EXPORT SONG AS AUDIO FILE",
        "mix.wav",
        "BIT DEPTH",
        "EXPORT",
    ] {
        assert!(texts.contains(&want), "{want} in {texts:?}");
    }
    press(&mut s, "CANCEL");
    assert!(!s.export_open());
    assert!(!path.exists());
    assert!(!s.core().audio_export_busy());
    // Esc does the same.
    s.export_audio_to(ExportScope::Loop, &path);
    escape(&mut s);
    assert!(!s.export_open());
}

#[test]
fn export_renders_with_a_progress_panel_then_closes_and_writes_the_file() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let path = t.file("mix.wav");
    s.export_audio_to(ExportScope::Song, &path);
    press(&mut s, "16-BIT");
    press(&mut s, "SHAPED");
    press(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    assert!(s.export_view().unwrap().in_progress());
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"EXPORTING SONG"));
    s.core_mut().wait_background();
    s.frame();
    assert!(!s.export_open(), "the panel closes when the export is over");
    let spec = hound::WavReader::open(&path).unwrap().spec();
    assert_eq!(spec.bits_per_sample, 16);
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Exported mix.wav")
    );
    // The choices are remembered, in the file and for the next panel.
    let saved = std::fs::read_to_string(t.file("export.json")).unwrap();
    assert!(saved.contains("\"bit_depth\": \"16\""), "{saved}");
    assert!(saved.contains("\"dither\": \"shaped\""), "{saved}");
    s.export_audio_to(ExportScope::Loop, &t.file("loop.wav"));
    let v = s.export_view().unwrap().values();
    assert_eq!(
        (v.depth, v.dither),
        (ExportDepth::Pcm16, ExportDither::Shaped)
    );
    assert_eq!(s.export_audio_dir(), Some(t.0.clone()));
    // A new shell on the same settings starts from them too.
    let s2 = shell(&t.0);
    let v = s2.export_values();
    assert_eq!(
        (v.depth, v.dither),
        (ExportDepth::Pcm16, ExportDither::Shaped)
    );
}

#[test]
fn cancel_during_the_render_stops_it_and_leaves_no_file() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let path = t.file("mix.wav");
    s.export_audio_to(ExportScope::Song, &path);
    // The worker waits at its start until CANCEL was pressed: a fast machine cannot finish the
    // render first.
    let release = crate::export_worker::hold::next_export();
    press(&mut s, "EXPORT");
    assert!(s.export_view().unwrap().in_progress());
    press(&mut s, "CANCEL");
    drop(release);
    s.core_mut().wait_background();
    s.frame();
    assert!(!s.export_open());
    assert!(!path.exists());
    assert!(s.core().last_rejection().unwrap().contains("cancelled"));
}

#[test]
fn the_menu_items_are_disabled_while_an_export_runs() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    press(&mut s, "EXPORT");
    assert!(!item(&s, AppCommand::ExportSongAudio).enabled);
    assert!(!item(&s, AppCommand::ExportLoopAudio).enabled);
    assert!(s.execute(AppCommand::ExportSongAudio).is_empty());
    assert!(s.core().last_rejection().unwrap().contains("running"));
    s.core_mut().wait_background();
    s.frame();
    assert!(item(&s, AppCommand::ExportSongAudio).enabled);
}

#[test]
fn export_stems_asks_for_a_folder_and_writes_a_file_per_strip_with_the_remembered_choices() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let stems = item(&s, AppCommand::ExportStems);
    assert_eq!(stems.label, "Export Stems as Audio Files\u{2026}");
    assert!(stems.enabled);
    assert_eq!(
        stems.command.command_id(),
        Some(("file.export_stems", None))
    );
    assert_eq!(
        s.execute(AppCommand::ExportStems),
        vec![ShellRequest::PickExportAudio(ExportScope::Stems)]
    );
    assert_eq!(s.export_audio_name(ExportScope::Stems), "Bounced project");
    let folder = t.file("Bounced mix");
    s.export_audio_to(ExportScope::Stems, &folder);
    assert!(s.export_open());
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    for want in ["EXPORT STEMS", "Bounced mix", "AFTER STRIP", "DRY SOURCE"] {
        assert!(dl.texts().contains(&want), "{want} in {:?}", dl.texts());
    }
    press(&mut s, "AIFF");
    press(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"EXPORTING STEMS"));
    s.core_mut().wait_background();
    s.frame();
    assert!(!s.export_open());
    let files = std::fs::read_dir(&folder).unwrap().count();
    assert_eq!(files, 2, "a channel and the master");
    // The choices are remembered: the next panel starts with AIFF, and in the folder's parent.
    assert_eq!(s.export_values().file, atomik_ui::ExportFile::Aiff);
    assert_eq!(s.export_audio_dir(), Some(t.0.clone()));
}

#[test]
fn export_selection_waits_for_a_selected_range_and_has_its_own_command_and_name() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let selection = item(&s, AppCommand::ExportSelectionAudio);
    assert_eq!(selection.label, "Export Selection as Audio File\u{2026}");
    assert_eq!(
        selection.command.command_id(),
        Some(("file.export_selection_audio", None))
    );
    assert!(!selection.enabled, "no range is selected");
    assert!(s.execute(AppCommand::ExportSelectionAudio).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Select a time range")
    );
    assert_eq!(
        s.export_audio_name(ExportScope::Selection),
        "project selection.wav"
    );
    // With a range selected on a lane the command asks for a file.
    let device = s.core().doc().song.tracks[0].device.clone();
    s.core_mut()
        .arrangement_mut()
        .set_lane_range(device, 0, 0, 960);
    s.frame();
    assert!(item(&s, AppCommand::ExportSelectionAudio).enabled);
    assert_eq!(
        s.execute(AppCommand::ExportSelectionAudio),
        vec![ShellRequest::PickExportAudio(ExportScope::Selection)]
    );
}

/// A shell whose FFmpeg is a fake that "encodes" by writing a few bytes.
fn shell_with_ffmpeg(t: &Tmp) -> (Shell<NullBackend>, Arc<atomik_video::fake::FakeTool>) {
    let programs = t.file("ffmpeg-folder");
    std::fs::create_dir_all(&programs).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        std::fs::write(
            programs.join(atomik_video::Platform::current().exe_name(name)),
            b"x",
        )
        .unwrap();
    }
    let tool = Arc::new(atomik_video::fake::FakeTool::new().audio(b"ENCODED".to_vec()));
    let mut s = shell(&t.0);
    s.core_mut()
        .set_video_tool(Arc::clone(&tool) as Arc<dyn atomik_video::VideoTool>);
    s.core_mut().set_video_folder(Some(programs));
    (s, tool)
}

#[test]
fn the_panel_exports_mp3_once_ffmpeg_is_found_and_remembers_the_format_and_quality() {
    let t = Tmp::new();
    let (mut s, tool) = shell_with_ffmpeg(&t);
    let path = t.file("mix.wav");
    s.export_audio_to(ExportScope::Song, &path);
    // FFmpeg is being looked for: the choice can be made but EXPORT waits for the answer.
    press(&mut s, "MP3");
    press(&mut s, "EXPORT");
    assert!(!s.core().audio_export_busy());
    assert!(s.export_open());
    s.core_mut().wait_background();
    s.frame();
    press(&mut s, "V2");
    press(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    s.core_mut().wait_background();
    s.frame();
    assert!(!s.export_open());
    assert_eq!(std::fs::read(t.file("mix.mp3")).unwrap(), b"ENCODED");
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Exported mix.mp3"),
    );
    let calls: Vec<_> = tool.calls().into_iter().filter(|c| c.has("-vn")).collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].after("-q:a").unwrap(), "2");
    // The format and the quality are remembered, in the file and for the next panel.
    let saved = std::fs::read_to_string(t.file("export.json")).unwrap();
    assert!(saved.contains("\"file\": \"mp3\""), "{saved}");
    assert!(saved.contains("\"mp3\": \"v2\""), "{saved}");
    s.export_audio_to(ExportScope::Loop, &t.file("loop.wav"));
    let v = s.export_view().unwrap().values();
    assert_eq!(
        (v.file, v.mp3),
        (ExportFile::Mp3, atomik_ui::ExportQuality::Vbr(2))
    );
    let s2 = shell(&t.0);
    assert_eq!(s2.export_values().file, ExportFile::Mp3);
}

#[test]
fn without_ffmpeg_the_panel_greys_the_compressed_formats_and_says_how_to_set_it_up() {
    let t = Tmp::new();
    // No FFmpeg tools in this shell: the panel finds that out and greys the formats.
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    s.core_mut().wait_background();
    s.frame();
    press(&mut s, "MP3");
    assert_eq!(s.export_view().unwrap().values().file, ExportFile::Wav);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(
        dl.texts().iter().any(|t| t.contains("Settings > Video")),
        "{:?}",
        dl.texts()
    );
    // A WAV export still goes through.
    press(&mut s, "EXPORT");
    s.core_mut().wait_background();
    s.frame();
    assert!(t.file("mix.wav").is_file());
}

// ---------------------------------------------------------------------------------------------
// Export presets: File > Export for, the Preset row and the user's presets file.

/// The index of the preset called `name` in File > Export for.
fn preset_index(s: &Shell<NullBackend>, name: &str) -> usize {
    s.export_presets()
        .iter()
        .position(|p| p.name == name)
        .unwrap()
}

/// Lets the answer to "is FFmpeg there" arrive.
fn settle(s: &mut Shell<NullBackend>) {
    s.core_mut().refresh_ffmpeg_status();
    s.core_mut().wait_background();
    s.frame();
}

fn click_at(s: &mut Shell<NullBackend>, p: Point) {
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

/// Opens the Preset list of the panel and picks the preset called `name`.
fn pick_preset(s: &mut Shell<NullBackend>, name: &str) {
    let label = s.export_view().unwrap().preset_label();
    press(s, &label);
    let index = s
        .export_view()
        .unwrap()
        .presets()
        .iter()
        .position(|p| p.name == name)
        .unwrap();
    let button = s.export_view().unwrap().button_rect(&label).unwrap();
    // The rows sit under the drop-down: 4 px of padding, then 20 px each.
    click_at(
        s,
        Point::new(
            button.x0 + 30.0,
            button.y1 + 2.0 + 4.0 + 20.0 * index as f64 + 10.0,
        ),
    );
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    s.handle(&UiEvent::Ime(atomik_ui::ImeEvent::Commit(text.to_owned())));
}

fn enter(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Enter,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

#[test]
fn the_file_menu_lists_the_presets_in_an_export_for_submenu() {
    let t = Tmp::new();
    let s = shell(&t.0);
    let file = &s.menu_model()[0];
    let rows: Vec<_> = file
        .items()
        .filter(|i| matches!(i.command, AppCommand::ExportFor(_)))
        .map(|i| (i.label.to_string(), i.enabled))
        .collect();
    // The FFmpeg answer is not in yet: nothing is greyed for it.
    assert_eq!(
        rows,
        [
            ("Streaming".to_owned(), true),
            ("Podcast".to_owned(), true),
            ("Video".to_owned(), true),
            ("Stems for mixing".to_owned(), true),
            ("Club".to_owned(), true),
        ]
    );
    let row = item(&s, AppCommand::ExportFor(0));
    assert_eq!(
        row.command.command_id(),
        Some(("file.export_for", Some("0")))
    );
}

#[test]
fn without_ffmpeg_the_presets_that_encode_are_disabled_and_say_why() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    settle(&mut s);
    let rows: Vec<_> = s.menu_model()[0]
        .items()
        .filter(|i| matches!(i.command, AppCommand::ExportFor(_)))
        .map(|i| (i.label.to_string(), i.enabled))
        .collect();
    assert_eq!(
        rows,
        [
            ("Streaming (Needs FFmpeg)".to_owned(), false),
            ("Podcast (Needs FFmpeg)".to_owned(), false),
            ("Video (Needs FFmpeg)".to_owned(), false),
            ("Stems for mixing".to_owned(), true),
            ("Club".to_owned(), true),
        ]
    );
    // The command says so too, and asks for nothing.
    let podcast = preset_index(&s, "Podcast");
    assert!(s.execute(AppCommand::ExportFor(podcast)).is_empty());
    assert!(s.core().last_rejection().unwrap().contains("needs FFmpeg"));
    // The panel lists them disabled.
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    settle(&mut s);
    let label = s.export_view().unwrap().preset_label();
    press(&mut s, &label);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"Podcast (Needs FFmpeg)"));
}

#[test]
fn export_for_club_asks_for_a_wav_and_starts_at_once_with_the_progress_panel() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let club = preset_index(&s, "Club");
    assert_eq!(
        s.execute(AppCommand::ExportFor(club)),
        vec![ShellRequest::PickExportAudio(ExportScope::Song)]
    );
    assert_eq!(s.export_audio_name(ExportScope::Song), "project.wav");
    s.export_audio_to(ExportScope::Song, &t.file("club.wav"));
    // No options: the render runs, with the progress panel and its CANCEL.
    assert!(s.core().audio_export_busy());
    assert!(s.export_view().unwrap().in_progress());
    s.core_mut().wait_background();
    s.frame();
    assert!(!s.export_open());
    let spec = hound::WavReader::open(t.file("club.wav")).unwrap().spec();
    assert_eq!(spec.bits_per_sample, 24);
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Exported club.wav") && message.contains("-9 LUFS guide"),
        "{message}"
    );
    // The panel's remembered choices are not those of the preset: only the folder is kept.
    let next = s.export_values();
    assert_eq!(next, ExportValues::default());
    assert_eq!(s.export_audio_dir(), Some(t.0.clone()));
}

#[test]
fn export_for_streaming_writes_the_wav_and_the_mp3_and_names_both() {
    let t = Tmp::new();
    let (mut s, tool) = shell_with_ffmpeg(&t);
    settle(&mut s);
    let streaming = preset_index(&s, "Streaming");
    assert_eq!(
        s.execute(AppCommand::ExportFor(streaming)),
        vec![ShellRequest::PickExportAudio(ExportScope::Song)]
    );
    // An MP3 that is already there is not replaced unasked.
    std::fs::write(t.file("mix.mp3"), b"mine").unwrap();
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    assert!(!s.core().audio_export_busy());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("mix.mp3 already exists")
    );
    std::fs::remove_file(t.file("mix.mp3")).unwrap();
    s.execute(AppCommand::ExportFor(streaming));
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    assert!(s.core().audio_export_busy());
    s.core_mut().wait_background();
    s.frame();
    assert!(t.file("mix.wav").is_file());
    assert_eq!(std::fs::read(t.file("mix.mp3")).unwrap(), b"ENCODED");
    let message = s.core().last_rejection().unwrap();
    assert!(
        message.contains("Exported mix.wav, and mix.mp3"),
        "{message}"
    );
    assert_eq!(tool.calls().into_iter().filter(|c| c.has("-vn")).count(), 1);
}

#[test]
fn export_for_podcast_proposes_an_mp3_name() {
    let t = Tmp::new();
    let (mut s, _tool) = shell_with_ffmpeg(&t);
    settle(&mut s);
    let podcast = preset_index(&s, "Podcast");
    assert_eq!(
        s.execute(AppCommand::ExportFor(podcast)),
        vec![ShellRequest::PickExportAudio(ExportScope::Song)]
    );
    assert_eq!(s.export_audio_extension(), "mp3");
    assert_eq!(s.export_audio_name(ExportScope::Song), "project.mp3");
    // A plain export afterwards proposes the last used format again, not the preset's.
    s.execute(AppCommand::ExportSongAudio);
    assert_eq!(s.export_audio_extension(), "wav");
}

#[test]
fn export_for_stems_for_mixing_asks_for_a_folder_and_writes_the_stems() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    let stems = preset_index(&s, "Stems for mixing");
    assert_eq!(
        s.execute(AppCommand::ExportFor(stems)),
        vec![ShellRequest::PickExportAudio(ExportScope::Stems)]
    );
    let name = s.export_audio_name(ExportScope::Stems);
    s.export_audio_to(ExportScope::Stems, &t.file(&name));
    assert!(s.export_view().unwrap().in_progress());
    s.core_mut().wait_background();
    s.frame();
    let files: Vec<_> = std::fs::read_dir(t.file(&name))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(!files.is_empty());
    for f in files {
        let spec = hound::WavReader::open(&f).unwrap().spec();
        assert_eq!(spec.bits_per_sample, 24);
    }
}

#[test]
fn a_command_that_is_not_a_preset_does_nothing_and_a_dialog_that_was_closed_leaves_no_trace() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    assert!(s.execute(AppCommand::ExportFor(20)).is_empty());
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("no such export preset")
    );
    // The Club dialog is closed without a file; the next ordinary export is not the preset's.
    let club = preset_index(&s, "Club");
    s.execute(AppCommand::ExportFor(club));
    s.execute(AppCommand::ExportSongAudio);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    assert!(s.export_open());
    assert!(!s.export_view().unwrap().in_progress(), "the options show");
}

#[test]
fn the_panel_has_the_presets_and_saving_and_deleting_one_writes_the_file() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    assert_eq!(s.export_view().unwrap().presets().len(), 5);
    press(&mut s, "16-BIT");
    press(&mut s, "-14 LUFS");
    press(&mut s, "SAVE PRESET...");
    assert!(s.wants_text_input(), "the name takes input methods");
    type_text(&mut s, "Loud one");
    enter(&mut s);
    assert!(!s.wants_text_input());
    assert_eq!(s.export_view().unwrap().preset(), Some("Loud one"));
    assert_eq!(s.export_view().unwrap().presets().len(), 6);
    let saved = std::fs::read_to_string(t.file("export_presets.json")).unwrap();
    assert!(
        saved.contains("\"version\": 1") && saved.contains("Loud one"),
        "{saved}"
    );
    assert!(saved.contains("\"loudness\": \"target-14\""), "{saved}");
    assert!(
        s.core()
            .last_rejection()
            .unwrap()
            .contains("Saved the export preset")
    );

    // A new shell on the same settings has it, in the menu too.
    let s2 = shell(&t.0);
    assert_eq!(s2.export_presets().len(), 6);
    assert!(
        s2.menu_model()[0]
            .items()
            .any(|i| i.label == "Loud one" && matches!(i.command, AppCommand::ExportFor(5)))
    );

    // Deleting it: only a preset of the user's has the button.
    press(&mut s, "DELETE PRESET");
    assert_eq!(s.export_view().unwrap().presets().len(), 5);
    assert!(s.export_view().unwrap().preset().is_none());
    let saved = std::fs::read_to_string(t.file("export_presets.json")).unwrap();
    assert!(!saved.contains("Loud one"), "{saved}");
}

#[test]
fn a_name_that_cannot_be_saved_is_said_under_the_options() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    press(&mut s, "SAVE PRESET...");
    type_text(&mut s, "Streaming");
    enter(&mut s);
    let v = s.export_view().unwrap();
    assert!(v.is_naming(), "the entry stays for another name");
    assert!(
        v.warning().unwrap().contains("built-in"),
        "{:?}",
        v.warning()
    );
    assert!(!t.file("export_presets.json").exists());
}

#[test]
fn choosing_stems_in_a_song_panel_exports_stems_to_a_folder_beside_the_file() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    pick_preset(&mut s, "Stems for mixing");
    assert_eq!(s.export_view().unwrap().scope(), ExportScope::Stems);
    press(&mut s, "EXPORT");
    assert!(s.core().audio_export_busy());
    s.core_mut().wait_background();
    s.frame();
    assert!(
        t.file("Bounced mix").is_dir(),
        "{:?}",
        s.core().last_rejection()
    );
    assert!(!t.file("mix.wav").exists());
}

#[test]
fn the_export_panel_choice_of_a_preset_marks_changes_and_starts_with_its_values() {
    let t = Tmp::new();
    let mut s = shell(&t.0);
    s.export_audio_to(ExportScope::Song, &t.file("mix.wav"));
    pick_preset(&mut s, "Club");
    assert_eq!(s.export_view().unwrap().preset_label(), "Club");
    press(&mut s, "16-BIT");
    assert_eq!(s.export_view().unwrap().preset_label(), "Club (changed)");
    press(&mut s, "24-BIT");
    assert_eq!(s.export_view().unwrap().preset_label(), "Club");
    press(&mut s, "EXPORT");
    s.core_mut().wait_background();
    s.frame();
    let message = s.core().last_rejection().unwrap();
    assert!(message.contains("-9 LUFS guide"), "{message}");
}

#[test]
fn a_turn_from_a_file_to_stems_and_back_moves_the_target() {
    use super::export_impl::retarget;
    let file = PathBuf::from("music/mix.wav");
    let folder = retarget(ExportScope::Stems, &file);
    assert_eq!(folder, PathBuf::from("music/Bounced mix"));
    assert_eq!(
        retarget(ExportScope::Song, &folder),
        PathBuf::from("music/mix")
    );
}

#[test]
fn the_video_preset_without_a_film_is_an_aac_export_of_the_song() {
    let t = Tmp::new();
    let (mut s, tool) = shell_with_ffmpeg(&t);
    settle(&mut s);
    let video = preset_index(&s, "Video");
    assert_eq!(
        s.execute(AppCommand::ExportFor(video)),
        vec![ShellRequest::PickExportAudio(ExportScope::Song)]
    );
    assert_eq!(s.export_audio_extension(), "m4a");
    s.export_audio_to(ExportScope::Song, &t.file("mix.m4a"));
    s.core_mut().wait_background();
    s.frame();
    let calls: Vec<_> = tool.calls().into_iter().filter(|c| c.has("-vn")).collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].after("-c:a").unwrap(), "aac");
    assert_eq!(calls[0].after("-b:a").unwrap(), "256k");
    assert_eq!(calls[0].after("-ar").unwrap(), "48000");
}
