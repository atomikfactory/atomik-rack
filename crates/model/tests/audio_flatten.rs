//! `flatten_audio`: ticks to frames through the tempo map, later-wins overlaps, mute and
//! solo, gain, and fades measured within the original clip.

mod common;

use atomik_model::song::{AudioRegion, effective_tracks, flatten_audio};
use atomik_model::{AudioClipContent, Command, Document, FadeCurve, SongCommand, TempoEvent};
use common::audio::*;
use common::song::*;
use common::*;

const SR: u32 = 48_000;
/// 120 BPM at 48 kHz: a beat (PPQ ticks) is 24 000 frames, a bar 96 000.
const BEAT_F: u64 = 24_000;
const BAR_F: u64 = 4 * BEAT_F;

fn flat(d: &Document, device: &str) -> Vec<AudioRegion> {
    let song = &d.doc().song;
    flatten_audio(song.track(&key(device)).unwrap(), song, SR)
}

fn setup() -> (atomik_device_sdk::Registry, Document) {
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    (reg, d)
}

fn clip_at(d: &mut Document, reg: &atomik_device_sdk::Registry, start: i64, len: i64, n: u8) {
    d.apply(add_audio_clip("audio-track", start, len, n), reg)
        .unwrap();
}

/// Turns the crossfade off on every clip (a hard cut where clips overlap).
fn cut_all(d: &mut Document, reg: &atomik_device_sdk::Registry) {
    let clips: Vec<_> = d.doc().song.tracks[0].lanes[0].clips.clone();
    for c in clips {
        let atomik_model::ClipContent::Audio(mut a) = c.content else {
            continue;
        };
        a.xfade = None;
        d.apply(
            sc(SongCommand::SetAudioClip {
                id: c.id,
                start: c.start,
                length: c.length,
                muted: c.muted,
                content: a,
                merge: None,
            }),
            reg,
        )
        .unwrap();
    }
}

#[test]
fn one_clip_becomes_one_region_with_clip_position_and_source_offset() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 500,
        fade_in: 100,
        fade_out: 200,
        fade_curve: FadeCurve::EqualPower,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", BAR, 2 * BAR, a), &reg)
        .unwrap();
    let r = flat(&d, "audio-track");
    assert_eq!(
        r,
        vec![AudioRegion {
            asset: hash(1),
            song_start_frame: BAR_F,
            source_offset: 500,
            frames: 2 * BAR_F,
            gain: 1.0,
            fade_in: 100,
            fade_out: 200,
            fade_curve: FadeCurve::EqualPower,
            clip_pos: 0,
            clip_frames: 2 * BAR_F,
            reverse: false,
            envelope: Vec::new(),
            x_in: 0,
            x_out: 0,
            x_curve: FadeCurve::EqualPower,
        }]
    );
}

#[test]
fn gain_is_the_linear_form_of_gain_db() {
    let (reg, mut d) = setup();
    for (i, db) in [0.0f32, -6.0, 6.0, -120.0].into_iter().enumerate() {
        let a = AudioClipContent {
            gain_db: db,
            ..audio(1)
        };
        d.apply(
            add_audio_clip_with("audio-track", i as i64 * BAR, BAR, a),
            &reg,
        )
        .unwrap();
    }
    let g: Vec<f32> = flat(&d, "audio-track").iter().map(|r| r.gain).collect();
    assert_eq!(g[0], 1.0);
    assert!((g[1] - 0.501_187).abs() < 1e-5);
    assert!((g[2] - 1.995_262).abs() < 1e-5);
    assert!(g[3] > 0.0 && g[3] < 2e-6);
}

#[test]
fn a_later_clip_cuts_an_earlier_one_into_regions() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        source_offset: 1000,
        fade_in: 24_000,
        fade_out: 24_000,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 4 * BAR, a), &reg)
        .unwrap();
    clip_at(&mut d, &reg, BAR, 2 * BAR, 2);
    let r = flat(&d, "audio-track");
    assert_eq!(r.len(), 3);
    // First piece of A.
    assert_eq!(
        (
            r[0].asset.as_str(),
            r[0].song_start_frame,
            r[0].source_offset,
            r[0].frames
        ),
        (hash(1).as_str(), 0, 1000, BAR_F)
    );
    assert_eq!((r[0].clip_pos, r[0].clip_frames), (0, 4 * BAR_F));
    // B in the middle, uncut.
    assert_eq!(
        (
            r[1].asset.as_str(),
            r[1].song_start_frame,
            r[1].source_offset,
            r[1].frames
        ),
        (hash(2).as_str(), BAR_F, 0, 2 * BAR_F)
    );
    // The tail of A resumes where the source would have been, not where it stopped.
    assert_eq!(
        (
            r[2].asset.as_str(),
            r[2].song_start_frame,
            r[2].source_offset,
            r[2].frames
        ),
        (hash(1).as_str(), 3 * BAR_F, 1000 + 3 * BAR_F, BAR_F)
    );
    assert_eq!((r[2].clip_pos, r[2].clip_frames), (3 * BAR_F, 4 * BAR_F));
}

