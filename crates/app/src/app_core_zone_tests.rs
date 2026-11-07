//! The decision zone in the app core: the worker, the readings kept by path, the verdicts and
//! the status texts. The model of the readings is tested in `atomik_render::zone`.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, decode_file_with, write_wav_f32};
use atomik_model::empty_doc;
use atomik_render::zone::Attribute;
use atomik_ui::{Standing, ZoneData, ZoneRowData};
use patch_preview_ops::{EFFECT_PHRASE, phrase_clip};
use std::time::{Duration, Instant};

/// A temp folder that removes itself.
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-zone-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A stereo file of 4 s: a 1 kHz tone of `amp`.
    fn tone(&self, name: &str, amp: f32) -> PathBuf {
        let rate = 48_000usize;
        let data: Vec<f32> = (0..rate * 4)
            .map(|n| amp * (std::f32::consts::TAU * 1000.0 * n as f32 / rate as f32).sin())
            .collect();
        let path = self.0.join(name);
        write_wav_f32(
            &path,
            &AudioData::new(48_000, vec![data.clone(), data]).unwrap(),
        )
        .unwrap();
        path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn empty_core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::new(), empty_doc(), None)
}

/// A song with a synth that plays a short phrase: an audible mix.
fn song_core() -> AppCore<RecordingSink> {
    let mut c = empty_core();
    let key = DeviceKey("lead".to_owned());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_owned(),
        key: Some(key.clone()),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key.clone(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key,
        lane: 0,
        clip: phrase_clip(EFFECT_PHRASE),
    }))
    .unwrap();
    c
}

/// Runs frames until the analysis has finished (or fails the test).
#[allow(clippy::disallowed_methods)] // a real worker is waited for in real time
fn wait_measured(c: &mut AppCore<RecordingSink>) {
    let start = Instant::now();
    while c.zone_busy() {
        c.frame();
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "the analysis did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    c.frame();
}

fn row<'a>(data: &'a ZoneData, label: &str) -> &'a ZoneRowData {
    data.rows.iter().find(|r| r.label == label).expect("a row")
}

#[test]
fn a_mix_louder_than_quiet_references_reads_above_and_the_song_is_untouched() {
    let tmp = Tmp::new("above");
    let mut c = song_core();
    c.add_reference(&tmp.tone("a.wav", 0.003)).unwrap();
    c.add_reference(&tmp.tone("b.wav", 0.004)).unwrap();
    let revision = c.document.revision();
    assert!(c.refresh_zone());
    assert!(c.zone_busy() && c.is_busy());
    // A second request while one runs starts nothing.
    assert!(!c.refresh_zone());
    wait_measured(&mut c);
    let data = c.zone_data();
    let loudness = row(&data, "Loudness");
    assert_eq!(loudness.standing, Standing::Above, "{loudness:?}");
    assert!(loudness.verdict.starts_with("above by"), "{loudness:?}");
    let (lo, hi) = loudness.range.expect("the references are measured");
    assert!(lo < hi && hi < -40.0, "{lo} {hi}");
    // The range is what the model reads from the files.
    let a = decode_file_with(&tmp.0.join("a.wav"), 1 << 26)
        .unwrap()
        .audio;
    assert_eq!(Some(lo), atomik_render::zone::measure(&a).integrated_lufs);
    // Every row reads the same as the model's own zone of the same readings.
    assert_eq!(c.zone_rows().len(), Attribute::ALL.len());
    // Measuring never edits the song.
    assert_eq!(c.document.revision(), revision);
    assert!(
        data.status.contains("measured against 2 references"),
        "{}",
        data.status
    );
    assert!(!data.busy);
}

