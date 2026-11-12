//! The gesture recogniser on synthetic hands: no camera, no models, no network.

mod support;

use atomik_vision::gesture::{GestureConfig, GestureEvent, HandId, HandState, Recogniser};
use atomik_vision::{Handedness, Observation};
use support::{Spec, load};

fn run(frames: &[Observation]) -> (Vec<GestureEvent>, Vec<Vec<HandState>>) {
    let mut r = Recogniser::new(GestureConfig::default());
    let mut events = Vec::new();
    let mut states = Vec::new();
    for f in frames {
        let u = r.update(f);
        events.extend(u.events);
        states.push(u.states);
    }
    (events, states)
}

fn count(events: &[GestureEvent], pick: impl Fn(&GestureEvent) -> bool) -> usize {
    events.iter().filter(|e| pick(e)).count()
}

fn twist_sum(events: &[GestureEvent], id: HandId) -> f32 {
    events
        .iter()
        .map(|e| match e {
            GestureEvent::Twist {
                hand,
                delta_radians,
            } if *hand == id => *delta_radians,
            _ => 0.0,
        })
        .sum()
}

fn is_start(e: &GestureEvent) -> bool {
    matches!(e, GestureEvent::PinchStart { .. })
}
fn is_end(e: &GestureEvent) -> bool {
    matches!(e, GestureEvent::PinchEnd { .. })
}
fn is_twist(e: &GestureEvent) -> bool {
    matches!(e, GestureEvent::Twist { .. })
}

/// Removes the hands from the frames `from..to`.
fn drop_frames(mut frames: Vec<Observation>, from: usize, to: usize) -> Vec<Observation> {
    for f in &mut frames[from..to] {
        f.hands.clear();
    }
    frames
}

#[test]
fn the_fixtures_are_what_the_generator_writes() {
    for (name, frames) in support::fixtures() {
        assert_eq!(
            load(name),
            frames,
            "{name} is stale: run with ATOMIK_UPDATE_GOLDEN=1"
        );
    }
}

#[test]
fn a_pinch_a_quarter_turn_and_a_release() {
    let (events, _) = run(&load("pinch_twist.json"));
    assert_eq!(count(&events, is_start), 1, "{events:?}");
    assert_eq!(count(&events, is_end), 1, "{events:?}");
    let first = events.iter().position(is_start).unwrap();
    let last = events.iter().position(is_end).unwrap();
    assert!(first < last);
    assert!(events[first..last].iter().any(is_twist));
    assert!(
        events[..first].iter().all(|e| !is_twist(e)),
        "no twist before the pinch"
    );
    assert!(
        events[last..].iter().all(|e| !is_twist(e)),
        "no twist after the release"
    );
    let total = twist_sum(&events, HandId(0)).to_degrees();
    assert!((total - 90.0).abs() <= 2.0, "twists sum to {total} degrees");
}

#[test]
fn a_still_hand_gives_no_twist_whatever_the_jitter() {
    let (events, _) = run(&load("still_jitter.json"));
    assert_eq!(count(&events, is_start), 1);
    assert_eq!(count(&events, is_twist), 0, "{events:?}");
}

#[test]
fn one_missed_frame_keeps_the_pinch_but_300_ms_of_loss_ends_it() {
    let frames = load("pinch_twist.json");
    // Frame 45 is half way through the turn.
    let (events, _) = run(&drop_frames(frames.clone(), 45, 46));
    assert_eq!(
        count(&events, is_end),
        1,
        "only the real release ends the pinch: {events:?}"
    );
    assert_eq!(count(&events, is_start), 1);
    assert!((twist_sum(&events, HandId(0)).to_degrees() - 90.0).abs() <= 2.0);

    // Nine frames (300 ms) lost in the hold after the turn: the last frame seen is 63.
    let lost = drop_frames(frames, 64, 73);
    let mut r = Recogniser::new(GestureConfig::default());
    let mut started = false;
    let mut ended_at = None;
    for (i, f) in lost.iter().enumerate() {
        let u = r.update(f);
        started |= u.events.iter().any(is_start);
        if u.events.iter().any(is_end) {
            ended_at = Some(i);
            break;
        }
    }
    let at = ended_at.expect("the pinch ended");
    assert!(started);
    // It ended during the loss, not at the release (the hand is back at frame 73), and not
    // before about 150 ms had passed.
    assert!((68..73).contains(&at), "ended at frame {at}");
}

