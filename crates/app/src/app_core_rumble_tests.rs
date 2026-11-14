//! Rumble Maker and Reverse Reverb Print in the app core: the renders run on the freeze worker,
//! the layer starts in phase with the kick and is ducked under its hits, the drum machine counts
//! its kick pad alone, the swell ends exactly where its source starts, and every result is one
//! undo step. The fixtures are procedural projects in a scratch bundle.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_dsp::rumble::RumbleMode;
use atomik_engine::SongAsset;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, TempoEvent, WorkflowCommand,
    insert_chain, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const DRUMS: &str = "atomik.drum-machine";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn notes(start: i64, length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
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
        content: ClipContent::Notes(notes.into()),
    }
}

fn set(c: &mut AppCore<RecordingSink>, device: &str, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key(device),
        param: param.to_owned(),
        value,
        merge: None,
    })
    .unwrap();
}

fn instrument(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(type_id.into()),
        key: Some(key(k)),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.apply_command(Command::Rename {
        key: key(k),
        name: k[..1].to_uppercase() + &k[1..],
    })
    .unwrap();
}

/// A drum machine `kick` whose first pad plays a synthesised kick and whose third plays a hat,
/// with a clip of one bar: a kick (key 36) on every beat and a hat (key 38) between them when
/// `hats` is set.
fn drums(hats: bool) -> AppCore<RecordingSink> {
    let mut hits: Vec<Note> = Vec::new();
    for beat in 0..4 {
        hits.push(Note::new(beat * PPQ, PPQ / 4, 36, 0.9));
        if hats {
            hits.push(Note::new(beat * PPQ + PPQ / 2, PPQ / 8, 38, 0.8));
        }
    }
    drum_core(hits)
}

/// [`drums`] with these notes in the clip.
fn drum_core(mut hits: Vec<Note>) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    instrument(&mut c, DRUMS, "kick");
    set(&mut c, "kick", "pad1.module", 1.0);
    set(&mut c, "kick", "pad3.module", 3.0);
    hits.sort_by_key(|n| (n.tick, n.key));
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("kick"),
        lane: 0,
        clip: notes(0, BAR, hits),
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

/// A project with a synth `lead` with a clip of two notes from tick `at`.
fn lead(at: i64) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    instrument(&mut c, "atomik.poly-synth", "lead");
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: notes(
            at,
            BAR,
            vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 67, 0.8)],
        ),
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

fn select_track(c: &mut AppCore<RecordingSink>, k: &str) {
    c.arrangement_mut().set_selected_track(Some(key(k)));
}

/// The stereo audio of an asset in the pool.
fn samples(c: &AppCore<RecordingSink>, hash: &str) -> Vec<Vec<f32>> {
    let SongAsset::Memory { data, .. } = &c.song_assets()[hash] else {
        panic!("the render is not in memory");
    };
    data.to_vec()
}

fn clip_audio(c: &AppCore<RecordingSink>, clip: &Clip) -> Vec<Vec<f32>> {
    let ClipContent::Audio(a) = &clip.content else {
        panic!("not an audio clip");
    };
    samples(c, &a.asset)
}

/// The lag within `max` frames at which `b` agrees best with `a`, and the correlation there: a
/// positive lag means `b` comes late.
fn best_lag(a: &[f32], b: &[f32], max: isize) -> (isize, f32) {
    let n = a.len() as isize;
    (-max..=max)
        .map(|lag| {
            let (from, to) = (lag.max(0), (n + lag).min(n));
            let x: Vec<f32> = (from..to).map(|i| a[(i - lag) as usize]).collect();
            let y: Vec<f32> = (from..to).map(|i| b[i as usize]).collect();
            (lag, correlation(&x, &y))
        })
        .fold((0, f32::MIN), |best, c| if c.1 > best.1 { c } else { best })
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let ea: f32 = a.iter().map(|x| x * x).sum();
    let eb: f32 = b.iter().map(|x| x * x).sum();
    dot / (ea * eb).sqrt()
}

