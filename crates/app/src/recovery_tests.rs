//! The recovery plumbing: atomic writes, the single snapshot writer, the session marker and the
//! scan for orphaned recordings.

use super::*;
use atomik_model::empty_doc;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-recovery-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_torn_write_leaves_the_original_intact() {
    let d = dir("atomic");
    let path = d.join("settings.json");
    atomic_write(&path, b"{\"a\":1}").unwrap();
    // A crash between writing the temporary file and renaming it leaves this behind.
    std::fs::write(d.join("settings.json.tmp"), b"{\"a\":").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    // The next write replaces the leftover and the file.
    atomic_write(&path, b"{\"a\":2}").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
    assert!(!d.join("settings.json.tmp").exists());
}

#[test]
fn a_corrupt_settings_file_is_quarantined_and_reported() {
    let d = dir("corrupt");
    let path = d.join("settings.json");
    std::fs::write(&path, "{\"audio\": tru").unwrap();
    let (settings, error) = crate::settings::load(&path);
    assert_eq!(settings, crate::settings::AudioSettings::default());
    let message = error.expect("a message").to_string();
    assert!(message.contains("settings.json.bad"), "{message}");
    assert!(!path.exists());
    assert!(d.join("settings.json.bad").is_file());
    // A second start is quiet.
    assert!(crate::settings::load(&path).1.is_none());
}

#[test]
fn a_corrupt_window_state_is_quarantined_and_reported() {
    let d = dir("ui-corrupt");
    let path = d.join("ui-state.json");
    std::fs::write(&path, "not json").unwrap();
    let (state, note) = crate::ui_state::UiState::load_checked(&path);
    assert_eq!(state, crate::ui_state::UiState::default());
    assert!(note.unwrap().contains("ui-state.json.bad"));
    assert!(d.join("ui-state.json.bad").is_file());
}

#[test]
fn twenty_back_to_back_snapshots_keep_the_newest_three() {
    let d = dir("writer");
    let recovery = RecoveryDir::new(&d);
    let writer = RecoveryWriter::new().unwrap();
    for sequence in 1..=20 {
        writer.submit(empty_doc(), recovery.clone(), "p".into(), sequence, 3, None);
    }
    writer.wait();
    let mut names: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names[0].contains("00000000000000000018"));
    assert!(names[2].contains("00000000000000000020"));
    let text = std::fs::read_to_string(recovery.newest().unwrap()).unwrap();
    assert_eq!(text, save_project(&empty_doc()));
}

#[test]
fn a_marker_that_was_not_removed_reports_an_unclean_exit() {
    let d = dir("marker");
    let (first, unclean) = SessionMarker::start(&d);
    assert!(!unclean);
    let mut first = first.unwrap();
    // While the process lives, its marker is not taken for a dead one.
    let (second, unclean) = SessionMarker::start(&d);
    assert!(!unclean);
    assert!(session_alive(&d, std::process::id()));
    first.clean_exit();
    drop(second);
    // The second was dropped without a clean exit: the lock is gone, the file is not.
    let (third, unclean) = SessionMarker::start(&d);
    assert!(unclean);
    let mut third = third.unwrap();
    third.clean_exit();
    let (fourth, unclean) = SessionMarker::start(&d);
    assert!(!unclean);
    fourth.unwrap().clean_exit();
}

#[test]
fn recording_names_parse() {
    assert_eq!(parse_recording_name("rec-12-3-1.wav"), Some((12, 3, 1)));
    assert_eq!(parse_recording_name("rec-12-3.wav"), None);
    assert_eq!(parse_recording_name("rec-12-3-1-9.wav"), None);
    assert_eq!(parse_recording_name("0123.wav"), None);
    assert_eq!(parse_recording_name("rec-x-3-1.wav"), None);
}

#[test]
fn the_scan_finds_only_recordings_of_dead_sessions() {
    let root = dir("scan");
    let audio = root.join("Audio");
    std::fs::create_dir_all(&audio).unwrap();
    for name in [
        "rec-7-0-0.wav",
        "rec-7-0-1.wav",
        "rec-8-0-0.wav",
        "other.wav",
    ] {
        std::fs::write(audio.join(name), b"x").unwrap();
    }
    let found = scan_orphans(&[root], None, &|pid| pid == 8);
    let names: Vec<_> = found
        .iter()
        .map(|o| o.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["rec-7-0-0.wav", "rec-7-0-1.wav"]);
}

