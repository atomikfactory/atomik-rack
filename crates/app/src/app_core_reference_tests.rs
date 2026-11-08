//! Reference tracks in the app core: the list as undo steps, the switch with its start position,
//! the level match and the settings that reach the engine.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_engine::{ReferenceBand, ReferenceSolo};
use atomik_model::empty_doc;
use std::time::{Duration, Instant};

/// A temp folder that removes itself.
struct Tmp(std::path::PathBuf);

impl Tmp {
    fn new(name: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-reference-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A stereo file of `seconds`: a 1 kHz tone of `amp`.
    fn tone(&self, name: &str, amp: f32, seconds: usize) -> std::path::PathBuf {
        let rate = 48_000usize;
        let data: Vec<f32> = (0..rate * seconds)
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

fn core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::new(), empty_doc(), None)
}

/// Runs frames until the selected reference has been decoded (or fails the test).
#[allow(clippy::disallowed_methods)] // a real worker is waited for in real time
fn wait_loaded(c: &mut AppCore<RecordingSink>) {
    let start = Instant::now();
    loop {
        c.frame();
        let info = c.reference_info();
        if !info.loading && (info.loudest_lufs.is_some() || info.error.is_some()) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the decode did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn reference_calls(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
    c.sink()
        .calls
        .iter()
        .filter(|call| {
            matches!(
                call,
                SinkCall::ReferenceStart(_) | SinkCall::ReferenceSet(_)
            )
        })
        .cloned()
        .collect()
}

#[test]
fn the_list_is_edited_in_undoable_steps() {
    let tmp = Tmp::new("list");
    let (a, b) = (tmp.tone("a.wav", 0.1, 1), tmp.tone("b.wav", 0.1, 1));
    let mut c = core();
    c.add_reference(&a).unwrap();
    c.add_reference(&b).unwrap();
    assert_eq!(c.reference_info().items.len(), 2);
    assert_eq!(c.reference_info().selected, 1);
    // The same file twice is refused and changes nothing.
    assert!(c.add_reference(&a).is_err());
    assert_eq!(c.reference_info().items.len(), 2);
    c.remove_reference(0);
    assert_eq!(c.reference_info().items.len(), 1);
    assert_eq!(c.reference_info().selected, 0);
    c.undo();
    assert_eq!(c.reference_info().items.len(), 2);
    c.undo();
    c.undo();
    assert!(c.reference_info().items.is_empty());
}

#[test]
fn the_list_is_kept_to_sixteen() {
    let tmp = Tmp::new("full");
    let mut c = core();
    for i in 0..16 {
        c.add_reference(&tmp.0.join(format!("{i}.wav"))).unwrap();
    }
    assert!(c.add_reference(&tmp.0.join("17.wav")).is_err());
    assert_eq!(c.reference_info().items.len(), 16);
}

#[test]
fn the_key_without_a_reference_says_so_and_sends_nothing() {
    let mut c = core();
    c.toggle_reference();
    assert!(!c.reference_listening());
    assert!(reference_calls(&c).is_empty());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Add a reference"))
    );
}

#[test]
fn the_key_switches_to_the_reference_and_back() {
    let tmp = Tmp::new("switch");
    let path = tmp.tone("loud.wav", 0.2, 5);
    let mut c = core();
    c.add_reference(&path).unwrap();
    // Not decoded yet: the key waits, says why, and nothing reaches the engine.
    c.toggle_reference();
    assert!(!c.reference_listening());
    wait_loaded(&mut c);
    let loudest = c.reference_info().loudest_lufs.expect("measured");
    assert!((loudest + 14.0).abs() < 0.6, "{loudest}");
    c.sink_mut().take();
    c.toggle_reference();
    assert!(c.reference_listening());
    let calls = reference_calls(&c);
    assert!(
        matches!(&calls[..], [SinkCall::ReferenceStart(s)] if s.listening && s.measure),
        "{calls:?}"
    );
    assert_eq!(c.sink().reference_streams.len(), 1);
    c.sink_mut().take();
    c.toggle_reference();
    assert!(!c.reference_listening());
    let calls = reference_calls(&c);
    assert!(
        matches!(&calls[..], [SinkCall::ReferenceSet(s)] if !s.listening),
        "{calls:?}"
    );
}

#[test]
fn the_gain_follows_the_mix_slowly_and_shows() {
    let tmp = Tmp::new("match");
    let path = tmp.tone("loud.wav", 0.2, 5);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    // A silent mix does not set a gain.
    c.sink_mut().mix_lufs = Some(-80.0);
    for _ in 0..40 {
        c.frame();
    }
    assert_eq!(c.reference_info().gain_db, None);
    // The first reading is taken at once: the reference is about 4 dB louder than a mix at -18.
    c.sink_mut().mix_lufs = Some(-18.0);
    for _ in 0..16 {
        c.frame();
    }
    let loudest = c.reference_info().loudest_lufs.unwrap();
    let first = c.reference_info().gain_db.expect("matched");
    assert!((first - (-18.0 - loudest)).abs() < 0.01, "{first}");
    assert!(first < -3.0 && first > -5.0, "{first}");
    // A louder mix moves it up by at most half a dB per update.
    c.sink_mut().mix_lufs = Some(-8.0);
    for _ in 0..15 {
        c.frame();
    }
    let second = c.reference_info().gain_db.unwrap();
    assert!((second - first - 0.5).abs() < 0.01, "{first} -> {second}");
    // The voice is told the new gain.
    let sent = reference_calls(&c)
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::ReferenceSet(s) => Some(s),
            _ => None,
        });
    let sent = sent.expect("the gain was sent");
    assert!((20.0 * sent.gain.log10() - second).abs() < 0.01);
    // With the match off the reference plays at its own level and nothing is shown.
    c.set_reference_level_match(false);
    assert_eq!(c.reference_info().gain_db, None);
    let last = reference_calls(&c)
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::ReferenceSet(s) => Some(s),
            _ => None,
        });
    assert!((last.expect("sent").gain - 1.0).abs() < 1e-6);
}

