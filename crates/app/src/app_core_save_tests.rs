//! Saving on a worker: the document stays editable, the baseline switches when the worker
//! reports in, and a failed or abandoned save changes nothing.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{Command, empty_doc};
use atomik_ui::ImportTarget;
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-save-{tag}-{}-{n}", std::process::id()));
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

fn core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::new(), empty_doc(), None)
}

fn edit(c: &mut AppCore<RecordingSink>, cents: f32) {
    c.apply_command(Command::SetMasterTune(cents)).unwrap();
}

fn import_wav(c: &mut AppCore<RecordingSink>, t: &Tmp) {
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let path = t.path("a.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
}

#[test]
fn a_background_save_writes_the_file_and_switches_the_baseline_when_it_is_applied() {
    let t = Tmp::new("basic");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 20.0);
    assert!(c.is_dirty());
    c.save_as_in_background(&file).unwrap();
    assert!(c.saving());
    assert_eq!(c.save_progress().as_deref(), Some("Saving..."));
    // Nothing switches until the worker's result is applied.
    assert!(c.file().is_none());
    assert!(c.is_dirty());
    c.settle_save();
    assert!(!c.saving());
    assert_eq!(c.file(), Some(file.as_path()));
    assert!(!c.is_dirty());
    assert!(file.exists());
    let reopened =
        atomik_model::load_project(&std::fs::read_to_string(&file).unwrap(), c.registry()).unwrap();
    assert_eq!(reopened.tune_cents, 20.0);
}

#[test]
fn the_bytes_of_a_background_save_equal_those_of_a_blocking_save() {
    let t = Tmp::new("bytes");
    let (a, b) = (t.path("a.atomik"), t.path("b.atomik"));
    let mut c = core();
    edit(&mut c, -12.0);
    c.save_as(&a).unwrap();
    c.save_as_in_background(&b).unwrap();
    c.settle_save();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn an_edit_during_the_save_stays_unsaved_and_undoing_it_is_clean() {
    let t = Tmp::new("during");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 10.0);
    c.save_as_in_background(&file).unwrap();
    edit(&mut c, 30.0);
    c.settle_save();
    assert!(c.is_dirty(), "the edit is newer than the saved state");
    let saved =
        atomik_model::load_project(&std::fs::read_to_string(&file).unwrap(), c.registry()).unwrap();
    assert_eq!(
        saved.tune_cents, 10.0,
        "the file holds the state at the click"
    );
    assert!(c.undo());
    assert!(!c.is_dirty(), "undo back to the saved state is clean");
}

#[test]
fn polling_applies_a_finished_save() {
    let t = Tmp::new("poll");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 5.0);
    c.save_as_in_background(&file).unwrap();
    for _ in 0..2_000 {
        c.poll_background();
        if !c.saving() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!c.saving());
    assert!(!c.is_dirty());
    assert!(c.last_rejection().is_none());
}

#[test]
fn a_save_that_fails_reports_and_changes_nothing() {
    let t = Tmp::new("fails");
    let missing = t.path("no-such-folder").join("song.atomik");
    let mut c = core();
    edit(&mut c, 7.0);
    c.save_as_in_background(&missing).unwrap();
    c.settle_save();
    assert!(!c.saving());
    assert!(c.file().is_none());
    assert!(c.is_dirty());
    assert!(c.last_rejection().is_some());
    assert!(!missing.exists());
}

#[test]
fn a_failed_save_leaves_the_previous_file_intact() {
    let t = Tmp::new("intact");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 1.0);
    c.save_as(&file).unwrap();
    let before = std::fs::read(&file).unwrap();
    // The temp file cannot be made: a directory sits where the project is written next to.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    edit(&mut c, 2.0);
    c.save_in_background().unwrap();
    c.settle_save();
    assert!(c.last_rejection().is_some());
    assert!(c.is_dirty());
    assert!(file.is_dir());
    assert!(!before.is_empty());
}

#[test]
fn a_save_as_with_audio_copies_the_bundle_on_the_worker_and_switches_on_completion() {
    let t = Tmp::new("bundle");
    let mut c = core();
    import_wav(&mut c, &t);
    let scratch = c.bundle_root().unwrap().to_path_buf();
    let dest = t.path("out");
    std::fs::create_dir_all(&dest).unwrap();
    c.save_as_in_background(&dest.join("song.atomik")).unwrap();
    assert!(c.saving());
    assert_eq!(
        c.bundle_root(),
        Some(scratch.as_path()),
        "still the scratch bundle"
    );
    c.settle_save();
    let root = dest.join("song");
    assert_eq!(c.bundle_root(), Some(root.as_path()));
    assert_eq!(c.file(), Some(root.join("song.atomik").as_path()));
    assert!(!c.is_dirty());
    assert!(
        !scratch.exists(),
        "the scratch bundle goes once the copy is done"
    );
    assert_eq!(
        std::fs::read_dir(root.join("Audio"))
            .unwrap()
            .flatten()
            .filter(|e| { e.path().extension().is_some_and(|x| x == "wav") })
            .count(),
        1
    );
}

