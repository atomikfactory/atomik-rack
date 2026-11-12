//! Capture on a thread of its own, with a handoff of one frame.
//!
//! The camera is opened and read on the capture thread. Each frame replaces the one in the slot
//! if nobody took it: the newest frame wins, nothing queues up, and a slow consumer costs
//! frames, never memory. Stopping asks the thread to leave; it drops the camera (which releases
//! the device) right after the frame it is waiting for.

use crate::camera::{Camera, CameraError, CameraId, FrameSource, WantedFormat};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long [`Capture::stop`] waits for a camera that does not answer before it lets go.
pub const STOP_PATIENCE: Duration = Duration::from_secs(3);

/// How many bad frames in a row end the capture.
const MAX_BAD_FRAMES: u32 = 30;

/// One frame, in memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGB8, `width * height * 3` bytes.
    pub rgb: Vec<u8>,
    /// Time since the camera was opened.
    pub timestamp: Duration,
    /// How many frames the camera had given when this one arrived (counting from 1).
    pub seq: u64,
}

/// What [`Capture::next`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// A frame (the newest; older ones that nobody took are gone).
    Frame(Frame),
    /// Nothing arrived in time.
    Timeout,
    /// The capture ended: the camera failed (`Some`) or a fake one ran out of frames (`None`).
    Ended(Option<CameraError>),
}

#[derive(Default)]
struct SlotState {
    frame: Option<Frame>,
    ended: Option<Option<CameraError>>,
    taken: u64,
    produced: u64,
}

#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
    ready: Condvar,
}

impl Slot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A running capture.
pub struct Capture {
    slot: Arc<Slot>,
    stop: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Opens the camera on a new thread and starts reading it. Returns when the camera is open
    /// (or why it is not), so the caller knows the device is in use.
    pub fn start(
        camera: Arc<dyn Camera>,
        id: CameraId,
        wanted: WantedFormat,
    ) -> Result<Self, CameraError> {
        let slot = Arc::new(Slot::default());
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let (opened_tx, opened_rx) = mpsc::channel::<Result<(), CameraError>>();
        let (t_slot, t_stop, t_done) = (Arc::clone(&slot), Arc::clone(&stop), Arc::clone(&done));
        let thread = std::thread::Builder::new()
            .name("atomik-camera".into())
            .spawn(move || {
                match camera.open(&id, wanted) {
                    Ok(source) => {
                        let _ = opened_tx.send(Ok(()));
                        run(source, &t_slot, &t_stop);
                    }
                    Err(e) => {
                        let _ = opened_tx.send(Err(e));
                    }
                }
                let (flag, cv) = &*t_done;
                *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
                cv.notify_all();
            })
            .map_err(|e| CameraError::Open(e.to_string()))?;
        match opened_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                slot,
                stop,
                done,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CameraError::Open(
                    "the camera thread ended before it opened".into(),
                ))
            }
        }
    }

    /// Waits up to `timeout` for a frame that was not taken yet.
    pub fn next(&self, timeout: Duration) -> Next {
        let mut state = self.slot.lock();
        if state.frame.is_none() && state.ended.is_none() {
            state = self
                .slot
                .ready
                .wait_timeout_while(state, timeout, |s| s.frame.is_none() && s.ended.is_none())
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if let Some(frame) = state.frame.take() {
            state.taken += 1;
            return Next::Frame(frame);
        }
        match state.ended.clone() {
            Some(reason) => Next::Ended(reason),
            None => Next::Timeout,
        }
    }

    /// How many frames were thrown away because a newer one arrived before they were taken.
    pub fn dropped(&self) -> u64 {
        let s = self.slot.lock();
        s.produced - s.taken - u64::from(s.frame.is_some())
    }

    /// Stops reading and releases the camera. Waits for the capture thread to finish (up to
    /// [`STOP_PATIENCE`]: a camera driver that hangs cannot freeze the caller).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let (flag, cv) = &*self.done;
        let guard = flag.lock().unwrap_or_else(|e| e.into_inner());
        let (finished, _) = cv
            .wait_timeout_while(guard, STOP_PATIENCE, |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        if *finished {
            drop(finished);
            let _ = thread.join();
        } else {
            log::warn!("the camera did not answer; it is released when it does");
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(mut source: Box<dyn FrameSource>, slot: &Slot, stop: &AtomicBool) {
    let mut spare: Vec<u8> = Vec::new();
    let mut bad = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let mut rgb = std::mem::take(&mut spare);
        match source.next_frame(&mut rgb) {
            Ok(meta) => {
                bad = 0;
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut s = slot.lock();
                s.produced += 1;
                let seq = s.produced;
                let old = s.frame.replace(Frame {
                    width: meta.width,
                    height: meta.height,
                    rgb,
                    timestamp: meta.timestamp,
                    seq,
                });
                drop(s);
                // A frame nobody took gives its buffer to the next one.
                spare = old.map(|f| f.rgb).unwrap_or_default();
                slot.ready.notify_all();
            }
            Err(CameraError::Ended) => {
                end(slot, None);
                break;
            }
            Err(e @ CameraError::Frame(_)) => {
                bad += 1;
                if bad >= MAX_BAD_FRAMES {
                    end(slot, Some(e));
                    break;
                }
            }
            Err(e) => {
                end(slot, Some(e));
                break;
            }
        }
    }
    // `source` is dropped here: the device is released.
}

fn end(slot: &Slot, reason: Option<CameraError>) {
    slot.lock().ended = Some(reason);
    slot.ready.notify_all();
}
