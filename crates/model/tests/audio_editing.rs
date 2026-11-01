//! Audio editing in the model: crossfade flattening, reverse regions, the gain envelope, the
//! halves of a split, `StripSilence`, the strip-silence detector, normalise, and the format.

mod common;

use atomik_model::song::{AudioRegion, flatten_audio};
use atomik_model::{
    AudioClipContent, ClipContent, Command, Document, FadeCurve, GainPoint, ModelError,
    PROJECT_VERSION, SongCommand, StripParams, load_project, normalise_gain_db, peak, save_project,
    slice_envelope, split_envelope, strip_silence,
};
use common::audio::*;
use common::song::*;
use common::*;
use serde_json::{Value, json};

const SR: u32 = 48_000;
/// 120 BPM at 48 kHz: a beat (PPQ ticks) is 24 000 frames.
const BEAT_F: u64 = 24_000;

fn flat(d: &Document) -> Vec<AudioRegion> {
    let song = &d.doc().song;
    flatten_audio(song.track(&key("audio-track")).unwrap(), song, SR)
}

fn setup() -> (atomik_device_sdk::Registry, Document) {
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    (reg, d)
}

fn put(
    d: &mut Document,
    reg: &atomik_device_sdk::Registry,
    start: i64,
    len: i64,
    a: AudioClipContent,
) -> atomik_model::ClipId {
    d.apply(
        add_audio_clip_with("audio-track", start, len, a.clone()),
        reg,
    )
    .unwrap();
    let id = last(d);
    // `AddAudioClip` has no reverse, envelope or crossfade: set the whole content.
    edit(d, reg, id, |c| *c = a).unwrap();
    id
}

/// Replaces a clip's content (one `SetAudioClip`).
fn edit(
    d: &mut Document,
    reg: &atomik_device_sdk::Registry,
    id: atomik_model::ClipId,
    f: impl FnOnce(&mut AudioClipContent),
) -> Result<(), ModelError> {
    let c = doc_clip(d.doc(), id).clone();
    let ClipContent::Audio(mut a) = c.content else {
        panic!("not audio")
    };
    f(&mut a);
    d.apply(
        sc(SongCommand::SetAudioClip {
            id,
            start: c.start,
            length: c.length,
            muted: c.muted,
            content: a,
            merge: None,
        }),
        reg,
    )
    .map(|_| ())
}

fn audio_of(d: &Document, id: atomik_model::ClipId) -> AudioClipContent {
    match &doc_clip(d.doc(), id).content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

#[test]
fn overlapping_clips_crossfade_with_a_tail_region_of_the_earlier_one() {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    put(&mut d, &reg, PPQ, 2 * PPQ, audio(2));
    let r = flat(&d);
    assert_eq!(r.len(), 3);
    // The head of the earlier clip, its tail under the later one, the later clip.
    assert_eq!((r[0].song_start_frame, r[0].frames), (0, BEAT_F));
    assert_eq!((r[1].song_start_frame, r[1].frames), (BEAT_F, BEAT_F));
    assert_eq!((r[2].song_start_frame, r[2].frames), (BEAT_F, 2 * BEAT_F));
    assert_eq!(r[1].asset, hash(1));
    assert_eq!(
        r[1].source_offset, BEAT_F,
        "the tail continues the earlier clip"
    );
    assert_eq!((r[1].x_out, r[1].x_in), (BEAT_F, 0));
    assert_eq!((r[2].x_in, r[2].x_out), (BEAT_F, 0));
    assert_eq!((r[0].x_in, r[0].x_out), (0, 0));
    // Equal power: the two crossfade gains square-sum to 1 over the overlap.
    for i in (0..BEAT_F).step_by(997) {
        let (out, inn) = (r[1].xfade_gain(i), r[2].xfade_gain(i));
        assert!((out * out + inn * inn - 1.0).abs() < 1e-5, "frame {i}");
    }
    assert_eq!(r[2].xfade_gain(0), 0.0);
    assert_eq!(r[2].xfade_gain(BEAT_F), 1.0);
    assert_eq!(r[1].xfade_gain(0), 1.0);
}

#[test]
fn a_linear_crossfade_sums_amplitudes_to_one() {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let b = put(&mut d, &reg, PPQ, 2 * PPQ, audio(2));
    edit(&mut d, &reg, b, |a| a.xfade = Some(FadeCurve::Linear)).unwrap();
    let r = flat(&d);
    for i in (0..BEAT_F).step_by(1009) {
        assert!((r[1].xfade_gain(i) + r[2].xfade_gain(i) - 1.0).abs() < 1e-6);
    }
}

#[test]
fn a_clip_inside_another_or_without_a_crossfade_is_a_hard_cut() {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    put(&mut d, &reg, PPQ, PPQ, audio(2));
    let r = flat(&d);
    assert_eq!(r.len(), 3, "head, the covering clip, the rest");
    assert!(r.iter().all(|x| x.x_in == 0 && x.x_out == 0));

    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let b = put(&mut d, &reg, PPQ, 2 * PPQ, audio(2));
    edit(&mut d, &reg, b, |a| a.xfade = None).unwrap();
    let r = flat(&d);
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|x| x.x_in == 0 && x.x_out == 0));
}