#[test]
fn the_angle_unwraps_across_180_degrees_both_ways() {
    let (events, _) = run(&load("cross_180.json"));
    let total = twist_sum(&events, HandId(0)).to_degrees();
    assert!((total - 30.0).abs() <= 2.0, "forward: {total}");
    let jump = events.iter().any(
        |e| matches!(e, GestureEvent::Twist { delta_radians, .. } if delta_radians.abs() > 0.5),
    );
    assert!(!jump, "no jump of a half turn: {events:?}");

    let (events, _) = run(&support::cross_180(false));
    let total = twist_sum(&events, HandId(0)).to_degrees();
    assert!((total + 30.0).abs() <= 2.0, "backward: {total}");
}

#[test]
fn no_flicker_between_the_thresholds() {
    let (events, _) = run(&load("threshold_hover.json"));
    assert_eq!(count(&events, is_start), 1, "{events:?}");
    assert_eq!(count(&events, is_end), 1, "{events:?}");
}

#[test]
fn two_hands_are_kept_apart() {
    let frames = load("two_hands.json");
    let (events, states) = run(&frames);
    let starts: Vec<(HandId, Handedness)> = events
        .iter()
        .filter_map(|e| match e {
            GestureEvent::PinchStart { hand, handedness } => Some((*hand, *handedness)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2, "{events:?}");
    assert_ne!(starts[0].0, starts[1].0);
    let left = starts.iter().find(|s| s.1 == Handedness::Left).unwrap().0;
    let right = starts.iter().find(|s| s.1 == Handedness::Right).unwrap().0;
    assert!((twist_sum(&events, left).to_degrees() - 40.0).abs() <= 2.0);
    assert!((twist_sum(&events, right).to_degrees() + 60.0).abs() <= 2.0);
    assert_eq!(count(&events, is_end), 2);
    // Both ids stay put for the whole sequence.
    for frame in &states {
        assert_eq!(frame.len(), 2);
        for s in frame {
            assert!(s.id == left || s.id == right);
        }
    }
}

#[test]
fn the_output_is_deterministic() {
    let frames = load("pinch_twist.json");
    let a = run(&frames);
    let b = run(&frames);
    assert_eq!(a, b);
}

#[test]
fn a_hand_the_model_doubts_is_ignored() {
    let frames = support::sequence(30, |_, _| {
        vec![Spec {
            pinch_ratio: 0.05,
            score: 0.3,
            ..Spec::right()
        }]
    });
    let (events, states) = run(&frames);
    assert!(events.is_empty());
    assert!(states.iter().all(|s| s.is_empty()));
}

#[test]
fn the_state_reports_pinch_strength_size_and_depth() {
    let near = support::sequence(5, |_, _| {
        vec![Spec {
            size: 0.4,
            pinch_ratio: 1.0,
            ..Spec::right()
        }]
    });
    let far = support::sequence(5, |_, _| {
        vec![Spec {
            size: 0.2,
            pinch_ratio: 0.05,
            ..Spec::right()
        }]
    });
    let (_, near) = run(&near);
    let (_, far) = run(&far);
    let (n, f) = (&near[4][0], &far[4][0]);
    assert!(n.present && !n.pinch && f.pinch);
    assert!(
        n.pinch_strength < 0.05 && f.pinch_strength > 0.95,
        "{} {}",
        n.pinch_strength,
        f.pinch_strength
    );
    assert!((n.hand_size - 0.4).abs() < 0.01 && (f.hand_size - 0.2).abs() < 0.01);
    assert!(n.depth_proxy > f.depth_proxy, "a closer hand is bigger");
    assert!((n.palm_center.0 - 0.5).abs() < 0.03 && (n.palm_center.1 - 0.55).abs() < 0.03);
    assert!(n.palm_height > 0.0);
    assert_eq!(n.handedness, Handedness::Right);
}

#[test]
fn a_reset_ends_the_pinches_that_are_open() {
    let frames = load("pinch_twist.json");
    let mut r = Recogniser::new(GestureConfig::default());
    for f in &frames[..40] {
        r.update(f);
    }
    let ended = r.reset();
    assert_eq!(ended.len(), 1);
    assert!(is_end(&ended[0]));
    assert!(r.reset().is_empty());
}

// ---- The open palm ----------------------------------------------------------------------------

fn is_palm_start(e: &GestureEvent) -> bool {
    matches!(e, GestureEvent::OpenPalmStart { .. })
}
fn is_palm_end(e: &GestureEvent) -> bool {
    matches!(e, GestureEvent::OpenPalmEnd { .. })
}

/// The frame index of the first event `pick` finds.
fn frame_of(frames: &[Observation], pick: impl Fn(&GestureEvent) -> bool) -> Option<usize> {
    let mut r = Recogniser::new(GestureConfig::default());
    frames
        .iter()
        .position(|f| r.update(f).events.iter().any(&pick))
}

#[test]
fn an_open_palm_held_for_300_ms_starts_and_closing_the_fingers_ends_it() {
    let frames = load("open_palm.json");
    let (events, states) = run(&frames);
    assert_eq!(count(&events, is_palm_start), 1, "{events:?}");
    assert_eq!(count(&events, is_palm_end), 1, "{events:?}");
    // The hand opens at 0.4 s (frame 12): the start comes 300 ms later, at frame 21.
    let start = frame_of(&frames, is_palm_start).unwrap();
    assert!((20..=22).contains(&start), "started at frame {start}");
    let end = frame_of(&frames, is_palm_end).unwrap();
    assert!((54..=56).contains(&end), "ended at frame {end}");
    assert!(!states[start - 1][0].open_palm && states[start][0].open_palm);
    assert!(!states[end][0].open_palm);
}

#[test]
fn a_palm_that_opens_for_200_ms_is_not_an_open_palm() {
    let (events, states) = run(&load("open_flash.json"));
    assert_eq!(count(&events, is_palm_start), 0, "{events:?}");
    assert_eq!(count(&events, is_palm_end), 0);
    assert!(states.iter().all(|f| f.iter().all(|s| !s.open_palm)));
}

#[test]
fn a_fist_is_not_an_open_palm() {
    let frames = support::sequence(60, |_, _| {
        vec![Spec {
            curl: 1.0,
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_pinch_ends_the_open_palm_and_an_open_palm_is_never_a_pinch() {
    let (events, _) = run(&load("pinch_twist.json"));
    // The hand starts open, so the palm starts, then the pinch takes over.
    let palm = events.iter().position(is_palm_start).expect("a palm");
    let pinch = events.iter().position(is_start).expect("a pinch");
    let palm_end = events.iter().position(is_palm_end).expect("the palm ends");
    assert!(palm < pinch && palm_end < events.iter().position(is_twist).unwrap());
    assert_eq!(count(&events, is_start), 1);
}

#[test]
fn losing_the_hand_ends_the_open_palm() {
    let frames = drop_frames(load("open_palm.json"), 30, 60);
    let (events, _) = run(&frames);
    assert_eq!(count(&events, is_palm_start), 1);
    assert_eq!(count(&events, is_palm_end), 1, "{events:?}");
    let end = frame_of(&frames, is_palm_end).unwrap();
    assert!(
        (34..=40).contains(&end),
        "ended at frame {end}, after the tolerance"
    );
}

#[test]
fn a_reset_ends_the_open_palm() {
    let frames = load("open_palm.json");
    let mut r = Recogniser::new(GestureConfig::default());
    for f in &frames[..30] {
        r.update(f);
    }
    let ended = r.reset();
    assert!(ended.iter().any(is_palm_end), "{ended:?}");
}
