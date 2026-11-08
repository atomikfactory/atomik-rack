use super::*;

const RATE: u32 = 48_000;

fn tone(hz: f32, amp: f32, frames: usize, active: (usize, usize)) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            if i < active.0 || i >= active.1 {
                return 0.0;
            }
            let cycles = (i as f64 * f64::from(hz) / f64::from(RATE)).fract();
            amp * (cycles * std::f64::consts::TAU).sin() as f32
        })
        .collect()
}

fn map_of(signal: &[f32]) -> BandMap {
    analyse(signal, signal, RATE)
}

const SECONDS: usize = 6;
const FRAMES: usize = RATE as usize * SECONDS;

#[test]
fn bands_are_third_octaves_from_forty_hertz() {
    assert!((band_centre(0) - 40.0).abs() < 0.01);
    assert!((band_centre(3) - 80.0).abs() < 0.01);
    let (lo, hi) = band_edges(9);
    assert!((hi / lo - 2.0f32.powf(1.0 / 3.0)).abs() < 1e-3);
    assert_eq!(band_of(250.0), band_of(band_centre(band_of(250.0))));
    assert!((band_centre(band_of(250.0)) / 250.0 - 1.0).abs() < 0.13);
    assert_eq!(band_of(5.0), 0);
    assert_eq!(band_of(90_000.0), BANDS - 1);
    assert_eq!(band_label(0), "40 Hz");
    assert!(band_label(BANDS - 1).ends_with("kHz"));
    // Edges touch: the upper edge of a band is the lower edge of the next.
    for i in 0..BANDS - 1 {
        let (_, hi) = band_edges(i);
        let (lo, _) = band_edges(i + 1);
        assert!((hi / lo - 1.0).abs() < 1e-4, "band {i}");
    }
}

#[test]
fn a_sine_reads_its_own_level_in_its_own_band() {
    let hz = band_centre(band_of(1000.0));
    let map = map_of(&tone(hz, 0.5, RATE as usize, (0, usize::MAX)));
    let row = map.levels[map.levels.len() / 2];
    let band = band_of(hz);
    let want = 20.0 * 0.5f32.log10();
    assert!(
        (row[band] - want).abs() < 1.5,
        "read {} want {want}",
        row[band]
    );
    // The neighbours read far less.
    for (b, level) in row.iter().enumerate() {
        if b.abs_diff(band) >= 2 {
            assert!(*level < row[band] - 20.0, "band {b} reads {level}");
        }
    }
}

#[test]
fn silence_reads_the_floor_and_is_never_active() {
    let map = map_of(&vec![0.0; RATE as usize]);
    assert!(map.levels.iter().flatten().all(|l| *l == SILENT_DB));
    assert_eq!(map.peak(), SILENT_DB);
    let loud = map_of(&tone(300.0, 0.5, RATE as usize, (0, usize::MAX)));
    assert!(rank_pairs(&[&map, &loud], 5).is_empty());
}

#[test]
fn the_analysis_is_deterministic_and_ignores_non_finite_samples() {
    let mut a = tone(440.0, 0.4, RATE as usize, (0, usize::MAX));
    let first = map_of(&a);
    assert_eq!(first, map_of(&a));
    a[1000] = f32::NAN;
    a[2000] = f32::INFINITY;
    let second = map_of(&a);
    assert!(second.levels.iter().flatten().all(|l| l.is_finite()));
}

#[test]
fn a_block_covers_the_frames_it_names() {
    // A tone in the second half of the track: the blocks of the first half read the floor.
    let half = FRAMES / 2;
    let map = map_of(&tone(500.0, 0.5, FRAMES, (half, FRAMES)));
    let band = band_of(500.0);
    let early = HOP * 2;
    assert!(map.levels[early / HOP][band] < ACTIVE_DB);
    let late = (half + HOP * 4) / HOP;
    assert!(map.levels[late][band] > -12.0);
    assert_eq!(BandMap::block_start(3), 3 * HOP as u64);
}

#[test]
fn two_tracks_in_one_band_rank_first_with_that_band_and_the_quieter_is_masked() {
    let both = (0, usize::MAX);
    // 250 Hz in both tracks, one louder; a third track elsewhere in the spectrum.
    let loud = map_of(&tone(250.0, 0.5, FRAMES, both));
    let quiet = map_of(&tone(250.0, 0.12, FRAMES, both));
    let high = map_of(&tone(4000.0, 0.3, FRAMES, both));
    let ranked = rank_pairs(&[&high, &quiet, &loud], 10);
    assert!(!ranked.is_empty());
    let top = &ranked[0];
    assert_eq!(top.masker, 2, "the loud 250 Hz track masks");
    assert_eq!(top.masked, 1, "the quiet 250 Hz track is covered");
    assert_eq!(top.worst, band_of(250.0));
    assert!(top.score > 50.0, "score {}", top.score);
    assert!(top.zone()[band_of(250.0)]);
    // The track at 4 kHz shares nothing with either of them.
    assert!(ranked.iter().all(|p| p.masker != 0 && p.masked != 0));
    // The spectra of the pair show the lead in the clash band.
    assert!(top.masker_db[top.worst] > top.masked_db[top.worst] + 6.0);
    // The whole song clashes: one range from the start to the end.
    assert_eq!(top.ranges.len(), 1);
    assert_eq!(top.ranges[0].0, 0);
}

