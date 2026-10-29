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
        writer.submit(empty_doc(), recovery.clone(), "p".into(), sequence, 3);
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
