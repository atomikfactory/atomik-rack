//! Audio to notes in the model: frames of the analysed audio become ticks through the tempo map
//! or the warp time map, a note clip lines up with its audio clip, and the clip goes in as one
//! undo step (a new track through `ImportMidi`, or `AddClip` on a note track) without changing
//! the file format.

mod common;

use atomik_model::{
    AudioClipContent, ClipContent, Command, ControllerKind, Curve, FoundBend, FoundNote,
    ImportedTrack, MidiImport, NoteOptions, SongCommand, TempoEvent, WarpMarker, WarpSettings,
    WorkflowCommand, load_project, notes_clip, save_project,
};
use common::song::*;
use common::*;

const SR: u32 = 48_000;
/// 120 BPM at 48 kHz: a beat (PPQ ticks) is 24 000 frames.
const BEAT_F: u64 = 24_000;

fn found(start: u64, length: u64, key: u8) -> FoundNote {
    FoundNote {
        start,
        length,
        key,
        velocity: 0.8,
        bend: Vec::new(),
    }
}

fn audio() -> AudioClipContent {
    common::audio::audio(1)
}

fn notes_of(c: &atomik_model::Clip) -> Vec<(i64, i64, u8)> {
    let ClipContent::Notes(n) = &c.content else {
        panic!("not a note clip")
    };
    n.iter().map(|n| (n.tick, n.length, n.key)).collect()
}

fn song_at(bpm: f64) -> atomik_model::Song {
    atomik_model::empty_song(bpm)
}

#[test]
fn notes_line_up_with_the_audio_at_a_constant_tempo() {
    let song = song_at(120.0);
    let found = [found(0, BEAT_F, 60), found(BEAT_F, BEAT_F / 2, 64)];
    let c = notes_clip(
        &song,
        SR,
        (BAR, 2 * BAR),
        &audio(),
        &found,
        NoteOptions::default(),
    )
    .unwrap();
    assert_eq!((c.start, c.length, c.offset), (BAR, 2 * BAR, 0));
    assert_eq!(notes_of(&c), vec![(0, PPQ, 60), (PPQ, PPQ / 2, 64)]);
}

#[test]
fn the_offset_into_the_asset_does_not_move_the_notes() {
    let song = song_at(120.0);
    let a = AudioClipContent {
        source_offset: 31_337,
        ..audio()
    };
    let c = notes_clip(
        &song,
        SR,
        (0, BAR),
        &a,
        &[found(BEAT_F, BEAT_F, 60)],
        NoteOptions::default(),
    )
    .unwrap();
    assert_eq!(notes_of(&c), vec![(PPQ, PPQ, 60)]);
}

#[test]
fn a_tempo_change_under_the_clip_moves_the_notes_with_the_audio() {
    let mut song = song_at(120.0);
    song.tempo = vec![TempoEvent::step(0, 120.0), TempoEvent::step(BAR, 60.0)];
    // Four beats at 120 BPM take 96 000 frames; after them a beat is 48 000 frames.
    let found = [
        found(96_000, 48_000, 60),
        found(96_000 + 72_000, 24_000, 62),
    ];
    let c = notes_clip(
        &song,
        SR,
        (0, 2 * BAR),
        &audio(),
        &found,
        NoteOptions::default(),
    )
    .unwrap();
    assert_eq!(
        notes_of(&c),
        vec![(BAR, PPQ, 60), (BAR + 3 * PPQ / 2, PPQ / 2, 62)]
    );
}

#[test]
fn a_warped_clip_goes_through_its_time_map() {
    let song = song_at(120.0);
    // A source at 60 BPM warped to 120: a source beat (48 000 frames) plays in one song beat.
    let a = AudioClipContent {
        warp: WarpSettings {
            enabled: true,
            source_bpm: 60.0,
            ..Default::default()
        },
        ..audio()
    };
    let found = [found(48_000, 24_000, 60), found(96_000, 48_000, 62)];
    let c = notes_clip(&song, SR, (0, BAR), &a, &found, NoteOptions::default()).unwrap();
    assert_eq!(notes_of(&c), vec![(PPQ, PPQ / 2, 60), (2 * PPQ, PPQ, 62)]);
    // A marker that holds the second source beat for two song beats.
    let marked = AudioClipContent {
        warp: WarpSettings {
            markers: vec![
                WarpMarker { tick: 0, source: 0 },
                WarpMarker {
                    tick: 3 * PPQ,
                    source: 96_000,
                },
            ],
            ..a.warp.clone()
        },
        ..a
    };
    let c = notes_clip(&song, SR, (0, BAR), &marked, &found, NoteOptions::default()).unwrap();
    // Source 48 000..72 000 sits halfway in the first stretch; the second note is past the
    // marker, where a source beat is a song beat again.
    assert_eq!(
        notes_of(&c),
        vec![(3 * PPQ / 2, 3 * PPQ / 4, 60), (3 * PPQ, PPQ, 62)]
    );
}