#[test]
fn a_crossfade_stops_where_a_later_clip_covers_the_fading_in_clip() {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, 3 * PPQ, audio(1));
    put(&mut d, &reg, PPQ, 3 * PPQ, audio(2));
    // C starts half a beat into B and hides it from there.
    put(&mut d, &reg, PPQ + PPQ / 2, PPQ, audio(1));
    let r = flat(&d);
    let b_region = r.iter().find(|x| x.asset == hash(2) && x.x_in > 0).unwrap();
    assert_eq!(b_region.x_in, BEAT_F / 2, "the zone ends at C's start");
    let tail = r
        .iter()
        .find(|x| x.x_out > 0 && x.asset == hash(1))
        .unwrap();
    assert_eq!(tail.frames, BEAT_F / 2);
    // Regions never mix more than two at once: sorted by start with non-decreasing ends.
    for w in r.windows(2) {
        assert!(w[0].song_start_frame <= w[1].song_start_frame);
        assert!(w[0].song_start_frame + w[0].frames <= w[1].song_start_frame + w[1].frames);
    }
}

#[test]
fn the_fades_and_the_gain_of_both_clips_apply_on_top_of_the_crossfade() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        gain_db: -6.0,
        fade_out: BEAT_F,
        ..audio(1)
    };
    put(&mut d, &reg, 0, 2 * PPQ, a);
    put(&mut d, &reg, PPQ, 2 * PPQ, audio(2));
    let r = flat(&d);
    let g = atomik_core::db_to_gain(-6.0);
    // Halfway through the overlap the earlier clip is half way through its own fade out too.
    let i = BEAT_F / 2;
    let want = g * 0.5 * r[1].xfade_gain(i);
    assert!((r[1].total_gain(i) - want).abs() < 1e-6);
}

#[test]
fn a_reversed_clip_reads_downwards_from_the_end_of_its_span() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 1_000,
        reverse: true,
        ..audio(1)
    };
    put(&mut d, &reg, 0, PPQ, a);
    let r = flat(&d);
    assert_eq!(r.len(), 1);
    assert!(r[0].reverse);
    assert_eq!(r[0].source_offset, 1_000 + BEAT_F - 1);
    // A cover on the right leaves the head: it still starts at the span's top.
    put(&mut d, &reg, PPQ / 2, PPQ, audio(2));
    let r = flat(&d);
    assert_eq!(r[0].source_offset, 1_000 + BEAT_F - 1);
    assert_eq!(r[0].frames, BEAT_F / 2);
    // A cover in the middle leaves the head and a tail; the tail plays the lower part.
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 1_000,
        reverse: true,
        ..audio(1)
    };
    put(&mut d, &reg, 0, 2 * PPQ, a);
    put(&mut d, &reg, PPQ / 2, PPQ / 2, audio(2));
    let r = flat(&d);
    let ones: Vec<_> = r.iter().filter(|x| x.asset == hash(1)).collect();
    assert_eq!(ones.len(), 2);
    assert_eq!(ones[0].source_offset, 1_000 + 2 * BEAT_F - 1);
    assert_eq!(ones[1].clip_pos, BEAT_F);
    assert_eq!(ones[1].source_offset, 1_000 + BEAT_F - 1);
}

