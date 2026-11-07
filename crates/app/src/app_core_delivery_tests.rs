//! The delivery targets and the Peak Surgeon in the app core: the master is measured on a worker
//! and nothing in the song changes, each target says what it does to the measured master and a
//! quiet master with hot peaks is warned about, Set Ceiling is one undo step that sets the
//! limiter's ceiling and the report target, the export's report compares against the target, and
//! the surgeon lists the peaks of the selected clip, dips them in its envelope as one undo step and
//! says how far the highest peak went down.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ConsoleCommand, CreateOptions, CreateSource, InsertEffect, Note, TARGETS,
    WorkflowCommand, new_project_doc,
};
use atomik_render::loudness::LoudnessReport;
use atomik_ui::{DeliveryPage, ImportTarget, TargetMark};
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// A temporary folder that removes itself.
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-delivery-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The starter project with a synth `lead` that plays two bars of notes.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 2 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 67, 0.8)].into(),
            ),
        },
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

/// Puts a `type_id` effect on the master's insert chain.
fn master_insert(c: &mut AppCore<RecordingSink>, type_id: &str) -> DeviceKey {
    let master = c.doc().console.master.clone().unwrap();
    c.apply_command(Command::Console(ConsoleCommand::SetInsert {
        channel: master.clone(),
        slot: 0,
        effect: Some(InsertEffect::New {
            type_id: type_id.to_owned(),
        }),
    }))
    .unwrap();
    atomik_model::insert_chain(c.doc(), &c.registry, &master)[0].clone()
}

fn report(lufs: f32, peak: f32) -> LoudnessReport {
    LoudnessReport {
        integrated_lufs: Some(lufs),
        range_lu: Some(5.0),
        true_peak_dbtp: Some(peak),
        sample_peak_dbfs: Some(peak - 0.1),
    }
}

fn row_of(c: &mut AppCore<RecordingSink>, name: &str) -> atomik_ui::TargetRow {
    c.delivery_data()
        .targets
        .rows
        .into_iter()
        .find(|r| r.name.starts_with(name))
        .unwrap()
}

// ------------------------------------------------------------------ the delivery targets

#[test]
fn the_measurement_runs_on_a_worker_and_changes_nothing() {
    let mut c = core();
    let before = c.doc().clone();
    assert!(c.delivery_report().is_none());
    c.start_delivery().expect("starts");
    assert!(c.delivery_running());
    assert!(c.start_delivery().is_err(), "one at a time");
    c.wait_background();
    assert!(!c.delivery_running());
    let (r, stale) = c.delivery_report().expect("a result");
    assert!(!stale);
    assert!(
        r.integrated_lufs.is_some() && r.true_peak_dbtp.is_some(),
        "{r:?}"
    );
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Master: ")),
        "{:?}",
        c.last_rejection()
    );
    let rows = c.delivery_data().targets.rows;
    assert_eq!(rows.len(), TARGETS.len());
    assert!(rows.iter().all(|r| r.mark != TargetMark::None), "{rows:?}");
    assert_eq!(*c.doc(), before, "a measurement never changes the song");
}

#[test]
fn the_result_goes_stale_when_the_song_changes() {
    let mut c = core();
    c.start_delivery().unwrap();
    c.wait_background();
    assert!(!c.delivery_data().targets.stale);
    c.apply_command(Command::Song(SongCommand::SetSongEnd(5 * BAR)))
        .unwrap();
    assert!(c.delivery_data().targets.stale);
    c.start_delivery().unwrap();
    c.wait_background();
    assert!(!c.delivery_data().targets.stale);
}

#[test]
fn a_song_without_a_master_or_without_tracks_is_refused_with_a_reason() {
    let mut doc = new_project_doc();
    doc.console.master = None;
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    let why = c.start_delivery().unwrap_err();
    assert!(why.contains("master"), "{why}");
    assert_eq!(c.last_rejection(), Some(why.as_str()));
    let mut empty = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    assert!(empty.start_delivery().unwrap_err().contains("empty"));
    assert!(!empty.delivery_running());
}

#[test]
fn a_new_project_drops_the_result() {
    let mut c = core();
    c.start_delivery().unwrap();
    c.wait_background();
    assert!(c.delivery_report().is_some());
    c.new_document();
    assert!(c.delivery_report().is_none());
}