#[test]
fn a_temporary_file_of_a_killed_store_is_not_a_recording() {
    let root = dir("tmp-store");
    let audio = root.join("Audio");
    std::fs::create_dir_all(&audio).unwrap();
    // What a process killed between writing an asset and renaming it leaves: the asset's name
    // plus the temporary suffix. The scan reads `rec-<pid>-<take>-<n>.wav` only, so it is never
    // offered as a take, however many times the scan runs.
    for name in [
        format!("{}.wav.tmp~", "ab".repeat(32)),
        "rec-7-0-0.wav.tmp~".to_owned(),
        "rec-7-0-1.wav".to_owned(),
    ] {
        std::fs::write(audio.join(name), b"x").unwrap();
    }
    for _ in 0..2 {
        let found = scan_orphans(std::slice::from_ref(&root), None, &|_| false);
        let names: Vec<_> = found
            .iter()
            .map(|o| o.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["rec-7-0-1.wav"]);
    }
    // The bundle's orphan clean-up removes the leftovers and keeps the recording.
    let mut bundle = atomik_audio_file::Bundle::open(&root).unwrap();
    bundle.remove_orphans(&Default::default()).unwrap();
    assert!(audio.join("rec-7-0-1.wav").exists());
    assert!(!audio.join("rec-7-0-0.wav.tmp~").exists());
    assert!(!audio.join(format!("{}.wav.tmp~", "ab".repeat(32))).exists());
}

#[test]
fn the_scratch_scan_skips_live_sessions_and_young_files() {
    let temp = dir("scratch");
    let audio = |pid: u32| {
        temp.join(format!("{SCRATCH_PREFIX}{pid}-0"))
            .join("1")
            .join("Audio")
    };
    for pid in [41, 42] {
        std::fs::create_dir_all(audio(pid)).unwrap();
        std::fs::write(audio(pid).join(format!("rec-{pid}-0-0.wav")), b"x").unwrap();
    }
    let alive = |pid: u32| pid == 42;
    let found = scan_orphans(&[], Some((&temp, Duration::ZERO)), &alive);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].pid, 41);
    // Written a moment ago: not yet an orphan.
    let found = scan_orphans(&[], Some((&temp, Duration::from_secs(3600))), &alive);
    assert!(found.is_empty());
}

#[test]
fn a_truncated_recording_never_panics_the_reader() {
    let d = dir("fuzz");
    let data = atomik_audio_file::AudioData::new(48_000, vec![vec![0.25; 4800]; 2]).unwrap();
    let whole = d.join("whole.wav");
    atomik_audio_file::write_wav_f32(&whole, &data).unwrap();
    let bytes = std::fs::read(&whole).unwrap();
    let cut = d.join("cut.wav");
    for len in (0..bytes.len()).step_by(7).chain([bytes.len() - 1]) {
        std::fs::write(&cut, &bytes[..len]).unwrap();
        let _ = atomik_audio_file::decode_file(&cut);
    }
    // A header that lies about the data length (the file was cut short, not refreshed).
    let mut lying = bytes.clone();
    lying[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&cut, &lying).unwrap();
    let _ = atomik_audio_file::decode_file(&cut);
}

/// A song folder `song-<n>` under `root` with one snapshot; the marker of its session is left
/// behind (`crashed`) or removed by a clean exit.
fn song_folder(root: &Path, n: u32, crashed: bool) -> PathBuf {
    let folder = root.join(format!("song-{n}"));
    let (marker, _) = SessionMarker::start(&folder);
    let writer = RecoveryWriter::new().unwrap();
    writer.submit(
        empty_doc(),
        RecoveryDir::new(&folder),
        format!("s{n}"),
        1,
        3,
        None,
    );
    writer.wait();
    let mut marker = marker.unwrap();
    if !crashed {
        marker.clean_exit();
    }
    // Dropped either way: the lock is released, the file stays only for a crash.
    drop(marker);
    folder
}

#[test]
fn the_scan_offers_the_newest_snapshot_of_each_song_folder_that_was_cut_off() {
    let root = dir("songs");
    let two = song_folder(&root, 2, true);
    let _clean = song_folder(&root, 3, false);
    let four = song_folder(&root, 4, true);
    // Not a song folder: ignored.
    std::fs::create_dir_all(root.join("other")).unwrap();
    let found = crashed_song_snapshots(&root);
    assert_eq!(
        found,
        vec![
            RecoveryDir::new(&two).newest().unwrap(),
            RecoveryDir::new(&four).newest().unwrap()
        ]
    );
    // The dead markers are gone: the next start offers nothing, and a song that takes the
    // folder over starts without a crash of its own.
    assert!(crashed_song_snapshots(&root).is_empty());
    let (marker, unclean) = SessionMarker::start(&two);
    assert!(!unclean);
    marker.unwrap().clean_exit();
}

