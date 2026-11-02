//! Fades and envelope points of an audio clip stay inside the clip: the commands refuse
//! longer ones and an older file that holds them loads clamped, with a warning.

mod common;

use atomik_model::{
    AudioClipContent, ClipContent, Document, GainPoint, ModelError, SongCommand,
    load_project_with_warnings, save_project,
};
use common::audio::*;
use common::song::*;

fn span_frames(d: &Document, ticks: i64) -> u64 {
    d.doc().song.frame_at(ticks, d.doc().sample_rate)
}

fn is_range(e: &ModelError) -> bool {
    matches!(e, ModelError::OutOfRange(_))
}

#[test]
fn a_fade_longer_than_its_clip_is_refused_by_add_and_set() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let span = span_frames(&d, BAR);
    let too_long = AudioClipContent {
        fade_out: span + 1,
        ..audio(1)
    };
    let e = d
        .apply(add_audio_clip_with("audio-track", 0, BAR, too_long), &reg)
        .unwrap_err();
    assert!(is_range(&e), "{e:?}");
    let exact = AudioClipContent {
        fade_in: span,
        fade_out: span,
        ..audio(1)
    };
    d.apply(add_audio_clip_with("audio-track", 0, BAR, exact), &reg)
        .unwrap();
    let id = last(&d);
    let set = |content| {
        sc(SongCommand::SetAudioClip {
            id,
            start: 0,
            length: BAR / 2,
            muted: false,
            content,
            merge: None,
        })
    };
    let half = span_frames(&d, BAR / 2);
    let e = d
        .apply(
            set(AudioClipContent {
                fade_in: half + 1,
                ..audio(1)
            }),
            &reg,
        )
        .unwrap_err();
    assert!(is_range(&e), "{e:?}");
    d.apply(
        set(AudioClipContent {
            fade_in: half,
            ..audio(1)
        }),
        &reg,
    )
    .unwrap();
}

#[test]
fn an_envelope_point_past_the_clip_is_refused() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let span = span_frames(&d, BAR);
    let env = |frame| AudioClipContent {
        envelope: vec![GainPoint {
            frame,
            gain_db: -6.0,
        }],
        ..audio(1)
    };
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let set = |content| {
        sc(SongCommand::SetAudioClip {
            id,
            start: 0,
            length: BAR,
            muted: false,
            content,
            merge: None,
        })
    };
    assert!(is_range(&d.apply(set(env(span + 1)), &reg).unwrap_err()));
    d.apply(set(env(span)), &reg).unwrap();
}

#[test]
fn strip_silence_refuses_a_fade_longer_than_the_clip() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let span = span_frames(&d, BAR);
    let strip = |fade| {
        sc(SongCommand::StripSilence {
            id,
            pieces: vec![(0, span / 4), (span / 2, span)],
            fade,
        })
    };
    assert!(is_range(&d.apply(strip(span + 1), &reg).unwrap_err()));
    d.apply(strip(span), &reg).unwrap();
}

#[test]
fn an_older_file_with_a_fade_past_its_clip_loads_clamped_with_a_warning() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    let span = span_frames(&d, BAR);
    let mut doc = d.doc().clone();
    for lane in &mut doc.song_mut().tracks[0].lanes {
        for c in &mut lane.clips {
            if let ClipContent::Audio(a) = &mut c.content {
                a.fade_in = span * 3;
                a.envelope = vec![
                    GainPoint {
                        frame: 0,
                        gain_db: 0.0,
                    },
                    GainPoint {
                        frame: span * 2,
                        gain_db: -12.0,
                    },
                ];
            }
        }
    }
    // Written before the bound existed: format 39.
    let mut v: serde_json::Value = serde_json::from_str(&save_project(&doc)).unwrap();
    v["version"] = serde_json::json!(39);
    let text = v.to_string();
    let (loaded, warnings) = load_project_with_warnings(&text, &reg).unwrap();
    let ClipContent::Audio(a) = &doc_clip(&loaded, id).content else {
        panic!("audio clip");
    };
    assert_eq!(a.fade_in, span);
    assert_eq!(a.envelope.last().unwrap().frame, span);
    assert!(a.envelope.last().unwrap().gain_db < 0.0);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("clamped"), "{warnings:?}");
    // A second load of the clamped file is clean.
    let again = save_project(&loaded);
    assert!(
        load_project_with_warnings(&again, &reg)
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn a_current_file_keeps_a_fade_past_its_clip_as_a_request() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let span = span_frames(&d, BAR);
    let mut doc = d.doc().clone();
    if let ClipContent::Audio(a) = &mut doc.song_mut().tracks[0].lanes[0].clips[0].content {
        a.fade_out = span * 3;
    }
    let (loaded, warnings) = load_project_with_warnings(&save_project(&doc), &reg).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(
        loaded, doc,
        "the stored value is a request; flattening clamps it"
    );
}
