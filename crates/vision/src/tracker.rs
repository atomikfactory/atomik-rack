//! The tracker: camera, models and recogniser on one worker thread.
//!
//! [`Tracker::start`] returns at once; the worker opens the camera (on its own capture thread),
//! builds the hand finder and then handles one frame at a time: find the hands, run the
//! recogniser, send [`TrackerEvent`]s on a bounded queue. The newest frame is the only one kept
//! (see [`crate::capture`]); a frame that takes longer than the camera's period costs frames,
//! never latency. The worker never touches the audio thread: the consumer reads the events.
//!
//! The camera is off until [`Tracker::start`] and off again when the tracker is stopped or
//! dropped; frames live in memory only and are gone with the tracker.

use crate::camera::convert::shrink_by;
use crate::camera::{Camera, CameraId, WantedFormat};
use crate::capture::{Capture, Frame, Next};
use crate::clock::now;
use crate::gesture::{GestureConfig, GestureEvent, HandState, Recogniser};
use crate::hand::{HandObservation, Observation};
use crate::models::{FrameView, HandFinder, HandModels, HandPipeline, ModelError, PipelineConfig};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the worker tells its consumer.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackerEvent {
    /// The camera is open and frames are being read.
    Started {
        /// The camera.
        camera: CameraId,
    },
    /// A gesture changed.
    Gesture(GestureEvent),
    /// One frame was handled. These are dropped first when the consumer is slow.
    Frame(FrameReport),
    /// The tracker cannot go on (the camera or a model failed); it has stopped.
    Failed(String),
    /// The tracker stopped (asked to, or a camera that replays frames ran out).
    Stopped,
}

/// What one frame gave.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReport {
    /// The time of the frame, in seconds since the camera was opened.
    pub t: f64,
    /// Every hand followed.
    pub hands: Vec<HandState>,
    /// Frames per second, smoothed.
    pub fps: f32,
    /// Milliseconds the models took for this frame, smoothed.
    pub infer_ms: f32,
}

/// A small picture of the last frame with the hands on it, for a preview window.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGB8, `width * height * 3` bytes.
    pub rgb: Vec<u8>,
    /// The raw landmarks of the hands found (normalised, 0 to 1 over the picture).
    pub hands: Vec<HandObservation>,
    /// The state of every hand followed.
    pub states: Vec<HandState>,
}

/// The numbers of a tracker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackerConfig {
    /// The picture asked of the camera.
    pub wanted: WantedFormat,
    /// The recogniser.
    pub gesture: GestureConfig,
    /// How many events wait for the consumer before the oldest are dropped.
    pub event_capacity: usize,
    /// How often the preview is made while it is wanted, in seconds.
    pub preview_period: f64,
    /// The preview is at most this wide.
    pub preview_width: u32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            wanted: WantedFormat::default(),
            gesture: GestureConfig::default(),
            event_capacity: 256,
            preview_period: 0.1,
            preview_width: 160,
        }
    }
}

/// Builds the hand finder on the worker thread (loading the models takes a moment).
pub type FinderFactory = Box<dyn FnOnce() -> Result<Box<dyn HandFinder>, ModelError> + Send>;

/// A factory for the real models in `dir`.
pub fn models_factory(dir: PathBuf, cfg: PipelineConfig) -> FinderFactory {
    Box::new(move || {
        Ok(Box::new(HandPipeline::new(HandModels::load(&dir)?, cfg)) as Box<dyn HandFinder>)
    })
}

// The queue of events: bounded, never blocks the worker.

struct Queue {
    items: Mutex<VecDeque<TrackerEvent>>,
    ready: Condvar,
    capacity: usize,
}

impl Queue {
    fn push(&self, event: TrackerEvent) {
        let mut q = self.items.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= self.capacity {
            // Make room: the oldest frame report, else the oldest twist, else the oldest.
            let at = q
                .iter()
                .position(|e| matches!(e, TrackerEvent::Frame(_)))
                .or_else(|| {
                    q.iter().position(|e| {
                        matches!(e, TrackerEvent::Gesture(GestureEvent::Twist { .. }))
                    })
                })
                .unwrap_or(0);
            q.remove(at);
        }
        q.push_back(event);
        drop(q);
        self.ready.notify_all();
    }
}

