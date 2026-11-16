//! The tonal balance meter through the core: the reading of the tap, the targets (built-in,
//! learned, saved, missing), the live learn, learning from files on the worker, and the saved
//! targets surviving a restart.

use super::tonal::{resolve_target, target_rows};
use super::*;
use crate::sink::RecordingSink;
use crate::tonal_targets::{self, Store};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_dsp::spectrum::band_centres;
use atomik_dsp::tonal::builtin;
use atomik_dsp::tonal::{SHAPES, shape_db};
use atomik_model::empty_doc;
use atomik_ui::tonal_panel::{TargetKind, TonalOp, TonalView};

const SR: f32 = 48_000.0;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(core: &mut AppCore<RecordingSink>, k: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.tonal-balance".into(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn set(core: &mut AppCore<RecordingSink>, k: &str, param: &str, value: f32) {
    core.apply_command(Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

fn param(core: &AppCore<RecordingSink>, k: &str, param: &str) -> f32 {
    let dev = core.doc().resolve(&key(k)).unwrap();
    let desc = core.registry.descriptor(&dev.type_id).unwrap();
    let p = desc.params.iter().find(|p| p.key == param).unwrap();
    atomik_ui::widget::current_value(dev, p)
}

/// `frames` frames of one sine at every band centre with the band levels `level_db(centre)`.
fn comb(level_db: impl Fn(f32) -> f32, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    for f in band_centres() {
        let bin = (f64::from(f) / f64::from(SR) * 16_384.0).round().max(1.0);
        let hz = bin * f64::from(SR) / 16_384.0;
        let amp = 10f32.powf(level_db(f) / 20.0) * 0.05;
        for (i, o) in out.iter_mut().enumerate() {
            *o += amp * (std::f64::consts::TAU * hz * i as f64 / f64::from(SR)).sin() as f32;
        }
    }
    out
}

fn feed(core: &mut AppCore<RecordingSink>, k: &str, signal: &[f32], head: u64) {
    let id = core.device_id(&key(k)).unwrap();
    core.sink_mut()
        .device_scopes
        .insert(id, (signal.to_vec(), signal.to_vec(), head));
    core.frame();
}

fn view(core: &AppCore<RecordingSink>, k: &str) -> TonalView {
    core.view()
        .analyser_data(&key(k))
        .expect("analyser data")
        .tonal
        .clone()
}

fn rig() -> AppCore<RecordingSink> {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "tb");
    core.frame();
    core
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-tonal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------ resolving

#[test]
fn a_value_names_a_built_in_the_learned_target_a_saved_one_or_falls_back_to_balanced() {
    let mut store = Store::default();
    let id = store.add("Mine", builtin(3)).unwrap();
    let pending = builtin(2);
    let (t, name, kind, missing) = resolve_target(&store, None, 1.0);
    assert_eq!(
        (name.as_str(), kind, missing),
        ("Bright", TargetKind::Builtin, false)
    );
    assert_eq!(t, builtin(1));
    let (t, name, kind, missing) = resolve_target(&store, Some(&pending), 4.0);
    assert_eq!(
        (name.as_str(), kind, missing),
        ("Learned", TargetKind::Learned, false)
    );
    assert_eq!(t, pending);
    let (t, name, kind, missing) = resolve_target(&store, None, 10.0 + id as f32);
    assert_eq!(
        (name.as_str(), kind, missing),
        ("Mine", TargetKind::User, false)
    );
    assert_eq!(t, store.get(id).unwrap().target);
    // Nothing learned, an id that is not saved, a value in the gap: Balanced, said to be missing.
    for value in [4.0, 17.0, 7.0, 500.0] {
        let (t, name, kind, missing) = resolve_target(&store, None, value);
        assert_eq!(name, "Balanced", "{value}");
        assert_eq!(kind, TargetKind::Builtin);
        assert!(missing, "{value}");
        assert_eq!(t, builtin(0));
    }
    let rows = target_rows(&store, true);
    let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Balanced",
            "Bright",
            "Warm",
            "Bass-heavy",
            "Learned",
            "Mine"
        ]
    );
    assert_eq!(rows[4].value, 4.0);
    assert_eq!(rows[5].value, 10.0 + id as f32);
    assert_eq!(target_rows(&store, false).len(), 5);
}