#[test]
fn a_louder_master_is_turned_down_and_a_quiet_one_with_hot_peaks_is_warned_about() {
    let mut c = core();
    c.delivery_open(DeliveryPage::Targets);
    assert_eq!(row_of(&mut c, "Streaming -14").effect, "not measured");
    // Loud: every target below it turns it down, nothing is lost.
    c.set_delivery_for_test(report(-9.0, -0.3));
    let r = row_of(&mut c, "Streaming -14");
    assert_eq!(
        (r.effect.as_str(), r.mark),
        ("turned down 5.0 dB", TargetMark::Fine)
    );
    assert!(c.delivery_data().targets.warning.is_empty());
    // Quiet with peaks near 0 dBFS: turned up into a limiter.
    c.set_delivery_for_test(report(-18.0, -0.2));
    let r = row_of(&mut c, "Streaming -14");
    assert_eq!(r.mark, TargetMark::Limiter);
    assert!(r.effect.contains("into its limiter"), "{}", r.effect);
    c.delivery_choose(1);
    let warning = c.delivery_data().targets.warning;
    assert!(
        warning.contains("limiter") && warning.contains("Streaming -14 LUFS"),
        "{warning}"
    );
    // The same loudness with room under the ceiling is only turned up.
    c.set_delivery_for_test(report(-18.0, -12.0));
    assert_eq!(row_of(&mut c, "Streaming -14").mark, TargetMark::Up);
    assert!(c.delivery_data().targets.warning.is_empty());
}

#[test]
fn set_ceiling_sets_the_mastering_ceiling_and_the_report_target_in_one_undo_step() {
    let mut c = core();
    let mastering = master_insert(&mut c, "atomik.mastering");
    let ceiling =
        |c: &AppCore<RecordingSink>| c.doc().param_value(&mastering, "ceiling", &c.registry);
    let was = ceiling(&c).unwrap();
    let depth = c.document.undo_depth();
    c.delivery_open(DeliveryPage::Targets);
    let broadcast = TARGETS
        .iter()
        .position(|t| t.name.starts_with("Broadcast -24"))
        .unwrap();
    c.delivery_choose(broadcast);
    assert!(
        c.delivery_data()
            .targets
            .ceiling_note
            .contains("Mastering ceiling to -2.0"),
        "{}",
        c.delivery_data().targets.ceiling_note
    );
    c.delivery_set_ceiling();
    assert_eq!(ceiling(&c), Some(-2.0));
    assert_eq!(
        c.doc().delivery.as_ref().map(|d| d.name.as_str()),
        Some("Broadcast -24 LUFS / -2 dBTP")
    );
    assert_eq!(c.document.undo_depth(), depth + 1, "one undo step");
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Delivery: Mastering ceiling -2.0")),
        "{:?}",
        c.last_rejection()
    );
    // The row of the project's target says so.
    assert!(row_of(&mut c, "Broadcast -24").current);
    // Doing it again changes nothing and says so.
    c.delivery_set_ceiling();
    assert!(c.last_rejection().unwrap().starts_with("Already set"));
    assert_eq!(c.document.undo_depth(), depth + 1);
    c.undo();
    assert_eq!(ceiling(&c), Some(was));
    assert!(c.doc().delivery.is_none());
}

#[test]
fn a_clipper_takes_the_ceiling_when_there_is_no_mastering_device_and_none_means_the_target_only() {
    let mut bare = core();
    bare.delivery_open(DeliveryPage::Targets);
    assert!(
        bare.delivery_data()
            .targets
            .ceiling_note
            .contains("report target only"),
        "{}",
        bare.delivery_data().targets.ceiling_note
    );
    bare.delivery_set_ceiling();
    assert!(bare.doc().delivery.is_some());
    let mut c = core();
    let clipper = master_insert(&mut c, "atomik.clipper");
    c.delivery_open(DeliveryPage::Targets);
    c.delivery_choose(0);
    c.delivery_set_ceiling();
    assert_eq!(
        c.doc().param_value(&clipper, "ceiling", &c.registry),
        Some(-1.0)
    );
    assert!(c.last_rejection().unwrap().contains("Clipper ceiling -1.0"));
}

#[test]
fn opening_the_panel_chooses_the_projects_target() {
    let mut c = core();
    c.delivery_open(DeliveryPage::Targets);
    c.delivery_choose(4);
    c.delivery_set_ceiling();
    let mut again = c;
    again.delivery_choose(0);
    again.delivery_open(DeliveryPage::Targets);
    assert_eq!(again.delivery_data().targets.chosen, 4);
}