#[test]
fn band_and_side_solo_reach_the_engine() {
    let tmp = Tmp::new("solo");
    let mut c = core();
    c.add_reference(&tmp.tone("a.wav", 0.1, 1)).unwrap();
    c.sink_mut().take();
    c.set_reference_band(ReferenceBand::Sub);
    c.set_reference_solo(ReferenceSolo::Side);
    let last = reference_calls(&c).into_iter().last();
    assert!(matches!(last, Some(SinkCall::ReferenceSet(s))
            if s.band == ReferenceBand::Sub && s.solo == ReferenceSolo::Side),);
    // Setting the same thing again sends nothing.
    c.sink_mut().take();
    c.set_reference_band(ReferenceBand::Sub);
    assert!(reference_calls(&c).is_empty());
}

#[test]
fn a_cue_is_one_undo_step_and_can_be_taken_away() {
    let tmp = Tmp::new("cue");
    let mut c = core();
    c.add_reference(&tmp.tone("a.wav", 0.1, 1)).unwrap();
    c.set_reference_cue(0, true);
    assert!(c.reference_info().items[0].cue_ms.is_some());
    c.set_reference_cue(0, false);
    assert_eq!(c.reference_info().items[0].cue_ms, None);
    c.undo();
    assert!(c.reference_info().items[0].cue_ms.is_some());
}

#[test]
fn a_new_engine_goes_back_to_the_mix() {
    let tmp = Tmp::new("reset");
    let mut c = core();
    c.add_reference(&tmp.tone("a.wav", 4.0 / 20.0, 4)).unwrap();
    wait_loaded(&mut c);
    c.toggle_reference();
    assert!(c.reference_listening());
    c.rebuild_engine();
    assert!(!c.reference_listening());
}

#[test]
fn a_reference_that_cannot_be_read_says_so() {
    let tmp = Tmp::new("bad");
    let path = tmp.0.join("nothing.wav");
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    let info = c.reference_info();
    assert!(info.error.is_some());
    c.toggle_reference();
    assert!(!c.reference_listening());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("cannot be used"))
    );
}

#[test]
fn the_list_survives_a_save_and_open() {
    let tmp = Tmp::new("save");
    let path = tmp.tone("a.wav", 0.1, 1);
    let mut c = core();
    c.add_reference(&path).unwrap();
    c.set_reference_cue(0, true);
    let text = atomik_model::save_project(c.doc());
    let back = atomik_model::load_project(&text, c.registry()).unwrap();
    assert_eq!(back.references, c.doc().references);
}

#[test]
fn saving_to_a_new_place_copies_the_references_into_the_project_in_one_undo_step() {
    let tmp = Tmp::new("collect");
    let outside = tmp.tone("outside.wav", 0.1, 1);
    let mut c = core();
    c.add_reference(&outside).unwrap();
    std::fs::create_dir_all(tmp.0.join("proj")).unwrap();
    c.save_as(&tmp.0.join("proj").join("song.atomik")).unwrap();
    c.wait_background();
    let copy = c.doc().references[0].path.clone();
    assert!(copy.contains("Files"), "{copy}");
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&outside).unwrap()
    );
    // The original stays; the saved project names the copy only.
    assert!(outside.is_file());
    let saved = std::fs::read_to_string(c.file().unwrap()).unwrap();
    let original = outside
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "\\\\");
    assert!(
        saved.contains("Files") && !saved.contains(&original),
        "{saved}"
    );
    let message = c.last_rejection().unwrap();
    assert!(message.contains("Reference tracks: 1 copied"), "{message}");
    // Collect Files finds nothing more to do and leaves the list as it is.
    c.collect_files().unwrap();
    c.wait_background();
    assert_eq!(c.doc().references[0].path, copy);
    // One undo step puts the old path back.
    assert!(c.undo());
    assert!(c.doc().references[0].path.contains("outside.wav"));
    assert!(!c.doc().references[0].path.contains("Files"));
}