/// The audio track after `device` (the one the last command added).
fn track_after<'a>(c: &'a AppCore<RecordingSink>, device: &str) -> &'a atomik_model::Track {
    let tracks = &c.doc().song.tracks;
    let at = tracks.iter().position(|t| t.device == key(device)).unwrap();
    &tracks[at + 1]
}

/// The kick of `c` as the instrument plays it alone: a dry bounce of its clip.
fn dry_kick(c: &mut AppCore<RecordingSink>) -> Vec<Vec<f32>> {
    let clip = c.doc().song.track(&key("kick")).unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[clip]);
    c.bounce_clips_dry();
    c.wait_background();
    let bounced = track_after(c, "kick").lanes[0].clips[0].clone();
    let audio = clip_audio(c, &bounced);
    c.undo();
    audio
}

#[test]
fn a_rumble_in_either_mode_starts_in_phase_with_the_kick_and_is_one_undo_step() {
    for mode in [RumbleMode::TailRepeat, RumbleMode::Reverb] {
        let mut c = drums(false);
        let kick = dry_kick(&mut c);
        let tracks = c.doc().song.tracks.len();
        select_track(&mut c, "kick");
        c.rumble_maker(mode);
        assert!(c.freeze_busy() && c.is_busy());
        assert!(
            c.freeze_progress()
                .is_some_and(|t| t.starts_with("Building a rumble from Kick")),
            "{:?}",
            c.freeze_progress()
        );
        c.wait_background();
        assert_eq!(c.doc().song.tracks.len(), tracks + 1, "{mode:?}");
        let track = track_after(&c, "kick");
        assert!(track.is_audio());
        assert_eq!(c.doc().device(&track.device).unwrap().name, "Kick Rumble");
        let clip = track.lanes[0].clips[0].clone();
        assert_eq!(clip.start, 0, "it starts where the kick does");
        let rumble = clip_audio(&c, &clip);
        // Every hit starts with the kick's own sub band, in phase: the two agree best at no lag.
        let beat = 24_000;
        for hit in 0..4 {
            let from = hit * beat;
            let w = 3_840; // 80 ms
            let (lag, r) = best_lag(&kick[0][from..from + w], &rumble[0][from..from + w], 480);
            assert!(lag.abs() <= 8, "{mode:?} hit {hit}: best lag {lag}");
            assert!(r > 0.6, "{mode:?} hit {hit}: correlation {r}");
        }
        // It rings on after the kick: at the end of the beat there is sound under the kick's.
        let late = beat - 4_800..beat - 480;
        assert!(
            rms(&rumble[0][late.clone()]) > 2.0 * rms(&kick[0][late]),
            "{mode:?}"
        );
        // The layer is no louder than the kick.
        let peak = |a: &[Vec<f32>]| a.iter().flatten().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak(&rumble) <= peak(&kick) * 1.001, "{mode:?}");
        // The new clip is selected and the status line says what was made.
        assert_eq!(c.arrangement().selected_clips(), vec![clip.id]);
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.starts_with("Rumble from Kick")),
            "{:?}",
            c.last_rejection()
        );
        // Its channel holds the duck, the low-pass and the EQ.
        let channel = c
            .doc()
            .console
            .channel_of(&track.device)
            .unwrap()
            .channel
            .clone();
        let chain = insert_chain(c.doc(), &c.registry, &channel);
        let types: Vec<&str> = chain
            .iter()
            .map(|k| c.doc().device(k).unwrap().type_id.as_str())
            .collect();
        assert_eq!(types, ["atomik.duck", "atomik.auto-filter", "atomik.eq"]);
        c.undo();
        assert_eq!(c.doc().song.tracks.len(), tracks, "one undo step");
    }
}

