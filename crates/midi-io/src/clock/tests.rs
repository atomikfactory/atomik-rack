use super::*;
use atomik_engine::{ExternalClock, START};

#[allow(clippy::disallowed_methods)] // Justification: a test anchor; nothing reads the wall clock.
fn base() -> Instant {
    Instant::now()
}

fn follower() -> (ClockFollower, Arc<ExternalClock>) {
    let cell = ExternalClock::new();
    (ClockFollower::new(Arc::clone(&cell)), cell)
}

/// The seconds between two clocks at `bpm`.
fn gap(bpm: f64) -> f64 {
    60.0 / (bpm * 24.0)
}

/// Sends `count` clocks from `*t` at `bpm`, advancing `*t`.
fn clocks_at(f: &mut ClockFollower, t0: Instant, t: &mut f64, count: usize, bpm: f64) {
    for _ in 0..count {
        f.on_message(&[0xF8], t0 + Duration::from_secs_f64(*t));
        *t += gap(bpm);
    }
}

#[test]
fn a_steady_clock_reads_its_tempo_within_a_fifth_of_a_bpm_after_two_seconds() {
    let (mut f, cell) = follower();
    let t0 = base();
    let mut t = 0.0;
    clocks_at(&mut f, t0, &mut t, 80, 100.0); // two seconds
    let bpm = cell.read().tempo.unwrap();
    assert!((bpm - 100.0).abs() < 0.2, "{bpm}");
    assert_eq!(cell.read().clocks, 80);
}

#[test]
fn there_is_no_tempo_before_a_quarter_of_a_beat_has_passed() {
    let (mut f, cell) = follower();
    let t0 = base();
    let mut t = 0.0;
    clocks_at(&mut f, t0, &mut t, 6, 100.0);
    assert_eq!(cell.read().tempo, None);
    clocks_at(&mut f, t0, &mut t, 1, 100.0);
    assert!(cell.read().tempo.is_some());
}

#[test]
fn port_jitter_averages_out() {
    let (mut f, cell) = follower();
    let t0 = base();
    // Each clock up to a millisecond early or late, in a fixed scramble.
    for k in 0..200u32 {
        let jitter = f64::from((k * 7919) % 2001) / 1000.0 - 1.0; // -1..1 ms
        let at = f64::from(k) * gap(120.0) + jitter / 1000.0;
        f.on_message(&[0xF8], t0 + Duration::from_secs_f64(at + 0.01));
    }
    let bpm = cell.read().tempo.unwrap();
    assert!((bpm - 120.0).abs() < 0.8, "{bpm}");
}

#[test]
fn a_ramp_from_100_to_140_is_followed_within_a_beat() {
    let (mut f, cell) = follower();
    let t0 = base();
    let mut t = 0.0;
    clocks_at(&mut f, t0, &mut t, 48, 100.0);
    let mut last = cell.read().tempo.unwrap();
    assert!((last - 100.0).abs() < 0.2);
    // Four beats of linear ramp, 24 clocks a beat.
    for k in 0..96 {
        let bpm = 100.0 + 40.0 * f64::from(k) / 96.0;
        clocks_at(&mut f, t0, &mut t, 1, bpm);
        let now = cell.read().tempo.unwrap();
        assert!(
            now >= last - 1e-9,
            "the estimate never goes back: {last} then {now}"
        );
        last = now;
    }
    // A beat of lag at most: the estimate is the mean of the last beat.
    assert!((130.0..140.0).contains(&last), "{last}");
    clocks_at(&mut f, t0, &mut t, 24, 140.0);
    let bpm = cell.read().tempo.unwrap();
    assert!((bpm - 140.0).abs() < 0.2, "{bpm}");
}

#[test]
fn start_continue_stop_and_the_position_change_the_transport() {
    let (mut f, cell) = follower();
    let t = base();
    f.on_message(&[0xF2, 8, 0], t);
    let s = cell.read();
    assert_eq!((s.position, s.locates, s.running), (2 * 15_360, 1, false));
    f.on_message(&[0xFB], t);
    assert!(cell.read().running);
    f.on_message(&[0xFC], t);
    assert!(!cell.read().running);
    f.on_message(&[START], t);
    let s = cell.read();
    assert_eq!((s.position, s.locates, s.running), (0, 2, true));
    // The two data bytes of a position are 7 bits each, least significant first.
    f.on_message(&[0xF2, 0x7F, 0x7F], t);
    assert_eq!(cell.read().position, 16_383 * 3_840);
}

#[test]
fn a_lost_clock_keeps_the_last_tempo_and_a_returning_one_measures_afresh() {
    let (mut f, cell) = follower();
    let t0 = base();
    let mut t = 0.0;
    clocks_at(&mut f, t0, &mut t, 48, 100.0);
    let before = cell.read();
    // A second of nothing: the tempo and the clock count stay as they were.
    t += 1.0;
    assert_eq!(cell.read().tempo, before.tempo);
    // It comes back at 160 BPM: the old intervals are forgotten, so seven clocks give the new tempo.
    clocks_at(&mut f, t0, &mut t, 6, 160.0);
    assert_eq!(cell.read().tempo, before.tempo, "not enough yet");
    clocks_at(&mut f, t0, &mut t, 1, 160.0);
    let bpm = cell.read().tempo.unwrap();
    assert!((bpm - 160.0).abs() < 0.2, "{bpm}");
}

#[test]
fn other_messages_are_ignored_and_nothing_panics() {
    let (mut f, cell) = follower();
    let t = base();
    for b in 0..=255u8 {
        for msg in [
            vec![b],
            vec![b, 1],
            vec![b, 2, 3],
            vec![b, 0xF8],
            Vec::new(),
        ] {
            f.on_message(&msg, t);
        }
    }
    let _ = cell.read();
    let (mut f, cell) = follower();
    f.on_message(&[0x90, 60, 100], t);
    f.on_message(&[0xF0, 1, 2, 0xF7], t);
    f.on_message(&[0xFE], t);
    assert_eq!(cell.read(), ExternalState::default());
}