#[test]
fn the_envelope_ramps_in_amplitude_between_points_and_holds_outside() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        gain_db: 0.0,
        envelope: vec![
            GainPoint {
                frame: 1_000,
                gain_db: -20.0,
            },
            GainPoint {
                frame: 3_000,
                gain_db: 0.0,
            },
        ],
        ..audio(1)
    };
    put(&mut d, &reg, 0, PPQ, a.clone());
    let r = flat(&d);
    let lo = atomik_core::db_to_gain(-20.0);
    assert_eq!(r[0].envelope, vec![(1_000, lo), (3_000, 1.0)]);
    assert_eq!(r[0].total_gain(0), lo, "held before the first point");
    assert_eq!(r[0].total_gain(5_000), 1.0, "held after the last point");
    let mid = r[0].total_gain(2_000);
    assert!((mid - (lo + 1.0) / 2.0).abs() < 1e-6);
    assert_eq!(a.envelope_gain_at(2_000), Some(mid));
    assert_eq!(audio(1).envelope_gain_at(0), None);
}

#[test]
fn an_envelope_is_validated_by_the_commands() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, PPQ, audio(1));
    let p = |frame, gain_db| GainPoint { frame, gain_db };
    for bad in [
        vec![p(5, 0.0), p(5, 1.0)],
        vec![p(9, 0.0), p(5, 1.0)],
        vec![p(1, 40.0)],
        vec![p(1, f32::NAN)],
        (0..513).map(|i| p(i, 0.0)).collect(),
    ] {
        let before = d.doc().clone();
        let e = edit(&mut d, &reg, id, |a| a.envelope = bad.clone());
        assert!(e.is_err(), "{bad:?}");
        assert_eq!(d.doc(), &before);
    }
    edit(&mut d, &reg, id, |a| {
        a.envelope = vec![p(0, -3.0), p(10, 6.0)]
    })
    .unwrap();
    d.undo(&reg).unwrap();
    assert!(audio_of(&d, id).envelope.is_empty());
}

#[test]
fn a_split_keeps_the_envelope_sounding_the_same_in_both_halves() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        envelope: vec![
            GainPoint {
                frame: 0,
                gain_db: -12.0,
            },
            GainPoint {
                frame: 4 * BEAT_F,
                gain_db: 0.0,
            },
        ],
        ..audio(1)
    };
    let id = put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    d.apply(sc(SongCommand::SplitClip { id, at: 2 * PPQ }), &reg)
        .unwrap();
    let right = last(&d);
    let (l, r) = (audio_of(&d, id), audio_of(&d, right));
    for n in [0, 1, 5_000, BEAT_F, 2 * BEAT_F - 1] {
        let want = a.envelope_gain_at(n).unwrap();
        assert!(
            (l.envelope_gain_at(n).unwrap() - want).abs() < 1e-5,
            "left {n}"
        );
        let want = a.envelope_gain_at(2 * BEAT_F + n).unwrap();
        assert!(
            (r.envelope_gain_at(n).unwrap() - want).abs() < 1e-5,
            "right {n}"
        );
    }
    d.undo(&reg).unwrap();
    assert_eq!(audio_of(&d, id), a);
}

#[test]
fn split_envelope_helpers_handle_points_on_the_cut() {
    let p = |frame, gain_db| GainPoint { frame, gain_db };
    let env = vec![p(10, -6.0), p(20, 0.0), p(30, -3.0)];
    let (right, left) = split_envelope(&env, 20);
    assert_eq!(left, vec![p(10, -6.0), p(20, 0.0)]);
    assert_eq!(right, vec![p(0, 0.0), p(10, -3.0)]);
    assert!(split_envelope(&[], 5).0.is_empty());
    assert_eq!(slice_envelope(&env, 0, u64::MAX), env);
}

