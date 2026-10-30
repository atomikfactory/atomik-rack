//! Warp in the model: the settings and their format, the time map through the tempo map, the
//! halves of a split, the render jobs, tempo estimation and audio quantize.

mod common;

use atomik_model::song::{TempoEvent, flatten_audio};
use atomik_model::{
    AudioClipContent, AudioQuantize, ClipContent, Command, Document, ModelError, PROJECT_VERSION,
    SongCommand, WarpMarker, WarpMode, WarpSettings, WorkflowCommand, bpm_for_beats, estimate_bpm,
    load_project, quantize_audio, save_project, source_at_tick, tick_at_source, warp_job,
    warp_jobs,
};
use common::audio::*;
use common::song::*;
use common::*;
use serde_json::{Value, json};

const SR: u32 = 48_000;
/// 120 BPM at 48 kHz: a beat (PPQ ticks) is 24 000 frames.
const BEAT_F: f64 = 24_000.0;

fn setup() -> (atomik_device_sdk::Registry, Document) {
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    (reg, d)
}

fn warped(bpm: f64) -> AudioClipContent {
    AudioClipContent {
        warp: WarpSettings {
            enabled: true,
            source_bpm: bpm,
            ..WarpSettings::default()
        },
        ..audio(1)
    }
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
    set(d, reg, id, |c| *c = a).unwrap();
    id
}