#[test]
fn the_ranges_are_the_times_both_tracks_sound() {
    let sec = RATE as usize;
    // The loud track plays seconds 1 to 3, the quiet one seconds 2 to 5: they meet in 2..3.
    let loud = map_of(&tone(250.0, 0.5, FRAMES, (sec, 3 * sec)));
    let quiet = map_of(&tone(250.0, 0.1, FRAMES, (2 * sec, 5 * sec)));
    let ranked = rank_pairs(&[&loud, &quiet], 3);
    let top = &ranked[0];
    assert_eq!((top.masker, top.masked), (0, 1));
    assert_eq!(top.ranges.len(), 1);
    let (from, to) = top.ranges[0];
    let start = BandMap::block_start(from) as f64 / f64::from(RATE);
    let end = BandMap::block_start(to) as f64 / f64::from(RATE);
    assert!((start - 2.0).abs() < 0.4, "starts at {start}");
    assert!((end - 3.0).abs() < 0.4, "ends at {end}");
    // Scored over the whole six seconds: one second of six.
    assert!(top.score > 10.0 && top.score < 25.0, "score {}", top.score);
}

#[test]
fn equal_levels_do_not_mask_and_tracks_that_never_meet_do_not_clash() {
    let both = (0, usize::MAX);
    let a = map_of(&tone(250.0, 0.3, FRAMES, both));
    let b = map_of(&tone(250.0, 0.29, FRAMES, both));
    assert!(
        rank_pairs(&[&a, &b], 3).is_empty(),
        "within the minimum gap"
    );
    let sec = RATE as usize;
    let first = map_of(&tone(250.0, 0.5, FRAMES, (0, 2 * sec)));
    let second = map_of(&tone(250.0, 0.1, FRAMES, (3 * sec, 5 * sec)));
    assert!(
        rank_pairs(&[&first, &second], 3).is_empty(),
        "never together"
    );
}

#[test]
fn the_noise_floor_of_a_band_limited_track_is_not_content() {
    // A loud 250 Hz track and a quiet 1 kHz track with a faint 250 Hz hum 50 dB below it: the
    // hum is under the activity span of the track's own peak, so it is not masked content.
    let both = (0, usize::MAX);
    let loud = map_of(&tone(250.0, 0.5, FRAMES, both));
    let mut mix = tone(1000.0, 0.3, FRAMES, both);
    for (m, h) in mix.iter_mut().zip(tone(250.0, 0.0008, FRAMES, both)) {
        *m += h;
    }
    let other = map_of(&mix);
    assert!(rank_pairs(&[&loud, &other], 3).is_empty());
}

#[test]
fn the_ranking_orders_by_score_and_respects_the_limit() {
    let both = (0, usize::MAX);
    let sec = RATE as usize;
    let a = map_of(&tone(250.0, 0.5, FRAMES, both));
    let b = map_of(&tone(250.0, 0.1, FRAMES, both));
    let c = map_of(&tone(250.0, 0.1, FRAMES, (0, 2 * sec)));
    let ranked = rank_pairs(&[&a, &b, &c], 10);
    assert!(ranked.len() >= 2);
    for w in ranked.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
    assert_eq!(rank_pairs(&[&a, &b, &c], 1).len(), 1);
}

#[test]
fn a_mean_over_ranges_ignores_the_rest() {
    let sec = RATE as usize;
    let map = map_of(&tone(500.0, 0.5, FRAMES, (0, 2 * sec)));
    let band = band_of(500.0);
    let inside = map.mean(&[(0, 2 * sec / HOP - 2)]);
    let all = map.mean(&[]);
    assert!(inside[band] > all[band] + 3.0);
    assert_eq!(map.mean(&[(1000, 1010)])[band], SILENT_DB);
}

#[test]
fn short_and_empty_tracks_do_not_panic() {
    assert!(analyse(&[], &[], RATE).levels.is_empty());
    let tiny = analyse(&[0.5; 10], &[], RATE);
    assert_eq!(tiny.levels.len(), 1);
    let mono = analyse(&tone(1000.0, 0.5, 4000, (0, 4000)), &[], RATE);
    assert_eq!(mono.levels.len(), 1);
    assert!(rank_pairs(&[], 4).is_empty());
}
