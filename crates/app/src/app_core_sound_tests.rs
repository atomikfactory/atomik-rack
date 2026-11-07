//! The Sound Analyst in the app core: a file is analysed on a worker and changes nothing, Make
//! Patch is one undo step that adds a track with the proposed synth and a clip that plays the
//! note, Match renders the patch as it is now, and a refused or stopped job leaves nothing behind.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::empty_doc;
use std::f64::consts::TAU;

const RATE: u32 = 44_100;

/// A temp folder that removes itself.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-sound-analyst-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str, samples: Vec<f32>) -> PathBuf {
        let path = self.0.join(name);
        write_wav_f32(&path, &AudioData::new(RATE, vec![samples]).unwrap()).unwrap();
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

/// A saw pluck at 220 Hz that falls to a tenth in 0.3 s.
fn pluck() -> Vec<f32> {
    let tau = 0.3 / 10f64.ln();
    (0..(1.2 * f64::from(RATE)) as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let sum: f64 = (1..=24)
                .map(|k| (TAU * 220.0 * k as f64 * t).sin() / k as f64)
                .sum();
            (0.4 * (t / 0.002).min(1.0) * (-t / tau).exp() * sum) as f32
        })
        .collect()
}

/// White noise through a first difference that falls to a tenth in 60 ms.
fn hat() -> Vec<f32> {
    let mut state = 0x1234_5678_u32;
    let mut last = 0.0f32;
    (0..(0.4 * f64::from(RATE)) as usize)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            let y = x - last;
            last = x;
            let t = i as f64 / f64::from(RATE);
            (0.4 * f64::from(y) * (-t / (0.06 / 10f64.ln())).exp()) as f32
        })
        .collect()
}

fn analysed(c: &mut AppCore<RecordingSink>, path: &Path) {
    c.analyse_sound_file(path).expect("starts");
    assert!(c.sound_running());
    c.wait_background();
    assert!(!c.sound_running());
}

#[test]
fn a_file_is_analysed_on_a_worker_and_changes_nothing() {
    let tmp = Tmp::new();
    let path = tmp.file("pluck.wav", pluck());
    let mut c = core();
    let before = c.doc().clone();
    assert!(c.sound_analysis().is_none());
    c.analyse_sound_file(&path).expect("starts");
    assert!(c.analyse_sound_file(&path).is_err(), "one job at a time");
    c.wait_background();
    assert_eq!(*c.doc(), before, "an analysis never changes the song");
    let a = c.sound_analysis().expect("analysed");
    assert_eq!(a.name, "pluck.wav");
    assert!((a.profile.pitch.expect("pitched").fundamental_hz - 220.0).abs() < 1.5);
    let data = c.sound_data();
    assert_eq!(
        data.file.as_deref().map(|f| f.starts_with("pluck.wav, ")),
        Some(true)
    );
    assert_eq!(data.numbers.len(), 4, "{:?}", data.numbers);
    assert!(
        data.numbers[0].starts_with("Envelope: attack"),
        "{:?}",
        data.numbers
    );
    assert!(data.numbers[2].contains("220"), "{:?}", data.numbers);
    assert!(
        data.plan.iter().any(|l| l.contains("saw")),
        "{:?}",
        data.plan
    );
    assert!(!data.level.target.is_empty() && !data.level.made.is_empty());
    assert!(data.pitch.is_some(), "a pitched sound has a pitch curve");
    let m = data.matched.expect("the proposal's match");
    assert_eq!(m.of, "Proposed patch");
    assert!(m.level.value < 3.0 && m.brightness.value < 0.6, "{m:?}");
    assert!(data.can_make && !data.can_match && !data.stale);
    assert!(
        c.last_rejection().is_some_and(|m| m.contains("pluck.wav")),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn make_patch_adds_a_track_with_the_synth_its_params_and_a_note_in_one_undo_step() {
    let tmp = Tmp::new();
    let mut c = core();
    assert!(c.make_sound_patch().is_err(), "nothing analysed yet");
    analysed(&mut c, &tmp.file("pluck.wav", pluck()));
    let before = c.doc().clone();
    c.make_sound_patch().expect("makes");
    let device = c.sound_patch_device().cloned().expect("the new device");
    let doc = c.doc();
    let d = doc.device(&device).expect("in the document");
    assert_eq!(d.type_id, "atomik.poly-synth");
    assert_eq!(
        d.params.get("osc1.wave").copied().unwrap_or(0.0),
        0.0,
        "a saw"
    );
    assert!(d.params.get("env.amp.decay").is_some_and(|v| *v > 500.0));
    let track = doc.song.track(&device).expect("a song track");
    let clips = &track.lanes[0].clips;
    assert_eq!(clips.len(), 1);
    let ClipContent::Notes(notes) = &clips[0].content else {
        panic!("a note clip");
    };
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].key, 57, "the note of 220 Hz");
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Made a starting patch")),
        "{:?}",
        c.last_rejection()
    );
    // Everything (the instrument, its channel, the params, the clip) is one undo step.
    assert!(c.undo());
    assert_eq!(*c.doc(), before);
    assert!(c.redo());
    assert!(c.doc().device(&device).is_some());
    let after = c.sound_data();
    assert!(after.can_match, "a made patch can be matched");
}