fn set(
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

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn the_default_is_no_warp_and_settings_are_checked() {
    let w = WarpSettings::default();
    assert!(!w.active() && w.validate().is_ok());
    assert!(
        WarpSettings {
            enabled: true,
            ..w.clone()
        }
        .active()
    );
    // Transposing alone is active, except in Repitch mode, which cannot transpose.
    let t = WarpSettings {
        transpose_cents: 150,
        ..w.clone()
    };
    assert!(t.active() && t.semitones() == 1 && t.cents() == 50);
    assert!(
        !WarpSettings {
            mode: WarpMode::Repitch,
            ..t.clone()
        }
        .active()
    );
    for bad in [
        WarpSettings {
            source_bpm: 10.0,
            ..w.clone()
        },
        WarpSettings {
            source_bpm: f64::NAN,
            ..w.clone()
        },
        WarpSettings {
            transpose_cents: 2401,
            ..w.clone()
        },
        WarpSettings {
            markers: vec![
                WarpMarker { tick: 5, source: 9 },
                WarpMarker {
                    tick: 5,
                    source: 10,
                },
            ],
            ..w.clone()
        },
        WarpSettings {
            markers: vec![
                WarpMarker { tick: 5, source: 9 },
                WarpMarker { tick: 6, source: 9 },
            ],
            ..w.clone()
        },
        WarpSettings {
            markers: vec![WarpMarker {
                tick: -1,
                source: 9,
            }],
            ..w
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    for m in WarpMode::ALL {
        assert_eq!(WarpMode::from_id(m.id()), Some(m));
    }
}

#[test]
fn set_audio_clip_takes_warp_and_undo_restores_it() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    set(&mut d, &reg, id, |a| {
        a.warp = WarpSettings {
            enabled: true,
            mode: WarpMode::Tonal,
            source_bpm: 96.5,
            transpose_cents: -250,
            markers: vec![WarpMarker {
                tick: 100,
                source: 2000,
            }],
        };
    })
    .unwrap();
    assert_eq!(audio_of(&d, id).warp.mode, WarpMode::Tonal);
    d.undo(&reg).unwrap();
    assert_eq!(audio_of(&d, id).warp, WarpSettings::default());
    d.redo(&reg).unwrap();
    assert_eq!(audio_of(&d, id).warp.transpose_cents, -250);
    // A bad value is refused and changes nothing.
    let before = d.doc().clone();
    assert!(set(&mut d, &reg, id, |a| a.warp.source_bpm = 5000.0).is_err());
    assert!(set(&mut d, &reg, id, |a| a.warp.transpose_cents = 3000).is_err());
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_reversed_clip_cannot_be_warped_or_transposed() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    set(&mut d, &reg, id, |a| a.reverse = true).unwrap();
    assert!(set(&mut d, &reg, id, |a| a.warp.enabled = true).is_err());
    assert!(set(&mut d, &reg, id, |a| a.warp.transpose_cents = 100).is_err());
    // Repitch cannot transpose, so a transpose stored there is inactive and allowed.
    set(&mut d, &reg, id, |a| {
        a.warp.mode = WarpMode::Repitch;
        a.warp.transpose_cents = 100;
    })
    .unwrap();
    let id2 = put(&mut d, &reg, 4 * PPQ, 2 * PPQ, warped(120.0));
    assert!(set(&mut d, &reg, id2, |a| a.reverse = true).is_err());
}

#[test]
fn the_format_round_trips_warp_and_omits_the_default() {
    assert_eq!(PROJECT_VERSION, 36);
    let (reg, mut d) = setup();
    let plain = put(&mut d, &reg, 0, PPQ, audio(1));
    let text = save_project(d.doc());
    assert!(!text.contains("warp"));
    let rich = AudioClipContent {
        warp: WarpSettings {
            enabled: true,
            mode: WarpMode::Repitch,
            source_bpm: 93.25,
            transpose_cents: 700,
            markers: vec![
                WarpMarker { tick: 0, source: 0 },
                WarpMarker {
                    tick: 7680,
                    source: 30_000,
                },
            ],
        },
        ..audio(1)
    };
    set(&mut d, &reg, plain, |a| a.warp = rich.warp.clone()).unwrap();
    let text = save_project(d.doc());
    assert!(
        text.contains("\"warp\": {\"mode\": \"repitch\", \"bpm\": 93.25, \"enabled\": true, \"transpose\": 700, \"markers\": [[0, 0], [7680, 30000]]}"),
        "{text}"
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
    // A disabled clip that only remembers its settings is stored too.
    set(&mut d, &reg, plain, |a| {
        a.warp = WarpSettings {
            mode: WarpMode::Tonal,
            ..WarpSettings::default()
        };
    })
    .unwrap();
    let text = save_project(d.doc());
    assert!(
        text.contains("\"warp\": {\"mode\": \"tonal\", \"bpm\": 120}"),
        "{text}"
    );
    assert_eq!(&load_project(&text, &reg).unwrap(), d.doc());
}

#[test]
fn bad_warp_fields_name_their_json_path_and_old_files_refuse_warp() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, PPQ, warped(100.0));
    let _ = id;
    let base: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    let cases: Vec<(&str, Value, &str)> = vec![
        ("mode", json!("granular"), "expected"),
        ("bpm", json!(5), "tempo"),
        ("bpm", json!("fast"), ""),
        ("transpose", json!(2500), "cents"),
        ("transpose", json!(1.5), ""),
        ("enabled", json!("yes"), ""),
        ("markers", json!([[1, 2], [1, 3]]), "increase"),
        ("markers", json!([[1]]), ""),
        ("extra", json!(1), ""),
    ];
    for (key, value, needle) in cases {
        let mut v = base.clone();
        v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["warp"][key] = value;
        let e = load_project(&v.to_string(), &reg)
            .expect_err(key)
            .to_string();
        assert!(e.contains("warp"), "{key}: {e}");
        assert!(e.contains(needle), "{key}: {e}");
    }
    // Version 23 knows no warp.
    let mut v = base.clone();
    v["version"] = json!(23);
    let e = load_project(&v.to_string(), &reg).unwrap_err().to_string();
    assert!(e.contains("warp"), "{e}");
    // A version 23 file without it loads, with the default warp.
    let mut v = base;
    v["version"] = json!(23);
    v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]
        .as_object_mut()
        .unwrap()
        .remove("warp");
    let loaded = load_project(&v.to_string(), &reg).unwrap();
    let ClipContent::Audio(a) = &loaded.song.tracks[0].lanes[0].clips[0].content else {
        panic!()
    };
    assert_eq!(a.warp, WarpSettings::default());
    // A reversed and warped clip is refused on load.
    let (reg2, mut d2) = setup();
    put(&mut d2, &reg2, 0, PPQ, warped(100.0));
    let mut v: Value = serde_json::from_str(&save_project(d2.doc())).unwrap();
    v["song"]["tracks"][0]["lanes"][0]["clips"][0]["audio"]["reverse"] = json!(true);
    let e = load_project(&v.to_string(), &reg2).unwrap_err().to_string();
    assert!(e.contains("reversed"), "{e}");
}