#[test]
fn the_rumble_is_ducked_under_every_kick_hit() {
    let mut c = drums(false);
    select_track(&mut c, "kick");
    c.rumble_maker(RumbleMode::TailRepeat);
    c.wait_background();
    let rumble_track = track_after(&c, "kick").clone();
    let rumble = clip_audio(&c, &rumble_track.lanes[0].clips[0]);
    // The rumble through its own chain, with the kick playing as the key.
    c.arrangement_mut()
        .set_selected_clips(&[rumble_track.lanes[0].clips[0].id]);
    c.bounce_in_place(false);
    c.wait_background();
    let through = clip_audio(
        &c,
        &track_after(&c, &rumble_track.device.0).lanes[0].clips[0].clone(),
    );
    let beat = 24_000;
    // How much of the rumble is left in a window, with the chain's own level change cancelled by
    // comparing two windows of the same beat.
    let share = |from: usize, len: usize| {
        rms(&through[0][from..from + len]) / rms(&rumble[0][from..from + len])
    };
    for hit in 1..4 {
        let at = hit * beat;
        let ducked = share(at + 480, 960);
        let recovered = share(at + 15_000, 4_800);
        assert!(
            ducked < 0.4 * recovered,
            "hit {hit}: {ducked} under the hit, {recovered} later"
        );
    }
}

#[test]
fn a_drum_machine_makes_its_rumble_from_the_kick_pad_alone() {
    let rumble_of = |hats: bool| {
        let mut c = drums(hats);
        select_track(&mut c, "kick");
        c.rumble_maker(RumbleMode::TailRepeat);
        c.wait_background();
        let clip = track_after(&c, "kick").lanes[0].clips[0].clone();
        clip_audio(&c, &clip)
    };
    // The hat pad plays between the kicks: the layer is the one the kicks make alone.
    assert!(rumble_of(true) == rumble_of(false));
}

#[test]
fn the_two_modes_make_different_layers() {
    let make = |mode| {
        let mut c = drums(false);
        select_track(&mut c, "kick");
        c.rumble_maker(mode);
        c.wait_background();
        let clip = track_after(&c, "kick").lanes[0].clips[0].clone();
        clip_audio(&c, &clip)
    };
    let (tail, reverb) = (make(RumbleMode::TailRepeat), make(RumbleMode::Reverb));
    assert!(tail != reverb);
    // The reverb layer is wide: its sides differ.
    let side: Vec<f32> = reverb[0]
        .iter()
        .zip(&reverb[1])
        .map(|(a, b)| a - b)
        .collect();
    assert!(rms(&side) > 1.0e-3, "{}", rms(&side));
    // The tail repeat of a mono kick is not.
    let side: Vec<f32> = tail[0].iter().zip(&tail[1]).map(|(a, b)| a - b).collect();
    assert!(rms(&side) < 1.0e-3, "{}", rms(&side));
}

#[test]
fn rumble_maker_says_why_it_cannot_work() {
    let mut c = drum_core(vec![Note::new(0, PPQ / 8, 38, 0.8)]);
    let tracks = c.doc().song.tracks.len();
    // Nothing is selected.
    c.arrangement_mut().set_selected_track(None);
    c.view.set_selection(&[], None);
    c.rumble_maker(RumbleMode::TailRepeat);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Select a kick track")),
        "{:?}",
        c.last_rejection()
    );
    assert!(!c.freeze_busy());
    // The kick has no hit: the notes are all on other pads.
    select_track(&mut c, "kick");
    c.rumble_maker(RumbleMode::TailRepeat);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("plays no kick hit")),
        "{:?}",
        c.last_rejection()
    );
    assert!(!c.freeze_busy());
    // A track with no clips.
    let mut empty = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    instrument(&mut empty, "atomik.poly-synth", "lead");
    select_track(&mut empty, "lead");
    empty.rumble_maker(RumbleMode::Reverb);
    assert!(
        empty
            .last_rejection()
            .is_some_and(|m| m.contains("no clips")),
        "{:?}",
        empty.last_rejection()
    );
    assert_eq!(c.doc().song.tracks.len(), tracks, "nothing was added");
}