#[test]
fn splitting_a_reversed_clip_hands_the_upper_part_to_the_left_half() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 1_000,
        reverse: true,
        ..audio(1)
    };
    let id = put(&mut d, &reg, 0, 4 * PPQ, a);
    d.apply(sc(SongCommand::SplitClip { id, at: PPQ }), &reg)
        .unwrap();
    let right = last(&d);
    // The left half (1 beat) plays source [1000 + 3 beats, 1000 + 4 beats) backwards.
    assert_eq!(audio_of(&d, id).source_offset, 1_000 + 3 * BEAT_F);
    assert_eq!(audio_of(&d, right).source_offset, 1_000);
    // Played in order the two halves still read the source from the top down.
    let song = &d.doc().song;
    let t = song.track(&key("audio-track")).unwrap();
    let r = flatten_audio(t, song, SR);
    assert_eq!(r[0].source_offset, 1_000 + 4 * BEAT_F - 1);
    assert_eq!(r[1].source_offset, 1_000 + 3 * BEAT_F - 1);
}

fn silence_clip(d: &mut Document, reg: &atomik_device_sdk::Registry) -> atomik_model::ClipId {
    let a = AudioClipContent {
        source_offset: 2_000,
        fade_in: 100,
        fade_out: 200,
        gain_db: -3.0,
        ..audio(1)
    };
    put(d, reg, PPQ, 4 * PPQ, a)
}

#[test]
fn strip_silence_replaces_a_clip_by_its_pieces_in_one_undo_step() {
    let (reg, mut d) = setup();
    let id = silence_clip(&mut d, &reg);
    let before = d.doc().clone();
    let next = before.song.next_clip_id;
    let pieces = vec![
        (0, BEAT_F),
        (2 * BEAT_F, 3 * BEAT_F),
        (3 * BEAT_F, 4 * BEAT_F),
    ];
    d.apply(
        sc(SongCommand::StripSilence {
            id,
            pieces,
            fade: 48,
        }),
        &reg,
    )
    .unwrap();
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 3);
    let mut clips = lane.clips.clone();
    clips.sort_by_key(|c| c.start);
    assert_eq!(clips[0].id, id, "the first piece keeps the id");
    assert_eq!(clips[1].id.0, next);
    assert_eq!(clips[2].id.0, next + 1);
    assert_eq!(d.last_clip_id().unwrap().0, next + 1);
    assert_eq!(d.doc().song.next_clip_id, next + 2);
    assert_eq!((clips[0].start, clips[0].length), (PPQ, PPQ));
    assert_eq!((clips[1].start, clips[1].length), (3 * PPQ, PPQ));
    assert_eq!((clips[2].start, clips[2].length), (4 * PPQ, PPQ));
    let a: Vec<AudioClipContent> = clips
        .iter()
        .map(|c| match &c.content {
            ClipContent::Audio(a) => a.clone(),
            _ => panic!(),
        })
        .collect();
    // Source offsets follow the pieces; the sound stays where it was.
    assert_eq!(a[0].source_offset, 2_000);
    assert_eq!(a[1].source_offset, 2_000 + 2 * BEAT_F);
    assert_eq!(a[2].source_offset, 2_000 + 3 * BEAT_F);
    // The first piece keeps the clip's own fade in, the last its fade out; the cuts get `fade`.
    assert_eq!((a[0].fade_in, a[0].fade_out), (100, 48));
    assert_eq!((a[1].fade_in, a[1].fade_out), (48, 48));
    assert_eq!((a[2].fade_in, a[2].fade_out), (48, 200));
    assert!(a.iter().all(|x| (x.gain_db + 3.0).abs() < 1e-6));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.tracks[0].lanes[0].clips.len(), 3);
}

#[test]
fn strip_silence_on_a_reversed_clip_keeps_the_sound_where_it_was() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 500,
        reverse: true,
        ..audio(1)
    };
    let id = put(&mut d, &reg, 0, 4 * PPQ, a);
    d.apply(
        sc(SongCommand::StripSilence {
            id,
            pieces: vec![(BEAT_F, 2 * BEAT_F)],
            fade: 0,
        }),
        &reg,
    )
    .unwrap();
    let song = &d.doc().song;
    let t = song.track(&key("audio-track")).unwrap();
    let r = flatten_audio(t, song, SR);
    assert_eq!(r.len(), 1);
    // The original clip played source frame `500 + 4 beats - 1 - n` at clip frame n.
    assert_eq!(r[0].song_start_frame, BEAT_F);
    assert_eq!(r[0].source_offset, 500 + 4 * BEAT_F - 1 - BEAT_F);
}

