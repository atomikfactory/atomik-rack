//! Reference tracks in the app core: the list as undo steps, the switch with its start position,
//! the level match and the settings that reach the engine.

use super::reference_ops::section_lufs;
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

// ------------------------------------------------------------------ seek follow

/// The tick of `beats` quarter notes (120 BPM: half a second each).
fn beat(beats: i64) -> i64 {
    beats * atomik_core::PPQ
}

fn starts(c: &AppCore<RecordingSink>) -> usize {
    reference_calls(c)
        .iter()
        .filter(|call| matches!(call, SinkCall::ReferenceStart(_)))
        .count()
}

#[test]
fn a_seek_moves_a_reference_that_follows_the_playhead_to_the_same_song_position() {
    let tmp = Tmp::new("seek");
    let path = tmp.tone("long.wav", 0.1, 12);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    c.transport_action(TransportAction::Locate(beat(4)));
    c.toggle_reference();
    assert!(c.reference_listening());
    assert_eq!(starts(&c), 1);
    assert!((c.reference_start_seconds().unwrap() - 2.0).abs() < 1e-3);
    // Seeking while it plays: a new stream from the new position, and it keeps listening.
    c.transport_action(TransportAction::Locate(beat(12)));
    assert_eq!(starts(&c), 2);
    assert!((c.reference_start_seconds().unwrap() - 6.0).abs() < 1e-3);
    c.transport_action(TransportAction::Locate(beat(1)));
    assert!((c.reference_start_seconds().unwrap() - 0.5).abs() < 1e-3);
    assert!(c.reference_listening());
    // Past the end of the file it ends there, no further.
    c.transport_action(TransportAction::Locate(beat(100)));
    assert!((c.reference_start_seconds().unwrap() - 12.0).abs() < 1e-3);
    // The settings in force travel with the new stream.
    c.set_reference_trim(-20);
    c.transport_action(TransportAction::Locate(beat(2)));
    let last = reference_calls(&c)
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::ReferenceStart(s) => Some(s),
            _ => None,
        });
    let last = last.expect("started");
    assert!(last.listening);
    assert!(
        (20.0 * last.gain.log10() + 2.0).abs() < 0.01,
        "{}",
        last.gain
    );
}

#[test]
fn a_seek_leaves_a_reference_with_a_cue_and_the_mix_alone() {
    let tmp = Tmp::new("seek-cue");
    let path = tmp.tone("long.wav", 0.1, 12);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    // The mix is heard: a seek sends nothing to the reference.
    c.transport_action(TransportAction::Locate(beat(6)));
    assert_eq!(starts(&c), 0);
    // A cue is not time-aligned with the song: the reference stays at its cue.
    c.set_reference_cue(0, true);
    c.toggle_reference();
    assert_eq!(starts(&c), 1);
    let at = c.reference_start_seconds().unwrap();
    assert!((at - 3.0).abs() < 1e-3, "{at}");
    c.transport_action(TransportAction::Locate(beat(2)));
    assert_eq!(starts(&c), 1, "no new stream");
    assert!((c.reference_start_seconds().unwrap() - 3.0).abs() < 1e-3);
    // Back to the mix: no position to report.
    c.toggle_reference();
    assert_eq!(c.reference_start_seconds(), None);
}

/// Plays the transport at `tick` having wrapped `laps` times, and runs a frame.
fn playing_at(c: &mut AppCore<RecordingSink>, tick: i64, laps: u64) {
    let state = &mut c.sink_mut().state;
    state.playing = true;
    state.engine_frame = 1;
    state.tick = tick;
    state.loop_count = laps;
    state.loop_enabled = true;
    c.frame();
}

