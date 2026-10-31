//! Templates, the default song, song information, the scratch folder, the project folder relink
//! and project archives in the app core. All fixtures are procedural and live in temporary
//! folders.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::SongInfo;
use atomik_ui::ImportTarget;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-templates-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The library folder of a core whose templates folder is `<tmp>/Templates`.
    fn library(&self) -> PathBuf {
        self.path("Library")
    }

    fn templates(&self) -> PathBuf {
        self.path("Templates")
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core(t: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_library_root(Some(t.library()));
    c
}

fn wav(dir: &Tmp, name: &str, frames: usize, seed: f32) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * seed)
        .collect();
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

/// Imports one file as a new track and returns its asset id.
fn import_one(
    c: &mut AppCore<RecordingSink>,
    t: &Tmp,
    name: &str,
    frames: usize,
    seed: f32,
) -> String {
    let before: Vec<String> = c.doc().assets.keys().cloned().collect();
    c.import_audio(vec![wav(t, name, frames, seed)], ImportTarget::NewTracks, 0);
    c.wait_background();
    c.doc()
        .assets
        .keys()
        .find(|h| !before.contains(h))
        .expect("a new asset")
        .clone()
}

fn names(list: &[TemplateEntry]) -> Vec<&str> {
    list.iter().map(|e| e.name.as_str()).collect()
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "{}").unwrap();
}

// ------------------------------------------------------------------ the list

#[test]
fn the_list_holds_project_files_and_project_folders_by_name_without_regard_to_case() {
    let t = Tmp::new("list");
    let dir = t.templates();
    touch(&dir.join("banana.atomik"));
    touch(&dir.join("Apple.ATOMIK"));
    touch(&dir.join("cherry").join("cherry.atomik"));
    touch(&dir.join("notes.txt"));
    touch(&dir.join(".hidden.atomik"));
    touch(&dir.join("other").join("not-the-name.atomik"));
    touch(&dir.join("Audio").join("x.wav"));
    let list = list_templates(&dir);
    assert_eq!(names(&list), ["Apple", "banana", "cherry"]);
    assert_eq!(list[2].path, dir.join("cherry").join("cherry.atomik"));
    assert!(list_templates(&t.path("missing")).is_empty());
}

#[test]
fn a_name_is_listed_once_and_the_list_is_capped() {
    let t = Tmp::new("cap");
    let dir = t.templates();
    touch(&dir.join("same.atomik"));
    touch(&dir.join("same").join("same.atomik"));
    for i in 0..30 {
        touch(&dir.join(format!("t{i:02}.atomik")));
    }
    let list = list_templates(&dir);
    assert_eq!(list.len(), MAX_TEMPLATES);
    assert_eq!(names(&list).iter().filter(|n| **n == "same").count(), 1);
}

#[test]
fn the_folder_is_next_to_the_library_and_the_list_follows_it() {
    let t = Tmp::new("dir");
    let mut c = core(&t);
    assert_eq!(c.templates_dir(), Some(t.templates()));
    assert!(c.templates().is_empty());
    touch(&t.templates().join("first.atomik"));
    // A look within the second of the last one changes nothing; a later one lists the file.
    assert!(c.refresh_templates(100.0));
    assert!(c.templates().is_empty() || names(c.templates()) == ["first"]);
    touch(&t.templates().join("second.atomik"));
    assert!(!c.refresh_templates(100.5));
    assert!(c.refresh_templates(101.5));
    assert_eq!(names(c.templates()), ["first", "second"]);
    std::fs::remove_file(t.templates().join("first.atomik")).unwrap();
    assert!(c.refresh_templates(103.0));
    assert_eq!(names(c.templates()), ["second"]);
    assert!(!c.refresh_templates(105.0), "nothing changed");
    // No library folder, no templates folder.
    c.set_library_root(None);
    assert_eq!(c.templates_dir(), None);
    assert!(c.reload_templates());
    assert!(c.templates().is_empty());
}

