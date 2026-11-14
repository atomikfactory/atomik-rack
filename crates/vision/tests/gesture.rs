//! The gesture recogniser on synthetic hands: no camera, no models, no network.

mod support;

use atomik_vision::gesture::{
    GestureConfig, GestureEvent, HandId, HandState, Pose, Recogniser, SwipeDirection,
};
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
    assert_eq!(count(&events, is_palm_start), 0, "{events:?}");
    assert_eq!(count(&events, is_start), 0, "{events:?}");
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

// ---- The poses --------------------------------------------------------------------------------

fn is_pose_start(pose: Pose) -> impl Fn(&GestureEvent) -> bool {
    move |e| matches!(e, GestureEvent::PoseStart { pose: p, .. } if *p == pose)
}
fn is_pose_end(pose: Pose) -> impl Fn(&GestureEvent) -> bool {
    move |e| matches!(e, GestureEvent::PoseEnd { pose: p, .. } if *p == pose)
}
fn any_pose(e: &GestureEvent) -> bool {
    matches!(
        e,
        GestureEvent::PoseStart { .. } | GestureEvent::PoseEnd { .. }
    )
}
fn swipes(events: &[GestureEvent]) -> Vec<SwipeDirection> {
    events
        .iter()
        .filter_map(|e| match e {
            GestureEvent::Swipe { direction, .. } => Some(*direction),
            _ => None,
        })
        .collect()
}

/// Each pose of the vocabulary starts once after the 300 ms hold, shows in the state while it
/// lasts, and ends when the hand opens, whatever the other poses do.
#[test]
fn each_pose_starts_after_300_ms_and_ends_when_the_hand_opens() {
    for (name, pose) in [
        ("fist.json", Pose::Fist),
        ("thumbs_up.json", Pose::ThumbsUp),
        ("point.json", Pose::Point),
        ("peace.json", Pose::Peace),
    ] {
        let frames = load(name);
        let (events, states) = run(&frames);
        assert_eq!(count(&events, is_pose_start(pose)), 1, "{name}: {events:?}");
        assert_eq!(count(&events, is_pose_end(pose)), 1, "{name}: {events:?}");
        assert_eq!(
            count(&events, any_pose),
            2,
            "{name} is the only pose: {events:?}"
        );
        // The hand holds the pose from the first frame: the start comes 300 ms later (frame 9).
        let start = frame_of(&frames, is_pose_start(pose)).unwrap();
        assert!((8..=10).contains(&start), "{name} started at frame {start}");
        // It opens at 1.2 s (frame 36).
        let end = frame_of(&frames, is_pose_end(pose)).unwrap();
        assert!((36..=37).contains(&end), "{name} ended at frame {end}");
        assert_eq!(states[start][0].pose, Some(pose), "{name}");
        assert_eq!(states[start - 1][0].pose, None, "{name}");
        assert_eq!(states[end][0].pose, None, "{name}");
        // None of them is a pinch, and the open hand afterwards is an open palm.
        assert_eq!(count(&events, is_start), 0, "{name}: {events:?}");
        assert_eq!(count(&events, is_palm_start), 1, "{name}: {events:?}");
    }
}

#[test]
fn an_open_hand_is_no_pose() {
    let frames = support::sequence(60, |_, _| vec![Spec::right()]);
    let (events, states) = run(&frames);
    assert_eq!(count(&events, any_pose), 0, "{events:?}");
    assert!(states.iter().all(|f| f[0].pose.is_none()));
}