/// The consumer's end of the events. Clone it freely.
#[derive(Clone)]
pub struct Events(Arc<Queue>);

impl Events {
    /// The oldest event waiting, if any.
    pub fn try_recv(&self) -> Option<TrackerEvent> {
        self.0
            .items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    /// Waits up to `timeout` for an event.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<TrackerEvent> {
        let q = self.0.items.lock().unwrap_or_else(|e| e.into_inner());
        let mut q = self
            .0
            .ready
            .wait_timeout_while(q, timeout, |q| q.is_empty())
            .unwrap_or_else(|e| e.into_inner())
            .0;
        q.pop_front()
    }
}

enum Command {
    Switch(CameraId),
}

struct Shared {
    stop: AtomicBool,
    running: AtomicBool,
    preview_wanted: AtomicBool,
    preview: Mutex<Option<Preview>>,
    commands: Mutex<VecDeque<Command>>,
    queue: Arc<Queue>,
}

/// A running tracker. Dropping it stops the worker and releases the camera.
pub struct Tracker {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Tracker {
    /// Starts tracking: the worker opens `camera` and runs until [`stop`](Self::stop), a drop or
    /// a failure. The camera is in use from the moment this is called until then.
    pub fn start(
        camera: Arc<dyn Camera>,
        id: CameraId,
        make_finder: FinderFactory,
        cfg: TrackerConfig,
    ) -> Self {
        let queue = Arc::new(Queue {
            items: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            capacity: cfg.event_capacity.max(4),
        });
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            running: AtomicBool::new(true),
            preview_wanted: AtomicBool::new(false),
            preview: Mutex::new(None),
            commands: Mutex::new(VecDeque::new()),
            queue,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("atomik-vision".into())
            .spawn(move || {
                work(&worker_shared, camera, id, make_finder, cfg);
                worker_shared.running.store(false, Ordering::SeqCst);
            })
            .ok();
        if worker.is_none() {
            shared.running.store(false, Ordering::SeqCst);
            shared.queue.push(TrackerEvent::Failed(
                "cannot start the tracking thread".into(),
            ));
        }
        Self { shared, worker }
    }

    /// The events of this tracker.
    pub fn events(&self) -> Events {
        Events(Arc::clone(&self.shared.queue))
    }