#[test]
fn showing_the_folder_makes_it() {
    let t = Tmp::new("show");
    let mut c = core(&t);
    assert!(!t.templates().exists());
    assert_eq!(c.template_folder_for_show(), Some(t.templates()));
    assert!(t.templates().is_dir());
}

// ------------------------------------------------------------------ opening a template

#[test]
fn a_template_opens_as_an_untitled_song_that_save_cannot_overwrite() {
    let t = Tmp::new("open");
    let mut c = core(&t);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(DeviceKey("lead".into())),
    })
    .unwrap();
    c.save_as(&t.path("original.atomik")).unwrap();
    let template = c.save_as_template("My Template").unwrap();
    let bytes = std::fs::read(&template).unwrap();

    let mut other = core(&t);
    other.open_template(&template).unwrap();
    assert_eq!(other.file(), None);
    assert!(!other.is_dirty());
    assert_eq!(other.title(), "Atomik Rack \u{2014} Untitled");
    assert!(other.doc().devices.iter().any(|d| d.key.0 == "lead"));
    assert!(matches!(other.save(), Err(AppError::NoPath)));
    // Edits and a Save As elsewhere leave the template alone.
    other
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey("second".into())),
        })
        .unwrap();
    other.save_as(&t.path("made.atomik")).unwrap();
    assert_eq!(std::fs::read(&template).unwrap(), bytes);
    assert!(other.recent().iter().all(|p| p != &template));
}

#[test]
fn a_template_that_is_not_a_project_changes_nothing() {
    let t = Tmp::new("bad");
    let mut c = core(&t);
    let before = c.doc().clone();
    std::fs::write(t.path("bad.atomik"), "not json").unwrap();
    assert!(c.open_template(&t.path("bad.atomik")).is_err());
    assert!(c.open_template(&t.path("missing.atomik")).is_err());
    assert_eq!(c.doc(), &before);
}

#[test]
fn a_template_with_audio_plays_and_the_audio_is_copied_on_the_first_save() {
    let t = Tmp::new("audio");
    let mut c = core(&t);
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let template = c.save_as_template("Drums").unwrap();
    let source = t
        .templates()
        .join("Drums")
        .join("Audio")
        .join(format!("{hash}.wav"));
    assert!(source.is_file(), "the template folder holds its audio");
    let original = std::fs::read(&source).unwrap();

    let mut other = core(&t);
    other.open_template(&template).unwrap();
    other.wait_background();
    assert!(other.missing_assets().is_empty());
    assert_eq!(other.song_assets()[&hash].frames(), 4_800, "it plays");
    assert!(
        other
            .bundle_root()
            .is_some_and(|r| r != t.templates().join("Drums"))
    );
    other.save_as(&t.path("copy.atomik")).unwrap();
    let copied = t.path("copy").join("Audio").join(format!("{hash}.wav"));
    assert!(std::fs::read(&copied).unwrap() == original, "bit for bit");
    assert!(source.is_file(), "the template keeps its audio");
}

#[test]
fn a_template_that_lacks_audio_reports_it_and_opens() {
    let t = Tmp::new("lacks");
    let mut c = core(&t);
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let template = c.save_as_template("Lacking").unwrap();
    std::fs::remove_file(
        t.templates()
            .join("Lacking")
            .join("Audio")
            .join(format!("{hash}.wav")),
    )
    .unwrap();
    let mut other = core(&t);
    other.open_template(&template).unwrap();
    other.wait_background();
    assert_eq!(other.missing_assets().len(), 1);
    assert!(
        other
            .last_rejection()
            .is_some_and(|m| m.contains("missing"))
    );
}

#[test]
fn a_template_with_a_plugin_keeps_its_stand_in() {
    let t = Tmp::new("plugin");
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo/plugin-demo.atomik");
    let mut c = core(&t);
    c.open_template(&example).unwrap();
    assert!(!c.doc().plugin_devices().is_empty());
    assert_eq!(c.file(), None);
    for (_, d) in c.doc().plugin_devices() {
        assert!(
            c.registry().get(&d.type_id).is_some(),
            "{} is registered",
            d.type_id
        );
    }
}

// ------------------------------------------------------------------ saving templates