#[test]
fn fades_are_measured_within_the_original_clip_not_at_cut_points() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        fade_in: 24_000,
        fade_out: 24_000,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 4 * BAR, a), &reg)
        .unwrap();
    clip_at(&mut d, &reg, BAR, 2 * BAR, 2);
    let r = flat(&d, "audio-track");
    // The head piece has the fade in at its start and no fade out at the cut.
    assert_eq!(r[0].fade_gain(0), 0.0);
    assert!((r[0].fade_gain(12_000) - 0.5).abs() < 1e-6);
    assert_eq!(r[0].fade_gain(24_000), 1.0);
    assert_eq!(
        r[0].fade_gain(r[0].frames - 1),
        1.0,
        "no fade out at the cut"
    );
    // The tail piece has no fade in at the cut and the fade out at the clip end.
    assert_eq!(r[2].fade_gain(0), 1.0, "no fade in at the cut");
    let n = BAR_F; // region frames
    assert_eq!(r[2].frames, n);
    assert_eq!(r[2].fade_gain(n - 24_000 - 1), 1.0);
    assert_eq!(r[2].fade_gain(n - 24_000), 1.0);
    assert!((r[2].fade_gain(n - 12_000) - 0.5).abs() < 1e-6);
    assert!((r[2].fade_gain(n - 1) - 1.0 / 24_000.0).abs() < 1e-6);
    // The covering clip has its own (empty) fades.
    assert_eq!(r[1].fade_gain(0), 1.0);
}

#[test]
fn a_fade_that_spans_a_cut_continues_in_the_next_piece() {
    let (reg, mut d) = setup();
    // Fade in of 1 bar over a 2 bar clip; a clip covers [0.25, 0.5) bar.
    let a = AudioClipContent {
        fade_in: BAR_F,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 2 * BAR, a), &reg)
        .unwrap();
    clip_at(&mut d, &reg, BAR / 4, BAR / 4, 2);
    let r = flat(&d, "audio-track");
    assert_eq!(r.len(), 3);
    let tail = &r[2];
    assert_eq!(tail.clip_pos, BAR_F / 2);
    // Halfway through the clip's fade in: 0.5 at the start of the tail, not 0.
    assert!((tail.fade_gain(0) - 0.5).abs() < 1e-6);
    assert_eq!(tail.fade_gain(BAR_F / 2), 1.0);
}

#[test]
fn equal_power_fades_follow_a_quarter_sine() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        fade_in: 1000,
        fade_out: 1000,
        fade_curve: FadeCurve::EqualPower,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, BAR, a), &reg)
        .unwrap();
    let r = &flat(&d, "audio-track")[0];
    assert_eq!(r.fade_gain(0), 0.0);
    assert!((r.fade_gain(500) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert_eq!(r.fade_gain(1000), 1.0);
    let end = r.frames;
    assert!((r.fade_gain(end - 500) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
}

#[test]
fn fades_longer_than_the_clip_are_clamped_and_multiply() {
    let (reg, mut d) = setup();
    let a = AudioClipContent {
        fade_in: 10 * BAR_F,
        fade_out: 10 * BAR_F,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, BAR, a), &reg)
        .unwrap();
    let r = &flat(&d, "audio-track")[0];
    assert_eq!((r.fade_in, r.fade_out), (BAR_F, BAR_F));
    // Halfway: 0.5 in, 0.5 out.
    assert!((r.fade_gain(BAR_F / 2) - 0.25).abs() < 1e-6);
}

#[test]
fn covered_clips_vanish_and_muted_clips_neither_play_nor_hide() {
    let (reg, mut d) = setup();
    clip_at(&mut d, &reg, BAR, BAR, 1);
    clip_at(&mut d, &reg, BAR, 3 * BAR, 2); // same start, later id: covers the first fully
    let cover = last(&d);
    assert_eq!(flat(&d, "audio-track").len(), 1);
    assert_eq!(flat(&d, "audio-track")[0].asset, hash(2));
    // Mute the covering clip: it no longer hides the other.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: cover,
            start: BAR,
            length: 3 * BAR,
            offset: 0,
            loop_length: None,
            muted: true,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let r = flat(&d, "audio-track");
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].asset.as_str(), r[0].song_start_frame),
        (hash(1).as_str(), BAR_F)
    );
}