#[test]
fn the_map_follows_the_song_tempo_in_beats() {
    let (reg, mut d) = setup();
    // The source was recorded at 100 BPM; the song runs at 120.
    let a = warped(100.0);
    let id = put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let song = &d.doc().song;
    // At 120 BPM a beat is 24 000 frames; at 100 BPM the source holds 28 800 frames per beat.
    for beats in [0.0, 1.0, 2.5, 4.0] {
        let tick = (beats * PPQ as f64) as i64;
        let want = beats * 28_800.0;
        assert!(
            near(source_at_tick(song, SR, 0, &a, tick), want, 1e-6),
            "{beats}"
        );
        assert!(near(
            tick_at_source(song, SR, 0, &a, want),
            tick as f64,
            1e-6
        ));
    }
    // The job's knots follow: out frame 24 000 plays source frame 28 800.
    let c = doc_clip(d.doc(), id);
    let job = warp_job(song, SR, c.start, c.length, &a).unwrap();
    assert_eq!(job.frames, 96_000);
    assert_eq!(job.knots.len(), 2);
    assert!(near(job.knots[1].0, 96_000.0, 1e-6) && near(job.knots[1].1, 4.0 * 28_800.0, 1e-6));
    // A source offset shifts every knot.
    let b = AudioClipContent {
        source_offset: 1000,
        ..a.clone()
    };
    let job = warp_job(song, SR, 0, 4 * PPQ, &b).unwrap();
    assert!(near(job.knots[0].1, 1000.0, 1e-9));
    // The key changes with anything the buffer depends on.
    let base = warp_job(song, SR, 0, 4 * PPQ, &a).unwrap();
    assert_eq!(
        base.key(),
        warp_job(song, SR, 0, 4 * PPQ, &a).unwrap().key()
    );
    let mut c2 = a.clone();
    c2.warp.mode = WarpMode::Tonal;
    assert_ne!(
        base.key(),
        warp_job(song, SR, 0, 4 * PPQ, &c2).unwrap().key()
    );
    c2 = a.clone();
    c2.warp.transpose_cents = 100;
    assert_ne!(
        base.key(),
        warp_job(song, SR, 0, 4 * PPQ, &c2).unwrap().key()
    );
    assert_ne!(
        base.key(),
        warp_job(song, SR, 0, 3 * PPQ, &a).unwrap().key()
    );
}

#[test]
fn a_tempo_change_moves_the_frames_but_not_the_beats() {
    let (reg, mut d) = setup();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 60.0),
            TempoEvent::step(2 * PPQ, 120.0),
        ])),
        &reg,
    )
    .unwrap();
    let a = warped(120.0);
    put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let song = &d.doc().song;
    let job = warp_job(song, SR, 0, 4 * PPQ, &a).unwrap();
    // 2 beats at 60 BPM (96 000 frames), then 2 at 120 (48 000): the source is at its own
    // tempo (24 000 frames a beat), so it reads at half speed then at full speed... that is,
    // 1 source frame per frame in the second part and 0.5 in the first.
    assert_eq!(job.frames, 144_000);
    assert_eq!(job.knots.len(), 3);
    assert!(near(job.knots[1].0, 96_000.0, 1e-6) && near(job.knots[1].1, 2.0 * BEAT_F, 1e-6));
    assert!(near(job.knots[2].0, 144_000.0, 1e-6) && near(job.knots[2].1, 4.0 * BEAT_F, 1e-6));
    // The map is the same function of the beat whatever the tempo.
    assert!(near(source_at_tick(song, SR, 0, &a, PPQ), BEAT_F, 1e-6));
}

