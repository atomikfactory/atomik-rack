//! The library in the shell: where its folder comes from (the option, the Workflow tab, the
//! default), the Choose and Default buttons of the dialog, what the file browser is told, and
//! what dropped library items and projects do. Every test uses its own temporary directory.

use super::*;
use crate::app_core::LoadTarget;
use crate::keymap_store::WORKFLOW_FILE;
use crate::session::NullBackend;
use crate::settings_dialog::Tab;
use std::path::PathBuf;

fn config(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-library-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_with(dir: &Path, library_root: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

const VIEW: Size = Size::new(1000.0, 800.0);

fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
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

fn open_workflow(s: &mut Shell<NullBackend>) {
    s.execute(AppCommand::AudioSettings);
    let r = s
        .dialog()
        .unwrap()
        .layout(VIEW)
        .tabs
        .iter()
        .find(|(_, t)| *t == Tab::Workflow)
        .unwrap()
        .0;
    click(s, r.center());
}

fn press_apply(s: &mut Shell<NullBackend>) {
    let p = s.dialog().unwrap().buttons(VIEW).0.center();
    click(s, p);
}

#[test]
fn library_root_without_choice_or_default_is_none() {
    let s = shell_with(&config("none"), None);
    assert_eq!(s.library_root(), None);
    assert_eq!(s.core().library_root(), None);
}

#[test]
fn library_root_order_is_option_then_workflow_then_default() {
    let dir = config("order");
    let default = PathBuf::from("D:/Docs/Atomik Rack/Library");
    let chosen = PathBuf::from("D:/Sounds/Library");
    let forced = PathBuf::from("D:/Tests/Library");
    let mut s = shell_with(&dir, None);
    s.set_library_default(Some(default.clone()));
    assert_eq!(s.library_root(), Some(default.clone()));
    assert_eq!(s.core().library_root(), Some(default.clone()));
    // A saved choice beats the default.
    std::fs::write(
        dir.join(WORKFLOW_FILE),
        format!("{{\"library_folder\": {:?}}}", chosen.to_string_lossy()),
    )
    .unwrap();
    let mut t = shell_with(&dir, None);
    t.set_library_default(Some(default));
    assert_eq!(t.library_root(), Some(chosen.clone()));
    // The option beats both.
    let mut u = shell_with(&dir, Some(forced.clone()));
    u.set_library_default(Some(PathBuf::from("D:/Other")));
    assert_eq!(u.library_root(), Some(forced.clone()));
    assert_eq!(u.core().library_root(), Some(forced));
}

#[test]
fn library_dialog_choose_asks_the_platform_and_apply_saves_the_folder() {
    let dir = config("choose");
    let mut s = shell_with(&dir, None);
    open_workflow(&mut s);
    let l = s.dialog().unwrap().workflow_layout(VIEW).unwrap();
    assert_eq!(
        click(&mut s, l.library.choose.center()),
        vec![ShellRequest::PickLibraryFolder]
    );
    let picked = PathBuf::from("D:/Sounds/Library");
    s.set_library_folder(picked.clone());
    assert_eq!(s.library_root(), None, "nothing applies before Apply");
    press_apply(&mut s);
    assert_eq!(s.library_root(), Some(picked.clone()));
    assert_eq!(s.core().library_root(), Some(picked.clone()));
    assert_eq!(s.workflow().library_folder, Some(picked.clone()));
    let saved = std::fs::read_to_string(dir.join(WORKFLOW_FILE)).unwrap();
    assert!(saved.contains("library_folder"), "{saved}");
    // Default goes back.
    open_workflow(&mut s);
    let l = s.dialog().unwrap().workflow_layout(VIEW).unwrap();
    click(&mut s, l.library.default.center());
    press_apply(&mut s);
    assert_eq!(s.library_root(), None);
    assert!(
        !std::fs::read_to_string(dir.join(WORKFLOW_FILE))
            .unwrap()
            .contains("library_folder")
    );
}

#[test]
fn library_folder_picked_without_a_dialog_applies_and_saves_at_once() {
    let dir = config("direct");
    let mut s = shell_with(&dir, None);
    let picked = PathBuf::from("D:/Sounds/Library");
    s.set_library_folder(picked.clone());
    assert_eq!(s.library_root(), Some(picked));
    assert!(dir.join(WORKFLOW_FILE).exists());
}

// ------------------------------------------------------------------ drops and loads

fn key(k: &str) -> atomik_model::DeviceKey {
    atomik_model::DeviceKey(k.to_string())
}

/// A shell whose library folder is `<dir>/Library` with a delay and a reverb saved there as one
/// patch (the document is left dirty); returns the shell, the folder and the patch path.
fn shell_with_patch(name: &str) -> (Shell<NullBackend>, PathBuf, PathBuf) {
    let dir = config(name);
    let mut s = shell_with(&dir, Some(dir.join("Library")));
    for (k, type_id) in [("a", "atomik.stereo-delay"), ("b", "atomik.reverb")] {
        s.core_mut()
            .apply_command(atomik_model::Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.to_string(),
                key: Some(key(k)),
            })
            .unwrap();
    }
    s.core_mut().after_view_change();
    s.core_mut()
        .view_mut()
        .set_selection(&[key("a"), key("b")], None);
    s.core_mut().save_selection_to_library();
    let path = dir.join("Library/Patches/Stereo Delay+1.atomikpatch");
    assert!(path.is_file(), "{path:?}");
    (s, dir, path)
}

/// A clip item file of one bar with one note.
fn clip_item(dir: &Path) -> PathBuf {
    let clip = atomik_model::Clip {
        id: atomik_model::ClipId(0),
        start: 0,
        length: 4 * atomik_core::PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: atomik_model::ClipContent::Notes(
            vec![atomik_model::Note::new(0, atomik_core::PPQ, 60, 0.8)].into(),
        ),
    };
    let text = atomik_model::save_clip_file(&atomik_model::ClipFile {
        tempo_bpm: 120.0,
        meter: (4, 4),
        clip,
    })
    .unwrap();
    let path = dir.join("Library/Clips/Riff.atomikclip");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn device_count(s: &Shell<NullBackend>) -> usize {
    s.core().doc().devices.len()
}

#[test]
fn file_event_a_patch_dropped_on_the_rack_inserts_it_as_one_undo_step() {
    let (mut s, _dir, path) = shell_with_patch("drop-patch");
    let before = device_count(&s);
    let at = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![path],
    });
    assert_eq!(device_count(&s), before + 2);
    assert!(s.core_mut().undo());
    assert_eq!(device_count(&s), before);
}