#[test]
fn match_renders_the_patch_as_it_is_now_and_a_change_makes_the_old_match_stale() {
    let tmp = Tmp::new();
    let mut c = core();
    analysed(&mut c, &tmp.file("pluck.wav", pluck()));
    assert!(c.start_sound_match().is_err(), "no patch was made");
    c.make_sound_patch().unwrap();
    let device = c.sound_patch_device().cloned().unwrap();
    let first = c.sound_data();
    assert!(!first.stale);
    let first_score = first.matched.as_ref().unwrap().overall;
    // Wreck the sound: a very slow, very dull envelope.
    for (param, value) in [("env.amp.attack", 2_000.0), ("filter.cutoff", 400.0)] {
        c.apply_command(Command::SetParam {
            key: device.clone(),
            param: param.into(),
            value,
            merge: None,
        })
        .unwrap();
    }
    assert!(c.sound_data().stale, "the match is of the patch as it was");
    c.start_sound_match().expect("starts");
    assert!(c.start_sound_match().is_err(), "one job at a time");
    c.wait_background();
    let data = c.sound_data();
    assert!(!data.stale);
    let m = data.matched.unwrap();
    assert!(m.of.starts_with("Patch on"), "{}", m.of);
    assert!(
        m.overall < first_score,
        "{} then {}",
        first_score,
        m.overall
    );
    assert!(m.level.value > 3.0, "the slow attack shows: {m:?}");
}

#[test]
fn an_unpitched_file_gets_the_noise_voice_and_no_pitch_curve() {
    let tmp = Tmp::new();
    let mut c = core();
    analysed(&mut c, &tmp.file("hat.wav", hat()));
    let data = c.sound_data();
    assert!(data.pitch.is_none());
    assert!(data.numbers[2].contains("unpitched"), "{:?}", data.numbers);
    assert!(data.matched.unwrap().pitch.is_none());
    c.make_sound_patch().unwrap();
    let device = c.sound_patch_device().unwrap().clone();
    let d = c.doc().device(&device).unwrap();
    assert_eq!(d.params.get("noise.level").copied(), Some(1.0));
}

#[test]
fn a_file_that_cannot_be_analysed_leaves_nothing_and_says_why() {
    let tmp = Tmp::new();
    let mut c = core();
    let before = c.doc().clone();
    // Not audio.
    let junk = tmp.0.join("junk.wav");
    std::fs::write(&junk, b"not a wav").unwrap();
    c.analyse_sound_file(&junk).expect("the worker starts");
    c.wait_background();
    assert!(c.sound_analysis().is_none());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Sound analysis failed")),
        "{:?}",
        c.last_rejection()
    );
    // Silence.
    let silent = tmp.file("silent.wav", vec![0.0; 22_050]);
    c.analyse_sound_file(&silent).unwrap();
    c.wait_background();
    assert!(c.sound_analysis().is_none());
    assert!(c.last_rejection().is_some_and(|m| m.contains("silent")));
    // A file that is not there.
    c.analyse_sound_file(&tmp.0.join("gone.wav")).unwrap();
    c.wait_background();
    assert!(c.sound_analysis().is_none());
    assert_eq!(*c.doc(), before);
    assert!(!c.sound_data().can_make);
}

#[test]
fn a_stopped_analysis_leaves_no_result_and_a_new_project_drops_the_old_one() {
    let tmp = Tmp::new();
    let mut c = core();
    let path = tmp.file("pluck.wav", pluck());
    // Eight seconds take the analysis long enough that the stop is seen between its stages.
    let long: Vec<f32> = pluck()
        .iter()
        .cycle()
        .take(8 * RATE as usize)
        .copied()
        .collect();
    let long_path = tmp.file("long.wav", long);
    c.analyse_sound_file(&long_path).unwrap();
    c.cancel_sound();
    c.wait_background();
    assert!(!c.sound_running());
    assert!(c.sound_analysis().is_none());
    assert!(c.last_rejection().is_some_and(|m| m.contains("cancelled")));
    analysed(&mut c, &path);
    assert!(c.sound_analysis().is_some());
    c.new_document();
    assert!(c.sound_analysis().is_none());
    assert!(c.sound_patch_device().is_none());
    assert!(c.sound_data().file.is_none());
}

#[test]
fn make_patch_is_refused_while_a_job_runs() {
    let tmp = Tmp::new();
    let mut c = core();
    let path = tmp.file("pluck.wav", pluck());
    analysed(&mut c, &path);
    let before = c.doc().clone();
    c.analyse_sound_file(&path).unwrap();
    assert!(c.make_sound_patch().is_err());
    assert!(!c.sound_data().can_make);
    c.wait_background();
    assert_eq!(*c.doc(), before);
}

#[test]
fn the_note_names_follow_the_midi_keys() {
    assert_eq!(sound::note_name(57.0), "A3");
    assert_eq!(sound::note_name(60.2), "C4");
    assert_eq!(sound::note_name(45.0), "A2");
    assert_eq!(sound::note_name(61.0), "C#4");
}