#[test]
fn a_tempo_ramp_adds_knots_and_stays_monotone_and_on_the_beats() {
    let (reg, mut d) = setup();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent {
                tick: 0,
                bpm: 80.0,
                ramp: true,
            },
            TempoEvent::step(4 * PPQ, 160.0),
        ])),
        &reg,
    )
    .unwrap();
    let a = warped(120.0);
    put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let song = &d.doc().song;
    let job = warp_job(song, SR, 0, 4 * PPQ, &a).unwrap();
    assert!(job.knots.len() > 10, "{} knots", job.knots.len());
    assert!(
        job.knots
            .windows(2)
            .all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1)
    );
    // Every knot sits on the beat map: the frame the tempo map gives for the tick that plays
    // that source frame.
    let table = song.tempo_table();
    for &(frame, src) in &job.knots {
        let tick = tick_at_source(song, SR, 0, &a, src);
        let want = table.map().frame(tick, f64::from(SR));
        assert!(near(frame, want, 1.0e-3), "{frame} vs {want}");
    }
    assert!(near(job.knots.last().unwrap().1, 4.0 * BEAT_F, 1e-6));
}

#[test]
fn markers_pin_source_frames_to_ticks() {
    let (reg, mut d) = setup();
    let mut a = warped(120.0);
    a.warp.markers = vec![
        WarpMarker {
            tick: PPQ,
            source: 30_000,
        },
        WarpMarker {
            tick: 2 * PPQ,
            source: 40_000,
        },
    ];
    put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let song = &d.doc().song;
    assert!(near(source_at_tick(song, SR, 0, &a, PPQ), 30_000.0, 1e-6));
    assert!(near(
        source_at_tick(song, SR, 0, &a, 2 * PPQ),
        40_000.0,
        1e-6
    ));
    // Straight between, at the source tempo outside (24 000 frames a beat).
    assert!(near(
        source_at_tick(song, SR, 0, &a, PPQ + PPQ / 2),
        35_000.0,
        1e-6
    ));
    assert!(near(
        source_at_tick(song, SR, 0, &a, 0),
        30_000.0 - BEAT_F,
        1e-6
    ));
    assert!(near(
        source_at_tick(song, SR, 0, &a, 3 * PPQ),
        40_000.0 + BEAT_F,
        1e-6
    ));
    for src in [10_000.0, 30_000.0, 33_333.0, 40_000.0, 70_000.0] {
        let t = tick_at_source(song, SR, 0, &a, src);
        assert!(
            near(source_at_tick(song, SR, 0, &a, t.round() as i64), src, 5.0),
            "{src}"
        );
    }
    // The knots break at the markers.
    let job = warp_job(song, SR, 0, 4 * PPQ, &a).unwrap();
    assert!(
        job.knots
            .iter()
            .any(|k| near(k.1, 30_000.0, 1e-6) && near(k.0, BEAT_F, 1e-6))
    );
    assert!(
        job.knots
            .iter()
            .any(|k| near(k.1, 40_000.0, 1e-6) && near(k.0, 2.0 * BEAT_F, 1e-6))
    );
}