#[test]
fn strip_silence_slices_the_envelope_and_refuses_bad_pieces() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        envelope: vec![
            GainPoint {
                frame: 0,
                gain_db: -20.0,
            },
            GainPoint {
                frame: 4 * BEAT_F,
                gain_db: 0.0,
            },
        ],
        ..audio(1)
    };
    let id = put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let before = d.doc().clone();
    for bad in [
        vec![],
        vec![(5, 5)],
        vec![(0, 4 * BEAT_F + 1)],
        vec![(10, 20), (15, 30)],
        vec![(30, 40), (0, 10)],
    ] {
        let e = d.apply(
            sc(SongCommand::StripSilence {
                id,
                pieces: bad.clone(),
                fade: 0,
            }),
            &reg,
        );
        assert!(e.is_err(), "{bad:?}");
        assert_eq!(d.doc(), &before);
    }
    d.apply(
        sc(SongCommand::StripSilence {
            id,
            pieces: vec![(2 * BEAT_F, 3 * BEAT_F)],
            fade: 0,
        }),
        &reg,
    )
    .unwrap();
    let piece = audio_of(&d, id);
    for n in [0, 100, BEAT_F - 1] {
        let want = a.envelope_gain_at(2 * BEAT_F + n).unwrap();
        assert!(
            (piece.envelope_gain_at(n).unwrap() - want).abs() < 1e-4,
            "{n}"
        );
    }
    // Not an audio clip, or no clip at all.
    assert!(
        d.apply(
            sc(SongCommand::StripSilence {
                id: atomik_model::ClipId(999),
                pieces: vec![(0, 1)],
                fade: 0,
            }),
            &reg
        )
        .is_err()
    );
}

fn tone(frames: usize, start: usize, end: usize, level: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            if (start..end).contains(&i) {
                level
            } else {
                0.0
            }
        })
        .collect()
}

#[test]
fn the_detector_cuts_long_silences_pads_and_joins() {
    // Sound in [100, 200) and [500, 600); silence elsewhere.
    let mut ch = tone(800, 100, 200, 0.5);
    for (i, s) in tone(800, 500, 600, 0.5).into_iter().enumerate() {
        ch[i] += s;
    }
    let params = |min_silence, pad_before, pad_after| StripParams {
        threshold_db: -40.0,
        min_silence,
        pad_before,
        pad_after,
    };
    let planes = [ch.clone(), ch.clone()];
    assert_eq!(
        strip_silence(&planes, 0, 800, &params(50, 0, 0)),
        vec![(100, 200), (500, 600)]
    );
    assert_eq!(
        strip_silence(&planes, 0, 800, &params(50, 10, 20)),
        vec![(90, 220), (490, 620)]
    );
    // A silence shorter than the minimum stays: 300 frames lie between the sounds.
    assert_eq!(
        strip_silence(&planes, 0, 800, &params(301, 0, 0)),
        vec![(0, 800)]
    );
    assert_eq!(
        strip_silence(&planes, 0, 800, &params(250, 0, 0)),
        vec![(0, 200), (500, 800)]
    );
    // Padding that closes the gap joins the pieces, and never leaves the span.
    assert_eq!(
        strip_silence(&planes, 0, 800, &params(50, 160, 160)),
        vec![(0, 760)]
    );
    // A sub-range reports positions relative to its start.
    assert_eq!(
        strip_silence(&planes, 90, 210, &params(5, 0, 0)),
        vec![(10, 110)]
    );
    // Silence everywhere gives nothing; the level threshold decides what is silent.
    assert!(strip_silence(&[vec![0.0; 100]], 0, 100, &params(1, 0, 0)).is_empty());
    let quiet = [tone(100, 0, 100, 0.001)];
    assert!(strip_silence(&quiet, 0, 100, &params(1, 0, 0)).is_empty());
    let p = StripParams {
        threshold_db: -80.0,
        ..params(1, 0, 0)
    };
    assert_eq!(strip_silence(&quiet, 0, 100, &p), vec![(0, 100)]);
    // Any channel above the threshold keeps a frame.
    let mixed = [vec![0.0; 100], tone(100, 40, 60, 0.9)];
    assert_eq!(
        strip_silence(&mixed, 0, 100, &params(1, 0, 0)),
        vec![(40, 60)]
    );
}

