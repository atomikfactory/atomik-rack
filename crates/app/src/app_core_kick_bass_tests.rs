//! The kick and bass check in the app core: the picks, the worker, the one-step fix and the
//! guard against a song that changed after the check.

use super::*;
use crate::kick_bass::tests::scene;
use crate::sink::RecordingSink;
use atomik_model::save_project;
use atomik_render::default_registry;
use atomik_ui::RunView;
use std::time::Duration;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn core(bass_start: f32) -> AppCore<RecordingSink> {
    AppCore::with_registry(
        RecordingSink::new(),
        default_registry(),
        scene(bass_start),
        None,
    )
}

/// Runs frames until the check is no longer running (it renders a few seconds of audio).
fn wait(c: &mut AppCore<RecordingSink>) {
    let mut waited = 0;
    while c.kick_bass_running() {
        waited += 1;
        assert!(waited < 6_000, "the check did not finish");
        std::thread::sleep(Duration::from_millis(20));
        c.transport_frame(0.0);
    }
}

fn picked(c: &mut AppCore<RecordingSink>) {
    c.kick_bass_open();
}

#[test]
fn the_tracks_are_listed_and_the_names_guess_the_picks() {
    let mut c = core(180.0);
    let names: Vec<String> = c.kick_bass_tracks().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["Kick Drum", "Sub Bass"]);
    picked(&mut c);
    let info = c.kick_bass_info();
    assert_eq!((info.kick, info.bass), (Some(0), Some(1)));
    // Stepping wraps round the list and drops nothing else.
    c.kick_bass_pick(true, 1);
    assert_eq!(c.kick_bass_info().bass, Some(0));
    c.kick_bass_pick(true, -1);
    assert_eq!(c.kick_bass_info().bass, Some(1));
    c.kick_bass_pick(false, -1);
    assert_eq!(c.kick_bass_info().kick, Some(1));
}

#[test]
fn running_without_two_different_tracks_says_what_is_missing() {
    let mut c = core(180.0);
    c.kick_bass_run();
    assert!(matches!(c.kick_bass_info().run, RunView::Failed(ref w) if w.contains("Pick")));
    picked(&mut c);
    c.kick_bass_pick(false, 1);
    c.kick_bass_run();
    assert!(matches!(c.kick_bass_info().run, RunView::Failed(ref w) if w.contains("two tracks")));
    assert!(!c.kick_bass_running());
}

#[test]
fn a_check_gives_the_strip_and_fixes_without_touching_the_song() {
    let mut c = core(180.0);
    picked(&mut c);
    let before = save_project(c.doc());
    let undo_before = c.document.undo_label();
    c.kick_bass_run();
    assert!(c.kick_bass_running());
    wait(&mut c);
    let info = c.kick_bass_info();
    assert_eq!(info.run, RunView::Done);
    let strip = info.strip.expect("a strip");
    assert!(strip.hits.len() >= 6);
    assert!(info.summary.contains("cancel"), "{}", info.summary);
    assert!(!info.fixes.is_empty());
    assert!(
        info.fixes[0].gain.starts_with('+'),
        "{}",
        info.fixes[0].gain
    );
    assert_eq!(save_project(c.doc()), before);
    assert_eq!(c.document.undo_label(), undo_before);
}

#[test]
fn applying_a_fix_is_one_undo_step_says_what_it_did_and_checks_again() {
    let mut c = core(180.0);
    picked(&mut c);
    c.kick_bass_run();
    wait(&mut c);
    let before = save_project(c.doc());
    let base_db = c.kick_bass.result.as_ref().unwrap().0.base.overall_db;
    c.kick_bass_apply(0);
    let said = c.last_rejection().unwrap().to_string();
    assert!(said.starts_with("Kick and Bass: "), "{said}");
    assert_ne!(save_project(c.doc()), before);
    // The check runs again on the fixed song and its strip is better.
    assert!(c.kick_bass_running());
    wait(&mut c);
    let after = c.kick_bass.result.as_ref().unwrap().0.base.overall_db;
    assert!(after > base_db + 2.0, "{base_db} then {after}");
    // One undo reverts the whole fix.
    assert!(c.undo());
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn a_song_that_changed_after_the_check_gets_no_fix() {
    let mut c = core(180.0);
    picked(&mut c);
    c.kick_bass_run();
    wait(&mut c);
    c.apply_command(Command::SetParam {
        key: key("bass"),
        param: "master.level".to_owned(),
        value: -20.0,
        merge: None,
    })
    .unwrap();
    let info = c.kick_bass_info();
    assert!(info.fixes.is_empty());
    assert!(info.summary.contains("changed"), "{}", info.summary);
    let before = save_project(c.doc());
    c.kick_bass_apply(0);
    assert!(c.last_rejection().unwrap().contains("changed"));
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn a_cancelled_check_leaves_no_result() {
    let mut c = core(180.0);
    picked(&mut c);
    c.kick_bass_run();
    c.kick_bass_cancel();
    assert!(!c.kick_bass_running());
    assert_eq!(c.kick_bass_info().run, RunView::Idle);
}

#[test]
fn applying_before_a_check_is_refused() {
    let mut c = core(180.0);
    let before = save_project(c.doc());
    c.kick_bass_apply(0);
    assert!(c.last_rejection().unwrap().contains("Run the kick"));
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn the_peak_evening_row_shows_the_headroom_and_applies_like_a_fix() {
    let mut doc = Document::new(scene(0.0));
    let registry = default_registry();
    for (param, value) in [
        ("osc2.wave", 3.0),
        ("osc2.octave", 1.0),
        ("osc.mix", 0.4),
        ("osc.phase_start", 60.0),
    ] {
        doc.apply(
            Command::SetParam {
                key: key("bass"),
                param: param.to_owned(),
                value,
                merge: None,
            },
            &registry,
        )
        .unwrap();
    }
    let mut c = AppCore::with_registry(
        RecordingSink::new(),
        default_registry(),
        doc.doc().clone(),
        None,
    );
    picked(&mut c);
    c.kick_bass_run();
    wait(&mut c);
    let info = c.kick_bass_info();
    let row = info.fixes.last().expect("the helper row");
    assert!(row.gain.ends_with("dB headroom"), "{}", row.gain);
    assert!(row.label.contains("Even out the peaks"), "{}", row.label);
    let before = save_project(c.doc());
    c.kick_bass_apply(info.fixes.len() - 1);
    assert!(c.last_rejection().unwrap().contains("Rotate the phase"));
    let tool = c
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == "atomik.stereo-tool")
        .expect("a Stereo Tool on the bass channel");
    let rotate = c
        .doc()
        .param_value(&tool.key, "rotate", c.registry())
        .unwrap();
    assert!(rotate > 0.0);
    wait(&mut c);
    assert!(c.undo());
    assert_eq!(save_project(c.doc()), before);
}
