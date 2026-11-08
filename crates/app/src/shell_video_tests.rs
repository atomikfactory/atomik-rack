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
fn the_file_menu_has_a_video_submenu_with_the_three_commands() {
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
            ("Prepare Video\u{2026}".to_owned(), true),
            ("Check FFmpeg".to_owned(), true),
            ("Cancel Video Job".to_owned(), false),
        ]
    );
}

#[test]
fn prepare_video_asks_the_platform_for_a_film() {
    let t = Tmp::new("pick");
    let mut s = shell(&t, FakeTool::new());
    assert_eq!(
        s.execute(AppCommand::PrepareVideo),
        vec![ShellRequest::PickVideoFile]
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
    s.prepare_video_from(&film);
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