#[test]
fn a_rumble_of_a_track_that_changed_meanwhile_is_thrown_away() {
    let mut c = drums(false);
    select_track(&mut c, "kick");
    c.rumble_maker(RumbleMode::TailRepeat);
    // The tempo changes while the kick renders: the audio would not match any more.
    c.apply_command(Command::SetTempo(97.0)).unwrap();
    c.wait_background();
    assert_eq!(
        c.doc().song.tracks.len(),
        1,
        "no rumble track: {:?}",
        c.last_rejection()
    );
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("changed while it was rendered")),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn the_hits_of_an_audio_kick_are_found_in_the_audio() {
    // Four kicks, a beat apart, as audio.
    let beat = 24_000;
    let mut kick = vec![0.0_f32; 4 * beat];
    for hit in 0..4 {
        for i in 0..6_000 {
            let t = i as f32 / 48_000.0;
            kick[hit * beat + i] =
                0.9 * (-t / 0.08).exp() * (2.0 * std::f32::consts::PI * 55.0 * t).sin();
        }
    }
    let audio = atomik_audio_file::AudioData::new(48_000, vec![kick.clone(), kick]).unwrap();
    let params = atomik_dsp::rumble::RumbleParams::default();
    let found = crate::freeze_worker::rumble_of(&audio, None, &params).unwrap();
    // A piece per hit: sound at the end of every beat that the kick itself no longer has.
    for hit in 0..4 {
        let late = hit * beat + beat - 4_800..hit * beat + beat - 480;
        assert!(rms(&found.channels[0][late.clone()]) > 5.0e-3, "hit {hit}");
        assert!(rms(&audio.channels[0][late]) < 1.0e-3);
    }
    // Silence, and no hit given, are refused.
    let silent = atomik_audio_file::AudioData::new(48_000, vec![vec![0.0; 48_000]; 2]).unwrap();
    assert!(crate::freeze_worker::rumble_of(&silent, None, &params).is_err());
    assert!(crate::freeze_worker::rumble_of(&audio, Some(&[]), &params).is_err());
}

// ---------------------------------------------------------------- reverse reverb print

/// The frame of tick `tick` in `c`'s song.
fn frame(c: &AppCore<RecordingSink>, tick: i64) -> u64 {
    c.doc().song.frame_at(tick, c.doc().sample_rate)
}

#[test]
fn the_swell_of_a_clip_ends_at_the_clip_start_and_is_one_undo_step() {
    let mut c = lead(BAR);
    let source = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].clone();
    let tracks = c.doc().song.tracks.len();
    c.arrangement_mut().set_selected_clips(&[source.id]);
    c.reverse_reverb_print(1.0);
    assert!(
        c.freeze_progress()
            .is_some_and(|t| t.starts_with("Printing a reverse reverb of Lead")),
        "{:?}",
        c.freeze_progress()
    );
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), tracks + 1);
    let track = track_after(&c, "lead");
    assert_eq!(
        c.doc().device(&track.device).unwrap().name,
        "Lead (reverse reverb)"
    );
    let clip = track.lanes[0].clips[0].clone();
    // The print ends exactly at the first frame of the source.
    let end = frame(&c, clip.end());
    let start = frame(&c, source.start);
    assert!(
        end.abs_diff(start) <= 1,
        "ends at {end}, the source starts at {start}"
    );
    assert!(clip.start < source.start);
    // The swell is about a second long (whole ticks that fit in it) and rises toward its end.
    let audio = clip_audio(&c, &clip);
    let frames = audio[0].len();
    assert!((46_000..=48_000).contains(&frames), "{frames} frames");
    assert_eq!(frames as u64, frame(&c, clip.end()) - frame(&c, clip.start));
    let tenth = frames / 10;
    assert!(rms(&audio[0][frames - tenth..]) > 3.0 * rms(&audio[0][..tenth]));
    assert_eq!(c.arrangement().selected_clips(), vec![clip.id]);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Printed a reverse reverb of Lead")),
        "{:?}",
        c.last_rejection()
    );
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), tracks, "one undo step");
}