#[test]
fn a_song_folder_of_a_live_session_is_not_offered() {
    let root = dir("songs-live");
    let folder = root.join("song-2");
    let (marker, _) = SessionMarker::start(&folder);
    let mut marker = marker.unwrap();
    let writer = RecoveryWriter::new().unwrap();
    writer.submit(
        empty_doc(),
        RecoveryDir::new(&folder),
        "s".into(),
        1,
        3,
        None,
    );
    writer.wait();
    assert!(crashed_song_snapshots(&root).is_empty());
    marker.clean_exit();
}

#[test]
fn a_missing_recovery_folder_offers_nothing() {
    assert!(crashed_song_snapshots(&dir("songs-none").join("absent")).is_empty());
}

/// A writer whose sink blocks until the test releases it; `entered` says it got a snapshot.
fn blocked_writer(bound: Duration) -> (RecoveryWriter, Sender<()>, Receiver<()>) {
    let (release, released) = channel::<()>();
    let (entered_tx, entered) = channel::<()>();
    let released = std::sync::Mutex::new(released);
    let writer = RecoveryWriter::spawn(bound, move |_| {
        let _ = entered_tx.send(());
        let _ = released.lock().unwrap().recv();
    })
    .unwrap();
    (writer, release, entered)
}

/// Runs `f` on a thread and fails the test when it has not returned within a generous bound (a
/// regression to an unbounded wait would otherwise hang the whole run).
fn returns_within_a_generous_bound<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the call must return: its wait is bounded")
}

fn submit_one(writer: &RecoveryWriter, d: &Path) {
    writer.submit(empty_doc(), RecoveryDir::new(d), "s".into(), 1, 3, None);
}

#[test]
fn a_blocked_writer_does_not_hold_the_bounded_wait_forever() {
    let d = dir("blocked-wait");
    let (writer, release, entered) = blocked_writer(Duration::from_millis(50));
    submit_one(&writer, &d);
    entered
        .recv_timeout(Duration::from_secs(60))
        .expect("the sink got the snapshot");
    let (writer, done) = returns_within_a_generous_bound(move || {
        let done = writer.wait_for(Duration::from_millis(100));
        (writer, done)
    });
    assert!(!done, "the writer is still blocked: the wait says so");
    // The caller goes on; once the sink returns, the wait succeeds.
    release.send(()).unwrap();
    assert!(writer.wait_for(Duration::from_secs(60)));
}

#[test]
fn dropping_a_blocked_writer_returns_after_the_bound() {
    let d = dir("blocked-drop");
    let (writer, release, entered) = blocked_writer(Duration::from_millis(50));
    submit_one(&writer, &d);
    entered
        .recv_timeout(Duration::from_secs(60))
        .expect("the sink got the snapshot");
    returns_within_a_generous_bound(move || drop(writer));
    // Let the left-behind thread end.
    release.send(()).unwrap();
}

#[test]
fn an_idle_writer_ends_cleanly_and_writes_what_was_queued() {
    let d = dir("clean-drop");
    let writer = RecoveryWriter::new().unwrap();
    submit_one(&writer, &d);
    assert!(writer.wait_for(Duration::from_secs(60)));
    drop(writer);
    assert!(RecoveryDir::new(&d).newest().is_some());
}

#[test]
fn bounded_work_reports_whether_it_finished() {
    assert!(returns_within_a_generous_bound(|| run_bounded(
        Duration::from_secs(60),
        || {}
    )));
    let (release, released) = channel::<()>();
    let done = returns_within_a_generous_bound(move || {
        let done = run_bounded(Duration::from_millis(50), move || {
            let _ = released.recv();
        });
        let _ = release.send(());
        done
    });
    assert!(!done, "the work was still blocked at the bound");
}

#[test]
fn the_scratch_folder_is_removed_before_the_call_returns_when_the_disk_is_fine() {
    let d = dir("scratch-removal");
    std::fs::create_dir_all(d.join("a/b")).unwrap();
    std::fs::write(d.join("a/b/x.wav"), b"x").unwrap();
    assert!(remove_dir_bounded(&d, Duration::from_secs(60)));
    assert!(!d.exists());
}