#[test]
fn the_reference_goes_back_with_the_playhead_at_the_loop_wrap() {
    let tmp = Tmp::new("loop");
    let path = tmp.tone("long.wav", 0.1, 12);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    c.apply_command(Command::Song(atomik_model::SongCommand::SetLoop {
        range: (beat(4), beat(12)),
        enabled: true,
    }))
    .unwrap();
    c.transport_action(TransportAction::Locate(beat(4)));
    c.toggle_reference();
    assert_eq!(starts(&c), 1);
    // The song plays through the loop: the reference plays on by itself.
    playing_at(&mut c, beat(4) + 10, 0);
    playing_at(&mut c, beat(9), 0);
    playing_at(&mut c, beat(11), 0);
    assert_eq!(starts(&c), 1, "no wrap, no new stream");
    // The playhead wraps: the engine counts a lap and the reference starts at the loop start.
    playing_at(&mut c, beat(4) + 20, 1);
    assert_eq!(starts(&c), 2);
    let at = c.reference_start_seconds().unwrap();
    assert!(
        (at - 2.0).abs() < 0.05,
        "back at the loop start, not at {at}"
    );
    assert!(c.reference_listening());
    // The same lap is not counted twice; the next one is.
    playing_at(&mut c, beat(6), 1);
    assert_eq!(starts(&c), 2);
    playing_at(&mut c, beat(4) + 20, 2);
    assert_eq!(starts(&c), 3);
}

#[test]
fn a_wrap_leaves_a_reference_with_a_cue_and_the_mix_alone() {
    let tmp = Tmp::new("loop-cue");
    let path = tmp.tone("long.wav", 0.1, 12);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    // Listening to the mix: a lap sends nothing to the reference.
    playing_at(&mut c, beat(5), 0);
    playing_at(&mut c, beat(4), 1);
    assert_eq!(starts(&c), 0);
    // A reference with a cue is not time-aligned: it stays where it is.
    c.set_reference_cue(0, true);
    c.toggle_reference();
    assert_eq!(starts(&c), 1);
    playing_at(&mut c, beat(6), 1);
    playing_at(&mut c, beat(4), 2);
    assert_eq!(starts(&c), 1, "no new stream for a cue");
    // A stopped song is not looping: a count that moved while it stood still is no wrap.
    c.toggle_reference();
    c.set_reference_cue(0, false);
    c.toggle_reference();
    let before = starts(&c);
    c.sink_mut().state.playing = false;
    c.sink_mut().state.loop_count = 9;
    c.frame();
    assert_eq!(starts(&c), before);
}

// ------------------------------------------------------------------ trim

#[test]
fn the_trim_is_added_to_the_matched_gain_one_undo_step_and_saved_with_the_reference() {
    let tmp = Tmp::new("trim");
    let path = tmp.tone("loud.wav", 0.2, 5);
    let mut c = core();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    assert!((c.reference_info().trim_db).abs() < f32::EPSILON);
    let sent_gain = |c: &AppCore<RecordingSink>| {
        let last = reference_calls(c)
            .into_iter()
            .rev()
            .find_map(|call| match call {
                SinkCall::ReferenceSet(s) => Some(s),
                _ => None,
            });
        20.0 * last.expect("sent").gain.log10()
    };
    // The match first: a mix at -18 LUFS.
    c.sink_mut().mix_lufs = Some(-18.0);
    for _ in 0..16 {
        c.frame();
    }
    let matched = c.reference_info().gain_db.expect("matched");
    // 0.1 dB steps, in either direction, and the gain the voice is told follows.
    let depth = c.document().undo_depth();
    c.step_reference_trim(15);
    assert!((c.reference_info().trim_db - 1.5).abs() < 1e-6);
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    assert!((sent_gain(&c) - (matched + 1.5)).abs() < 0.01);
    c.step_reference_trim(-1);
    assert!((c.reference_info().trim_db - 1.4).abs() < 1e-6);
    // The shown matched gain is the match alone: the trim is its own number.
    assert!((c.reference_info().gain_db.unwrap() - matched).abs() < 1e-6);
    // The limits are 12 dB either way.
    c.set_reference_trim(500);
    assert!((c.reference_info().trim_db - 12.0).abs() < 1e-6);
    c.set_reference_trim(-500);
    assert!((c.reference_info().trim_db + 12.0).abs() < 1e-6);
    c.set_reference_trim(-25);
    // Saved with the reference, and the same after a load.
    let text = atomik_model::save_project(c.doc());
    assert!(text.contains("\"trim_db\": -2.5"), "{text}");
    let back = atomik_model::load_project(&text, c.registry()).unwrap();
    assert_eq!(back.references, c.doc().references);
    assert_eq!(back.references[0].trim_tenths, -25);
    // Undo walks it back.
    c.undo();
    assert!((c.reference_info().trim_db + 12.0).abs() < 1e-6);
    // With the match off the trim still moves the reference from its own level.
    c.set_reference_level_match(false);
    c.set_reference_trim(30);
    assert!((sent_gain(&c) - 3.0).abs() < 0.01, "{}", sent_gain(&c));
}