#[test]
fn a_pose_held_for_less_than_300_ms_is_not_a_pose() {
    // A fist for 200 ms between two open hands.
    let frames = support::sequence(60, |_, t| {
        vec![Spec {
            curl: if (0.4..0.6).contains(&t) { 1.0 } else { 0.0 },
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert_eq!(count(&events, any_pose), 0, "{events:?}");
}

#[test]
fn a_pose_that_changes_to_another_ends_and_the_other_starts_after_its_own_hold() {
    // A point for a second, then a peace for a second.
    let frames = support::sequence(60, |_, t| {
        vec![Spec {
            curl: 1.0,
            folded: if t < 1.0 {
                [false, true, true, true]
            } else {
                [false, false, true, true]
            },
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    let point_end = events.iter().position(is_pose_end(Pose::Point)).unwrap();
    let peace_start = events.iter().position(is_pose_start(Pose::Peace)).unwrap();
    assert!(point_end < peace_start, "{events:?}");
    assert_eq!(count(&events, is_pose_start(Pose::Point)), 1);
    assert_eq!(count(&events, is_pose_start(Pose::Peace)), 1);
    let at = frame_of(&frames, is_pose_start(Pose::Peace)).unwrap();
    assert!(
        (38..=40).contains(&at),
        "the peace holds 300 ms from 1.0 s: frame {at}"
    );
}

#[test]
fn a_pose_is_held_again_after_the_hand_changes_pose() {
    // Fist, open, fist: the second fist starts a second time (that is how a command re-arms).
    let frames = support::sequence(90, |_, t| {
        vec![Spec {
            curl: if (1.0..1.8).contains(&t) { 0.0 } else { 1.0 },
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert_eq!(count(&events, is_pose_start(Pose::Fist)), 2, "{events:?}");
    assert_eq!(count(&events, is_pose_end(Pose::Fist)), 1, "{events:?}");
}

#[test]
fn no_pose_while_the_fingers_are_pinched() {
    // A hand with the other fingers folded and the thumb on the index finger is a pinch.
    let frames = support::sequence(60, |_, _| {
        vec![Spec {
            curl: 1.0,
            folded: [false, true, true, true],
            pinch_ratio: 0.05,
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert_eq!(count(&events, is_start), 1, "{events:?}");
    assert_eq!(count(&events, any_pose), 0, "{events:?}");
}

#[test]
fn losing_the_hand_ends_the_pose_and_a_reset_does_too() {
    let frames = drop_frames(load("fist.json"), 20, 40);
    let (events, _) = run(&frames);
    assert_eq!(count(&events, is_pose_end(Pose::Fist)), 1, "{events:?}");

    let frames = load("peace.json");
    let mut r = Recogniser::new(GestureConfig::default());
    for f in &frames[..20] {
        r.update(f);
    }
    let ended = r.reset();
    assert!(ended.iter().any(is_pose_end(Pose::Peace)), "{ended:?}");
}

#[test]
fn a_fist_of_one_hand_is_told_apart_by_hand() {
    let frames = support::sequence(40, |_, _| {
        vec![
            Spec {
                center: (0.25, 0.55),
                curl: 1.0,
                ..Spec::left()
            },
            Spec {
                center: (0.75, 0.55),
                ..Spec::right()
            },
        ]
    });
    let (events, _) = run(&frames);
    let hand = events
        .iter()
        .find_map(|e| match e {
            GestureEvent::PoseStart { handedness, .. } => Some(*handedness),
            _ => None,
        })
        .expect("the left fist starts");
    assert_eq!(hand, Handedness::Left);
    assert_eq!(count(&events, any_pose), 1);
}

// ---- The swipe --------------------------------------------------------------------------------

#[test]
fn a_fast_sweep_across_the_frame_is_one_swipe_in_its_direction() {
    // The raw image is not mirrored: a palm that goes to the left of the raw image goes to the
    // right of the preview the user looks at, and that is a swipe right.
    let (events, _) = run(&load("swipe_left.json"));
    assert_eq!(swipes(&events), vec![SwipeDirection::Right], "{events:?}");
    let (events, _) = run(&load("swipe_right.json"));
    assert_eq!(swipes(&events), vec![SwipeDirection::Left], "{events:?}");
}

#[test]
fn a_slow_drift_over_the_same_distance_is_no_swipe() {
    let (events, _) = run(&load("drift.json"));
    assert!(swipes(&events).is_empty(), "{events:?}");
}

#[test]
fn a_swipe_carries_the_hand_and_comes_once_per_stroke() {
    let frames = support::sequence(100, |_, t| {
        let t = t as f32;
        // Right, rest, back: two strokes of 300 ms.
        let x = if t < 0.5 {
            0.25
        } else if t < 0.8 {
            support::lerp(0.25, 0.8, (t - 0.5) / 0.3)
        } else if t < 1.6 {
            0.8
        } else if t < 1.9 {
            support::lerp(0.8, 0.25, (t - 1.6) / 0.3)
        } else {
            0.25
        };
        vec![Spec {
            center: (x, 0.55),
            ..Spec::left()
        }]
    });
    let (events, _) = run(&frames);
    assert_eq!(
        swipes(&events),
        vec![SwipeDirection::Left, SwipeDirection::Right],
        "{events:?}"
    );
    assert!(events.iter().all(|e| match e {
        GestureEvent::Swipe { handedness, .. } => *handedness == Handedness::Left,
        _ => true,
    }));
}

#[test]
fn a_pinched_hand_that_moves_fast_is_no_swipe() {
    let frames = support::sequence(60, |_, t| {
        let f = (t as f32 - 0.5) / 0.3;
        vec![Spec {
            center: (support::lerp(0.8, 0.25, f), 0.55),
            pinch_ratio: 0.05,
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert!(swipes(&events).is_empty(), "{events:?}");
}

#[test]
fn a_vertical_sweep_is_no_swipe() {
    let frames = support::sequence(60, |_, t| {
        let f = (t as f32 - 0.5) / 0.3;
        vec![Spec {
            center: (0.5, support::lerp(0.85, 0.2, f)),
            ..Spec::right()
        }]
    });
    let (events, _) = run(&frames);
    assert!(swipes(&events).is_empty(), "{events:?}");
}

// ---- The two-hand pad -------------------------------------------------------------------------

fn pad_events(events: &[GestureEvent]) -> (usize, usize) {
    (
        count(events, |e| matches!(e, GestureEvent::PadStart)),
        count(events, |e| matches!(e, GestureEvent::PadEnd)),
    )
}

#[test]
fn both_hands_pinched_make_a_pad_with_the_midpoint_and_the_spread() {
    let frames = load("two_hand_pad.json");
    let mut r = Recogniser::new(GestureConfig::default());
    let mut events = Vec::new();
    let mut pads = Vec::new();
    for f in &frames {
        let u = r.update(f);
        events.extend(u.events);
        pads.push(u.pad);
    }
    assert_eq!(pad_events(&events), (1, 1), "{events:?}");
    let start = events
        .iter()
        .position(|e| matches!(e, GestureEvent::PadStart))
        .unwrap();
    assert_eq!(
        events[..start].iter().filter(|e| is_start(e)).count(),
        2,
        "the pad starts after both pinches: {events:?}"
    );
    // No pad before the right hand pinches at 0.5 s (frame 15) and none after 2 s (frame 60).
    assert!(pads[..15].iter().all(Option::is_none));
    assert!(pads[66..].iter().all(Option::is_none));
    // At 0.93 s the hands are still at 0.4 and 0.6: the midpoint is the middle of the frame.
    let early = pads[28].expect("pad at 0.93 s");
    assert!((early.x - 0.5).abs() < 0.02, "x {}", early.x);
    assert!((early.y - 0.6).abs() < 0.02, "y {}", early.y);
    // The spread is the distance between the palms in frame heights (0.2 of the width).
    let want = 0.2 * support::ASPECT;
    assert!(
        (early.spread - want).abs() < 0.03,
        "spread {}",
        early.spread
    );
    // At 1.93 s the hands are at 0.2 and 0.8, both at 0.4 high: spread 0.6 of the width, y up.
    let late = pads[58].expect("pad at 1.93 s");
    assert!((late.x - 0.5).abs() < 0.03, "x {}", late.x);
    assert!((late.y - 0.4).abs() < 0.03, "y {}", late.y);
    let want = 0.6 * support::ASPECT;
    assert!((late.spread - want).abs() < 0.05, "spread {}", late.spread);
    assert!(late.spread > early.spread);
}

#[test]
fn one_pinched_hand_is_no_pad() {
    let (events, _) = run(&load("pinch_twist.json"));
    assert_eq!(pad_events(&events), (0, 0));
}

#[test]
fn the_pad_ends_when_either_hand_lets_go_or_is_lost() {
    let frames = load("two_hand_pad.json");
    // Both hands vanish at 1.5 s for good: the pad ends once the tolerance is over.
    let lost = drop_frames(frames.clone(), 45, 90);
    let (events, _) = run(&lost);
    assert_eq!(pad_events(&events), (1, 1), "{events:?}");
    // A reset ends it too.
    let mut r = Recogniser::new(GestureConfig::default());
    for f in &frames[..40] {
        r.update(f);
    }
    let ended = r.reset();
    assert!(
        ended.iter().any(|e| matches!(e, GestureEvent::PadEnd)),
        "{ended:?}"
    );
}