#[test]
fn replacing_the_project_waits_for_a_running_save() {
    let t = Tmp::new("replace");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 3.0);
    c.save_as_in_background(&file).unwrap();
    c.new_document();
    assert!(!c.saving());
    assert!(
        file.exists(),
        "the save finished before the project was replaced"
    );
    assert!(c.file().is_none());
    assert!(!c.is_dirty());
}

#[test]
fn an_import_waits_for_a_running_save_before_it_touches_the_bundle() {
    let t = Tmp::new("import");
    let file = t.path("song.atomik");
    let mut c = core();
    edit(&mut c, 3.0);
    c.save_as_in_background(&file).unwrap();
    import_wav(&mut c, &t);
    assert!(!c.saving());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.doc().assets.len(), 1);
}

#[test]
fn a_save_runs_in_the_foreground_while_audio_is_still_loading() {
    let t = Tmp::new("busy");
    let file = t.path("song.atomik");
    let mut c = core();
    let data: Vec<f32> = (0..4_800).map(|i| i as f32 / 4_800.0).collect();
    let path = t.path("a.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    assert!(c.is_busy());
    c.save_as_in_background(&file).unwrap();
    assert!(!c.saving(), "a blocking save ran instead");
    assert!(file.exists());
}

/// `cargo test -p atomik-app --release --lib save_speed -- --ignored --nocapture`
#[test]
#[ignore = "timing: run by hand"]
#[allow(clippy::disallowed_methods)]
fn save_speed_for_a_large_project() {
    let t = Tmp::new("speed");
    let file = t.path("big.atomik");
    let probe = core();
    let doc = atomik_model::stress::large_song_doc(probe.registry(), 100, 500, 200_000);
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    let best = |f: &mut dyn FnMut() -> std::time::Duration| (0..5).map(|_| f()).min().unwrap();
    let blocking = best(&mut || {
        let start = std::time::Instant::now();
        c.save_as(&file).unwrap();
        start.elapsed()
    });
    let ui_thread = best(&mut || {
        let start = std::time::Instant::now();
        c.save_as_in_background(&file).unwrap();
        let blocked = start.elapsed();
        c.settle_save();
        blocked
    });
    println!(
        "save, 200 000 notes: blocking {blocking:?}, UI thread blocked {ui_thread:?} \
         (the project text is {} bytes)",
        std::fs::metadata(&file).unwrap().len()
    );
}

/// Imports a ramp of `frames` frames and returns its asset id.
fn import_ramp(c: &mut AppCore<RecordingSink>, t: &Tmp, name: &str, frames: usize) -> String {
    let before: BTreeSet<String> = c.doc().assets.keys().cloned().collect();
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    let path = t.path(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    c.doc()
        .assets
        .keys()
        .find(|h| !before.contains(*h))
        .cloned()
        .unwrap()
}

fn wav_of(root: &Path, hash: &str) -> PathBuf {
    root.join("Audio").join(format!("{hash}.wav"))
}

/// A project with one audio file in `<tmp>/proj/` (the bundle folder holding `proj.atomik`), and
/// the id of that file.
fn project_with_audio(t: &Tmp) -> (AppCore<RecordingSink>, String) {
    let mut c = core();
    let hash = import_ramp(&mut c, t, "one.wav", 4_800);
    std::fs::create_dir_all(t.path("proj")).unwrap();
    c.save_as(&t.path("proj").join("proj.atomik")).unwrap();
    (c, hash)
}

#[test]
fn collecting_files_gives_a_project_that_shares_an_audio_folder_audio_of_its_own() {
    let t = Tmp::new("collect-shared");
    let (_, hash) = project_with_audio(&t);
    let shared = t.path("proj");
    let other = Bundle::open(&shared)
        .unwrap()
        .add_recording(
            &AudioData::new(48_000, vec![vec![0.25; 900]]).unwrap(),
            "other",
        )
        .unwrap();
    // A second project file in the same folder: its bundle is the shared folder.
    let file = shared.join("song.atomik");
    std::fs::copy(shared.join("proj.atomik"), &file).unwrap();
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    assert_eq!(c.bundle_root(), Some(shared.as_path()));

    c.collect_files().unwrap();
    c.wait_background();

    let own = shared.join("song");
    assert_eq!(c.bundle_root(), Some(own.as_path()));
    assert_eq!(c.file(), Some(file.as_path()), "the project file stays");
    assert!(!c.is_dirty());
    assert!(
        std::fs::read(wav_of(&own, &hash)).unwrap()
            == std::fs::read(wav_of(&shared, &hash)).unwrap(),
        "the audio is copied bit for bit"
    );
    assert!(
        !wav_of(&own, &other.hash).exists(),
        "audio of others stays behind"
    );
    assert!(
        wav_of(&shared, &other.hash).exists(),
        "the shared folder is not touched"
    );
    assert!(wav_of(&shared, &hash).exists());
    let message = c.last_rejection().unwrap();
    assert!(message.starts_with("Collected 1 audio file ("), "{message}");
    assert!(message.contains("into song"), "{message}");
    assert_eq!(
        atomik_render::bundle::existing_bundle_root(&file),
        Some(own)
    );
}

#[test]
fn collecting_files_finds_audio_a_moved_project_left_in_the_folder_above() {
    let t = Tmp::new("collect-moved");
    let (_, hash) = project_with_audio(&t);
    let above = t.path("moved");
    let holder = Bundle::create(&above).unwrap();
    let wanted = BTreeSet::from([hash.clone()]);
    holder.collect_from(&[t.path("proj")], &wanted).unwrap();
    let file = above.join("sub").join("song.atomik");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::copy(t.path("proj").join("proj.atomik"), &file).unwrap();

    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    assert_eq!(
        c.missing_assets().len(),
        1,
        "the project opens without its audio"
    );

    c.collect_files().unwrap();
    c.wait_background();

    let own = above.join("sub").join("song");
    assert!(wav_of(&own, &hash).is_file());
    assert!(
        c.missing_assets().is_empty(),
        "the audio loads from the new bundle"
    );
    assert!(c.song_assets().contains_key(&hash));
    assert!(
        c.last_rejection()
            .unwrap()
            .starts_with("Collected 1 audio file")
    );
}

#[test]
fn collecting_files_in_a_project_that_has_them_all_changes_no_audio() {
    let t = Tmp::new("collect-nothing");
    let (mut c, hash) = project_with_audio(&t);
    let root = t.path("proj");
    let before = std::fs::read(wav_of(&root, &hash)).unwrap();
    c.collect_files().unwrap();
    c.wait_background();
    assert_eq!(c.bundle_root(), Some(root.as_path()));
    assert!(std::fs::read(wav_of(&root, &hash)).unwrap() == before);
    assert_eq!(
        c.last_rejection(),
        Some("All 1 audio file is already in proj")
    );
}

#[test]
fn collecting_files_names_the_audio_it_cannot_find() {
    let t = Tmp::new("collect-lost");
    let (_, _) = project_with_audio(&t);
    let file = t.path("away").join("song.atomik");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::copy(t.path("proj").join("proj.atomik"), &file).unwrap();
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    c.collect_files().unwrap();
    c.wait_background();
    assert_eq!(
        c.last_rejection(),
        Some("Nothing was collected into song; 1 not found: one.wav")
    );
    assert_eq!(
        c.missing_assets().len(),
        1,
        "it stays missing and relinkable"
    );
}

#[test]
fn collecting_files_needs_a_project_file_and_some_audio() {
    let t = Tmp::new("collect-refused");
    let mut c = core();
    import_ramp(&mut c, &t, "one.wav", 480);
    assert!(matches!(c.collect_files(), Err(AppError::NoPath)));
    let mut plain = core();
    plain.save_as(&t.path("plain.atomik")).unwrap();
    plain.collect_files().unwrap();
    assert_eq!(
        plain.last_rejection(),
        Some("The project uses no audio files")
    );
    assert!(!t.path("plain").exists(), "no bundle is made for nothing");
}

#[test]
fn save_as_copies_only_the_audio_the_project_uses() {
    let t = Tmp::new("saveas-used");
    let (_, hash) = project_with_audio(&t);
    let shared = t.path("proj");
    let other = Bundle::open(&shared)
        .unwrap()
        .add_recording(
            &AudioData::new(48_000, vec![vec![0.25; 900]]).unwrap(),
            "other",
        )
        .unwrap();
    let file = shared.join("song.atomik");
    std::fs::copy(shared.join("proj.atomik"), &file).unwrap();
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    let out = t.path("out");
    std::fs::create_dir_all(&out).unwrap();
    c.save_as(&out.join("copy.atomik")).unwrap();
    let root = out.join("copy");
    assert!(wav_of(&root, &hash).is_file());
    assert!(!wav_of(&root, &other.hash).exists());
    assert!(
        wav_of(&shared, &other.hash).exists(),
        "the source keeps its files"
    );
}

#[test]
fn a_background_collect_runs_on_the_worker_and_reports_when_it_is_applied() {
    let t = Tmp::new("collect-bg");
    let (_, hash) = project_with_audio(&t);
    let shared = t.path("proj");
    let file = shared.join("song.atomik");
    std::fs::copy(shared.join("proj.atomik"), &file).unwrap();
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    c.set_background_save(true);
    c.collect_files().unwrap();
    c.settle_save();
    assert!(wav_of(&shared.join("song"), &hash).is_file());
    assert!(
        c.last_rejection()
            .unwrap()
            .starts_with("Collected 1 audio file")
    );
}

/// Two project files in `shared` use one `Audio/` folder; each holds one asset of its own.
/// Returns the core that opened `song.atomik` and the two assets (song's, other's).
fn two_projects_sharing_audio(t: &Tmp) -> (AppCore<RecordingSink>, PathBuf, String, String) {
    let (_, mine) = project_with_audio(t);
    let shared = t.path("proj");
    let theirs = Bundle::open(&shared)
        .unwrap()
        .add_recording(
            &AudioData::new(48_000, vec![vec![0.25; 900]]).unwrap(),
            "other",
        )
        .unwrap()
        .hash;
    // The other project's file names the audio it uses: its own and the one it shares.
    std::fs::write(
        shared.join("other.atomik"),
        format!("{{\"assets\": [\"{mine}\", \"{theirs}\"]}}"),
    )
    .unwrap();
    let song = shared.join("song.atomik");
    std::fs::copy(shared.join("proj.atomik"), &song).unwrap();
    let mut c = core();
    c.open(&song).unwrap();
    c.wait_background();
    assert_eq!(c.bundle_root(), Some(shared.as_path()));
    (c, shared, mine, theirs)
}

#[test]
fn a_plain_save_keeps_the_audio_of_other_projects_in_a_shared_audio_folder() {
    let t = Tmp::new("shared-save");
    let (mut c, shared, mine, theirs) = two_projects_sharing_audio(&t);
    edit(&mut c, 7.0);
    c.save().unwrap();
    c.wait_background();
    assert!(!c.is_dirty());
    assert!(wav_of(&shared, &mine).exists());
    assert!(
        wav_of(&shared, &theirs).exists(),
        "audio the open project does not use is another project's"
    );
}

#[test]
fn a_background_save_keeps_the_audio_of_other_projects_in_a_shared_audio_folder() {
    let t = Tmp::new("shared-bg");
    let (mut c, shared, _, theirs) = two_projects_sharing_audio(&t);
    edit(&mut c, 3.0);
    c.save_in_background().unwrap();
    c.settle_save();
    assert!(wav_of(&shared, &theirs).exists());
}

#[test]
fn a_project_alone_in_its_bundle_still_drops_audio_it_no_longer_uses() {
    let t = Tmp::new("alone-prune");
    let (mut c, hash) = project_with_audio(&t);
    let root = t.path("proj");
    let stray = Bundle::open(&root)
        .unwrap()
        .add_recording(
            &AudioData::new(48_000, vec![vec![0.5; 700]]).unwrap(),
            "stray",
        )
        .unwrap()
        .hash;
    assert!(wav_of(&root, &stray).exists());
    edit(&mut c, 5.0);
    c.save().unwrap();
    c.wait_background();
    assert!(wav_of(&root, &hash).exists());
    assert!(!wav_of(&root, &stray).exists(), "unused audio is tidied");
}

// ---------------------------------------------------------------- collect preview, cancel, move

/// A song of the shared folder `<tmp>/proj/` (next to `proj.atomik`), opened, with its audio still
/// in the shared folder.
fn shared_song(t: &Tmp) -> (AppCore<RecordingSink>, PathBuf, String) {
    let (_, hash) = project_with_audio(t);
    let shared = t.path("proj");
    let file = shared.join("song.atomik");
    std::fs::copy(shared.join("proj.atomik"), &file).unwrap();
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    (c, file, hash)
}

#[test]
fn the_collect_preview_says_what_would_be_copied_and_writes_nothing() {
    let t = Tmp::new("collect-preview");
    let (c, _, hash) = shared_song(&t);
    let before = std::fs::read_dir(t.path("proj")).unwrap().count();

    let preview = c.collect_preview().unwrap();
    assert_eq!(preview.folder, t.path("proj").join("song"));
    assert_eq!(
        preview
            .plan
            .to_copy
            .iter()
            .map(|(h, _)| h.clone())
            .collect::<Vec<_>>(),
        vec![hash.clone()]
    );
    assert!(preview.plan.bytes() > 0 && preview.plan.missing.is_empty());
    let text = c.collect_preview_message().unwrap();
    assert!(
        text.starts_with("Collect Files would copy 1 audio file ("),
        "{text}"
    );
    assert!(text.contains("into song"), "{text}");
    assert!(
        !t.path("proj").join("song").exists(),
        "a preview writes nothing"
    );
    assert_eq!(std::fs::read_dir(t.path("proj")).unwrap().count(), before);

    // After the collection there is nothing left to copy.
    let mut c = c;
    c.collect_files().unwrap();
    c.wait_background();
    let text = c.collect_preview_message().unwrap();
    assert!(
        text.starts_with("Collect Files would copy nothing"),
        "{text}"
    );
    assert!(c.collect_preview().unwrap().plan.to_copy.is_empty());
}

#[test]
fn the_collect_preview_names_audio_no_folder_has_and_needs_a_project() {
    let t = Tmp::new("collect-preview-missing");
    assert!(matches!(
        core().collect_preview().unwrap_err(),
        AppError::NoPath
    ));
    let (c, _, hash) = shared_song(&t);
    std::fs::remove_file(wav_of(&t.path("proj"), &hash)).unwrap();
    let preview = c.collect_preview().unwrap();
    assert!(preview.plan.to_copy.is_empty());
    assert_eq!(preview.plan.missing, vec![hash]);
    let text = c.collect_preview_message().unwrap();
    assert!(text.contains("not found"), "{text}");
    // No audio at all.
    let mut bare = core();
    bare.save_as(&t.path("bare.atomik")).unwrap();
    assert_eq!(
        bare.collect_preview_message().unwrap(),
        "The project uses no audio files"
    );
}

#[test]
fn a_cancelled_collection_writes_nothing_and_leaves_the_project_alone() {
    let t = Tmp::new("collect-cancel");
    let (mut c, file, hash) = shared_song(&t);
    let text_before = std::fs::read_to_string(&file).unwrap();
    let root = collect_root(&file);
    let job = c.prepare_save(&file, Some(root.clone()), true);
    job.progress.cancel();
    c.launch(job, false, true).unwrap();
    assert!(c.saving() && c.collect_busy());
    c.settle_save();
    assert!(!c.saving() && !c.collect_busy());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text_before);
    assert!(
        !wav_of(&root, &hash).exists(),
        "no audio was copied into the new folder"
    );
    assert!(wav_of(&t.path("proj"), &hash).exists());
    assert_eq!(c.bundle_root(), Some(t.path("proj").as_path()));
    let message = c.last_rejection().unwrap();
    assert!(
        message.starts_with("Collect Files cancelled after 0 of 1"),
        "{message}"
    );
    assert!(message.contains("nothing was removed"), "{message}");

    // Doing it again, uncancelled, finishes the job.
    c.collect_files().unwrap();
    c.wait_background();
    assert!(wav_of(&root, &hash).exists());
}

#[test]
fn cancelling_with_no_save_running_does_nothing() {
    let mut c = core();
    assert!(!c.cancel_collect());
    assert!(!c.collect_busy());
    assert!(c.save_progress().is_none());
}

#[test]
fn a_running_collection_shows_its_progress_and_can_be_cancelled() {
    let t = Tmp::new("collect-progress");
    let (mut c, file, _) = shared_song(&t);
    let job = c.prepare_save(&file, Some(collect_root(&file)), true);
    c.launch(job, false, true).unwrap();
    // The status line names the work while it runs (or the save already ended).
    if let Some(text) = c.save_progress() {
        assert!(
            text == "Saving..." || text.starts_with("Collecting audio"),
            "{text}"
        );
    }
    assert!(c.cancel_collect() || !c.saving());
    c.settle_save();
    assert!(!c.saving());
}

fn recent_core(t: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core();
    c.set_recent_path(t.path("recent.json"));
    c
}

#[test]
fn move_project_copies_the_project_then_removes_the_old_folder() {
    let t = Tmp::new("move-ok");
    let mut c = recent_core(&t);
    let hash = import_ramp(&mut c, &t, "one.wav", 4_800);
    let old = t.path("old");
    std::fs::create_dir_all(&old).unwrap();
    c.save_as(&old.join("song.atomik")).unwrap();
    let old_file = c.file().unwrap().to_path_buf();
    let old_root = c.bundle_root().unwrap().to_path_buf();
    assert!(old_file.is_file() && wav_of(&old_root, &hash).is_file());
    let original = std::fs::read(wav_of(&old_root, &hash)).unwrap();

    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();

    let new_root = t.path("new").join("song");
    assert_eq!(c.file(), Some(new_root.join("song.atomik").as_path()));
    assert_eq!(c.bundle_root(), Some(new_root.as_path()));
    assert!(!c.is_dirty());
    assert_eq!(std::fs::read(wav_of(&new_root, &hash)).unwrap(), original);
    assert!(!old_root.exists(), "the old project folder is gone");
    assert!(!old_file.exists());
    let message = c.last_rejection().unwrap();
    assert!(message.starts_with("Moved to "), "{message}");
    assert!(
        message.contains("old project folder was removed"),
        "{message}"
    );
    assert_eq!(c.recent().first(), Some(&new_root.join("song.atomik")));
    assert!(!c.recent().contains(&old_file));
    // The moved project opens on its own.
    let mut again = core();
    again.open(&new_root.join("song.atomik")).unwrap();
    again.wait_background();
    assert!(!again.doc().assets.is_empty());
    assert!(
        again
            .doc()
            .assets
            .keys()
            .all(|h| wav_of(&new_root, h).is_file())
    );
}

#[test]
fn move_project_in_the_background_does_the_same_on_the_worker() {
    let t = Tmp::new("move-bg");
    let mut c = recent_core(&t);
    let hash = import_ramp(&mut c, &t, "one.wav", 4_800);
    std::fs::create_dir_all(t.path("old")).unwrap();
    c.save_as(&t.path("old").join("song.atomik")).unwrap();
    let old_root = c.bundle_root().unwrap().to_path_buf();
    c.set_background_save(true);
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.settle_save();
    c.wait_background();
    assert!(!old_root.exists());
    assert!(wav_of(&t.path("new").join("song"), &hash).is_file());
    assert!(c.last_rejection().unwrap().starts_with("Moved to "));
}

#[test]
fn move_project_never_removes_audio_another_project_in_the_folder_uses() {
    let t = Tmp::new("move-shared");
    let (mut c, shared, mine, theirs) = two_projects_sharing_audio(&t);
    let other_text = std::fs::read_to_string(shared.join("other.atomik")).unwrap();
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();

    assert!(
        !shared.join("song.atomik").exists(),
        "the old project file goes"
    );
    assert_eq!(
        std::fs::read_to_string(shared.join("other.atomik")).unwrap(),
        other_text
    );
    assert!(wav_of(&shared, &theirs).exists(), "their audio stays");
    assert!(
        wav_of(&shared, &mine).exists(),
        "shared audio stays where it is"
    );
    assert!(wav_of(&t.path("new").join("song"), &mine).is_file());
    let message = c.last_rejection().unwrap();
    assert!(message.contains("another project uses stay"), "{message}");
}

#[test]
fn move_project_removes_the_old_audio_no_other_project_uses() {
    let t = Tmp::new("move-split");
    let (mut c, shared, mine, theirs) = two_projects_sharing_audio(&t);
    // The first project file of the folder (a copy of the song) is not wanted any more, and the
    // other project no longer names the audio of the song: only its own.
    std::fs::remove_file(shared.join("proj.atomik")).unwrap();
    std::fs::write(
        shared.join("other.atomik"),
        format!("{{\"assets\": [\"{theirs}\"]}}"),
    )
    .unwrap();
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();
    assert!(!shared.join("song.atomik").exists());
    assert!(!wav_of(&shared, &mine).exists(), "nobody else uses it");
    assert!(wav_of(&shared, &theirs).exists(), "their audio stays");
    assert!(wav_of(&t.path("new").join("song"), &mine).is_file());
    let message = c.last_rejection().unwrap();
    assert!(message.contains("no other project uses"), "{message}");
}

#[test]
fn move_project_removes_no_audio_when_a_project_in_the_folder_cannot_be_read() {
    let t = Tmp::new("move-unreadable");
    let (mut c, shared, mine, theirs) = two_projects_sharing_audio(&t);
    std::fs::write(shared.join("other.atomik"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();
    assert!(!shared.join("song.atomik").exists());
    assert!(wav_of(&shared, &mine).exists());
    assert!(wav_of(&shared, &theirs).exists());
    assert!(wav_of(&t.path("new").join("song"), &mine).is_file());
    let message = c.last_rejection().unwrap();
    assert!(message.contains("cannot be read"), "{message}");
}

#[test]
fn move_project_never_touches_the_factory_folder() {
    let t = Tmp::new("move-factory");
    let (mut c, shared, mine, _) = two_projects_sharing_audio(&t);
    // The folder the song lives in is the factory folder.
    c.set_factory_root(Some(shared.clone()));
    assert!(c.in_factory(&shared.join("song.atomik")));
    let before: Vec<_> = std::fs::read_dir(&shared)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();
    assert!(shared.join("song.atomik").is_file());
    assert!(wav_of(&shared, &mine).is_file());
    assert!(!t.path("new").join("song").exists(), "nothing was written");
    let after: Vec<_> = std::fs::read_dir(&shared)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before.len(), after.len());
    let message = c.last_rejection().unwrap();
    assert!(message.contains("factory"), "{message}");
}

#[test]
fn move_project_keeps_a_folder_that_holds_other_files() {
    let t = Tmp::new("move-leftovers");
    let mut c = recent_core(&t);
    import_ramp(&mut c, &t, "one.wav", 4_800);
    std::fs::create_dir_all(t.path("old")).unwrap();
    c.save_as(&t.path("old").join("song.atomik")).unwrap();
    let old_root = c.bundle_root().unwrap().to_path_buf();
    std::fs::write(old_root.join("notes.txt"), "mine").unwrap();
    std::fs::create_dir_all(t.path("new")).unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();
    assert_eq!(
        std::fs::read_to_string(old_root.join("notes.txt")).unwrap(),
        "mine"
    );
    assert!(!old_root.join("song.atomik").exists());
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("other files in the old folder stay"),
        "{message}"
    );
}

#[test]
fn move_project_into_a_folder_that_has_that_project_moves_nothing() {
    let t = Tmp::new("move-taken");
    let (mut c, file, hash) = shared_song(&t);
    std::fs::create_dir_all(t.path("new").join("song")).unwrap();
    std::fs::write(t.path("new").join("song").join("keep.txt"), "x").unwrap();
    c.move_project(&t.path("new")).unwrap();
    c.wait_background();
    assert_eq!(c.file(), Some(file.as_path()), "the project did not move");
    assert!(file.is_file() && wav_of(&t.path("proj"), &hash).is_file());
    assert!(t.path("new").join("song").join("keep.txt").is_file());
    assert!(!t.path("new").join("song").join("song.atomik").exists());
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("already exists in that folder"),
        "{message}"
    );
}