#[test]
fn a_trim_without_a_reference_says_so() {
    let mut c = core();
    c.set_reference_trim(10);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Add a reference"))
    );
    assert!(c.reference_info().items.is_empty());
}

// ------------------------------------------------------------------ section match

const BAR: i64 = 4 * atomik_core::PPQ;

fn sustained(start: i64, length: i64, keys: &[u8]) -> atomik_model::Clip {
    atomik_model::Clip {
        id: atomik_model::ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: atomik_model::ClipContent::Notes(
            keys.iter()
                .map(|&k| atomik_model::Note::new(0, length, k, 0.9))
                .collect::<Vec<_>>()
                .into(),
        ),
    }
}

/// Two bars at -24 dB of a held chord, then two bars of the same chord at -6 dB.
fn two_levels() -> AppCore<RecordingSink> {
    use atomik_model::{
        CreateOptions, CreateSource, SongCommand, WorkflowCommand, new_project_doc,
    };
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    for (k, level, start) in [("quiet", -24.0, 0), ("loud", -6.0, 2 * BAR)] {
        let key = atomik_model::DeviceKey(k.to_owned());
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key.clone()),
            options: CreateOptions::default(),
        }))
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 0.0),
            ("master.level", level),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
        ] {
            c.apply_command(Command::SetParam {
                key: key.clone(),
                param: p.into(),
                value: v,
                merge: None,
            })
            .unwrap();
        }
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key,
            lane: 0,
            clip: sustained(start, 2 * BAR, &[48, 55]),
        }))
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::SetSongEnd(4 * BAR)))
        .unwrap();
    c
}