// ------------------------------------------------------------------ the reading

#[test]
fn a_mix_shaped_like_the_target_reads_centred_and_a_boosted_low_end_reads_too_loud() {
    let mut core = rig();
    let balanced = comb(|f| shape_db(&SHAPES[0], f), 16_384);
    feed(&mut core, "tb", &balanced, 16_384);
    let v = view(&core, "tb");
    assert!(v.ready);
    assert_eq!(v.target_name, "Balanced");
    assert!(!v.missing);
    assert_eq!(v.bands.len(), 31);
    assert_eq!(v.target_bands.len(), 31);
    for d in v.deviation {
        assert!(d.abs() < 1.0, "{:?}", v.deviation);
    }
    assert_eq!(v.tolerance, [3.0; 4]);
    // A level change alone moves nothing (a fresh device: the average starts from the signal).
    let mut quiet = rig();
    let scaled: Vec<f32> = balanced.iter().map(|s| s * 0.2).collect();
    feed(&mut quiet, "tb", &scaled, 16_384);
    let w = view(&quiet, "tb");
    for i in 0..4 {
        assert!((w.deviation[i] - v.deviation[i]).abs() < 0.05, "{i}");
    }
    // Too much bass.
    let mut boomy = rig();
    let loud = comb(
        |f| shape_db(&SHAPES[0], f) + if f < 150.0 { 8.0 } else { 0.0 },
        16_384,
    );
    feed(&mut boomy, "tb", &loud, 16_384);
    let b = view(&boomy, "tb");
    assert!(b.deviation[0] > 3.0, "{:?}", b.deviation);
    assert!(b.deviation[0] > b.tolerance[0]);
    for i in 1..4 {
        assert!(b.deviation[i].abs() < 1.0, "{:?}", b.deviation);
    }
}

#[test]
fn another_target_changes_the_reading_at_once_even_with_a_stopped_engine() {
    let mut core = rig();
    let warm = comb(|f| shape_db(&SHAPES[2], f), 16_384);
    feed(&mut core, "tb", &warm, 16_384);
    let against_balanced = view(&core, "tb");
    assert!(
        against_balanced.deviation[3] < -1.0,
        "{:?}",
        against_balanced.deviation
    );
    set(&mut core, "tb", "hidden.target", 2.0);
    core.frame();
    let v = view(&core, "tb");
    assert_eq!(v.target_name, "Warm");
    assert_eq!(v.selected, 2);
    for d in v.deviation {
        assert!(d.abs() < 1.0, "{:?}", v.deviation);
    }
}

#[test]
fn a_missing_target_falls_back_to_balanced_and_says_so() {
    let mut core = rig();
    set(&mut core, "tb", "hidden.target", 25.0);
    core.frame();
    let v = view(&core, "tb");
    assert!(v.missing);
    assert_eq!(v.target_name, "Balanced");
    assert_eq!(v.selected, 0);
    assert_eq!(v.targets.len(), 4);
    // The learned target of a session that is gone.
    set(&mut core, "tb", "hidden.target", 4.0);
    core.frame();
    assert!(view(&core, "tb").missing);
}

#[test]
fn silence_reads_nothing_and_a_tap_that_did_not_move_is_not_analysed_again() {
    let mut core = rig();
    feed(&mut core, "tb", &vec![0.0; 16_384], 16_384);
    let v = view(&core, "tb");
    assert!(!v.ready && v.bands.is_empty());
    let balanced = comb(|f| shape_db(&SHAPES[0], f), 16_384);
    feed(&mut core, "tb", &balanced, 32_768);
    assert!(view(&core, "tb").ready);
    // The same head with other audio: not read again.
    let boomy = comb(
        |f| shape_db(&SHAPES[0], f) + if f < 150.0 { 12.0 } else { 0.0 },
        16_384,
    );
    feed(&mut core, "tb", &boomy, 32_768);
    assert!(view(&core, "tb").deviation[0].abs() < 1.0);
}