#[test]
fn file_event_a_device_row_dropped_on_the_rack_adds_the_device() {
    let dir = config("drop-device");
    let mut s = shell_with(&dir, Some(dir.join("Library")));
    let before = device_count(&s);
    let at = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![atomik_library::device_uri("atomik.arpeggiator")],
    });
    assert_eq!(device_count(&s), before + 1);
}

#[test]
fn file_event_library_paths_never_import_at_playhead() {
    let (mut s, dir, patch) = shell_with_patch("never-import");
    let clip = clip_item(&dir);
    let devices = device_count(&s);
    // The transport bar takes no files: audio dropped there would become new tracks.
    let nowhere = s.layout().transport.center();
    for paths in [
        vec![patch],
        vec![clip],
        vec![atomik_library::device_uri("atomik.arpeggiator")],
    ] {
        s.handle(&UiEvent::FileDrop {
            pos: nowhere,
            paths,
        });
    }
    assert!(!s.core().is_busy(), "no audio import started");
    assert!(s.core().doc().song.tracks.is_empty());
    assert_eq!(device_count(&s), devices);
}

#[test]
fn file_event_a_clip_dropped_on_the_rack_says_where_it_goes() {
    let (mut s, dir, _patch) = shell_with_patch("clip-on-rack");
    let clip = clip_item(&dir);
    let at = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![clip],
    });
    let message = s.core().last_rejection().unwrap_or_default();
    assert!(message.contains("note lane"), "{message}");
}