#[test]
fn saving_a_template_copies_the_song_and_leaves_it_untouched() {
    let t = Tmp::new("save");
    let mut c = core(&t);
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    // Audio that only the history holds is not copied.
    let doc = c.doc().clone();
    let (file, dirty) = (c.file().map(Path::to_path_buf), c.is_dirty());
    let first = c.save_as_template("Song").unwrap();
    let second = c.save_as_template("Song").unwrap();
    assert_eq!(first, t.templates().join("Song").join("Song.atomik"));
    assert_eq!(second, t.templates().join("Song 2").join("Song 2.atomik"));
    assert_eq!(c.doc(), &doc);
    assert_eq!(
        (c.file().map(Path::to_path_buf), c.is_dirty()),
        (file, dirty)
    );
    assert_eq!(names(c.templates()), ["Song", "Song 2"]);
    for folder in ["Song", "Song 2"] {
        let wavs: Vec<_> = std::fs::read_dir(t.templates().join(folder).join("Audio"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        assert_eq!(wavs, [format!("{hash}.wav")], "{folder}");
    }
    // A song without audio is one plain file; a name with characters a file cannot hold is cleaned.
    let mut plain = core(&t);
    let file = plain.save_as_template("a/b:c").unwrap();
    assert_eq!(file, t.templates().join("a_b_c.atomik"));
    assert!(!t.templates().join("a_b_c").exists());
}

#[test]
fn a_saved_template_opens_to_the_song_it_was_saved_from() {
    let t = Tmp::new("roundtrip");
    let mut c = core(&t);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(DeviceKey("lead".into())),
    })
    .unwrap();
    c.set_song_info(SongInfo {
        title: "Kept".into(),
        ..SongInfo::default()
    })
    .unwrap();
    let template = c.save_as_template(&c.template_name()).unwrap();
    assert_eq!(template, t.templates().join("Kept.atomik"));
    let mut other = core(&t);
    other.open_template(&template).unwrap();
    assert_eq!(other.doc(), c.doc());
}

#[test]
fn the_default_song_is_replaced_not_added() {
    let t = Tmp::new("default");
    let mut c = core(&t);
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let first = c.save_default_song().unwrap();
    assert_eq!(
        first,
        t.templates()
            .join(DEFAULT_SONG_NAME)
            .join(format!("{DEFAULT_SONG_NAME}.atomik"))
    );
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(DeviceKey("later".into())),
    })
    .unwrap();
    let second = c.save_default_song().unwrap();
    assert_eq!(first, second);
    assert_eq!(names(c.templates()), [DEFAULT_SONG_NAME]);
    let mut other = core(&t);
    other.open_template(&second).unwrap();
    other.wait_background();
    assert!(other.doc().devices.iter().any(|d| d.key.0 == "later"));
    assert_eq!(other.song_assets()[&hash].frames(), 4_800);
    // Nothing of the swap is left in the folder.
    let left: Vec<String> = std::fs::read_dir(t.templates())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, [DEFAULT_SONG_NAME]);
    // And a song without audio replaces it by a plain file.
    let mut plain = core(&t);
    let third = plain.save_default_song().unwrap();
    assert_eq!(
        third,
        t.templates().join(format!("{DEFAULT_SONG_NAME}.atomik"))
    );
    assert!(!t.templates().join(DEFAULT_SONG_NAME).exists());
}

// ------------------------------------------------------------------ song information

fn info() -> SongInfo {
    SongInfo {
        title: "Example Song".into(),
        author: "Example Author".into(),
        notes: "one\ntwo".into(),
        url: "https://example.org".into(),
        email: "author@example.org".into(),
    }
}

#[test]
fn the_title_shows_in_the_window_title() {
    let t = Tmp::new("title");
    let mut c = core(&t);
    assert_eq!(c.title(), "Atomik Rack \u{2014} Untitled");
    c.set_song_info(SongInfo {
        title: "  My Song ".into(),
        ..SongInfo::default()
    })
    .unwrap();
    assert_eq!(c.title(), "Atomik Rack \u{2014} My Song *");
    c.save_as(&t.path("file.atomik")).unwrap();
    assert_eq!(c.title(), "Atomik Rack \u{2014} My Song (file.atomik)");
    // Information without a title keeps the file name.
    c.set_song_info(SongInfo {
        author: "Someone".into(),
        ..SongInfo::default()
    })
    .unwrap();
    assert!(c.title().starts_with("Atomik Rack \u{2014} file.atomik"));
}

