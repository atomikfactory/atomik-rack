use super::*;

const RATE: u32 = 48_000;
const FRAMES: usize = RATE as usize * 6;

fn tone(hz: f32, amp: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let cycles = (i as f64 * f64::from(hz) / f64::from(RATE)).fract();
            amp * (cycles * std::f64::consts::TAU).sin() as f32
        })
        .collect()
}

#[test]
fn silence_and_an_empty_mix_read_silent() {
    let silent = analyse(&vec![0.0; FRAMES], &vec![0.0; FRAMES], RATE);
    let f = silent.features(0, FRAMES as u64);
    assert_eq!(f.loudness_db, SILENT_DB);
    assert_eq!(f.brightness_hz, 0.0);
    assert_eq!(f.width, 0.0);
    let empty = analyse(&[], &[], RATE);
    assert!(empty.rows.is_empty());
    assert_eq!(empty.features(0, 100), Features::SILENT);
    // An empty range reads silent too.
    let s = analyse(&tone(440.0, 0.5, FRAMES), &[], RATE);
    assert_eq!(s.features(500, 500), Features::SILENT);
}

#[test]
fn six_decibels_more_amplitude_reads_six_decibels_louder() {
    let quiet = analyse(&tone(1000.0, 0.1, FRAMES), &[], RATE);
    let loud = analyse(&tone(1000.0, 0.2, FRAMES), &[], RATE);
    let range = (RATE as u64, 5 * RATE as u64);
    let gap =
        loud.features(range.0, range.1).loudness_db - quiet.features(range.0, range.1).loudness_db;
    assert!((gap - 6.02).abs() < 0.2, "gap {gap}");
}

#[test]
fn a_higher_tone_reads_brighter() {
    let low = analyse(&tone(100.0, 0.5, FRAMES), &[], RATE);
    let high = analyse(&tone(4000.0, 0.5, FRAMES), &[], RATE);
    let range = (RATE as u64, 5 * RATE as u64);
    let (l, h) = (
        low.features(range.0, range.1).brightness_hz,
        high.features(range.0, range.1).brightness_hz,
    );
    assert!(l > 60.0 && l < 160.0, "low reads {l}");
    assert!(h > 2800.0 && h < 5600.0, "high reads {h}");
}

#[test]
fn width_runs_from_mono_to_opposite_phase() {
    let a = tone(300.0, 0.4, FRAMES);
    let b = tone(1700.0, 0.4, FRAMES);
    let inverted: Vec<f32> = a.iter().map(|x| -x).collect();
    let range = (RATE as u64, 5 * RATE as u64);
    let mono = analyse(&a, &a, RATE).features(range.0, range.1).width;
    let opposite = analyse(&a, &inverted, RATE)
        .features(range.0, range.1)
        .width;
    let unrelated = analyse(&a, &b, RATE).features(range.0, range.1).width;
    assert!(mono < 0.01, "mono {mono}");
    assert!(opposite > 0.99, "opposite {opposite}");
    assert!((unrelated - 0.707).abs() < 0.05, "unrelated {unrelated}");
    // A mono request (no right channel) is the same as equal channels.
    let single = analyse(&a, &[], RATE).features(range.0, range.1).width;
    assert!(single < 0.01);
}

#[test]
fn a_range_reads_only_its_own_stretch() {
    let mut signal = tone(200.0, 0.05, FRAMES / 2);
    signal.extend(tone(200.0, 0.5, FRAMES / 2));
    let blocks = analyse(&signal, &signal, RATE);
    let half = (FRAMES / 2) as u64;
    let first = blocks.features(0, half).loudness_db;
    let second = blocks.features(half, 2 * half).loudness_db;
    assert!(second - first > 15.0, "{first} then {second}");
}

#[test]
fn a_range_shorter_than_a_block_reads_the_nearest_block() {
    let blocks = analyse(&tone(500.0, 0.3, FRAMES), &[], RATE);
    // Between two block centres: nothing is inside, the nearest block is used.
    let from = centre(3) + 10;
    let f = blocks.features(from, from + 20);
    assert!(f.loudness_db > -30.0);
    // Past the end of the mix reads the last block.
    let late = blocks.features(10 * FRAMES as u64, 11 * FRAMES as u64);
    assert!(late.loudness_db > -30.0);
}

#[test]
fn non_finite_samples_count_as_silence() {
    let mut a = tone(500.0, 0.3, FRAMES);
    a[1000] = f32::NAN;
    a[2000] = f32::INFINITY;
    let f = analyse(&a, &a, RATE).features(0, FRAMES as u64);
    assert!(f.loudness_db.is_finite() && f.width.is_finite() && f.brightness_hz.is_finite());
}

#[test]
fn the_same_audio_gives_the_same_numbers() {
    let a = tone(440.0, 0.3, FRAMES);
    let b = tone(660.0, 0.2, FRAMES);
    let x = analyse(&a, &b, RATE);
    let y = analyse(&a, &b, RATE);
    assert_eq!(x, y);
}