#[test]
fn beats_timing_rounds_starts_to_the_grid_and_keeps_lengths() {
    let song = song_at(120.0);
    let grid = PPQ / 4;
    // 3900 and 5800 ticks: 61 and 91 grid steps' worth of frames.
    let to_frames = |ticks: i64| (ticks as f64 * 24_000.0 / PPQ as f64).round() as u64;
    let found = [
        found(to_frames(3_900), to_frames(1_000), 60),
        found(to_frames(5_800), to_frames(900), 62),
    ];
    let c = notes_clip(
        &song,
        SR,
        (0, BAR),
        &audio(),
        &found,
        NoteOptions {
            snap: grid,
            transpose: 0,
        },
    )
    .unwrap();
    let n = notes_of(&c);
    assert_eq!(n[0].0, grid);
    assert_eq!(n[1].0, 2 * grid, "5800 rounds to 7680 / 3840 = 2 steps");
    assert!(
        (n[0].1 - 1_000).abs() <= 1 && (n[1].1 - 900).abs() <= 1,
        "{n:?}"
    );
    let free = notes_clip(
        &song,
        SR,
        (0, BAR),
        &audio(),
        &found,
        NoteOptions::default(),
    )
    .unwrap();
    assert!((notes_of(&free)[0].0 - 3_900).abs() <= 1);
}

#[test]
fn snapping_never_overlaps_notes_of_one_key_and_merges_the_same_tick() {
    let song = song_at(120.0);
    let grid = PPQ / 4;
    let f = |tick: i64, len: i64, key: u8| {
        let fr = |t: i64| (t as f64 * 24_000.0 / PPQ as f64).round() as u64;
        found(fr(tick), fr(len), key)
    };
    // Two notes of key 60 whose starts both round to tick 0, a long one that runs into the next
    // 60 (starting at 4000 -> 3840), and a 62 that overlaps them (other keys may overlap).
    let found = [
        f(100, 3_000, 60),
        f(900, 500, 60),
        f(3_000, 6_000, 60),
        f(4_000, 500, 62),
    ];
    let c = notes_clip(
        &song,
        SR,
        (0, BAR),
        &audio(),
        &found,
        NoteOptions {
            snap: grid,
            transpose: 0,
        },
    )
    .unwrap();
    let n = notes_of(&c);
    assert_eq!(n.iter().filter(|n| n.2 == 60).count(), 2, "{n:?}");
    let sixty: Vec<_> = n.iter().filter(|n| n.2 == 60).collect();
    assert_eq!(sixty[0].0, 0);
    assert!(sixty[0].0 + sixty[0].1 <= sixty[1].0, "{n:?}");
    assert!(n.iter().any(|n| n.2 == 62));
}

#[test]
fn transposition_clamps_and_the_clip_bounds_the_notes() {
    let song = song_at(120.0);
    let found = [
        found(0, BEAT_F, 126),
        found(BEAT_F, 10 * BEAT_F, 3),
        found(4 * BEAT_F, BEAT_F, 60),
    ];
    let c = notes_clip(
        &song,
        SR,
        (0, 2 * PPQ),
        &audio(),
        &found,
        NoteOptions {
            snap: 0,
            transpose: 5,
        },
    )
    .unwrap();
    assert_eq!(notes_of(&c), vec![(0, PPQ, 127), (PPQ, PPQ, 8)]);
    let c = notes_clip(
        &song,
        SR,
        (0, 2 * PPQ),
        &audio(),
        &[found[1].clone()],
        NoteOptions {
            snap: 0,
            transpose: -12,
        },
    )
    .unwrap();
    assert_eq!(notes_of(&c)[0].2, 0);
}

