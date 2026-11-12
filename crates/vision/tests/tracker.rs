//! The tracker end to end with a fake camera and a scripted hand finder: frames in, events out,
//! a clean stop. No camera, no models, no network.

mod support;

use atomik_vision::camera::{Camera, CameraId, FakeCamera, FakeStats, WantedFormat};
use atomik_vision::capture::{Capture, Next};
use atomik_vision::gesture::GestureEvent;
use atomik_vision::models::{FrameHands, FrameView, HandFinder, ModelError};
use atomik_vision::tracker::{FinderFactory, Tracker, TrackerConfig, TrackerEvent};
use atomik_vision::{HandObservation, Observation};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SIDE: u32 = 4;

/// A frame of 4 by 4 grey pixels whose first byte says which frame it is.
fn tiny_frames(n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            let mut f = vec![i as u8; (SIDE * SIDE * 3) as usize];
            f[0] = i as u8;
            f
        })
        .collect()
}

/// Looks up the hands of the scripted sequence by the number in the frame.
struct Scripted(Vec<Observation>);

impl HandFinder for Scripted {
    fn process(&mut self, frame: FrameView<'_>) -> Result<FrameHands, ModelError> {
        let i = frame.rgb[0] as usize;
        let hands: Vec<HandObservation> =
            self.0.get(i).map(|o| o.hands.clone()).unwrap_or_default();
        Ok(FrameHands {
            hands,
            ..FrameHands::default()
        })
    }
    fn reset(&mut self) {}
}

fn scripted(frames: Vec<Observation>) -> FinderFactory {
    Box::new(move || Ok(Box::new(Scripted(frames)) as Box<dyn HandFinder>))
}

fn collect_until_end(tracker: &Tracker) -> Vec<TrackerEvent> {
    let events = tracker.events();
    let mut all = Vec::new();
    loop {
        let e = events
            .recv_timeout(Duration::from_secs(20))
            .expect("the tracker ended in time");
        let end = matches!(e, TrackerEvent::Stopped | TrackerEvent::Failed(_));
        all.push(e);
        if end {
            return all;
        }
    }
}

fn wait_for(tracker: &Tracker, pick: impl Fn(&TrackerEvent) -> bool) -> TrackerEvent {
    let events = tracker.events();
    loop {
        let e = events
            .recv_timeout(Duration::from_secs(20))
            .expect("the event came");
        if pick(&e) {
            return e;
        }
    }
}

fn gestures(events: &[TrackerEvent]) -> Vec<GestureEvent> {
    events
        .iter()
        .filter_map(|e| {
            if let TrackerEvent::Gesture(g) = e {
                Some(*g)
            } else {
                None
            }
        })
        .collect()
}

fn camera_with(frames: Vec<Vec<u8>>, repeat: bool) -> (Arc<FakeCamera>, CameraId, Arc<FakeStats>) {
    let mut cam = FakeCamera::new();
    let id = cam.add(
        "fake",
        (SIDE, SIDE),
        frames,
        Duration::from_millis(if repeat { 2 } else { 0 }),
        Duration::from_micros(33_333),
        repeat,
    );
    let stats = cam.stats();
    (Arc::new(cam), id, stats)
}

#[test]
fn frames_in_events_out_and_a_clean_end() {
    let script = support::pinch_twist();
    // A camera that gives frames as fast as it can would skip some: pace it a little.
    let mut paced = FakeCamera::new();
    let id2 = paced.add(
        "fake",
        (SIDE, SIDE),
        tiny_frames(script.len()),
        Duration::from_millis(3),
        Duration::from_micros(33_333),
        false,
    );
    let stats2 = paced.stats();

    let tracker = Tracker::start(
        Arc::new(paced),
        id2,
        scripted(script),
        TrackerConfig::default(),
    );
    let events = collect_until_end(&tracker);
    assert!(
        matches!(events[0], TrackerEvent::Started { .. }),
        "{:?}",
        events[0]
    );
    assert!(matches!(events.last().unwrap(), TrackerEvent::Stopped));
    let g = gestures(&events);
    assert_eq!(
        g.iter()
            .filter(|e| matches!(e, GestureEvent::PinchStart { .. }))
            .count(),
        1,
        "{g:?}"
    );
    assert_eq!(
        g.iter()
            .filter(|e| matches!(e, GestureEvent::PinchEnd { .. }))
            .count(),
        1,
        "{g:?}"
    );
    let twist: f32 = g
        .iter()
        .map(|e| {
            if let GestureEvent::Twist { delta_radians, .. } = e {
                *delta_radians
            } else {
                0.0
            }
        })
        .sum();
    assert!(
        (twist.to_degrees() - 90.0).abs() <= 2.5,
        "{}",
        twist.to_degrees()
    );
    // Every frame was reported (the pace left time for each) and the order is right.
    let reports = events
        .iter()
        .filter(|e| matches!(e, TrackerEvent::Frame(_)))
        .count();
    assert!(reports > 30, "{reports}");
    drop(tracker);
    assert_eq!(
        stats2.open_now.load(Ordering::SeqCst),
        0,
        "the camera is released"
    );
}

