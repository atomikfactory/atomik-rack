//! The counters of caught UI panics, the catch itself and the test switch.

use super::*;

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn run_returns_the_value_and_catches_a_panic_with_its_text() {
    assert_eq!(run(|| 7), Ok(7));
    let caught = run(|| -> u32 { panic!("boom {}", 3) });
    assert_eq!(caught, Err("boom 3".to_owned()));
    let caught = run(|| -> u32 { panic!("plain") });
    assert_eq!(caught, Err("plain".to_owned()));
}

#[test]
fn the_first_panic_raises_the_notice_and_a_second_soon_after_does_not() {
    let mut g = UiGuard::default();
    assert_eq!(
        g.caught(Dispatch::Event, s(10)),
        Verdict {
            notice: true,
            halt: false
        }
    );
    assert!(!g.caught(Dispatch::Event, s(11)).notice, "same point");
    assert!(
        !g.caught(Dispatch::Frame, s(12)).notice,
        "another point, still quiet"
    );
    assert_eq!(g.total(), 3);
    // After the quiet period a new panic raises it again.
    assert!(g.caught(Dispatch::Event, s(10) + NOTICE_QUIET).notice);
}

#[test]
fn consecutive_panicking_frames_halt_the_window_once() {
    let mut g = UiGuard::default();
    for i in 1..MAX_CONSECUTIVE_PANICS {
        let v = g.caught(Dispatch::Paint, s(u64::from(i)));
        assert!(!v.halt, "paint {i}");
    }
    assert!(!g.is_halted());
    let v = g.caught(Dispatch::Paint, s(6));
    assert!(v.halt && v.notice, "the halt always says so");
    assert!(g.is_halted());
    // Already halted: no second halt, no new notice.
    let v = g.caught(Dispatch::Paint, s(7));
    assert!(!v.halt && !v.notice);
}

#[test]
fn a_run_without_a_panic_resets_that_points_streak_only() {
    let mut g = UiGuard::default();
    for _ in 0..MAX_CONSECUTIVE_PANICS - 1 {
        g.caught(Dispatch::Frame, s(1));
        g.caught(Dispatch::Paint, s(1));
    }
    g.completed(Dispatch::Frame);
    assert!(
        !g.caught(Dispatch::Frame, s(2)).halt,
        "its streak started over"
    );
    assert!(
        g.caught(Dispatch::Paint, s(2)).halt,
        "the paints went on panicking while ticks succeeded"
    );
}

#[test]
fn event_panics_never_halt_the_content() {
    let mut g = UiGuard::default();
    for i in 0..MAX_CONSECUTIVE_PANICS * 3 {
        assert!(!g.caught(Dispatch::Event, s(u64::from(i))).halt);
    }
    assert!(!g.is_halted());
}

#[test]
fn an_unattended_run_that_caught_a_panic_exits_with_a_failure() {
    assert!(exit_failure(true, 1).is_some());
    assert!(exit_failure(true, 7).unwrap().contains('7'));
    assert!(exit_failure(true, 0).is_none());
    assert!(
        exit_failure(false, 3).is_none(),
        "an attended session exits normally"
    );
    assert!(exit_failure(false, 0).is_none());
}

#[test]
fn the_test_switch_parses_and_counts_down() {
    assert_eq!(ForcedPanic::parse(None), None);
    assert_eq!(ForcedPanic::parse(Some("")), None);
    assert_eq!(ForcedPanic::parse(Some("sideways")), None);
    assert_eq!(ForcedPanic::parse(Some("paint:0")), None);
    assert_eq!(ForcedPanic::parse(Some("paint:x")), None);
    let mut f = ForcedPanic::parse(Some("frame:2")).unwrap();
    assert!(!f.take(Dispatch::Paint));
    assert!(f.take(Dispatch::Frame));
    assert!(f.take(Dispatch::Frame));
    assert!(!f.take(Dispatch::Frame), "used up");
    let mut once = ForcedPanic::parse(Some(" event ")).unwrap();
    assert!(once.take(Dispatch::Event));
    assert!(!once.take(Dispatch::Event));
}
