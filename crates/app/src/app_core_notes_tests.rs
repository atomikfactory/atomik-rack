//! Audio to Notes in the app core: a worker reads the clip, the notes arrive as one note clip in
//! one undo step, and every refusal says why. The fixtures are procedural files in temporary
//! folders (a sine melody and a drum pattern with known notes and hits).

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{AudioClipContent, ClipContent, WarpSettings, empty_doc};
use atomik_ui::{ImportTarget, NotesMode, NotesValues};
use std::sync::atomic::{AtomicU64, Ordering};

const SR: u32 = 48_000;
/// 120 BPM: a second is two beats.
const SECOND: i64 = 2 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-notes-{tag}-{}-{n}", std::process::id()));
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

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.sink_mut().take();
    c
}

fn write(dir: &Tmp, name: &str, data: Vec<f32>) -> PathBuf {
    let path = dir.0.join(name);
    write_wav_f32(&path, &AudioData::new(SR, vec![data]).unwrap()).unwrap();
    path
}

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(SR)).round() as usize
}

fn hz(key: u8) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

/// Sine notes `(key, from, to)` in seconds with 5 ms fades, `seconds` long.
fn sines(seconds: f64, notes: &[(u8, f64, f64)]) -> Vec<f32> {
    let mut x = vec![0.0f32; frames(seconds)];
    for &(key, from, to) in notes {
        let (a, b) = (frames(from), frames(to));
        let fade = frames(0.005) as f64;
        for (i, v) in x[a..b].iter_mut().enumerate() {
            let env = (i as f64 / fade).min(((b - a - i) as f64) / fade).min(1.0);
            let t = i as f64 / f64::from(SR);
            *v += (0.5 * env * (std::f64::consts::TAU * hz(key) * t).sin()) as f32;
        }
    }
    x
}

const MELODY: [(u8, f64, f64); 3] = [(60, 0.1, 0.5), (64, 0.6, 1.0), (67, 1.1, 1.5)];

/// Noise in `-1..1` from a seeded xorshift.
fn noise(state: &mut u32) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    f64::from(*state) / f64::from(u32::MAX) * 2.0 - 1.0
}

/// A kick at 0.1 s, a snare at 0.5 s and a closed hat at 0.9 s in 1.4 s of silence.
fn drums() -> Vec<f32> {
    let mut x = vec![0.0f32; frames(1.4)];
    let mut rng = 12_345u32;
    let mut phase = 0.0f64;
    for i in 0..frames(0.4) {
        let t = i as f64 / f64::from(SR);
        phase += std::f64::consts::TAU * (50.0 + 100.0 * (-t / 0.03).exp()) / f64::from(SR);
        x[frames(0.1) + i] += (0.9 * (-t / 0.12).exp() * phase.sin()) as f32;
    }
    let k = (-std::f64::consts::TAU * 3000.0 / f64::from(SR)).exp();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for i in 0..frames(0.4) {
        let t = i as f64 / f64::from(SR);
        s1 = noise(&mut rng) * (1.0 - k) + s1 * k;
        s2 = s1 * (1.0 - k) + s2 * k;
        let body = 0.3 * (std::f64::consts::TAU * 190.0 * t).sin() * (-t / 0.05).exp();
        x[frames(0.5) + i] += (0.8 * (body + 3.0 * s2 * (-t / 0.07).exp())) as f32;
    }
    let (mut p1, mut p2) = (0.0f64, 0.0f64);
    for i in 0..frames(0.1) {
        let t = i as f64 / f64::from(SR);
        let n = noise(&mut rng);
        let hp = n - 2.0 * p1 + p2;
        (p2, p1) = (p1, n);
        x[frames(0.9) + i] += (0.6 * 0.25 * hp * (-t / 0.012).exp()) as f32;
    }
    x
}

/// Imports `path` on a new track at tick 0 and selects its clip.
fn import(c: &mut AppCore<RecordingSink>, path: PathBuf) -> ClipId {
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    id
}

fn pitched() -> NotesValues {
    NotesValues::default()
}

/// The notes of the last track's first clip as `(tick, length, key)`.
fn last_notes(c: &AppCore<RecordingSink>) -> Vec<(i64, i64, u8)> {
    let clip = &c.doc().song.tracks.last().unwrap().lanes[0].clips[0];
    let ClipContent::Notes(n) = &clip.content else {
        panic!("not a note clip")
    };
    n.iter().map(|n| (n.tick, n.length, n.key)).collect()
}