#[test]
fn a_mix_quieter_than_loud_references_reads_below() {
    let tmp = Tmp::new("below");
    let mut c = song_core();
    c.add_reference(&tmp.tone("loud.wav", 0.9)).unwrap();
    assert!(c.refresh_zone());
    wait_measured(&mut c);
    let data = c.zone_data();
    let loudness = row(&data, "Loudness");
    assert_eq!(loudness.standing, Standing::Below, "{loudness:?}");
    assert!(loudness.verdict.starts_with("below by"), "{loudness:?}");
    assert!(loudness.mix.is_some());
    // The status line names the rows that are outside.
    let summary = c.zone_summary();
    assert!(summary.contains("Loudness below"), "{summary}");
}

#[test]
fn a_silent_mix_has_readings_for_the_references_and_no_verdict() {
    let tmp = Tmp::new("silent");
    let mut c = empty_core();
    c.add_reference(&tmp.tone("a.wav", 0.1)).unwrap();
    assert!(c.refresh_zone());
    wait_measured(&mut c);
    let data = c.zone_data();
    for r in &data.rows {
        assert!(r.range.is_some(), "{r:?}");
        assert_eq!(r.mix, None, "{r:?}");
        assert_eq!(r.standing, Standing::Unknown, "{r:?}");
    }
    assert!(c.zone_summary().contains("nothing to compare"));
}

#[test]
fn without_a_reference_nothing_starts_and_the_panel_says_what_to_do() {
    let mut c = song_core();
    assert!(!c.refresh_zone());
    assert!(!c.zone_busy());
    let data = c.zone_data();
    assert!(data.references.is_empty());
    assert!(data.status.contains("Add a reference"), "{}", data.status);
    assert!(
        data.rows
            .iter()
            .all(|r| r.range.is_none() && r.mix.is_none())
    );
}

#[test]
fn a_file_that_cannot_be_read_is_named_and_the_others_still_count() {
    let tmp = Tmp::new("broken");
    let mut c = song_core();
    c.add_reference(&tmp.tone("good.wav", 0.1)).unwrap();
    c.add_reference(&tmp.0.join("missing.wav")).unwrap();
    assert!(c.refresh_zone());
    wait_measured(&mut c);
    let data = c.zone_data();
    assert!(
        data.references[0].detail.starts_with("loudest"),
        "{:?}",
        data.references
    );
    assert!(!data.references[1].detail.is_empty());
    assert!(!data.references[1].detail.starts_with("loudest"));
    assert!(
        data.status.contains("1 could not be read"),
        "{}",
        data.status
    );
    assert!(row(&data, "Loudness").range.is_some());
}

#[test]
fn a_refresh_keeps_the_readings_of_the_files_it_has_and_notices_a_changed_song() {
    let tmp = Tmp::new("again");
    let mut c = song_core();
    let path = tmp.tone("a.wav", 0.1);
    c.add_reference(&path).unwrap();
    assert!(c.refresh_zone());
    wait_measured(&mut c);
    let first = c.zone_data();
    assert!(first.status.contains("measured against 1 reference"));
    // The file is gone, but its reading stays while it is listed.
    std::fs::remove_file(&path).unwrap();
    assert!(c.refresh_zone());
    wait_measured(&mut c);
    assert_eq!(c.zone_data().rows, first.rows);
    // An edit makes the measured mix old.
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".to_owned(),
        key: Some(DeviceKey("other".to_owned())),
    })
    .unwrap();
    assert!(c.zone_data().status.contains("song changed"));
    // A reference that leaves the list leaves the readings.
    c.remove_reference(0);
    assert!(c.zone_data().references.is_empty());
    assert!(c.zone_rows().iter().all(|r| r.range.is_none()));
}

#[test]
fn before_the_first_analysis_the_panel_says_to_refresh() {
    let tmp = Tmp::new("fresh");
    let mut c = song_core();
    c.add_reference(&tmp.tone("a.wav", 0.1)).unwrap();
    let data = c.zone_data();
    assert!(data.status.contains("REFRESH"), "{}", data.status);
    assert!(!c.zone_measured());
    assert_eq!(data.references.len(), 1);
    assert_eq!(data.references[0].name, "a.wav");
}