#[test]
fn song_information_is_one_undo_step_and_survives_save_and_open_byte_for_byte() {
    let t = Tmp::new("info");
    let mut c = core(&t);
    c.set_song_info(info()).unwrap();
    assert!(c.is_dirty());
    c.undo();
    assert!(c.song_info().is_empty());
    assert!(!c.is_dirty());
    c.redo();
    assert_eq!(c.song_info(), &info());
    c.save_as(&t.path("a.atomik")).unwrap();
    let first = std::fs::read(t.path("a.atomik")).unwrap();
    let mut other = core(&t);
    other.open(&t.path("a.atomik")).unwrap();
    assert_eq!(other.song_info(), &info());
    other.save_as(&t.path("b.atomik")).unwrap();
    assert_eq!(std::fs::read(t.path("b.atomik")).unwrap(), first);
}

#[test]
fn a_text_over_its_limit_is_refused_with_a_message_and_changes_nothing() {
    let t = Tmp::new("limit");
    let mut c = core(&t);
    let bad = SongInfo {
        title: "x".repeat(atomik_model::MAX_INFO_TITLE_CHARS + 1),
        ..SongInfo::default()
    };
    assert!(c.set_song_info(bad).is_err());
    assert!(c.song_info().is_empty());
    assert!(!c.is_dirty());
    assert!(c.last_rejection().is_some_and(|m| m.contains("title")));
    // The same information again is not a step.
    c.set_song_info(info()).unwrap();
    c.set_song_info(info()).unwrap();
    c.undo();
    assert!(c.song_info().is_empty());
}

// ------------------------------------------------------------------ the scratch folder

#[test]
fn the_scratch_folder_holds_the_session_bundle_from_the_next_import() {
    let t = Tmp::new("scratch");
    let mut c = core(&t);
    assert_eq!(c.scratch_folder(), None);
    let first = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let temp_root = c.bundle_root().unwrap().to_path_buf();
    assert!(temp_root.starts_with(std::env::temp_dir()));
    // The folder counts from the next import that needs a session bundle: a new song.
    let scratch = t.path("scratch-here");
    c.set_scratch_folder(Some(scratch.clone()));
    assert_eq!(c.scratch_folder(), Some(scratch.as_path()));
    assert_eq!(c.bundle_root().unwrap(), temp_root, "the open bundle stays");
    c.new_document();
    let second = import_one(&mut c, &t, "b.wav", 4_800, 0.5);
    let root = c.bundle_root().unwrap().to_path_buf();
    assert!(root.starts_with(&scratch), "{root:?}");
    assert!(root.join("Audio").join(format!("{second}.wav")).is_file());
    assert_ne!(first, second);
    // Saving moves the audio into the project and removes the session bundle.
    c.save_as(&t.path("song.atomik")).unwrap();
    assert!(!root.exists());
    // Back to the temporary folder.
    c.new_document();
    c.set_scratch_folder(None);
    import_one(&mut c, &t, "c.wav", 4_800, 0.25);
    assert!(c.bundle_root().unwrap().starts_with(std::env::temp_dir()));
}

#[test]
fn a_scratch_folder_that_cannot_be_used_falls_back_to_the_temporary_folder() {
    let t = Tmp::new("scratch-bad");
    let mut c = core(&t);
    // A file where the folder should be.
    std::fs::write(t.path("not-a-folder"), "x").unwrap();
    c.set_scratch_folder(Some(t.path("not-a-folder")));
    import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    assert!(c.bundle_root().unwrap().starts_with(std::env::temp_dir()));
    assert_eq!(c.doc().assets.len(), 1, "the import still worked");
    // An empty path is the same as none.
    c.set_scratch_folder(Some(PathBuf::new()));
    assert_eq!(c.scratch_folder(), None);
}

