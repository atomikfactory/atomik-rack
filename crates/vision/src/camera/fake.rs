//! A camera that replays frames from memory, for tests and for runs without a device.

use super::{Camera, CameraError, CameraId, CameraInfo, FrameMeta, FrameSource, WantedFormat};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// What the fake cameras were asked to do, readable from any thread.
#[derive(Debug, Default)]
pub struct FakeStats {
    /// How many times a camera was opened.
    pub opened: AtomicUsize,
    /// How many are open now (a dropped source is closed).
    pub open_now: AtomicUsize,
    /// How many frames were handed out.
    pub frames: AtomicUsize,
}

struct Device {
    info: CameraInfo,
    width: u32,
    height: u32,
    frames: Arc<Vec<Vec<u8>>>,
    pace: Duration,
    step: Duration,
    repeat: bool,
}

/// A set of fake cameras.
#[derive(Default)]
pub struct FakeCamera {
    devices: Vec<Device>,
    stats: Arc<FakeStats>,
    denied: bool,
}

impl FakeCamera {
    /// No camera yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a camera that gives `frames` (each `width * height * 3` bytes, RGB), waiting `pace`
    /// before each one; the timestamp of frame `n` is `n * step` whatever the pace is, so a
    /// test can play 30 frames per second of picture in a few milliseconds. With `repeat` it
    /// starts over at the end, otherwise it then fails with [`CameraError::Ended`].
    pub fn add(
        &mut self,
        name: &str,
        (width, height): (u32, u32),
        frames: Vec<Vec<u8>>,
        pace: Duration,
        step: Duration,
        repeat: bool,
    ) -> CameraId {
        let id = CameraId(format!("fake-{}", self.devices.len()));
        self.devices.push(Device {
            info: CameraInfo {
                id: id.clone(),
                name: name.to_string(),
                description: "fake".to_string(),
            },
            width,
            height,
            frames: Arc::new(frames),
            pace,
            step,
            repeat,
        });
        id
    }

    /// Makes every open fail as if the system had refused the program.
    pub fn deny_access(&mut self) {
        self.denied = true;
    }

    /// The counters of what the cameras did.
    pub fn stats(&self) -> Arc<FakeStats> {
        Arc::clone(&self.stats)
    }
}

struct Source {
    width: u32,
    height: u32,
    frames: Arc<Vec<Vec<u8>>>,
    pace: Duration,
    step: Duration,
    repeat: bool,
    next: usize,
    count: usize,
    stats: Arc<FakeStats>,
}

impl FrameSource for Source {
    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<FrameMeta, CameraError> {
        if !self.pace.is_zero() {
            std::thread::sleep(self.pace);
        }
        if self.next >= self.frames.len() {
            if !self.repeat || self.frames.is_empty() {
                return Err(CameraError::Ended);
            }
            self.next = 0;
        }
        buf.clear();
        buf.extend_from_slice(&self.frames[self.next]);
        let timestamp = self.step * u32::try_from(self.count).unwrap_or(u32::MAX);
        self.count += 1;
        self.next += 1;
        self.stats.frames.fetch_add(1, Ordering::SeqCst);
        Ok(FrameMeta {
            width: self.width,
            height: self.height,
            timestamp,
        })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stats.open_now.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Camera for FakeCamera {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError> {
        Ok(self.devices.iter().map(|d| d.info.clone()).collect())
    }

    fn open(
        &self,
        id: &CameraId,
        _wanted: WantedFormat,
    ) -> Result<Box<dyn FrameSource>, CameraError> {
        if self.denied {
            return Err(CameraError::PermissionDenied);
        }
        let d = self
            .devices
            .iter()
            .find(|d| &d.info.id == id)
            .ok_or_else(|| CameraError::NotFound(id.0.clone()))?;
        self.stats.opened.fetch_add(1, Ordering::SeqCst);
        self.stats.open_now.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Source {
            width: d.width,
            height: d.height,
            frames: Arc::clone(&d.frames),
            pace: d.pace,
            step: d.step,
            repeat: d.repeat,
            next: 0,
            count: 0,
            stats: Arc::clone(&self.stats),
        }))
    }
}