/// Runs frames until the section measurement is done.
#[allow(clippy::disallowed_methods)] // a real worker is waited for in real time
fn wait_matched(c: &mut AppCore<RecordingSink>) {
    let start = Instant::now();
    while c.reference_info().matching.is_some() {
        c.frame();
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the measurement did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    c.frame();
}

/// The integrated loudness of the master over `[from, to)` ticks, rendered here the same way.
fn mix_lufs(c: &AppCore<RecordingSink>, from: i64, to: i64) -> f32 {
    use atomik_render::project::{PluginBounce, ProjectOptions};
    let registry = atomik_render::default_registry();
    let options = ProjectOptions::default();
    let mut bounce = PluginBounce::default();
    let audio = atomik_render::energy::render_master_span(
        c.doc(),
        &registry,
        (&options, None),
        &mut bounce,
        atomik_render::export::ExportSpan::Range(from, to),
        Default::default(),
    )
    .unwrap()
    .expect("rendered");
    let frames = audio.channels[0].len();
    section_lufs(&audio, 0, frames).expect("audible")
}

#[test]
fn the_section_loudness_of_two_sine_files_is_the_level_difference() {
    let rate = 48_000u32;
    let tone = |amp: f32| -> AudioData {
        let ch: Vec<f32> = (0..rate as usize * 3)
            .map(|n| amp * (std::f32::consts::TAU * 1000.0 * n as f32 / rate as f32).sin())
            .collect();
        AudioData::new(rate, vec![ch.clone(), ch]).unwrap()
    };
    let (loud, quiet) = (tone(0.4), tone(0.1));
    let a = section_lufs(&loud, 0, 3 * rate as usize).unwrap();
    let b = section_lufs(&quiet, 0, 3 * rate as usize).unwrap();
    // Four times the amplitude is 12.04 dB.
    assert!((a - b - 12.04).abs() < 0.05, "{a} {b}");
    // Only the frames asked for count, and too short a part reads nothing.
    assert!((section_lufs(&loud, rate as usize, rate as usize).unwrap() - a).abs() < 0.05);
    assert_eq!(section_lufs(&loud, 0, 100), None);
    assert_eq!(
        section_lufs(
            &AudioData::new(rate, vec![vec![0.0; 96_000]]).unwrap(),
            0,
            96_000
        ),
        None
    );
}

#[test]
fn the_section_match_uses_the_time_selection_and_sets_the_gain_from_both_loudnesses() {
    let tmp = Tmp::new("section");
    let path = tmp.tone("ref.wav", 0.1, 10);
    let mut c = two_levels();
    c.add_reference(&path).unwrap();
    wait_loaded(&mut c);
    let ref_lufs = c.reference_lufs_over(0.0, 4.0).expect("audible");
    // Without a selection the loop locators (the whole song) are the section.
    assert_eq!(c.reference_section(), Some((0, 4 * BAR)));
    // The quiet half.
    c.arrangement_mut()
        .set_lane_range(atomik_model::DeviceKey("quiet".to_owned()), 0, 0, 2 * BAR);
    c.match_reference_section().expect("starts");
    assert!(c.reference_info().matching.is_some());
    assert!(c.match_reference_section().is_err(), "one at a time");
    wait_matched(&mut c);
    let quiet = c.reference_info().gain_db.expect("matched");
    let expect_quiet = mix_lufs(&c, 0, 2 * BAR) - ref_lufs;
    assert!((quiet - expect_quiet).abs() < 0.1, "{quiet} {expect_quiet}");
    assert!(c.reference_info().section_held);
    // The loud half, with the loop locators set to the quiet half: the selection wins.
    c.apply_command(Command::Song(atomik_model::SongCommand::SetLoop {
        range: (0, 2 * BAR),
        enabled: true,
    }))
    .unwrap();
    c.arrangement_mut().set_lane_range(
        atomik_model::DeviceKey("loud".to_owned()),
        0,
        2 * BAR,
        4 * BAR,
    );
    assert_eq!(c.reference_section(), Some((2 * BAR, 4 * BAR)));
    c.match_reference_section().expect("starts");
    wait_matched(&mut c);
    let loud = c.reference_info().gain_db.expect("matched");
    let expect_loud = mix_lufs(&c, 2 * BAR, 4 * BAR) - ref_lufs;
    assert!((loud - expect_loud).abs() < 0.1, "{loud} {expect_loud}");
    // The levels of the two tracks are 18 dB apart.
    assert!((loud - quiet - 18.0).abs() < 1.0, "{loud} {quiet}");
    // The slow match of the loudest section leaves a section gain alone ...
    c.sink_mut().mix_lufs = Some(-30.0);
    for _ in 0..40 {
        c.frame();
    }
    assert!((c.reference_info().gain_db.unwrap() - loud).abs() < 1e-6);
    // ... until the match is switched off and on again.
    c.set_reference_level_match(false);
    c.set_reference_level_match(true);
    assert!(!c.reference_info().section_held);
    // Without a selection the loop locators are the section.
    c.arrangement_mut().clear_lane_range();
    assert_eq!(c.reference_section(), Some((0, 2 * BAR)));
    c.match_reference_section().expect("starts");
    wait_matched(&mut c);
    let located = c.reference_info().gain_db.unwrap();
    assert!((located - quiet).abs() < 0.1, "{located} {quiet}");
    // The match is the gain the voice is told, and the trim goes on top of it.
    c.set_reference_trim(20);
    let sent = reference_calls(&c)
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::ReferenceSet(s) => Some(s),
            _ => None,
        });
    assert!((20.0 * sent.expect("sent").gain.log10() - (located + 2.0)).abs() < 0.01);
}

#[test]
fn a_section_match_needs_a_decoded_reference_and_a_long_enough_section() {
    let tmp = Tmp::new("section-refuse");
    let path = tmp.tone("ref.wav", 0.1, 10);
    let mut c = two_levels();
    // No reference.
    c.arrangement_mut()
        .set_lane_range(atomik_model::DeviceKey("quiet".to_owned()), 0, 0, BAR);
    assert!(c.match_reference_section().is_err());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Add a reference"))
    );
    // Not decoded yet.
    c.add_reference(&path).unwrap();
    assert!(c.match_reference_section().is_err());
    wait_loaded(&mut c);
    // A section of a fifth of a beat is too short to read.
    c.arrangement_mut().set_lane_range(
        atomik_model::DeviceKey("quiet".to_owned()),
        0,
        0,
        atomik_core::PPQ / 5,
    );
    let e = c.match_reference_section().unwrap_err();
    assert!(e.contains("silent or too short"), "{e}");
    assert_eq!(c.reference_info().gain_db, None);
}