#[test]
fn move_project_needs_a_saved_project() {
    let t = Tmp::new("move-nopath");
    let mut c = core();
    assert!(matches!(
        c.move_project(&t.path("new")).unwrap_err(),
        AppError::NoPath
    ));
}

#[test]
fn a_cancelled_move_removes_nothing() {
    let t = Tmp::new("move-cancel");
    let mut c = recent_core(&t);
    let hash = import_ramp(&mut c, &t, "one.wav", 4_800);
    std::fs::create_dir_all(t.path("old")).unwrap();
    c.save_as(&t.path("old").join("song.atomik")).unwrap();
    let old_file = c.file().unwrap().to_path_buf();
    let old_root = c.bundle_root().unwrap().to_path_buf();
    let new_root = t.path("new").join("song");
    let mut job = c.prepare_save(&new_root.join("song.atomik"), Some(new_root.clone()), false);
    job.relocate = Some(Relocate {
        old_file: old_file.clone(),
        old_root: Some(old_root.clone()),
    });
    job.progress.cancel();
    c.launch(job, true, false).unwrap();
    c.settle_save();
    assert!(old_file.is_file() && wav_of(&old_root, &hash).is_file());
    assert_eq!(c.file(), Some(old_file.as_path()));
    assert!(!new_root.join("song.atomik").exists());
    let message = c.last_rejection().unwrap();
    assert!(message.starts_with("Move Project cancelled"), "{message}");
    assert!(c.recent().contains(&old_file));
}