#[test]
fn track_mute_and_solo_apply_through_effective_tracks() {
    let (reg, mut d) = setup();
    d.apply(add_audio_track(), &reg).unwrap();
    clip_at(&mut d, &reg, 0, BAR, 1);
    d.apply(add_audio_clip("audio-track-2", 0, BAR, 2), &reg)
        .unwrap();
    assert_eq!(flat(&d, "audio-track").len(), 1);
    assert_eq!(flat(&d, "audio-track-2").len(), 1);
    let flags = |device: &str, muted, solo| {
        sc(SongCommand::SetTrackFlags {
            device: key(device),
            muted,
            solo,
            armed: false,
        })
    };
    d.apply(flags("audio-track", true, false), &reg).unwrap();
    assert!(flat(&d, "audio-track").is_empty());
    assert_eq!(flat(&d, "audio-track-2").len(), 1);
    d.apply(flags("audio-track", false, false), &reg).unwrap();
    d.apply(flags("audio-track-2", false, true), &reg).unwrap();
    assert!(
        flat(&d, "audio-track").is_empty(),
        "another track is soloed"
    );
    assert_eq!(flat(&d, "audio-track-2").len(), 1);
    // Mute beats solo (effective_tracks).
    d.apply(flags("audio-track-2", true, true), &reg).unwrap();
    assert!(flat(&d, "audio-track-2").is_empty());
    assert!(effective_tracks(&d.doc().song).is_empty() || flat(&d, "audio-track").is_empty());
}

#[test]
fn tempo_changes_stretch_the_clip_in_frames_but_not_its_audio() {
    let (reg, mut d) = setup();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 60.0),
            TempoEvent::step(PPQ, 120.0),
        ])),
        &reg,
    )
    .unwrap();
    let a = AudioClipContent {
        source_offset: 7,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, 4 * PPQ, a), &reg)
        .unwrap();
    // 1 beat at 60 BPM (48 000 frames) + 3 beats at 120 BPM (3 x 24 000).
    let r = flat(&d, "audio-track");
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].frames, r[0].source_offset), (120_000, 7));
    // A cover at beat 2 splits the clip at frame 72 000.
    d.apply(add_audio_clip("audio-track", 2 * PPQ, PPQ, 2), &reg)
        .unwrap();
    let r = flat(&d, "audio-track");
    assert_eq!(r.len(), 3);
    assert_eq!((r[0].song_start_frame, r[0].frames), (0, 72_000));
    assert_eq!((r[1].song_start_frame, r[1].frames), (72_000, 24_000));
    assert_eq!(
        (r[2].song_start_frame, r[2].frames, r[2].source_offset),
        (96_000, 24_000, 7 + 96_000)
    );
}

#[test]
fn pieces_tile_the_clip_without_gaps_or_overlaps() {
    let (reg, mut d) = setup();
    // Odd ticks that do not land on whole frames.
    d.apply(add_audio_clip("audio-track", 3, 4 * BAR + 1, 1), &reg)
        .unwrap();
    for (s, l) in [(1001, 777), (2500, 3001), (40_001, 12_345)] {
        d.apply(add_audio_clip("audio-track", s, l, 2), &reg)
            .unwrap();
    }
    let r = flat(&d, "audio-track");
    assert!(r.len() >= 5);
    for w in r.windows(2) {
        assert!(
            w[0].song_start_frame + w[0].frames <= w[1].song_start_frame,
            "regions never overlap"
        );
    }
    // The pieces of clip 1 are contiguous in the source as well.
    let ones: Vec<_> = r.iter().filter(|x| x.asset == hash(1)).collect();
    for x in &ones {
        assert_eq!(x.source_offset, x.clip_pos);
        assert_eq!(x.clip_frames, ones[0].clip_frames);
    }
}

#[test]
fn spans_that_round_to_no_frame_are_dropped() {
    let (reg, mut d) = setup();
    clip_at(&mut d, &reg, 0, BAR, 1);
    clip_at(&mut d, &reg, 1, BAR, 2); // leaves tick span [0, 1) of the first clip
    cut_all(&mut d, &reg);
    let song = &d.doc().song;
    let t = song.track(&key("audio-track")).unwrap();
    // At 8 kHz one tick is 0.26 frames: the one-tick head has no frame.
    let r = flatten_audio(t, song, 8_000);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].asset, hash(2));
    // At 48 kHz it has 2.
    assert_eq!(flatten_audio(t, song, SR).len(), 2);
}

#[test]
fn notes_tracks_and_empty_lanes_give_no_regions() {
    let (reg, mut d) = setup();
    assert!(flat(&d, "audio-track").is_empty());
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.5)])),
        &reg,
    )
    .unwrap();
    assert!(flat(&d, "synth").is_empty());
}

#[test]
fn frame_at_follows_the_tempo_map() {
    let (reg, mut d) = setup();
    let song = &d.doc().song;
    assert_eq!(song.frame_at(0, SR), 0);
    assert_eq!(song.frame_at(-5, SR), 0);
    assert_eq!(song.frame_at(PPQ, SR), BEAT_F);
    assert_eq!(song.frame_at(1, SR), 2); // 1.5625 rounds to 2
    assert_eq!(song.frame_at(PPQ, 44_100), 22_050);
    d.apply(Command::SetTempo(60.0), &reg).unwrap();
    assert_eq!(d.doc().song.frame_at(PPQ, SR), 2 * BEAT_F);
}