#[test]
fn nothing_inside_the_clip_is_an_error() {
    let song = song_at(120.0);
    let none = notes_clip(&song, SR, (0, PPQ), &audio(), &[], NoteOptions::default());
    assert!(none.unwrap_err().contains("no notes"));
    let late = notes_clip(
        &song,
        SR,
        (0, PPQ),
        &audio(),
        &[found(5 * BEAT_F, BEAT_F, 60)],
        NoteOptions::default(),
    );
    assert!(late.is_err());
}

#[test]
fn a_non_finite_velocity_is_repaired() {
    let song = song_at(120.0);
    let mut f = found(0, BEAT_F, 60);
    f.velocity = f32::NAN;
    let c = notes_clip(&song, SR, (0, PPQ), &audio(), &[f], NoteOptions::default()).unwrap();
    let ClipContent::Notes(n) = &c.content else {
        panic!()
    };
    assert!(n[0].velocity > 0.0 && n[0].velocity <= 1.0);
}

fn imported(d: &atomik_model::Document, found: &[FoundNote]) -> atomik_model::Clip {
    notes_clip(
        &d.doc().song,
        SR,
        (BAR, 2 * BAR),
        &audio(),
        found,
        NoteOptions::default(),
    )
    .unwrap()
}

#[test]
fn a_new_track_with_the_notes_is_one_undo_step() {
    let reg = registry();
    let mut d = song_start(&reg);
    let before = d.doc().clone();
    let clip = imported(&d, &[found(0, BEAT_F, 60), found(BEAT_F, BEAT_F, 64)]);
    let import = MidiImport {
        instrument: SYNTH.to_owned(),
        tracks: vec![ImportedTrack {
            name: "Vocal notes".to_owned(),
            clip: Some(clip),
            controllers: Vec::new(),
        }],
        ..MidiImport::default()
    };
    d.apply(Command::Workflow(WorkflowCommand::ImportMidi(import)), &reg)
        .unwrap();
    let doc = d.doc();
    assert_eq!(doc.song.tracks.len(), before.song.tracks.len() + 1);
    let track = doc.song.tracks.last().unwrap();
    let c = &track.lanes[0].clips[0];
    assert_eq!((c.start, c.length), (BAR, 2 * BAR));
    assert_eq!(notes_of(c), vec![(0, PPQ, 60), (PPQ, PPQ, 64)]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "one undo removes the track and the clip");
    d.redo().unwrap();
    assert_eq!(d.doc().song.tracks.len(), before.song.tracks.len() + 1);
}

#[test]
fn the_clip_goes_onto_a_note_track_in_one_step_and_the_format_is_unchanged() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    let before = d.doc().clone();
    let clip = imported(&d, &[found(0, BEAT_F, 60)]);
    d.apply(
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip,
        }),
        &reg,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    assert_eq!(
        notes_of(d.doc().song.clip(id).unwrap().2),
        vec![(0, PPQ, 60)]
    );
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

// ------------------------------------------------------------------ pitch bend

fn bent(start: u64, length: u64, key: u8, bend: &[(u64, f32)]) -> FoundNote {
    FoundNote {
        bend: bend
            .iter()
            .map(|&(frame, semitones)| FoundBend { frame, semitones })
            .collect(),
        ..found(start, length, key)
    }
}

/// The points of the clip's pitch-bend lane as `(tick, value, holds)`.
fn bend_points(c: &atomik_model::Clip) -> Vec<(i64, f32, bool)> {
    assert!(c.controllers.len() <= 1);
    c.controllers.first().map_or_else(Vec::new, |l| {
        assert_eq!(l.kind, ControllerKind::PitchBend);
        l.points
            .iter()
            .map(|p| (p.tick, p.value, p.curve == Curve::Step))
            .collect()
    })
}

fn clip_of(found: &[FoundNote], options: NoteOptions) -> atomik_model::Clip {
    notes_clip(&song_at(120.0), SR, (0, 2 * BAR), &audio(), found, options).unwrap()
}

#[test]
fn no_contour_means_no_lane() {
    let c = clip_of(&[found(0, BEAT_F, 60)], NoteOptions::default());
    assert!(c.controllers.is_empty());
}