#[test]
fn the_exports_report_compares_with_the_delivery_target() {
    let t = Tmp::new("export");
    let mut c = core();
    c.delivery_open(DeliveryPage::Targets);
    c.delivery_set_ceiling();
    let name = c.doc().delivery.as_ref().unwrap().name.clone();
    c.start_audio_export(
        atomik_ui::ExportScope::Song,
        t.0.join("mix.wav"),
        atomik_ui::ExportValues::default(),
    )
    .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains(&format!("; {name}: ")) && message.contains("loudness target"),
        "{message}"
    );
    // Without a target the report is as it was.
    let mut plain = core();
    plain
        .start_audio_export(
            atomik_ui::ExportScope::Song,
            t.0.join("plain.wav"),
            atomik_ui::ExportValues::default(),
        )
        .unwrap();
    plain.wait_background();
    assert!(!plain.last_rejection().unwrap().contains("loudness target"));
}

// ------------------------------------------------------------------ the peak surgeon

/// A mono file of 2 s at 48 kHz: a quiet tone with three sharp peaks.
fn peaky_wav(t: &Tmp) -> PathBuf {
    let mut data: Vec<f32> = (0..96_000)
        .map(|i| 0.2 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
        .collect();
    data[10_000] = 0.95;
    data[40_000] = 0.80;
    data[70_000] = 0.60;
    let path = t.0.join("peaky.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn audio_core(t: &Tmp) -> (AppCore<RecordingSink>, ClipId) {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_warp_imports(crate::import_warp::WarpImports::Off);
    c.import_audio(vec![peaky_wav(t)], ImportTarget::NewTracks, 0);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let id = c
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter().map(|c| c.id)))
        .next()
        .unwrap();
    c.arrangement_mut().set_selected_clips(&[id]);
    (c, id)
}

fn envelope(c: &AppCore<RecordingSink>, id: ClipId) -> usize {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.envelope.len(),
        _ => panic!("not audio"),
    }
}

#[test]
fn the_page_lists_the_highest_peaks_of_the_selected_clip_with_their_times() {
    let t = Tmp::new("list");
    let (mut c, _) = audio_core(&t);
    c.delivery_open(DeliveryPage::Peaks);
    let p = c.delivery_data().peaks;
    assert!(!p.clip.is_empty(), "{p:?}");
    let top: Vec<(String, i32)> = p
        .rows
        .iter()
        .take(3)
        .map(|r| (r.time.clone(), (r.db * 10.0).round() as i32))
        .collect();
    // 0.95, 0.80 and 0.60 are -0.4, -1.9 and -4.4 dBFS at frames 10 000, 40 000 and 70 000.
    assert_eq!(
        top,
        vec![
            ("0:00.208".to_owned(), -4),
            ("0:00.833".to_owned(), -19),
            ("0:01.458".to_owned(), -44)
        ]
    );
    assert_eq!(p.rows[0].headroom, 0.0);
    assert!((p.rows[1].headroom - 1.5).abs() < 0.05);
    // The threshold starts a dB under the highest peak.
    assert!((p.threshold_db + 1.5).abs() < 0.11, "{}", p.threshold_db);
    assert!(p.can_apply && p.preview.contains("peak"), "{}", p.preview);
}

#[test]
fn without_an_audio_clip_the_page_says_why() {
    let mut c = core();
    c.delivery_open(DeliveryPage::Peaks);
    let p = c.delivery_data().peaks;
    assert!(
        p.clip.is_empty() && p.message.contains("Select an audio clip"),
        "{p:?}"
    );
    c.surgeon_apply_all();
    assert!(c.last_rejection().unwrap().contains("Select an audio clip"));
}

#[test]
fn apply_all_dips_the_peaks_above_the_threshold_in_the_envelope_as_one_undo_step() {
    let t = Tmp::new("apply");
    let (mut c, id) = audio_core(&t);
    c.delivery_open(DeliveryPage::Peaks);
    let before = c.doc().clone();
    let depth = c.document.undo_depth();
    c.surgeon_threshold(0.0);
    c.surgeon_apply_all();
    // Only the highest peak (-0.4 dBFS) is above -1.5 dBFS: it needs four steps of 0.3 dB.
    assert_eq!(c.document.undo_depth(), depth + 1, "one undo step");
    assert!(envelope(&c, id) >= 3, "{}", envelope(&c, id));
    let message = c.last_rejection().unwrap();
    assert!(message.starts_with("Peak Surgeon: "), "{message}");
    // The clip was read again: the highest peak is at or under the threshold now.
    let after = c.delivery_data().peaks;
    assert!(after.rows[0].db <= after.threshold_db + 0.05, "{after:?}");
    // The file is untouched and the clip's gain is as it was.
    let (ClipContent::Audio(old), ClipContent::Audio(new)) = (
        &before.song.clip(id).unwrap().2.content,
        &c.doc().song.clip(id).unwrap().2.content,
    ) else {
        panic!("audio")
    };
    assert_eq!((&old.asset, old.gain_db), (&new.asset, new.gain_db));
    c.undo();
    assert_eq!(envelope(&c, id), 0);
    assert_eq!(*c.doc(), before);
}

