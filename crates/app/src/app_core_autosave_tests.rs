//! Autosave and crash recovery in the app core: snapshots of a changed project with no plugin,
//! the unclean-exit banner and the recovery of unfinished recordings.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{DeviceKey, empty_doc, save_project};
use atomik_ui::BannerKind;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-autosave-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn core_in(recovery: &Path) -> AppCore<RecordingSink> {
    let mut core =
        AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    core.configure_plugins(PluginConfig {
        paths: PluginPaths {
            recovery: Some(recovery.to_path_buf()),
            ..PluginPaths::default()
        },
        ..PluginConfig::default()
    });
    core
}

fn edit(core: &mut AppCore<RecordingSink>, key: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_string(),
        key: Some(DeviceKey(key.to_string())),
    })
    .unwrap();
}

fn snapshots(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".recover.atomik"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn autosave_writes_snapshot_for_plugin_free_dirty_project() {
    let dir = temp("auto");
    let mut core = core_in(&dir);
    // Clean: nothing to write, however long it waits.
    core.plugin_tick(0.0);
    core.plugin_tick(10_000.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 0);

    edit(&mut core, "a");
    core.plugin_tick(10_001.0);
    core.plugin_tick(10_000.0 + 299.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 0, "before the period");
    core.plugin_tick(10_000.0 + 301.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 1);

    // The snapshot is the state at that moment.
    let newest = atomik_plugin_host::RecoveryDir::new(&dir).newest().unwrap();
    let mut reopened = core_in(&temp("auto-reopen"));
    reopened.open_recovery(&newest).unwrap();
    assert_eq!(reopened.doc(), core.doc());

    // No change since: no new snapshot; a change: one more after the period.
    core.plugin_tick(20_000.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 1);
    edit(&mut core, "b");
    core.plugin_tick(30_000.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 2);
}

#[test]
fn autosave_off_writes_nothing() {
    let dir = temp("off");
    let mut core = core_in(&dir);
    core.set_autosave_minutes(0);
    edit(&mut core, "a");
    core.plugin_tick(0.0);
    core.plugin_tick(100_000.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 0);
}

#[test]
fn a_run_that_did_not_quit_cleanly_offers_the_snapshot() {
    let dir = temp("unclean");
    let mut first = core_in(&dir);
    edit(&mut first, "a");
    first.plugin_tick(0.0);
    first.plugin_tick(1_000.0);
    first.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 1);
    // Dropped without a clean exit: the marker stays behind, its lock is gone.
    drop(first);

    let mut second = core_in(&dir);
    let banner = second.take_plugin_banner_change();
    assert!(
        matches!(
            banner,
            Some(Some(BannerKind::UncleanExit { ref plugins, has_recovery: true })) if plugins.is_empty()
        ),
        "{banner:?}"
    );
    second.shutdown_plugins();

    // A clean quit leaves no marker: the next start is quiet.
    let mut third = core_in(&dir);
    assert_eq!(third.take_plugin_banner_change(), None);
    third.shutdown_plugins();
}

/// Writes a finished WAV named like an unfinished recording of a process that is gone.
fn orphan(audio: &Path, name: &str, frames: usize) {
    std::fs::create_dir_all(audio).unwrap();
    let data = atomik_audio_file::AudioData::new(48_000, vec![vec![0.5; frames]]).unwrap();
    atomik_audio_file::write_wav_f32(&audio.join(name), &data).unwrap();
}

#[test]
fn an_unfinished_recording_comes_back_as_a_recovered_track() {
    let base = temp("orphan");
    let project = base.join("song");
    let audio = project.join("Audio");
    orphan(&audio, "rec-4000000000-0-0.wav", 48_000);
    let file = project.join("song.atomik");
    std::fs::write(&file, save_project(&empty_doc())).unwrap();

    let mut core = core_in(&base.join("recovery"));
    core.open(&file).unwrap();
    core.wait_background();
    assert_eq!(core.doc().song.tracks.len(), 1);
    assert!(core.is_dirty());
    assert_eq!(core.doc().assets.values().next().unwrap().frames, 48_000);
    assert!(audio.join("rec-4000000000-0-0.wav").exists());

    // The same file is not taken twice in a run.
    assert_eq!(core.recover_orphan_recordings(), 0);

    // Undo removes the track (the clip, then the track); the file is still there.
    while core.undo() {}
    assert!(core.doc().song.tracks.is_empty());
    while core.redo() {}
    assert_eq!(core.doc().song.tracks.len(), 1);
    assert!(audio.join("rec-4000000000-0-0.wav").exists());

    // Saving puts the audio in the project and removes the file.
    core.save().unwrap();
    assert!(!audio.join("rec-4000000000-0-0.wav").exists());
}