#[test]
fn a_split_keeps_the_sound_where_it_was() {
    let (reg, mut d) = setup();
    let mut a = warped(100.0);
    a.warp.markers = vec![
        WarpMarker {
            tick: PPQ,
            source: 30_000,
        },
        WarpMarker {
            tick: 3 * PPQ,
            source: 90_000,
        },
    ];
    let id = put(&mut d, &reg, 0, 4 * PPQ, a.clone());
    let before = d.doc().song.clone();
    let at = PPQ + PPQ / 2;
    d.apply(sc(SongCommand::SplitClip { id, at }), &reg)
        .unwrap();
    let right = last(&d);
    let (l, r) = (audio_of(&d, id), audio_of(&d, right));
    let rc = doc_clip(d.doc(), right);
    assert_eq!(rc.start, at);
    // The right half plays the same source at every tick, and so does the left.
    for tick in [at, at + 1000, 2 * PPQ, 3 * PPQ, 3 * PPQ + 777] {
        let want = source_at_tick(&before, SR, 0, &a, tick);
        let got = source_at_tick(&d.doc().song, SR, at, &r, tick);
        assert!(near(got, want, 1.0), "tick {tick}: {got} vs {want}");
    }
    for tick in [0, PPQ / 2, PPQ, at - 1] {
        let want = source_at_tick(&before, SR, 0, &a, tick);
        assert!(near(
            source_at_tick(&d.doc().song, SR, 0, &l, tick),
            want,
            1e-6
        ));
    }
    assert!(
        r.warp
            .markers
            .first()
            .is_some_and(|m| m.tick == 0 && m.source == 0)
    );
    assert!(r.warp.validate().is_ok());
    // A clip without warp splits as before (1:1 frames).
    let (reg2, mut d2) = setup();
    let id2 = put(&mut d2, &reg2, 0, 4 * PPQ, audio(1));
    d2.apply(sc(SongCommand::SplitClip { id: id2, at: PPQ }), &reg2)
        .unwrap();
    assert_eq!(audio_of(&d2, last(&d2)).source_offset, 24_000);
}

#[test]
fn regions_carry_the_job_and_the_real_time_fallback() {
    let (reg, mut d) = setup();
    let plain = put(&mut d, &reg, 0, 2 * PPQ, audio(1));
    let _ = plain;
    let w = warped(100.0);
    put(&mut d, &reg, 4 * PPQ, 2 * PPQ, w);
    let song = &d.doc().song;
    let r = flatten_audio(song.track(&key("audio-track")).unwrap(), song, SR);
    assert_eq!(r.len(), 2);
    assert!(r[0].warp.is_none());
    let job = r[1].warp.as_ref().expect("warped region");
    assert_eq!(job.frames, r[1].clip_frames);
    // The fallback reads the asset in real time from the clip start.
    assert_eq!((r[1].source_offset, r[1].reverse), (0, false));
    // A later clip covering the start of the warped one leaves a region at clip_pos > 0.
    put(&mut d, &reg, 4 * PPQ, PPQ, audio(2));
    let song = &d.doc().song;
    let r = flatten_audio(song.track(&key("audio-track")).unwrap(), song, SR);
    let cut = r.iter().find(|x| x.warp.is_some()).unwrap();
    assert_eq!(cut.clip_pos, 24_000);
    assert_eq!(cut.source_offset, 24_000);
}