fn audio(c: &AppCore<RecordingSink>, id: ClipId) -> AudioClipContent {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn set_audio(c: &mut AppCore<RecordingSink>, id: ClipId, a: AudioClipContent) {
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id,
        start: clip.start,
        length: clip.length,
        muted: false,
        content: a,
        merge: None,
    }))
    .unwrap();
}

#[test]
fn a_melody_becomes_a_note_clip_on_a_new_track_in_one_undo_step() {
    let t = Tmp::new("melody");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let before = c.doc().clone();
    let audio_clip = c.doc().song.clip(id).unwrap().2.clone();
    c.sink_mut().take();
    assert!(c.audio_to_notes(&pitched()));
    assert!(c.notes_busy());
    c.wait_background();
    assert!(!c.notes_busy());
    crate::sink::assert_new_devices_routed(&c.sink_mut().take());
    let doc = c.doc();
    assert_eq!(doc.song.tracks.len(), before.song.tracks.len() + 1);
    let track = doc.song.tracks.last().unwrap();
    let name = &doc.device(&track.device).unwrap().name;
    assert!(name.ends_with("notes"), "{name}");
    let clip = &track.lanes[0].clips[0];
    assert_eq!(
        (clip.start, clip.length, clip.offset),
        (audio_clip.start, audio_clip.length, 0),
        "lined up with the audio clip"
    );
    let notes = last_notes(&c);
    let keys: Vec<u8> = notes.iter().map(|n| n.2).collect();
    assert_eq!(keys, vec![60, 64, 67], "{notes:?}");
    for (n, (_, from, to)) in notes.iter().zip(MELODY) {
        let (want_on, want_len) = (
            (from * SECOND as f64) as i64,
            ((to - from) * SECOND as f64) as i64,
        );
        assert!(
            (n.0 - want_on).abs() < SECOND / 25,
            "start {n:?} for {from}"
        );
        assert!(
            (n.1 - want_len).abs() < SECOND / 12,
            "length {n:?} for {from}"
        );
    }
    assert_eq!(
        c.arrangement().selected_clips(),
        vec![clip.id],
        "the new clip is selected"
    );
    let said = c.last_rejection().unwrap();
    assert!(said.contains("3 notes on"), "{said}");
    // One undo step removes the track and its clip; redo brings them back.
    c.undo();
    assert_eq!(c.doc(), &before);
    c.redo();
    assert_eq!(last_notes(&c).len(), 3);
}

#[test]
fn the_notes_can_go_on_the_selected_note_track() {
    let t = Tmp::new("target");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let lead = DeviceKey("lead".to_owned());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: IMPORT_INSTRUMENT.to_owned(),
        key: Some(lead.clone()),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: lead.clone(),
    }))
    .unwrap();
    let values = NotesValues {
        new_track: false,
        ..pitched()
    };
    // No note track selected: refused, nothing started.
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(!c.audio_to_notes(&values));
    assert!(c.last_rejection().unwrap().contains("Select a note track"));
    assert!(!c.notes_busy());
    let tracks = c.doc().song.tracks.len();
    c.view_mut().set_selected(Some(lead.clone()));
    let before = c.doc().clone();
    assert!(c.audio_to_notes(&values));
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), tracks, "no new track");
    let clip = &c.doc().song.track(&lead).unwrap().lanes[0].clips[0];
    assert_eq!(clip.start, 0);
    let ClipContent::Notes(n) = &clip.content else {
        panic!()
    };
    assert_eq!(n.len(), 3);
    c.undo();
    assert_eq!(c.doc(), &before);
}

#[test]
fn a_drum_loop_gets_kick_snare_and_hat_on_the_general_midi_keys() {
    let t = Tmp::new("drums");
    let mut c = core();
    import(&mut c, write(&t, "Drums.wav", drums()));
    let values = NotesValues {
        mode: NotesMode::Drums,
        ..pitched()
    };
    assert!(c.audio_to_notes(&values));
    c.wait_background();
    let notes = last_notes(&c);
    let keys: Vec<u8> = notes.iter().map(|n| n.2).collect();
    assert_eq!(keys, vec![36, 38, 42], "{notes:?}");
    for (n, at) in notes.iter().zip([0.1, 0.5, 0.9]) {
        assert!(
            (n.0 - (at * SECOND as f64) as i64).abs() < SECOND / 100,
            "{n:?}"
        );
    }
    let name = c
        .doc()
        .device(&c.doc().song.tracks.last().unwrap().device)
        .unwrap()
        .name
        .clone();
    assert!(name.ends_with("drums"), "{name}");
}