#[test]
fn trim_dips_the_chosen_peak_by_one_step_and_the_step_and_width_cycle() {
    let t = Tmp::new("trim");
    let (mut c, id) = audio_core(&t);
    c.delivery_open(DeliveryPage::Peaks);
    let top = c.delivery_data().peaks.rows[0].db;
    c.surgeon_choose(0);
    c.surgeon_trim();
    let now = c.delivery_data().peaks.rows[0].db;
    assert!(
        (top - now - 0.3).abs() < 0.02,
        "one step of 0.3 dB: {top} to {now}"
    );
    assert_eq!(envelope(&c, id), 3);
    // A second trim of the same peak deepens the same dip.
    c.surgeon_trim();
    assert_eq!(envelope(&c, id), 3);
    let again = c.delivery_data().peaks.rows[0].db;
    assert!((now - again - 0.3).abs() < 0.02, "{now} to {again}");
    // The step cycles through 0.2, 0.3 and 0.4; the width through four widths.
    assert_eq!(c.delivery_data().peaks.step_db, 0.3);
    c.surgeon_cycle_step();
    assert_eq!(c.delivery_data().peaks.step_db, 0.4);
    c.surgeon_cycle_step();
    assert_eq!(c.delivery_data().peaks.step_db, 0.2);
    let widths: Vec<u32> = (0..4)
        .map(|_| {
            c.surgeon_cycle_width();
            c.delivery_data().peaks.width
        })
        .collect();
    assert_eq!(widths, vec![16, 32, 4, 8]);
    assert_eq!(c.surgeon_step_for_test(), 0.2);
}

#[test]
fn nothing_above_the_threshold_trims_nothing_and_says_so() {
    let t = Tmp::new("none");
    let (mut c, id) = audio_core(&t);
    c.delivery_open(DeliveryPage::Peaks);
    for _ in 0..40 {
        c.surgeon_threshold(0.1);
    }
    assert_eq!(c.delivery_data().peaks.threshold_db, 0.0);
    c.surgeon_apply_all();
    assert!(c.last_rejection().unwrap().contains("nothing to trim"));
    assert_eq!(envelope(&c, id), 0);
}

#[test]
fn a_warped_clip_is_refused_with_a_reason() {
    let t = Tmp::new("refuse");
    let (mut c, id) = audio_core(&t);
    let (clip, content) = {
        let (_, _, clip) = c.doc().song.clip(id).unwrap();
        let ClipContent::Audio(a) = &clip.content else {
            panic!("audio")
        };
        (clip.clone(), a.clone())
    };
    let mut warped = content.clone();
    warped.warp.enabled = true;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id,
        start: clip.start,
        length: clip.length,
        muted: false,
        content: warped,
        merge: None,
    }))
    .unwrap();
    c.delivery_open(DeliveryPage::Peaks);
    let p = c.delivery_data().peaks;
    assert!(
        p.clip.is_empty() && p.message.contains("own speed"),
        "{p:?}"
    );
}

#[test]
fn a_reversed_clip_lists_the_peaks_at_their_times_in_the_clip() {
    let t = Tmp::new("reverse");
    let (mut c, id) = audio_core(&t);
    let (clip, content) = {
        let (_, _, clip) = c.doc().song.clip(id).unwrap();
        let ClipContent::Audio(a) = &clip.content else {
            panic!("audio")
        };
        (clip.clone(), a.clone())
    };
    let mut reversed = content;
    reversed.reverse = true;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id,
        start: clip.start,
        length: clip.length,
        muted: false,
        content: reversed,
        merge: None,
    }))
    .unwrap();
    c.delivery_open(DeliveryPage::Peaks);
    let p = c.delivery_data().peaks;
    // The 0.95 peak was at 0.208 s of 2 s: reversed it plays at 1.792 s.
    assert_eq!(p.rows[0].time, "0:01.791", "{p:?}");
    c.surgeon_trim();
    assert_eq!(envelope(&c, id), 3);
    let after = c.delivery_data().peaks;
    assert!(after.rows[0].db < p.rows[0].db - 0.25, "{after:?}");
}

#[test]
fn a_new_project_forgets_the_clip() {
    let t = Tmp::new("forget");
    let (mut c, _) = audio_core(&t);
    c.delivery_open(DeliveryPage::Peaks);
    assert!(!c.delivery_data().peaks.clip.is_empty());
    c.new_document();
    c.delivery_open(DeliveryPage::Peaks);
    assert!(c.delivery_data().peaks.clip.is_empty());
}