// ------------------------------------------------------------------ the project folder

/// A saved project with audio, then only its `.atomik` file moved away.
fn moved_project(t: &Tmp) -> (PathBuf, PathBuf, String) {
    let mut c = core(t);
    let hash = import_one(&mut c, t, "a.wav", 4_800, 1.0);
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    let bundle = t.path("home").join("song");
    let moved = t.path("moved").join("song.atomik");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::copy(bundle.join("song.atomik"), &moved).unwrap();
    (moved, bundle, hash)
}

#[test]
fn the_project_folder_gives_the_missing_audio_back_by_content() {
    let t = Tmp::new("folder");
    let (moved, bundle, hash) = moved_project(&t);
    let mut c = core(&t);
    c.open(&moved).unwrap();
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 1);
    c.relink_project_folder(&bundle);
    c.wait_background();
    assert!(c.missing_assets().is_empty(), "{:?}", c.last_rejection());
    assert_eq!(c.song_assets()[&hash].frames(), 4_800);
    assert!(!c.is_dirty(), "the document did not change");
    // The first save copies it beside the moved project.
    c.save().unwrap();
    assert!(
        t.path("moved")
            .join("song")
            .join("Audio")
            .join(format!("{hash}.wav"))
            .is_file()
    );
}

#[test]
fn the_audio_folder_or_the_folder_beside_the_project_file_will_do() {
    let t = Tmp::new("folder-forms");
    let (moved, bundle, hash) = moved_project(&t);
    for chosen in [bundle.join("Audio"), t.path("home")] {
        let mut c = core(&t);
        c.open(&moved).unwrap();
        c.wait_background();
        c.relink_project_folder(&chosen);
        c.wait_background();
        assert!(
            c.missing_assets().is_empty(),
            "{chosen:?}: {:?}",
            c.last_rejection()
        );
        assert!(c.song_assets().contains_key(&hash));
    }
}

#[test]
fn a_folder_without_the_audio_says_so_and_changes_nothing() {
    let t = Tmp::new("folder-wrong");
    let (moved, _, _) = moved_project(&t);
    let mut c = core(&t);
    c.open(&moved).unwrap();
    c.wait_background();
    c.relink_project_folder(&t.path("empty"));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("no Audio folder"))
    );
    std::fs::create_dir_all(t.path("empty").join("Audio")).unwrap();
    c.relink_project_folder(&t.path("empty"));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("None of the missing"))
    );
    assert_eq!(c.missing_assets().len(), 1);
    // Nothing is missing in a project that has everything.
    let mut whole = core(&t);
    whole.relink_project_folder(&t.path("empty"));
    assert!(
        whole
            .last_rejection()
            .is_some_and(|m| m.contains("No missing"))
    );
}

// ------------------------------------------------------------------ archives

/// A song with audio saved in `t/home`, and its archive in `t/song.atomikz`.
fn archived(t: &Tmp) -> (AppCore<RecordingSink>, String) {
    let mut c = core(t);
    let hash = import_one(&mut c, t, "a.wav", 9_600, 1.0);
    c.set_song_info(info()).unwrap();
    c.save_as(&t.path("home").join("song.atomik")).unwrap();
    c.start_archive_export(t.path("song.atomikz")).unwrap();
    assert!(c.archive_busy() || t.path("song.atomikz").exists());
    c.wait_background();
    assert!(!c.archive_busy());
    (c, hash)
}