/// A temp root with a scratch folder of process `pid` holding 10 bytes, and an empty recovery
/// folder. Returns `(temp, recovery, scratch)`.
fn sweep_rig(name: &str, pid: u32) -> (PathBuf, PathBuf, PathBuf) {
    let root = dir(name);
    let temp = root.join("temp");
    let recovery = root.join("recovery");
    let scratch = temp.join(format!("{SCRATCH_PREFIX}{pid}-0"));
    std::fs::create_dir_all(scratch.join("1").join("Audio")).unwrap();
    std::fs::create_dir_all(&recovery).unwrap();
    std::fs::write(scratch.join("1").join("Audio").join("a.wav"), b"0123456789").unwrap();
    (temp, recovery, scratch)
}

#[test]
fn the_sweep_removes_an_old_unreferenced_folder_of_a_dead_session() {
    let (temp, recovery, scratch) = sweep_rig("sweep-dead", 4_000_001);
    // Not a scratch folder, and the folder of a name that has no process: both are left alone.
    std::fs::create_dir_all(temp.join("something-else")).unwrap();
    std::fs::create_dir_all(temp.join(format!("{SCRATCH_PREFIX}x-0"))).unwrap();
    let report = sweep_scratch(&temp, &recovery, Duration::ZERO);
    assert_eq!(
        report,
        SweepReport {
            folders: 1,
            bytes: 10
        }
    );
    assert!(!scratch.exists());
    assert!(temp.join("something-else").is_dir());
    assert!(temp.join(format!("{SCRATCH_PREFIX}x-0")).is_dir());
}

#[test]
fn the_sweep_keeps_a_recent_folder() {
    let (temp, recovery, scratch) = sweep_rig("sweep-recent", 4_000_002);
    let report = sweep_scratch(&temp, &recovery, SWEEP_AGE);
    assert_eq!(report, SweepReport::default());
    assert!(scratch.is_dir());
}

#[test]
fn the_sweep_keeps_the_folder_of_a_live_session() {
    let (temp, recovery, scratch) = sweep_rig("sweep-live", 4_000_003);
    // Another session's marker, locked as a running process holds its own.
    let marker = recovery.join("session-4000003-0.lock");
    let held_open = File::create(&marker).unwrap();
    held_open.lock().unwrap();
    assert_eq!(
        sweep_scratch(&temp, &recovery, Duration::ZERO),
        SweepReport::default()
    );
    assert!(scratch.is_dir());
    // The process ends: the lock is released and the folder goes.
    drop(held_open);
    assert_eq!(sweep_scratch(&temp, &recovery, Duration::ZERO).folders, 1);
    assert!(!scratch.exists());
    // This process's own folder is never swept.
    let own = temp.join(format!("{SCRATCH_PREFIX}{}-9", std::process::id()));
    std::fs::create_dir_all(&own).unwrap();
    assert_eq!(sweep_scratch(&temp, &recovery, Duration::ZERO).folders, 0);
    assert!(own.is_dir());
}

#[test]
fn the_sweep_keeps_a_folder_a_recovery_note_points_at() {
    let (temp, recovery, scratch) = sweep_rig("sweep-noted", 4_000_004);
    let snapshot = recovery.join("p-00000000000000000001.recover.atomik");
    std::fs::write(&snapshot, b"x").unwrap();
    let note = RecoveryDir::bundle_note_path(&snapshot);
    std::fs::write(&note, scratch.join("1").to_string_lossy().as_bytes()).unwrap();
    assert_eq!(
        sweep_scratch(&temp, &recovery, Duration::ZERO),
        SweepReport::default()
    );
    assert!(scratch.is_dir());
}

#[test]
fn the_sweep_keeps_everything_it_cannot_decide() {
    let (temp, recovery, scratch) = sweep_rig("sweep-unknown", 4_000_005);
    // No recovery folder to read the markers from.
    let missing = recovery.join("nowhere");
    assert_eq!(
        sweep_scratch(&temp, &missing, Duration::ZERO),
        SweepReport::default()
    );
    // A note that cannot be read as text.
    let snapshot = recovery.join("p-00000000000000000001.recover.atomik");
    std::fs::write(&snapshot, b"x").unwrap();
    std::fs::create_dir(RecoveryDir::bundle_note_path(&snapshot)).unwrap();
    assert_eq!(
        sweep_scratch(&temp, &recovery, Duration::ZERO),
        SweepReport::default()
    );
    assert!(scratch.is_dir());
}