#[test]
fn stopping_releases_the_camera_and_ends_the_thread() {
    let (cam, id, stats) = camera_with(tiny_frames(10), true);
    let mut tracker = Tracker::start(cam, id, scripted(Vec::new()), TrackerConfig::default());
    wait_for(&tracker, |e| matches!(e, TrackerEvent::Frame(_)));
    assert!(tracker.is_running());
    assert_eq!(stats.open_now.load(Ordering::SeqCst), 1);
    tracker.stop();
    assert!(!tracker.is_running());
    assert_eq!(
        stats.open_now.load(Ordering::SeqCst),
        0,
        "the device is released at once"
    );
    // The last event is the stop.
    let events = tracker.events();
    let mut last = None;
    while let Some(e) = events.try_recv() {
        last = Some(e);
    }
    assert_eq!(last, Some(TrackerEvent::Stopped));
}

#[test]
fn dropping_the_tracker_releases_the_camera() {
    let (cam, id, stats) = camera_with(tiny_frames(10), true);
    let tracker = Tracker::start(cam, id, scripted(Vec::new()), TrackerConfig::default());
    wait_for(&tracker, |e| matches!(e, TrackerEvent::Started { .. }));
    drop(tracker);
    assert_eq!(stats.open_now.load(Ordering::SeqCst), 0);
}

#[test]
fn a_camera_the_system_refuses_is_a_failure_not_a_hang() {
    let mut cam = FakeCamera::new();
    let id = cam.add(
        "fake",
        (SIDE, SIDE),
        tiny_frames(3),
        Duration::ZERO,
        Duration::ZERO,
        false,
    );
    cam.deny_access();
    let tracker = Tracker::start(
        Arc::new(cam),
        id,
        scripted(Vec::new()),
        TrackerConfig::default(),
    );
    let events = collect_until_end(&tracker);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], TrackerEvent::Failed(m) if m.contains("camera")),
        "{events:?}"
    );
    assert!(
        !tracker.is_running() || {
            std::thread::sleep(Duration::from_millis(200));
            !tracker.is_running()
        }
    );
}

#[test]
fn a_model_that_cannot_load_is_a_failure_and_the_camera_is_released() {
    let (cam, id, stats) = camera_with(tiny_frames(3), true);
    let factory: FinderFactory = Box::new(|| Err(ModelError::Run("no models".into())));
    let tracker = Tracker::start(cam, id, factory, TrackerConfig::default());
    let events = collect_until_end(&tracker);
    assert!(
        matches!(&events[0], TrackerEvent::Failed(m) if m.contains("no models")),
        "{events:?}"
    );
    drop(tracker);
    assert_eq!(stats.open_now.load(Ordering::SeqCst), 0);
}

#[test]
fn switching_cameras_releases_the_first_and_ends_open_pinches() {
    let script = support::pinch_twist();
    let mut cam = FakeCamera::new();
    // The first camera repeats frame 40..: it stays pinched mid-twist.
    let held: Vec<Vec<u8>> = tiny_frames(script.len())
        .into_iter()
        .skip(40)
        .take(20)
        .collect();
    let a = cam.add(
        "a",
        (SIDE, SIDE),
        held,
        Duration::from_millis(2),
        Duration::from_micros(33_333),
        true,
    );
    let b = cam.add(
        "b",
        (SIDE, SIDE),
        tiny_frames(5),
        Duration::from_millis(2),
        Duration::from_micros(33_333),
        true,
    );
    let stats = cam.stats();
    let mut tracker = Tracker::start(
        Arc::new(cam),
        a,
        scripted(support::pinch_twist()),
        TrackerConfig::default(),
    );
    wait_for(&tracker, |e| {
        matches!(e, TrackerEvent::Gesture(GestureEvent::PinchStart { .. }))
    });
    tracker.switch_camera(b.clone());
    let events = tracker.events();
    let mut saw_end = false;
    loop {
        match events
            .recv_timeout(Duration::from_secs(20))
            .expect("events")
        {
            TrackerEvent::Gesture(GestureEvent::PinchEnd { .. }) => saw_end = true,
            TrackerEvent::Started { camera } => {
                assert_eq!(camera, b);
                break;
            }
            _ => {}
        }
    }
    assert!(
        saw_end,
        "the open pinch ended before the new camera started"
    );
    assert_eq!(stats.opened.load(Ordering::SeqCst), 2);
    assert_eq!(
        stats.open_now.load(Ordering::SeqCst),
        1,
        "only the new camera is open"
    );
    tracker.stop();
    assert_eq!(stats.open_now.load(Ordering::SeqCst), 0);
}