#[test]
fn file_event_a_clip_dropped_on_a_note_lane_lands_there_as_one_undo_step() {
    let dir = config("drop-clip");
    let mut s = shell_with(&dir, Some(dir.join("Library")));
    s.core_mut().load_library_item(
        &atomik_library::device_uri("atomik.poly-synth"),
        LoadTarget::Default,
    );
    assert_eq!(
        s.core().doc().song.tracks.len(),
        1,
        "the instrument's track"
    );
    let clip = clip_item(&dir);
    let song = s.layout().song.unwrap();
    let mut y = song.y0;
    let at = loop {
        assert!(y < song.y1, "no note lane found");
        let p = Point::new(song.x0 + 500.0, y);
        if s.core().arrangement().clip_drop_target_at(p).is_some() {
            break p;
        }
        y += 2.0;
    };
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![clip],
    });
    assert_eq!(s.core().doc().song.tracks[0].lanes[0].clips.len(), 1);
    assert!(s.core_mut().undo());
    assert!(s.core().doc().song.tracks[0].lanes[0].clips.is_empty());
}

#[test]
fn file_event_a_project_dropped_anywhere_asks_before_discarding_changes() {
    let (mut s, dir, _patch) = shell_with_patch("drop-project");
    let project = dir.join("Song.atomik");
    std::fs::write(
        &project,
        atomik_model::save_project(&atomik_model::empty_doc()),
    )
    .unwrap();
    assert!(s.core().is_dirty());
    let at = s.layout().transport.center();
    let requests = s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![project.clone()],
    });
    assert_eq!(
        requests,
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::OpenFile(project)
        }]
    );
    assert!(!s.core().is_busy(), "not imported as audio");
}

#[test]
fn file_event_a_project_dropped_on_a_clean_document_opens_it() {
    let dir = config("drop-clean");
    let mut s = shell_with(&dir, None);
    let mut other = shell_with(&dir, None);
    other
        .core_mut()
        .apply_command(atomik_model::Command::AddDevice {
            index: 0,
            type_id: "atomik.stereo-delay".to_string(),
            key: Some(key("opened")),
        })
        .unwrap();
    let project = dir.join("Song.atomik");
    std::fs::write(&project, atomik_model::save_project(other.core().doc())).unwrap();
    assert!(!s.core().is_dirty());
    let at = s.layout().rack.center();
    let requests = s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![project],
    });
    assert!(requests.is_empty(), "{requests:?}");
    assert!(s.core().doc().devices.iter().any(|d| d.key.0 == "opened"));
}

#[test]
fn library_open_request_from_the_browser_follows_the_dirty_rule() {
    let (mut s, dir, patch) = shell_with_patch("open-request");
    s.core_mut()
        .browse(atomik_ui::BrowseAction::OpenProject(patch.clone()));
    assert_eq!(
        s.handle(&UiEvent::FileHoverCancel),
        vec![ShellRequest::ConfirmSave {
            then: PendingAction::OpenFile(patch.clone())
        }]
    );
    // The same request on a clean document opens the item as a project.
    let mut clean = shell_with(&dir, Some(dir.join("Library")));
    clean
        .core_mut()
        .browse(atomik_ui::BrowseAction::OpenProject(patch));
    assert!(clean.handle(&UiEvent::FileHoverCancel).is_empty());
    let reverbs = clean
        .core()
        .doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.reverb")
        .count();
    assert_eq!(reverbs, 1);
}

#[test]
fn saving_with_the_panel_open_gives_the_panel_the_keys_for_the_name() {
    let dir = config("save-focus");
    let mut s = shell_with(&dir, Some(dir.join("Library")));
    s.execute(AppCommand::ToggleFileBrowser);
    for _ in 0..200 {
        s.frame();
        if !s.core().is_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    s.core_mut()
        .apply_command(atomik_model::Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.stereo-delay".to_string(),
            key: Some(key("a")),
        })
        .unwrap();
    s.core_mut().after_view_change();
    s.core_mut().view_mut().set_selection(&[key("a")], None);
    assert_ne!(s.focus(), Pane::FileBrowser);
    s.core_mut().save_selection_to_library();
    for _ in 0..200 {
        s.frame();
        if !s.core().is_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    s.frame();
    assert_eq!(s.focus(), Pane::FileBrowser);
    assert!(s.core().file_browser().is_text_owner());
}
