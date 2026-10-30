//! The warp-on-import decision: metadata, bar-length detection and the three choices.

use super::*;

const RATE: u32 = 48_000;
const BAR: i64 = 4 * PPQ;

/// Frames of `bars` 4/4 bars at `bpm`.
fn bars_at(bars: u32, bpm: f64) -> u64 {
    (f64::from(bars) * 4.0 * 60.0 * f64::from(RATE) / bpm).round() as u64
}

fn decide(meta: &FileMeta, frames: u64, mode: WarpImports) -> Option<ImportWarp> {
    import_warp(meta, frames, RATE, 120.0, BAR, mode)
}

#[test]
fn choices_have_ids_labels_and_cycle() {
    for m in WarpImports::ALL {
        assert_eq!(WarpImports::from_id(m.id()), Some(m));
    }
    assert_eq!(WarpImports::from_id("nope"), None);
    assert_eq!(WarpImports::default(), WarpImports::Loops);
    assert_eq!(WarpImports::Loops.label(), "Loops only");
    let mut m = WarpImports::Off;
    for _ in 0..3 {
        m = m.next();
    }
    assert_eq!(m, WarpImports::Off);
}

#[test]
fn a_four_bar_loop_at_100_bpm_warps_to_exactly_four_bars() {
    let w = decide(&FileMeta::default(), bars_at(4, 100.0), WarpImports::Loops).unwrap();
    assert!((w.source_bpm - 100.0).abs() < 0.01, "{}", w.source_bpm);
    assert_eq!(w.length, 4 * BAR);
}

#[test]
fn one_shots_and_long_recordings_are_left_alone() {
    let none = FileMeta::default();
    // Half a second, a hit; 37.3 seconds, a take; 10.4 seconds, no round tempo.
    for frames in [24_000u64, 1_790_400, 500_000] {
        assert_eq!(decide(&none, frames, WarpImports::Loops), None, "{frames}");
    }
}

#[test]
fn the_tempo_of_the_file_wins_and_its_beats_set_the_length() {
    let meta = FileMeta {
        tempo_bpm: Some(93.0),
        beats: Some(8),
        ..FileMeta::default()
    };
    // The file is a little longer than its 8 beats at 93 BPM (a tail): the stated beat count
    // is within 2% of the stated tempo, so the exact tempo is used and the clip is 8 beats.
    let frames = (8.0 * 60.0 * 48_000.0 / 93.0 * 1.01) as u64;
    let w = decide(&meta, frames, WarpImports::Loops).unwrap();
    assert_eq!(w.length, 8 * PPQ);
    assert!((w.source_bpm - 93.0 / 1.01).abs() < 0.01);
    // A stated tempo without beats: the length follows from the frames.
    let meta = FileMeta {
        tempo_bpm: Some(100.0),
        ..FileMeta::default()
    };
    let w = decide(&meta, bars_at(2, 100.0), WarpImports::Loops).unwrap();
    assert_eq!((w.source_bpm, w.length), (100.0, 2 * BAR));
    // A beat count that disagrees with the tempo is ignored.
    let meta = FileMeta {
        tempo_bpm: Some(100.0),
        beats: Some(3),
        ..FileMeta::default()
    };
    let w = decide(&meta, bars_at(2, 100.0), WarpImports::Loops).unwrap();
    assert_eq!((w.source_bpm, w.length), (100.0, 2 * BAR));
}

#[test]
fn a_file_with_metadata_warps_whatever_its_length_and_a_one_shot_flag_wins() {
    let meta = FileMeta {
        tempo_bpm: Some(128.0),
        ..FileMeta::default()
    };
    assert!(decide(&meta, 1_790_400, WarpImports::Loops).is_some());
    let shot = FileMeta {
        one_shot: true,
        ..meta
    };
    assert_eq!(decide(&shot, bars_at(4, 100.0), WarpImports::Loops), None);
}

#[test]
fn off_never_warps_and_all_warps_everything() {
    let meta = FileMeta {
        tempo_bpm: Some(128.0),
        ..FileMeta::default()
    };
    assert_eq!(decide(&meta, bars_at(4, 100.0), WarpImports::Off), None);
    let w = decide(&FileMeta::default(), 24_000, WarpImports::All).unwrap();
    // Unknown: the song tempo, so the clip keeps its real duration (0.5 s at 120 BPM: 1 beat).
    assert_eq!((w.source_bpm, w.length), (120.0, PPQ));
}

#[test]
fn of_several_bar_counts_the_tempo_nearest_the_song_wins_and_zero_frames_give_none() {
    // 8 seconds is 4 bars at 120 BPM and 8 bars at 240 (out of range): 4 bars at 120.
    let w = decide(&FileMeta::default(), 8 * 48_000, WarpImports::Loops).unwrap();
    assert_eq!((w.source_bpm, w.length), (120.0, 4 * BAR));
    assert_eq!(decide(&FileMeta::default(), 0, WarpImports::All), None);
    assert_eq!(
        import_warp(&FileMeta::default(), 1, 0, 120.0, BAR, WarpImports::All),
        None
    );
}

#[test]
fn bars_follow_the_meter() {
    // 3/4: a 2-bar loop at 90 BPM is 6 beats = 4 seconds.
    let bar34 = 3 * PPQ;
    let w = import_warp(
        &FileMeta::default(),
        4 * 48_000,
        RATE,
        90.0,
        bar34,
        WarpImports::Loops,
    )
    .unwrap();
    assert_eq!((w.source_bpm, w.length), (90.0, 2 * bar34));
}