// ---------------------------------------------------------------- files plugin states name

mod plugin_files {
    use super::*;
    use atomik_device_sdk::{
        Channels, PluginFormat, PluginKindHint, PluginLayout, PluginRef, PluginStateBlob,
        descriptor_for,
    };
    use atomik_model::{DeviceKey, PluginCommand};

    fn layout() -> PluginLayout {
        PluginLayout {
            name: "Test Fx".into(),
            vendor: "Atomik Test".into(),
            version: "1.0.0".into(),
            kind_hint: PluginKindHint::Effect,
            main_in: Some(Channels::Stereo),
            main_out: Some(Channels::Stereo),
            sidechain: false,
            notes_in: false,
            notes_out: false,
            aux_out: 0,
            params: Vec::new(),
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A song with one plugin per state (keys `fx0`, `fx1`...), each holding that state.
    fn song_with_plugins(states: &[String]) -> AppCore<RecordingSink> {
        let mut c = core();
        for (i, state) in states.iter().enumerate() {
            let plugin = PluginRef {
                format: PluginFormat::Clap,
                id: format!("test.fx{i}"),
            };
            let desc = descriptor_for(&plugin, &layout()).unwrap();
            c.registry.register_plugin(plugin.type_id(), desc).unwrap();
            let key = DeviceKey(format!("fx{i}"));
            c.apply_command(Command::Plugin(PluginCommand::Add {
                index: usize::MAX,
                container: None,
                plugin,
                layout: layout(),
                path_hint: None,
                key: Some(key.clone()),
                auto_split: false,
            }))
            .unwrap();
            c.apply_command(Command::Plugin(PluginCommand::SetState {
                device: key,
                state: PluginStateBlob(state.as_bytes().to_vec()),
                merge: None,
            }))
            .unwrap();
        }
        c
    }

    fn state_of(c: &AppCore<RecordingSink>, i: usize) -> String {
        let key = DeviceKey(format!("fx{i}"));
        let (_, d) = c
            .doc()
            .plugin_devices()
            .into_iter()
            .find(|(k, _)| *k == key)
            .unwrap();
        String::from_utf8(d.plugin.as_ref().unwrap().state.as_ref().unwrap().0.clone()).unwrap()
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// The song saved as `<tmp>/<place>/song.atomik` while its plugins hold no file names, then
    /// given `states` (so that nothing was collected by the save itself).
    fn saved_in(t: &Tmp, place: &str, states: &[String]) -> AppCore<RecordingSink> {
        let blanks: Vec<String> = states.iter().map(|_| "<none/>".to_owned()).collect();
        let mut c = song_with_plugins(&blanks);
        std::fs::create_dir_all(t.path(place)).unwrap();
        c.save_as(&t.path(place).join("song.atomik")).unwrap();
        for (i, state) in states.iter().enumerate() {
            c.apply_command(Command::Plugin(PluginCommand::SetState {
                device: DeviceKey(format!("fx{i}")),
                state: PluginStateBlob(state.as_bytes().to_vec()),
                merge: None,
            }))
            .unwrap();
        }
        c
    }

    #[test]
    fn collect_files_copies_what_plugin_states_name_in_one_undo_step() {
        let t = Tmp::new("plugin-files");
        let (hall, pad) = (t.path("outside/hall.wav"), t.path("elsewhere/pad.fxp"));
        write(&hall, b"impulse");
        write(&pad, b"preset");
        let states = [
            format!("<ir file=\"{}\"/>", text(&hall)),
            format!("<a>{}</a><b>{}</b>", text(&hall), text(&pad)),
        ];
        let mut c = saved_in(&t, "proj", &states);

        c.collect_files().unwrap();
        c.wait_background();

        let folder = t.path("proj").join("song").join("Files");
        assert_eq!(std::fs::read(folder.join("hall.wav")).unwrap(), b"impulse");
        assert_eq!(std::fs::read(folder.join("pad.fxp")).unwrap(), b"preset");
        assert_eq!(
            state_of(&c, 0),
            format!("<ir file=\"{}\"/>", text(&folder.join("hall.wav")))
        );
        assert_eq!(
            state_of(&c, 1),
            format!(
                "<a>{}</a><b>{}</b>",
                text(&folder.join("hall.wav")),
                text(&folder.join("pad.fxp"))
            )
        );
        // The originals are untouched, and the saved project names no outside file.
        assert_eq!(std::fs::read(&hall).unwrap(), b"impulse");
        let message = c.last_rejection().unwrap();
        assert!(message.contains("plugin files: 2 copied"), "{message}");
        let saved = std::fs::read_to_string(c.file().unwrap()).unwrap();
        assert!(!saved.contains("outside") && !saved.contains("elsewhere"));
        assert!(!c.is_dirty());
        // One undo step puts both plugins back.
        assert!(c.undo());
        assert!(state_of(&c, 0).contains("outside"));
        assert!(state_of(&c, 1).contains("elsewhere"));
    }

    #[test]
    fn files_that_cannot_be_found_are_listed_and_never_block_the_rest() {
        let t = Tmp::new("plugin-files-missing");
        let (hall, gone) = (t.path("outside/hall.wav"), t.path("outside/gone.wav"));
        write(&hall, b"impulse");
        let states = [format!("<a>{}</a><b>{}</b>", text(&hall), text(&gone))];
        let mut c = saved_in(&t, "proj", &states);

        c.collect_files().unwrap();
        c.wait_background();

        let folder = t.path("proj").join("song").join("Files");
        assert!(folder.join("hall.wav").is_file());
        assert!(!folder.join("gone.wav").exists());
        // The missing path stays as it was; the found one points at its copy.
        let state = state_of(&c, 0);
        assert!(state.contains(&text(&gone)), "{state}");
        assert!(state.contains(&text(&folder.join("hall.wav"))), "{state}");
        let message = c.last_rejection().unwrap();
        assert!(message.contains("1 not found: gone.wav"), "{message}");
        assert!(!c.is_dirty());
    }

    #[test]
    fn the_preview_says_what_would_be_copied_and_writes_nothing() {
        let t = Tmp::new("plugin-files-preview");
        let (hall, gone) = (t.path("outside/hall.wav"), t.path("outside/gone.wav"));
        write(&hall, b"impulse");
        let states = [format!("<a>{}</a><b>{}</b>", text(&hall), text(&gone))];
        let c = saved_in(&t, "proj", &states);
        let before = state_of(&c, 0);
        let message = c.collect_preview_message().unwrap();
        assert!(message.contains("copy 1 plugin file"), "{message}");
        assert!(message.contains("not found: gone.wav"), "{message}");
        assert_eq!(state_of(&c, 0), before);
        assert!(!t.path("proj").join("song").join("Files").exists());
    }

    #[test]
    fn save_as_to_a_new_place_brings_the_files_along() {
        let t = Tmp::new("plugin-files-save-as");
        let hall = t.path("outside/hall.wav");
        write(&hall, b"impulse");
        let states = [format!("<ir file=\"{}\"/>", text(&hall))];
        let mut c = song_with_plugins(&states);
        c.save_as(&t.path("first/song.atomik")).unwrap();
        let first = state_of(&c, 0);
        assert!(
            first.contains("first") && !first.contains("outside"),
            "{first}"
        );
        c.save_as(&t.path("second/song.atomik")).unwrap();
        let second = state_of(&c, 0);
        assert!(second.contains("second"), "{second}");
        let copy = second
            .trim_start_matches("<ir file=\"")
            .trim_end_matches("\"/>");
        assert_eq!(std::fs::read(copy).unwrap(), b"impulse");
        // The old place and the original stay as they were.
        assert_eq!(std::fs::read(&hall).unwrap(), b"impulse");
    }

    #[test]
    fn a_plain_save_leaves_the_files_where_they_are() {
        let t = Tmp::new("plugin-files-save");
        let hall = t.path("outside/hall.wav");
        write(&hall, b"impulse");
        let states = [format!("<ir file=\"{}\"/>", text(&hall))];
        let mut c = saved_in(&t, "proj", &states);
        let before = state_of(&c, 0);
        c.apply_command(Command::SetMasterTune(3.0)).unwrap();
        c.save().unwrap();
        assert_eq!(state_of(&c, 0), before, "Save does not collect");
    }

    #[test]
    fn nothing_is_collected_into_the_factory_folder() {
        let t = Tmp::new("plugin-files-factory");
        let hall = t.path("outside/hall.wav");
        write(&hall, b"impulse");
        let states = [format!("<ir file=\"{}\"/>", text(&hall))];
        let mut c = saved_in(&t, "proj", &states);
        c.set_factory_root(Some(t.path("proj")));
        let before = state_of(&c, 0);
        let listing = |p: &Path| {
            let mut v: Vec<_> = std::fs::read_dir(p)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            v.sort();
            v
        };
        let names = listing(&t.path("proj"));
        c.collect_files().unwrap();
        c.wait_background();
        assert_eq!(state_of(&c, 0), before);
        assert_eq!(listing(&t.path("proj")), names, "nothing was written");
        let message = c.last_rejection().unwrap();
        assert!(message.contains("factory"), "{message}");
    }

    #[test]
    fn move_project_brings_the_files_to_the_new_place_and_says_so() {
        let t = Tmp::new("plugin-files-move");
        let hall = t.path("outside/hall.wav");
        write(&hall, b"impulse");
        let states = [format!("<ir file=\"{}\"/>", text(&hall))];
        let mut c = saved_in(&t, "old", &states);
        std::fs::create_dir_all(t.path("new")).unwrap();
        c.move_project(&t.path("new")).unwrap();
        c.wait_background();
        let copy = t.path("new").join("song").join("Files").join("hall.wav");
        assert_eq!(std::fs::read(&copy).unwrap(), b"impulse");
        assert!(state_of(&c, 0).contains(&text(&copy)));
        assert!(!t.path("old").join("song.atomik").exists());
        let message = c.last_rejection().unwrap();
        assert!(message.starts_with("Moved to "), "{message}");
        assert!(message.contains("plugin files: 1 copied"), "{message}");
    }
}