#[test]
fn an_archive_round_trip_gives_the_same_project_text_and_audio() {
    let t = Tmp::new("archive");
    let (c, hash) = archived(&t);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Wrote song.atomikz")),
        "{:?}",
        c.last_rejection()
    );
    assert!(!t.path("song.atomikz.part").exists());
    let before = std::fs::read(t.path("home").join("song").join("song.atomik")).unwrap();
    let audio = std::fs::read(
        t.path("home")
            .join("song")
            .join("Audio")
            .join(format!("{hash}.wav")),
    )
    .unwrap();

    let mut other = core(&t);
    other.start_archive_import(t.path("song.atomikz")).unwrap();
    other.wait_background();
    let project = other
        .take_open_request()
        .expect("the project is to be opened");
    assert_eq!(project, t.path("song").join("song.atomik"));
    assert_eq!(
        std::fs::read(&project).unwrap(),
        before,
        "byte-identical project text"
    );
    assert!(
        std::fs::read(t.path("song").join("Audio").join(format!("{hash}.wav"))).unwrap() == audio,
        "identical audio"
    );
    other.open(&project).unwrap();
    other.wait_background();
    assert!(other.missing_assets().is_empty());
    assert_eq!(other.song_info(), &info());
    assert_eq!(other.doc(), c.doc());
    assert!(!other.archive_busy());
    // A second unpack never touches the first folder: it goes to `song 2`.
    other.start_archive_import(t.path("song.atomikz")).unwrap();
    other.wait_background();
    assert_eq!(
        other.take_open_request(),
        Some(t.path("song 2").join("song 2.atomik"))
    );
}

#[test]
fn an_archive_of_a_song_with_missing_audio_counts_what_it_lacks() {
    let t = Tmp::new("archive-missing");
    let (moved, _, _) = moved_project(&t);
    let mut c = core(&t);
    c.open(&moved).unwrap();
    c.wait_background();
    c.start_archive_export(t.path("lacking.atomikz")).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap_or_default().to_owned();
    assert!(
        message.contains("1 missing audio file not included"),
        "{message}"
    );
}

#[test]
fn a_damaged_or_foreign_archive_fails_with_a_message_and_leaves_nothing() {
    let t = Tmp::new("archive-bad");
    archived(&t);
    let mut bytes = std::fs::read(t.path("song.atomikz")).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    bytes[at - 20] ^= 0xFF;
    std::fs::write(t.path("damaged.atomikz"), &bytes).unwrap();
    std::fs::write(t.path("foreign.atomikz"), b"not an archive").unwrap();
    for name in ["damaged", "foreign"] {
        let mut c = core(&t);
        c.start_archive_import(t.path(&format!("{name}.atomikz")))
            .unwrap();
        c.wait_background();
        assert!(c.take_open_request().is_none(), "{name}");
        assert!(
            c.last_rejection().is_some_and(|m| m.contains("failed")),
            "{name}: {:?}",
            c.last_rejection()
        );
        assert!(!t.path(name).exists(), "no folder for {name}");
        assert!(!t.path(&format!("{name}.part")).exists());
    }
    let mut c = core(&t);
    assert!(c.start_archive_import(t.path("missing.atomikz")).is_err());
    c.cancel_archive();
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("No project archive"))
    );
}

#[test]
fn an_archive_holds_only_what_the_song_uses_and_the_song_is_untouched() {
    let t = Tmp::new("archive-used");
    let mut c = core(&t);
    import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let kept = import_one(&mut c, &t, "b.wav", 4_800, 0.5);
    // Remove the first clip: its audio is only in the history now.
    let first = c.doc().song.tracks[0].device.clone();
    c.apply_command(Command::RemoveDevice { key: first })
        .unwrap();
    let doc = c.doc().clone();
    let dirty = c.is_dirty();
    c.start_archive_export(t.path("used.atomikz")).unwrap();
    c.wait_background();
    assert_eq!(c.doc(), &doc);
    assert_eq!(c.is_dirty(), dirty);
    let mut other = core(&t);
    other.start_archive_import(t.path("used.atomikz")).unwrap();
    other.wait_background();
    let files: Vec<String> = std::fs::read_dir(t.path("used").join("Audio"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, [format!("{kept}.wav")]);
}

#[test]
fn a_name_the_archive_cannot_use_becomes_project() {
    let t = Tmp::new("archive-name");
    let mut c = core(&t);
    assert_eq!(c.archive_name(), "Untitled");
    c.set_song_info(SongInfo {
        title: "a/b".into(),
        ..SongInfo::default()
    })
    .unwrap();
    assert_eq!(c.archive_name(), "project");
}