#[test]
fn a_contour_becomes_a_pitch_bend_lane_scaled_to_the_bend_range() {
    // A note 0.5 sharp that glides to 1 semitone sharp, then one flat by 0.25.
    let c = clip_of(
        &[
            bent(
                0,
                BEAT_F,
                60,
                &[(0, 0.5), (BEAT_F / 2, 0.5), (BEAT_F - 1, 1.0)],
            ),
            bent(2 * BEAT_F, BEAT_F, 62, &[(2 * BEAT_F, -0.25)]),
        ],
        NoteOptions::default(),
    );
    let pts = bend_points(&c);
    let q = |v: f32| ControllerKind::PitchBend.quantize(v);
    assert_eq!(pts.len(), 5, "{pts:?}");
    assert_eq!((pts[0].0, pts[0].1, pts[0].2), (0, q(0.25), false));
    assert_eq!((pts[1].0, pts[1].1), (PPQ / 2, q(0.25)));
    assert_eq!(
        (pts[2].0, pts[2].2),
        (PPQ - 1, true),
        "the last point holds"
    );
    assert!((pts[2].1 - 0.5).abs() < 1e-3);
    assert_eq!((pts[3].0, pts[3].1, pts[3].2), (2 * PPQ, q(-0.125), true));
    // The bend returns to the centre where the last note ends.
    assert_eq!((pts[4].0, pts[4].1), (3 * PPQ, 0.0));
    for w in pts.windows(2) {
        assert!(w[0].0 < w[1].0, "{pts:?}");
    }
}

#[test]
fn the_contour_starts_with_its_note_and_the_next_note_without_one_recentres() {
    // The first contour point is a little late; the second note has no contour.
    let c = clip_of(
        &[
            bent(0, 2 * BEAT_F, 60, &[(BEAT_F / 4, 0.4), (BEAT_F, 0.4)]),
            found(2 * BEAT_F, BEAT_F, 64),
        ],
        NoteOptions::default(),
    );
    let pts = bend_points(&c);
    assert_eq!(pts[0].0, 0, "the first point is moved to the note start");
    assert_eq!(pts.last().map(|p| (p.0, p.1)), Some((2 * PPQ, 0.0)));
    assert_eq!(pts.len(), 4, "{pts:?}");
    assert_eq!(pts[1].0, PPQ / 4, "the late point keeps its place");
}

#[test]
fn a_snapped_note_takes_its_contour_with_it() {
    // The note starts 30 ticks before the 16th grid line; its contour moves the same distance.
    let sixteenth = PPQ / 4;
    let early = (sixteenth - 30) as u64 * BEAT_F / PPQ as u64;
    let c = clip_of(
        &[bent(
            early,
            BEAT_F,
            60,
            &[(early, 0.2), (early + BEAT_F / 2, 0.6)],
        )],
        NoteOptions {
            snap: sixteenth,
            ..NoteOptions::default()
        },
    );
    let n = notes_of(&c);
    assert_eq!(n[0].0, sixteenth);
    let pts = bend_points(&c);
    assert_eq!(pts[0].0, sixteenth);
    assert!(pts[1].0 > sixteenth + PPQ / 2 - 5 && pts[1].0 < sixteenth + PPQ / 2 + 5);
}

#[test]
fn points_outside_the_clip_or_not_finite_are_dropped() {
    // A point after the clip end is dropped; non-finite points never get in.
    let c = clip_of(
        &[bent(
            0,
            BEAT_F,
            60,
            &[(0, 0.3), (BEAT_F / 2, f32::NAN), (50 * BEAT_F, 0.3)],
        )],
        NoteOptions::default(),
    );
    let pts = bend_points(&c);
    assert_eq!(pts.len(), 2, "{pts:?}");
    assert!(pts.iter().all(|p| p.0 < 2 * BAR));
}

#[test]
fn a_contour_beyond_the_range_is_clamped_and_the_clip_validates() {
    let reg = registry();
    let c = clip_of(
        &[bent(0, BEAT_F, 60, &[(0, 5.0), (BEAT_F / 2, -9.0)])],
        NoteOptions::default(),
    );
    let pts = bend_points(&c);
    assert_eq!((pts[0].1, pts[1].1), (1.0, -1.0));
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: c,
        }),
        &reg,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(
        loaded.song.clip(id).unwrap().2.controllers.len(),
        1,
        "the lane survives a save"
    );
}