#[test]
fn beats_timing_rounds_the_starts_to_the_grid() {
    let t = Tmp::new("grid");
    let mut c = core();
    import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let values = NotesValues {
        grid: 16,
        ..pitched()
    };
    assert!(c.audio_to_notes(&values));
    c.wait_background();
    let notes = last_notes(&c);
    assert_eq!(notes.len(), 3);
    for n in &notes {
        assert_eq!(n.0 % (PPQ / 4), 0, "{n:?} on the 1/16 grid");
    }
}

#[test]
fn a_clip_that_starts_later_with_an_offset_lines_up_in_the_song() {
    let t = Tmp::new("offset");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    // Trim the first 0.5 s and move the clip to bar 2.
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    let mut a = audio(&c, id);
    a.source_offset = frames(0.5) as u64;
    let start = 4 * PPQ;
    c.apply_command(Command::Song(SongCommand::SetClipGeometry {
        id,
        start,
        length: clip.length - SECOND / 2,
        offset: 0,
        loop_length: None,
        muted: false,
        merge: None,
    }))
    .unwrap();
    set_audio(&mut c, id, a);
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(c.audio_to_notes(&pitched()));
    c.wait_background();
    let track = c.doc().song.tracks.last().unwrap();
    assert_eq!(track.lanes[0].clips[0].start, start);
    let notes = last_notes(&c);
    // 0.6 s into the file is 0.1 s into the clip.
    assert_eq!(
        notes.iter().map(|n| n.2).collect::<Vec<_>>(),
        vec![64, 67],
        "{notes:?}"
    );
    assert!((notes[0].0 - SECOND / 10).abs() < SECOND / 25, "{notes:?}");
}

#[test]
fn a_reversed_clip_is_read_as_it_plays() {
    let t = Tmp::new("reverse");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let mut a = audio(&c, id);
    a.reverse = true;
    set_audio(&mut c, id, a);
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(c.audio_to_notes(&pitched()));
    c.wait_background();
    let notes = last_notes(&c);
    let keys: Vec<u8> = notes.iter().map(|n| n.2).collect();
    assert_eq!(keys, vec![67, 64, 60], "{notes:?}");
    // The last note of the file (1.1 to 1.5 s of 2 s) plays first, at 0.5 s.
    assert!((notes[0].0 - SECOND / 2).abs() < SECOND / 25, "{notes:?}");
}

#[test]
fn a_transposed_clip_adds_its_transposition_to_the_keys() {
    let t = Tmp::new("transpose");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let mut a = audio(&c, id);
    a.warp = WarpSettings {
        transpose_cents: 200,
        ..WarpSettings::default()
    };
    set_audio(&mut c, id, a);
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(c.audio_to_notes(&pitched()));
    c.wait_background();
    let keys: Vec<u8> = last_notes(&c).iter().map(|n| n.2).collect();
    assert_eq!(keys, vec![62, 66, 69]);
}

#[test]
fn a_clip_warped_in_repitch_mode_is_refused() {
    let t = Tmp::new("repitch");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let mut a = audio(&c, id);
    a.warp = WarpSettings {
        enabled: true,
        mode: atomik_model::WarpMode::Repitch,
        ..WarpSettings::default()
    };
    set_audio(&mut c, id, a);
    c.arrangement_mut().set_selected_clips(&[id]);
    let tracks = c.doc().song.tracks.len();
    assert!(!c.audio_to_notes(&pitched()));
    assert!(c.last_rejection().unwrap().contains("Repitch"));
    assert_eq!(c.doc().song.tracks.len(), tracks);
    assert!(!c.notes_busy());
}

#[test]
fn a_beats_warped_clip_goes_through_its_time_map() {
    let t = Tmp::new("warped");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    // The file was "recorded at 60 BPM" and plays at 120: everything is twice as fast.
    let mut a = audio(&c, id);
    a.warp = WarpSettings {
        enabled: true,
        mode: atomik_model::WarpMode::Beats,
        source_bpm: 60.0,
        ..WarpSettings::default()
    };
    set_audio(&mut c, id, a);
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(c.audio_to_notes(&pitched()));
    c.wait_background();
    let notes = last_notes(&c);
    assert_eq!(
        notes.iter().map(|n| n.2).collect::<Vec<_>>(),
        vec![60, 64, 67],
        "{notes:?}"
    );
    // The first note starts 0.1 s into the file: 0.05 s of song.
    assert!((notes[0].0 - SECOND / 20).abs() < SECOND / 40, "{notes:?}");
    assert!(
        (notes[1].0 - SECOND * 3 / 10).abs() < SECOND / 40,
        "{notes:?}"
    );
}