#[test]
fn the_swell_of_a_range_and_of_an_audio_clip_end_at_their_start_too() {
    // A range: the locators around the clip.
    let mut c = lead(BAR);
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (2 * BAR, 2 * BAR + PPQ),
        enabled: false,
    }))
    .unwrap();
    select_track(&mut c, "lead");
    // The clip plays from BAR to 2 BAR: nothing plays in the range.
    c.reverse_reverb_print(2.0);
    assert!(!c.freeze_busy());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("nothing in the range")),
        "{:?}",
        c.last_rejection()
    );
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (BAR + PPQ, BAR + 2 * PPQ),
        enabled: false,
    }))
    .unwrap();
    c.reverse_reverb_print(2.0);
    c.wait_background();
    let clip = track_after(&c, "lead").lanes[0].clips[0].clone();
    let (end, start) = (frame(&c, clip.end()), frame(&c, BAR + PPQ));
    assert!(end.abs_diff(start) <= 1, "{end} {start}");

    // An audio clip: bounce the lead, then print the reverse reverb of the audio.
    let mut c = lead(BAR);
    let source = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[source]);
    c.bounce_in_place(false);
    c.wait_background();
    let audio_clip = track_after(&c, "lead").lanes[0].clips[0].clone();
    let device = track_after(&c, "lead").device.0.clone();
    c.arrangement_mut().set_selected_clips(&[audio_clip.id]);
    c.reverse_reverb_print(0.5);
    c.wait_background();
    let printed = track_after(&c, &device).lanes[0].clips[0].clone();
    let (end, start) = (frame(&c, printed.end()), frame(&c, audio_clip.start));
    assert!(end.abs_diff(start) <= 1, "{end} {start}");
    assert_eq!(
        c.doc()
            .device(&track_after(&c, &device).device)
            .unwrap()
            .name,
        "Lead (bounce) (reverse reverb)"
    );
}

#[test]
fn a_swell_longer_than_the_room_before_the_source_is_cut_to_fit() {
    // The clip starts half a second into the song: a four second swell fits half of it.
    let mut c = lead(PPQ);
    let source = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].clone();
    c.arrangement_mut().set_selected_clips(&[source.id]);
    c.reverse_reverb_print(4.0);
    c.wait_background();
    let clip = track_after(&c, "lead").lanes[0].clips[0].clone();
    assert_eq!((clip.start, clip.length), (0, PPQ));
    assert!(frame(&c, clip.end()).abs_diff(frame(&c, PPQ)) <= 1);
    // The swell's last half second: it still rises to its end.
    let audio = clip_audio(&c, &clip);
    let n = audio[0].len();
    assert!(rms(&audio[0][n - n / 10..]) > rms(&audio[0][..n / 10]));
}

#[test]
fn a_source_at_the_start_of_the_song_has_no_room_for_a_swell() {
    let mut c = lead(0);
    let source = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    let tracks = c.doc().song.tracks.len();
    c.arrangement_mut().set_selected_clips(&[source]);
    c.reverse_reverb_print(1.0);
    assert!(!c.freeze_busy());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("no room before it")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(c.doc().song.tracks.len(), tracks);
    // A swell outside its range, and nothing selected, are said too.
    c.reverse_reverb_print(50.0);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("between 0.2 and 20"))
    );
    let mut none = lead(BAR);
    none.arrangement_mut().set_selected_track(None);
    none.reverse_reverb_print(1.0);
    assert!(
        none.last_rejection().is_some_and(
            |m| m.contains("Select an audio clip, or a range") || m.contains("Select a track")
        ),
        "{:?}",
        none.last_rejection()
    );
}

#[test]
fn the_swell_lands_on_whole_ticks_whatever_the_tempo() {
    let mut doc = new_project_doc();
    doc.song_mut().tempo = vec![TempoEvent::step(0, 93.0), TempoEvent::step(3 * PPQ, 151.0)];
    let rate = doc.sample_rate;
    let clock = doc.song.frame_clock(rate);
    for anchor in [PPQ, 7 * PPQ + 123, 40 * PPQ] {
        for want in [4_800_u64, 48_000, 96_001] {
            let (start, length, frames) =
                crate::freeze_worker::swell_place(&doc, want, anchor).unwrap();
            assert_eq!(start + length, anchor);
            let span = clock.at(anchor) - clock.at(start);
            assert_eq!(span as usize, frames);
            // The longest that fits: a tick more would not (unless the song starts there).
            assert!(span <= want);
            if start > 0 {
                assert!(
                    clock.at(anchor) - clock.at(start - 1) > want,
                    "anchor {anchor} want {want}"
                );
            }
        }
    }
    // At tick 0 there is no room.
    assert!(crate::freeze_worker::swell_place(&doc, 48_000, 0).is_err());
}