#[test]
fn a_bypassed_folded_or_absent_panel_is_not_read() {
    let mut core = rig();
    let balanced = comb(|f| shape_db(&SHAPES[0], f), 16_384);
    set(&mut core, "tb", "bypass", 1.0);
    feed(&mut core, "tb", &balanced, 16_384);
    assert!(!view(&core, "tb").ready, "bypassed");
    set(&mut core, "tb", "bypass", 0.0);
    core.apply_command(Command::SetFolded {
        key: key("tb"),
        folded: true,
    })
    .unwrap();
    feed(&mut core, "tb", &balanced, 16_384);
    assert!(!view(&core, "tb").ready, "folded");
    core.apply_command(Command::SetFolded {
        key: key("tb"),
        folded: false,
    })
    .unwrap();
    feed(&mut core, "tb", &balanced, 16_384);
    assert!(view(&core, "tb").ready);
    // No tonal balance device: nothing happens.
    let mut empty = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    empty.frame();
}

#[test]
fn the_channel_choice_reads_the_mid_or_the_side() {
    let mut core = rig();
    let signal = comb(|f| shape_db(&SHAPES[0], f), 16_384);
    let id = core.device_id(&key("tb")).unwrap();
    // Left only: in the mid and in the side alike, 6 dB down from a mono signal in the mid.
    core.sink_mut().device_scopes.insert(
        id,
        (signal.clone(), signal.iter().map(|s| -s).collect(), 16_384),
    );
    set(&mut core, "tb", "channel", 2.0);
    core.frame();
    let side = view(&core, "tb");
    assert!(side.ready, "an out-of-phase signal is all side");
    let mut mid = rig();
    let id = mid.device_id(&key("tb")).unwrap();
    mid.sink_mut().device_scopes.insert(
        id,
        (signal.clone(), signal.iter().map(|s| -s).collect(), 16_384),
    );
    set(&mut mid, "tb", "channel", 1.0);
    mid.frame();
    assert!(!view(&mid, "tb").ready, "an out-of-phase signal has no mid");
}

// ------------------------------------------------------------------ learning and saving

#[test]
fn the_live_learn_takes_the_balance_of_what_plays_and_makes_the_learned_target() {
    let mut core = rig();
    let warm = comb(|f| shape_db(&SHAPES[2], f), 16_384);
    feed(&mut core, "tb", &warm, 16_384);
    set(&mut core, "tb", "learn", 1.0);
    core.frame();
    assert!(view(&core, "tb").learning);
    // Seven seconds of audio, half a second at a time.
    let mut head = 16_384u64;
    for _ in 0..14 {
        head += 24_000;
        feed(&mut core, "tb", &warm, head);
    }
    let v = view(&core, "tb");
    assert!(v.learning);
    assert!(
        (v.learned_seconds - 7.0).abs() < 0.6,
        "{}",
        v.learned_seconds
    );
    assert!(
        core.tonal_pending().is_none(),
        "nothing until the learn ends"
    );
    set(&mut core, "tb", "learn", 0.0);
    core.frame();
    let pending = core.tonal_pending().expect("a learned target");
    let want = builtin(2);
    for b in 3..31 {
        assert!((pending.bands[b] - want.bands[b]).abs() < 1.0, "band {b}");
    }
    assert_eq!(
        param(&core, "tb", "hidden.target"),
        4.0,
        "the device points at it"
    );
    core.frame();
    let v = view(&core, "tb");
    assert!(v.pending && !v.missing && !v.learning);
    assert_eq!(v.target_name, "Learned");
    assert_eq!(v.targets.len(), 5);
    for d in v.deviation {
        assert!(d.abs() < 1.5, "{:?}", v.deviation);
    }
    // A learn of silence leaves the learned target as it was and says so.
    set(&mut core, "tb", "learn", 1.0);
    core.frame();
    set(&mut core, "tb", "learn", 0.0);
    core.frame();
    assert!(core.tonal_pending().is_some());
}