#[test]
fn only_audible_warped_clips_make_jobs() {
    let (reg, mut d) = setup();
    assert!(warp_jobs(d.doc()).is_empty());
    let a = put(&mut d, &reg, 0, 2 * PPQ, warped(100.0));
    put(&mut d, &reg, 4 * PPQ, 2 * PPQ, warped(100.0));
    put(&mut d, &reg, 8 * PPQ, PPQ, audio(1));
    // Two clips of equal settings and length make one job.
    assert_eq!(warp_jobs(d.doc()).len(), 1);
    set(&mut d, &reg, a, |c| c.warp.transpose_cents = 200).unwrap();
    assert_eq!(warp_jobs(d.doc()).len(), 2);
    // A muted clip makes none.
    d.apply(
        Command::Workflow(WorkflowCommand::SetClipsMuted {
            ids: vec![a],
            muted: true,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(warp_jobs(d.doc()).len(), 1);
    // The source span of a job covers what the map reads, with a margin.
    let job = warp_jobs(d.doc()).into_values().next().unwrap();
    let (lo, hi) = job.source_span();
    assert_eq!(lo, 0);
    assert!(hi as f64 >= job.knots.last().unwrap().1 + 8000.0);
}

#[test]
fn transposing_without_warp_reads_in_real_time() {
    let (_reg, d) = setup();
    let a = AudioClipContent {
        source_offset: 500,
        warp: WarpSettings {
            transpose_cents: 300,
            ..WarpSettings::default()
        },
        ..audio(1)
    };
    let song = &d.doc().song;
    let job = warp_job(song, SR, 0, 2 * PPQ, &a).unwrap();
    assert_eq!(job.transpose_cents, 300);
    assert_eq!(job.knots.len(), 2);
    assert!(near(job.knots[0].1, 500.0, 1e-9));
    assert!(near(
        job.knots[1].1 - job.knots[0].1,
        job.knots[1].0 - job.knots[0].0,
        1e-9
    ));
    // At song tempo changes it still plays real time: the map ignores the source tempo.
    assert!(near(
        source_at_tick(song, SR, 0, &a, PPQ),
        500.0 + BEAT_F,
        1e-6
    ));
    // Repitch cannot transpose, so there is nothing to render.
    let mut r = a;
    r.warp.mode = WarpMode::Repitch;
    assert!(warp_job(song, SR, 0, 2 * PPQ, &r).is_none());
}

#[test]
fn the_tempo_of_a_loop_and_of_a_set_of_onsets() {
    // 4 beats in 96 000 frames at 48 kHz is 120 BPM.
    assert!(near(bpm_for_beats(96_000, 4.0, SR).unwrap(), 120.0, 1e-9));
    assert!(near(bpm_for_beats(48_000, 8.0, SR).unwrap(), 480.0, 1e-9));
    assert_eq!(bpm_for_beats(0, 4.0, SR), None);
    assert_eq!(bpm_for_beats(96_000, 0.0, SR), None);
    assert_eq!(bpm_for_beats(480_000, 1.0, SR), None);
    // Hits on every eighth with a kick-snare accent at 128 BPM.
    let period = 60.0 * f64::from(SR) / 128.0;
    let onsets: Vec<u64> = (0..32)
        .filter(|k| k % 2 == 0 || k % 4 == 1)
        .map(|k| (period * f64::from(k) / 2.0 + 3000.0).round() as u64)
        .collect();
    let bpm = estimate_bpm(&onsets, SR).unwrap();
    assert!(near(bpm, 128.0, 0.6), "{bpm}");
    assert_eq!(estimate_bpm(&onsets[..3], SR), None);
    // No regularity, no tempo.
    let messy: Vec<u64> = (0..24u64)
        .map(|k| (k * k * 7919 + k * 1301) % 400_000)
        .collect();
    let mut messy = messy;
    messy.sort_unstable();
    assert!(estimate_bpm(&messy, SR).is_none() || messy.len() < 4);
}

/// The tick at which the clip plays asset frame `src`, rounded.
fn plays_at(d: &Document, id: atomik_model::ClipId, src: u64) -> i64 {
    let c = doc_clip(d.doc(), id);
    tick_at_source(&d.doc().song, SR, c.start, &audio_of(d, id), src as f64).round() as i64
}

#[test]
fn quantize_moves_transients_to_the_grid_and_pins_the_ends() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let c = doc_clip(d.doc(), id).clone();
    let a = audio_of(&d, id);
    // Hits a little off the sixteenth grid (3 840 ticks = 6 000 frames).
    let hits: Vec<u64> = vec![0, 6_300, 11_500, 18_000, 24_400, 36_200, 47_700, 60_000];
    let grid = PPQ / 4;
    let q = quantize_audio(
        &d.doc().song,
        SR,
        c.start,
        c.length,
        &a,
        &hits,
        &AudioQuantize {
            grid,
            strength: 1.0,
        },
    )
    .unwrap();
    assert!(q.warp.enabled && q.warp.mode == WarpMode::Beats);
    assert!(near(q.warp.source_bpm, 120.0, 1e-9));
    q.warp.validate().unwrap();
    d.apply(
        sc(SongCommand::SetAudioClip {
            id,
            start: c.start,
            length: c.length,
            muted: false,
            content: q.clone(),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    for &h in &hits[1..] {
        let t = plays_at(&d, id, h);
        assert_eq!(t % grid, 0, "hit at {h} plays at tick {t}");
    }
    // The clip's span of the asset is unchanged: the end is pinned.
    let song = &d.doc().song;
    assert!(near(
        source_at_tick(song, SR, 0, &q, c.start + c.length),
        4.0 * BEAT_F,
        1.0
    ));
    assert!(near(source_at_tick(song, SR, 0, &q, 0), 0.0, 1e-9));
    // Half strength moves each hit half way.
    let half = quantize_audio(
        &d.doc().song,
        SR,
        c.start,
        c.length,
        &a,
        &hits,
        &AudioQuantize {
            grid,
            strength: 0.5,
        },
    )
    .unwrap();
    let t_full = tick_at_source(song, SR, 0, &q, 11_500.0);
    let t_half = tick_at_source(song, SR, 0, &half, 11_500.0);
    let t_orig = tick_at_source(song, SR, 0, &a, 11_500.0);
    assert!(
        near(t_half, (t_orig + t_full) / 2.0, 2.0),
        "{t_orig} {t_half} {t_full}"
    );
    // Strength 0 changes no position.
    let zero = quantize_audio(
        &d.doc().song,
        SR,
        c.start,
        c.length,
        &a,
        &hits,
        &AudioQuantize {
            grid,
            strength: 0.0,
        },
    )
    .unwrap();
    for h in [6_300.0, 24_400.0, 47_700.0] {
        assert!(near(
            tick_at_source(song, SR, 0, &zero, h),
            tick_at_source(song, SR, 0, &a, h),
            2.0
        ));
    }
}

#[test]
fn quantize_reports_what_it_cannot_do() {
    let (reg, mut d) = setup();
    let id = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let c = doc_clip(d.doc(), id).clone();
    let a = audio_of(&d, id);
    let song = &d.doc().song;
    let q = AudioQuantize {
        grid: PPQ / 4,
        strength: 1.0,
    };
    assert!(quantize_audio(song, SR, c.start, c.length, &a, &[], &q).is_err());
    assert!(quantize_audio(song, SR, c.start, c.length, &a, &[0, 10_000_000], &q).is_err());
    let rev = AudioClipContent {
        reverse: true,
        ..a.clone()
    };
    assert!(quantize_audio(song, SR, c.start, c.length, &rev, &[6_300], &q).is_err());
    let bad = AudioQuantize {
        grid: 0,
        strength: 1.0,
    };
    assert!(quantize_audio(song, SR, c.start, c.length, &a, &[6_300], &bad).is_err());
    // A clip that is already warped keeps its mode and tempo and re-pins from its own map.
    let mut w = warped(100.0);
    w.warp.mode = WarpMode::Tonal;
    let got = quantize_audio(song, SR, c.start, c.length, &w, &[8_000, 20_000], &q).unwrap();
    assert_eq!(
        (got.warp.mode, got.warp.source_bpm),
        (WarpMode::Tonal, 100.0)
    );
}

#[test]
fn quantize_follows_the_tempo_map() {
    let (reg, mut d) = setup();
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            TempoEvent::step(0, 90.0),
            TempoEvent::step(2 * PPQ, 150.0),
        ])),
        &reg,
    )
    .unwrap();
    let id = put(&mut d, &reg, 0, 4 * PPQ, audio(1));
    let c = doc_clip(d.doc(), id).clone();
    let a = audio_of(&d, id);
    let song = &d.doc().song;
    // 1:1 reading: the frame of a tick comes from the tempo map.
    let table = song.tempo_table();
    let frame_of = |tick: i64| table.map().frame(tick as f64, f64::from(SR));
    let hits: Vec<u64> = [PPQ + 100, 3 * PPQ - 300]
        .iter()
        .map(|t| frame_of(*t).round() as u64)
        .collect();
    let q = quantize_audio(
        song,
        SR,
        c.start,
        c.length,
        &a,
        &hits,
        &AudioQuantize {
            grid: PPQ / 4,
            strength: 1.0,
        },
    )
    .unwrap();
    assert!(
        near(q.warp.source_bpm, 90.0, 1e-9),
        "the tempo at the clip start"
    );
    for h in &hits {
        let t = tick_at_source(song, SR, 0, &q, *h as f64);
        assert!(
            near(t % (PPQ / 4) as f64, 0.0, 1.0)
                || near(t % (PPQ / 4) as f64, (PPQ / 4) as f64, 1.0),
            "{t}"
        );
    }
}