#[test]
fn nothing_found_and_other_refusals_say_why_and_change_nothing() {
    let t = Tmp::new("refuse");
    let mut c = core();
    let id = import(&mut c, write(&t, "Silence.wav", vec![0.0; frames(1.0)]));
    // Nothing selected.
    c.arrangement_mut().set_selected_clips(&[]);
    assert!(!c.audio_to_notes(&pitched()));
    assert!(c.last_rejection().unwrap().contains("Select an audio clip"));
    assert_eq!(c.audio_notes_target(), (String::new(), false));
    // Silence.
    c.arrangement_mut().set_selected_clips(&[id]);
    let before = c.doc().clone();
    assert!(c.audio_to_notes(&pitched()));
    c.wait_background();
    assert!(c.last_rejection().unwrap().contains("No notes found"));
    assert_eq!(c.doc(), &before);
    assert!(!c.notes_busy());
    // The target names the file; a second request while one runs is refused.
    let (text, ok) = c.audio_notes_target();
    assert!(text.contains("Silence") && ok, "{text}");
    assert!(c.audio_to_notes(&pitched()));
    assert!(!c.audio_notes_target().1, "busy");
    assert!(!c.audio_to_notes(&pitched()));
    assert!(c.last_rejection().unwrap().contains("already running"));
    c.wait_background();
}

#[test]
fn a_clip_that_changed_meanwhile_drops_the_result() {
    let t = Tmp::new("changed");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    assert!(c.audio_to_notes(&pitched()));
    // Edit the clip before the result is applied (no frame has run: the worker's message waits).
    let mut a = audio(&c, id);
    a.gain_db = -6.0;
    set_audio(&mut c, id, a);
    let after_edit = c.doc().clone();
    c.wait_background();
    assert!(c.last_rejection().unwrap().contains("changed while"));
    assert_eq!(c.doc(), &after_edit, "nothing was added");
}

#[test]
fn a_long_clip_is_read_only_up_to_the_cap() {
    let melody = sines(4.0, &[(60, 0.1, 0.5), (64, 0.6, 1.0), (67, 3.0, 3.5)]);
    let asset = SongAsset::memory(std::sync::Arc::from(vec![melody]));
    let v = pitched();
    let all = analyse(&asset, (0, frames(4.0) as u64), false, (SR, 10), &v);
    assert!(!all.truncated);
    assert_eq!(all.notes.len(), 3);
    let cut = analyse(&asset, (0, frames(4.0) as u64), false, (SR, 2), &v);
    assert!(cut.truncated);
    assert_eq!(cut.notes.len(), 2, "only the first two seconds were read");
    // A reversed clip reads the end of its span first.
    let rev = analyse(&asset, (0, frames(4.0) as u64), true, (SR, 2), &v);
    assert!(rev.truncated);
    assert_eq!(rev.notes.len(), 1);
    assert_eq!(rev.notes[0].key, 67);
}

#[test]
fn the_notes_go_on_the_selected_lane_of_the_selected_note_track() {
    let t = Tmp::new("lane");
    let mut c = core();
    let id = import(&mut c, write(&t, "Melody.wav", sines(2.0, &MELODY)));
    let lead = DeviceKey("lead".to_owned());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: IMPORT_INSTRUMENT.to_owned(),
        key: Some(lead.clone()),
    })
    .unwrap();
    for command in [
        SongCommand::AddTrack {
            device: lead.clone(),
        },
        SongCommand::AddNoteLane {
            device: lead.clone(),
            name: None,
        },
    ] {
        c.apply_command(Command::Song(command)).unwrap();
    }
    let values = NotesValues {
        new_track: false,
        ..pitched()
    };
    c.arrangement_mut().set_selected_clips(&[id]);
    c.view_mut().set_selected(Some(lead.clone()));
    c.arrangement_mut().set_selected_lane(&lead, 1);
    assert!(c.audio_to_notes(&values));
    c.wait_background();
    let track = c.doc().song.track(&lead).unwrap();
    assert!(track.lanes[0].clips.is_empty());
    assert_eq!(track.lanes[1].clips.len(), 1);
}
