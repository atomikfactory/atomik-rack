//! `Shell` export tests: File > Export Song / Loop as Audio File, the file dialog request, the
//! options panel, the progress panel with CANCEL, and the choices remembered in `export.json`.

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, Note, SongCommand, WorkflowCommand,
};
use atomik_ui::{ExportDepth, ExportDither};
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
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Item(i) if i.command == command => Some(i.clone()),
            _ => None,
        })
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
    press(&mut s, "EXPORT");
    assert!(s.export_view().unwrap().in_progress());
    press(&mut s, "CANCEL");
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