#[test]
fn peak_and_normalise() {
    let ch = [vec![0.1, -0.25, 0.2], vec![0.0, 0.5, -0.4]];
    assert_eq!(peak(&ch, 0, 3), 0.5);
    assert_eq!(peak(&ch, 0, 1), 0.1);
    assert_eq!(peak(&ch, 2, 3), 0.4);
    assert_eq!(peak(&ch, 10, 20), 0.0, "past the end is silence");
    let db = normalise_gain_db(0.5, 0.0).unwrap();
    assert!((db - 6.0206).abs() < 1e-3);
    let g = atomik_core::db_to_gain(normalise_gain_db(0.25, -1.0).unwrap());
    assert!((0.25 * g - atomik_core::db_to_gain(-1.0)).abs() < 1e-6);
    assert_eq!(normalise_gain_db(0.0, 0.0), None);
    assert_eq!(normalise_gain_db(f32::NAN, 0.0), None);
    assert_eq!(
        normalise_gain_db(1e-9, 0.0),
        Some(24.0),
        "clamped to the range"
    );
}

fn project_with(a: AudioClipContent) -> (String, atomik_device_sdk::Registry) {
    let (reg, mut d) = setup();
    put(&mut d, &reg, 0, PPQ, a);
    (save_project(d.doc()), reg)
}

#[test]
fn the_new_fields_round_trip_and_defaults_are_omitted() {
    assert_eq!(PROJECT_VERSION, 39);
    let (plain, reg) = project_with(audio(1));
    assert!(!plain.contains("reverse") && !plain.contains("envelope") && !plain.contains("xfade"));
    let rich = AudioClipContent {
        reverse: true,
        fade_curve: FadeCurve::SCurve,
        envelope: vec![
            GainPoint {
                frame: 0,
                gain_db: -6.5,
            },
            GainPoint {
                frame: 99,
                gain_db: 3.0,
            },
        ],
        xfade: Some(FadeCurve::Exponential),
        ..audio(1)
    };
    let (text, reg2) = project_with(rich.clone());
    assert!(text.contains("\"reverse\": true") && text.contains("\"fade_curve\": \"s_curve\""));
    assert!(
        text.contains("\"envelope\": [[0, -6.5], [99, 3]]"),
        "{text}"
    );
    assert!(text.contains("\"xfade\": \"exponential\""));
    let loaded = load_project(&text, &reg2).unwrap();
    let ClipContent::Audio(a) = &loaded.song.tracks[0].lanes[0].clips[0].content else {
        panic!()
    };
    assert_eq!(a, &rich);
    assert_eq!(save_project(&loaded), text, "canonical");
    let off = AudioClipContent {
        xfade: None,
        ..audio(1)
    };
    let (text, reg3) = project_with(off);
    assert!(text.contains("\"xfade\": \"off\""));
    let loaded = load_project(&text, &reg3).unwrap();
    let ClipContent::Audio(a) = &loaded.song.tracks[0].lanes[0].clips[0].content else {
        panic!()
    };
    assert_eq!(a.xfade, None);
    let _ = reg;
}

#[test]
fn older_files_load_with_hard_cuts_and_refuse_the_new_fields() {
    let (text, reg) = project_with(audio(1));
    // A version 23 file without "xfade" crossfades; a version 22 one is a hard cut.
    let v23: Value = serde_json::from_str(&text).unwrap();
    let xfade = |v: &Value| {
        let doc = load_project(&v.to_string(), &reg).unwrap();
        match &doc.song.tracks[0].lanes[0].clips[0].content {
            ClipContent::Audio(a) => a.xfade,
            _ => panic!(),
        }
    };
    assert_eq!(xfade(&v23), Some(FadeCurve::EqualPower));
    let mut v22 = v23.clone();
    v22["version"] = json!(22);
    assert_eq!(xfade(&v22), None);
    // Version 22 knows neither the new curves nor the new fields.
    for (key, value) in [
        ("reverse", json!(true)),
        ("envelope", json!([[0, 0.0]])),
        ("xfade", json!("linear")),
        ("fade_curve", json!("s_curve")),
    ] {
        let mut v = v22.clone();
        v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"][key] = value;
        let e = load_project(&v.to_string(), &reg).expect_err(key);
        assert!(
            e.to_string().contains(key) || e.to_string().contains("version 23"),
            "{e}"
        );
    }
}