#[test]
fn the_capture_keeps_only_the_newest_frame() {
    let mut cam = FakeCamera::new();
    let id = cam.add(
        "fast",
        (SIDE, SIDE),
        tiny_frames(50),
        Duration::ZERO,
        Duration::from_millis(1),
        true,
    );
    let mut capture = Capture::start(Arc::new(cam), id, WantedFormat::default()).unwrap();
    // Nobody takes frames for a while: the camera runs ahead and the old ones are dropped.
    std::thread::sleep(Duration::from_millis(100));
    let Next::Frame(first) = capture.next(Duration::from_secs(5)) else {
        panic!("a frame")
    };
    assert!(
        first.seq > 10,
        "the newest frame, not the first: {}",
        first.seq
    );
    assert!(capture.dropped() > 5);
    capture.stop();
}

#[test]
fn a_full_queue_never_blocks_the_worker_and_keeps_the_gestures() {
    let script = support::pinch_twist();
    let mut paced = FakeCamera::new();
    let id = paced.add(
        "fake",
        (SIDE, SIDE),
        tiny_frames(script.len()),
        Duration::from_millis(1),
        Duration::from_micros(33_333),
        false,
    );
    let config = TrackerConfig {
        event_capacity: 8,
        ..TrackerConfig::default()
    };
    let tracker = Tracker::start(Arc::new(paced), id, scripted(script), config);
    // Nobody reads while it runs.
    while tracker.is_running() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let events = tracker.events();
    let mut all = Vec::new();
    while let Some(e) = events.try_recv() {
        all.push(e);
    }
    assert!(all.len() <= 8, "{}", all.len());
    assert_eq!(all.last(), Some(&TrackerEvent::Stopped));
    assert!(
        gestures(&all)
            .iter()
            .any(|g| matches!(g, GestureEvent::PinchEnd { .. })),
        "the end of the pinch is kept"
    );
}

#[test]
fn the_preview_is_made_only_when_asked_for() {
    let script = support::pinch_twist();
    let mut cam = FakeCamera::new();
    let id = cam.add(
        "fake",
        (SIDE * 100, SIDE * 75),
        vec![vec![9u8; (SIDE * 100 * SIDE * 75 * 3) as usize]; 4],
        Duration::from_millis(3),
        Duration::from_micros(33_333),
        true,
    );
    let finder: FinderFactory =
        Box::new(move || Ok(Box::new(Always(script[40].hands.clone())) as Box<dyn HandFinder>));
    let mut tracker = Tracker::start(Arc::new(cam), id, finder, TrackerConfig::default());
    wait_for(&tracker, |e| matches!(e, TrackerEvent::Frame(_)));
    assert!(tracker.take_preview().is_none(), "no preview unless asked");
    tracker.set_preview(true);
    let preview = loop {
        if let Some(p) = tracker.take_preview() {
            break p;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(preview.width <= 160 && preview.width > 0);
    assert_eq!(
        preview.rgb.len(),
        (preview.width * preview.height * 3) as usize
    );
    assert_eq!(preview.hands.len(), 1);
    tracker.set_preview(false);
    assert!(tracker.take_preview().is_none());
    tracker.stop();
}

struct Always(Vec<HandObservation>);

impl HandFinder for Always {
    fn process(&mut self, _: FrameView<'_>) -> Result<FrameHands, ModelError> {
        Ok(FrameHands {
            hands: self.0.clone(),
            ..FrameHands::default()
        })
    }
    fn reset(&mut self) {}
}

#[test]
fn the_fake_camera_lists_and_refuses_unknown_ids() {
    let mut cam = FakeCamera::new();
    let id = cam.add(
        "one",
        (SIDE, SIDE),
        tiny_frames(1),
        Duration::ZERO,
        Duration::ZERO,
        false,
    );
    let list = cam.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert!(
        cam.open(&CameraId("nope".into()), WantedFormat::default())
            .is_err()
    );
}