#[test]
fn a_learned_target_is_saved_under_a_name_renamed_and_deleted_and_the_file_follows() {
    let dir = temp_dir("save");
    let file = dir.join(tonal_targets::TARGETS_FILE);
    let mut core = rig();
    core.set_tonal_path(Some(file.clone()));
    assert!(core.tonal_targets().targets.is_empty());
    // Nothing learned: nothing to save.
    core.tonal_action(&key("tb"), TonalOp::Save("Nope".into()));
    assert!(core.tonal_targets().targets.is_empty());
    assert!(!file.exists());
    // Learn from the live signal, then save.
    let signal = comb(|f| shape_db(&SHAPES[1], f), 16_384);
    feed(&mut core, "tb", &signal, 16_384);
    set(&mut core, "tb", "learn", 1.0);
    core.frame();
    for i in 1..8 {
        feed(&mut core, "tb", &signal, 16_384 + i * 48_000);
    }
    set(&mut core, "tb", "learn", 0.0);
    core.frame();
    assert!(core.tonal_pending().is_some());
    core.tonal_action(&key("tb"), TonalOp::Save("  Club master  ".into()));
    assert!(core.tonal_pending().is_none());
    let saved = core.tonal_targets().targets[0].clone();
    assert_eq!((saved.id, saved.name.as_str()), (1, "Club master"));
    assert_eq!(param(&core, "tb", "hidden.target"), 11.0);
    core.frame();
    let v = view(&core, "tb");
    assert_eq!(v.target_name, "Club master");
    assert_eq!(v.targets.last().unwrap().name, "Club master");
    assert_eq!(v.selected, v.targets.len() - 1);
    assert!(!v.missing && !v.pending);
    // The file holds it: another session sees it.
    let mut other = rig();
    other.set_tonal_path(Some(file.clone()));
    assert_eq!(other.tonal_targets().targets.len(), 1);
    assert_eq!(other.tonal_targets().targets[0].name, "Club master");
    // Rename.
    core.tonal_action(&key("tb"), TonalOp::Rename("Radio edit".into()));
    assert_eq!(core.tonal_targets().targets[0].name, "Radio edit");
    core.frame();
    assert_eq!(view(&core, "tb").target_name, "Radio edit");
    let mut again = rig();
    again.set_tonal_path(Some(file.clone()));
    assert_eq!(again.tonal_targets().targets[0].name, "Radio edit");
    // A blank name changes nothing.
    core.tonal_action(&key("tb"), TonalOp::Rename("   ".into()));
    assert_eq!(core.tonal_targets().targets[0].name, "Radio edit");
    // A device that still points at it falls back to Balanced with the note.
    assert!(!resolve_target(core.tonal_targets(), None, 11.0).3);
    core.tonal_action(&key("tb"), TonalOp::Delete);
    assert!(core.tonal_targets().targets.is_empty());
    assert_eq!(
        param(&core, "tb", "hidden.target"),
        0.0,
        "the device that deleted it"
    );
    core.frame();
    assert!(!view(&core, "tb").missing);
    assert!(resolve_target(core.tonal_targets(), None, 11.0).3);
    let mut last = rig();
    last.set_tonal_path(Some(file.clone()));
    assert!(last.tonal_targets().targets.is_empty());
    // A built-in cannot be renamed or deleted.
    core.tonal_action(&key("tb"), TonalOp::Delete);
    core.tonal_action(&key("tb"), TonalOp::Rename("x".into()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_targets_keep_their_names_unique_and_a_newer_file_is_left_alone() {
    let dir = temp_dir("newer");
    let file = dir.join(tonal_targets::TARGETS_FILE);
    std::fs::write(&file, "{\"version\":9,\"targets\":[]}").unwrap();
    let mut core = rig();
    core.set_tonal_path(Some(file.clone()));
    core.tonal.pending = Some(builtin(0));
    core.tonal_action(&key("tb"), TonalOp::Save("Mine".into()));
    assert_eq!(
        core.tonal_targets().targets.len(),
        1,
        "kept for this session"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\"version\":9,\"targets\":[]}",
        "the newer file is not overwritten"
    );
    // A second with the same name gets a number.
    core.tonal.pending = Some(builtin(1));
    core.tonal_action(&key("tb"), TonalOp::Save("mine".into()));
    let names: Vec<_> = core
        .tonal_targets()
        .targets
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(names, ["Mine", "mine 2"]);
    // A damaged file is ignored and replaced by the next save.
    std::fs::write(&file, "not json").unwrap();
    let mut damaged = rig();
    damaged.set_tonal_path(Some(file.clone()));
    assert!(damaged.tonal_targets().targets.is_empty());
    damaged.tonal.pending = Some(builtin(0));
    damaged.tonal_action(&key("tb"), TonalOp::Save("Fresh".into()));
    let mut back = rig();
    back.set_tonal_path(Some(file));
    assert_eq!(back.tonal_targets().targets[0].name, "Fresh");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn learning_from_files_runs_on_a_worker_and_yields_the_shape_of_the_files() {
    let dir = temp_dir("files");
    // Two files with the same known shape (the Bright target), a third that is silent and a
    // fourth that is not audio.
    let bright = comb(|f| shape_db(&SHAPES[1], f), 16_384);
    let mut long = Vec::new();
    while long.len() < (7.0 * SR) as usize {
        long.extend_from_slice(&bright);
    }
    let mut paths = Vec::new();
    for (i, data) in [long.clone(), long.clone(), vec![0.0; 200_000]]
        .into_iter()
        .enumerate()
    {
        let p = dir.join(format!("f{i}.wav"));
        write_wav_f32(
            &p,
            &AudioData::new(48_000, vec![data.clone(), data]).unwrap(),
        )
        .unwrap();
        paths.push(p);
    }
    let junk = dir.join("junk.wav");
    std::fs::write(&junk, b"not audio").unwrap();
    paths.push(junk);
    let mut core = rig();
    // Not a tonal balance device, no paths: nothing starts.
    core.learn_tonal_files(&key("tb"), Vec::new());
    assert!(!core.tonal_job_running());
    core.learn_tonal_files(&key("tb"), paths.clone());
    assert!(core.tonal_job_running());
    // A second request while it runs is refused.
    core.learn_tonal_files(&key("tb"), paths);
    let mut tries = 0;
    while core.tonal_job_running() {
        core.frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
        tries += 1;
        assert!(tries < 1500, "the analysis never finished");
    }
    let pending = core.tonal_pending().expect("a learned target");
    let want = builtin(1);
    for b in 3..31 {
        assert!((pending.bands[b] - want.bands[b]).abs() < 1.0, "band {b}");
    }
    assert_eq!(param(&core, "tb", "hidden.target"), 4.0);
    core.frame();
    let v = view(&core, "tb");
    assert_eq!(v.target_name, "Learned");
    assert!(v.files.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_that_hold_no_usable_audio_learn_nothing() {
    let dir = temp_dir("junk");
    let junk = dir.join("junk.wav");
    std::fs::write(&junk, b"not audio").unwrap();
    let mut core = rig();
    core.learn_tonal_files(&key("tb"), vec![junk]);
    let mut tries = 0;
    while core.tonal_job_running() {
        core.frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
        tries += 1;
        assert!(tries < 500);
    }
    assert!(core.tonal_pending().is_none());
    assert_eq!(param(&core, "tb", "hidden.target"), 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shell_asks_for_files_and_the_core_ignores_the_action() {
    let mut core = rig();
    core.handle_action(atomik_ui::UiAction::TonalTarget {
        device: key("tb"),
        op: TonalOp::LearnFiles,
    });
    assert!(
        !core.tonal_job_running(),
        "the shell opens the dialog first"
    );
    // An action for a device that is not a tonal balance meter does nothing.
    core.handle_action(atomik_ui::UiAction::TonalTarget {
        device: key("nope"),
        op: TonalOp::Delete,
    });
}

#[test]
fn the_taps_start_again_after_an_engine_restart() {
    let mut core = rig();
    let balanced = comb(|f| shape_db(&SHAPES[0], f), 16_384);
    feed(&mut core, "tb", &balanced, 16_384);
    assert!(view(&core, "tb").ready);
    core.rebuild_engine();
    let id = core.device_id(&key("tb")).unwrap();
    // The new engine's tap is at frame 0 again: a head below the old one is read.
    core.sink_mut()
        .device_scopes
        .insert(id, (balanced.clone(), balanced, 16_384));
    core.frame();
    assert!(view(&core, "tb").ready);
}