    /// Whether the worker is running (the camera is in use).
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// Asks for the preview (or no longer wants it). While wanted, [`take_preview`](Self::take_preview)
    /// gets a new picture about ten times a second.
    pub fn set_preview(&self, wanted: bool) {
        self.shared.preview_wanted.store(wanted, Ordering::SeqCst);
        if !wanted {
            *self
                .shared
                .preview
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// The newest preview, once.
    pub fn take_preview(&self) -> Option<Preview> {
        self.shared
            .preview
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Switches to another camera: the first one is released, open pinches end, and the new one
    /// is opened. A camera that cannot be opened ends the tracker with a [`TrackerEvent::Failed`].
    pub fn switch_camera(&self, id: CameraId) {
        self.shared
            .commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(Command::Switch(id));
    }

    /// Stops the worker and releases the camera. Waits for the worker to finish.
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How the time of a frame without a camera clock is kept.
struct Pace {
    last_t: f64,
    last_wall: std::time::Instant,
    fps: f32,
    infer_ms: f32,
}

fn smooth(old: f32, new: f32) -> f32 {
    if old == 0.0 {
        new
    } else {
        old + (new - old) * 0.1
    }
}

fn work(
    shared: &Shared,
    camera: Arc<dyn Camera>,
    id: CameraId,
    make_finder: FinderFactory,
    cfg: TrackerConfig,
) {
    let queue = &shared.queue;
    let mut capture = match Capture::start(Arc::clone(&camera), id.clone(), cfg.wanted) {
        Ok(c) => c,
        Err(e) => {
            queue.push(TrackerEvent::Failed(e.to_string()));
            return;
        }
    };
    let mut finder = match make_finder() {
        Ok(f) => f,
        Err(e) => {
            capture.stop();
            queue.push(TrackerEvent::Failed(e.to_string()));
            return;
        }
    };
    queue.push(TrackerEvent::Started { camera: id });

    let mut recogniser = Recogniser::new(cfg.gesture);
    let mut pace = Pace {
        last_t: 0.0,
        last_wall: now(),
        fps: 0.0,
        infer_ms: 0.0,
    };
    let mut last_preview = f64::NEG_INFINITY;
    let mut failure: Option<String> = None;
    // The time of the first frame is the origin, so a camera that counts from anywhere works.
    let mut origin: Option<f64> = None;

    'run: loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let command = shared
            .commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front();
        if let Some(Command::Switch(next)) = command {
            capture.stop();
            for e in recogniser.reset() {
                queue.push(TrackerEvent::Gesture(e));
            }
            finder.reset();
            match Capture::start(Arc::clone(&camera), next.clone(), cfg.wanted) {
                Ok(c) => {
                    capture = c;
                    origin = None;
                    pace.last_t = 0.0;
                    queue.push(TrackerEvent::Started { camera: next });
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
            continue;
        }
        match capture.next(Duration::from_millis(100)) {
            Next::Frame(frame) => {
                let t0 = *origin.get_or_insert(frame.timestamp.as_secs_f64());
                let t = (frame.timestamp.as_secs_f64() - t0).max(0.0);
                let started = now();
                let found = match finder.process(FrameView {
                    width: frame.width as usize,
                    height: frame.height as usize,
                    rgb: &frame.rgb,
                }) {
                    Ok(f) => f,
                    Err(e) => {
                        failure = Some(e.to_string());
                        break 'run;
                    }
                };
                let took = started.elapsed().as_secs_f32() * 1000.0;
                let dt = t - pace.last_t;
                if dt > 0.0 && pace.last_t > 0.0 {
                    pace.fps = smooth(pace.fps, (1.0 / dt) as f32);
                }
                pace.last_t = t;
                pace.last_wall = now();
                pace.infer_ms = smooth(pace.infer_ms, took);
                let obs = Observation {
                    t,
                    aspect: frame.width as f32 / frame.height.max(1) as f32,
                    hands: found.hands,
                };
                let update = recogniser.update(&obs);
                for e in update.events {
                    queue.push(TrackerEvent::Gesture(e));
                }
                queue.push(TrackerEvent::Frame(FrameReport {
                    t,
                    hands: update.states.clone(),
                    fps: pace.fps,
                    infer_ms: pace.infer_ms,
                }));
                if shared.preview_wanted.load(Ordering::SeqCst)
                    && t - last_preview >= cfg.preview_period
                {
                    last_preview = t;
                    let preview = make_preview(&frame, obs.hands, update.states, cfg.preview_width);
                    *shared.preview.lock().unwrap_or_else(|e| e.into_inner()) = Some(preview);
                }
            }
            Next::Timeout => {
                // No frame for a while: time still passes for the pinch that waits for its hand.
                let waited = pace.last_wall.elapsed().as_secs_f64();
                if waited >= 0.2 && origin.is_some() {
                    let obs = Observation {
                        t: pace.last_t + waited,
                        aspect: 1.0,
                        hands: Vec::new(),
                    };
                    for e in recogniser.update(&obs).events {
                        queue.push(TrackerEvent::Gesture(e));
                    }
                }
            }
            Next::Ended(None) => break,
            Next::Ended(Some(e)) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }

    for e in recogniser.reset() {
        queue.push(TrackerEvent::Gesture(e));
    }
    capture.stop();
    match failure {
        Some(message) => queue.push(TrackerEvent::Failed(message)),
        None => queue.push(TrackerEvent::Stopped),
    }
}

fn make_preview(
    frame: &Frame,
    hands: Vec<HandObservation>,
    states: Vec<HandState>,
    max_width: u32,
) -> Preview {
    let factor = frame.width.div_ceil(max_width.max(1)).max(1) as usize;
    let (width, height) = (frame.width / factor as u32, frame.height / factor as u32);
    let rgb = if factor == 1 {
        frame.rgb.clone()
    } else {
        shrink_by(&frame.rgb, frame.width, frame.height, factor)
    };
    Preview {
        width,
        height,
        rgb,
        hands,
        states,
    }
}