#[test]
fn bad_new_fields_name_their_json_path() {
    let (text, reg) = project_with(audio(1));
    let base: Value = serde_json::from_str(&text).unwrap();
    let cases: Vec<(&str, Value, &str)> = vec![
        ("xfade", json!("sine"), "expected"),
        ("reverse", json!("yes"), "true or false"),
        (
            "envelope",
            json!([[5, 0.0], [5, 1.0]]),
            "strictly increasing",
        ),
        ("envelope", json!([[5, 90.0]]), "outside"),
        ("envelope", json!([[5]]), ""),
        ("envelope", json!("x"), ""),
    ];
    for (key, value, needle) in cases {
        let mut v = base.clone();
        v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"][key] = value;
        let e = load_project(&v.to_string(), &reg).expect_err(key);
        let msg = e.to_string();
        assert!(
            msg.contains("song.tracks[0].lanes[0].clips[0].audio"),
            "{msg}"
        );
        assert!(msg.contains(needle), "{msg}");
    }
}

#[test]
fn crossfades_lists_the_overlaps_the_regions_mix() {
    use atomik_model::song::crossfades;
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let b = put(&mut d, &reg, PPQ, 2 * PPQ, audio(2));
    // A clip inside another and a clip with no crossfade give none.
    put(&mut d, &reg, 6 * PPQ, 4 * PPQ, audio(1));
    let inside = put(&mut d, &reg, 7 * PPQ, PPQ, audio(2));
    let off = put(&mut d, &reg, 12 * PPQ, 2 * PPQ, audio(1));
    let _ = off;
    let off2 = put(&mut d, &reg, 13 * PPQ, 2 * PPQ, audio(2));
    edit(&mut d, &reg, off2, |x| x.xfade = None).unwrap();
    let lane = &d.doc().song.tracks[0].lanes[0];
    let x = crossfades(lane);
    assert_eq!(x.len(), 1, "{x:?}");
    assert_eq!(
        (x[0].later, x[0].earlier, x[0].start, x[0].end),
        (b, a, PPQ, 2 * PPQ)
    );
    assert_eq!(x[0].curve, FadeCurve::EqualPower);
    assert!(x.iter().all(|z| z.later != inside && z.later != off2));
    // The regions carry the same zone.
    let r = flat(&d);
    let xin = r.iter().find(|r| r.x_in > 0).unwrap();
    assert_eq!((xin.song_start_frame, xin.x_in), (BEAT_F, BEAT_F));
}

#[test]
fn audio_edits_batch_into_one_undo_step_with_consecutive_clip_ids() {
    let (reg, mut d) = setup();
    let a = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let b = put(&mut d, &reg, 8 * PPQ, 4 * PPQ, audio(2));
    let before = d.doc().clone();
    let next = before.song.next_clip_id;
    let mut gain = audio_of(&d, b);
    gain.gain_db = -4.0;
    let (cb, lb) = {
        let c = doc_clip(d.doc(), b);
        (c.start, c.length)
    };
    d.apply(
        Command::Batch(vec![
            sc(SongCommand::StripSilence {
                id: a,
                pieces: vec![(0, BEAT_F), (2 * BEAT_F, 3 * BEAT_F)],
                fade: 0,
            }),
            sc(SongCommand::SetAudioClip {
                id: b,
                start: cb,
                length: lb,
                muted: false,
                content: gain,
                merge: None,
            }),
            sc(SongCommand::StripSilence {
                id: b,
                pieces: vec![(BEAT_F, 2 * BEAT_F), (3 * BEAT_F, 4 * BEAT_F)],
                fade: 0,
            }),
        ]),
        &reg,
    )
    .unwrap();
    let lane = &d.doc().song.tracks[0].lanes[0];
    assert_eq!(lane.clips.len(), 4);
    let mut ids: Vec<u64> = lane.clips.iter().map(|c| c.id.0).collect();
    ids.sort();
    assert_eq!(ids, [a.0, b.0, next, next + 1], "ids follow one another");
    assert_eq!(d.doc().song.next_clip_id, next + 2);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "the whole batch is one undo step");
}
