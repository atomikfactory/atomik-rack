//! The camera seam: what the tracker needs from a camera, and nothing about how it is read.
//!
//! * [`Camera`] lists the cameras and opens one; [`FrameSource`] gives the frames of an open
//!   camera, always as RGB8 at 640 by 480 or less (a bigger picture is shrunk on the capture
//!   thread, so the rest of the program never sees one).
//! * [`SystemCamera`] is the real one (Media Foundation on Windows, AVFoundation on macOS, Video4Linux
//!   on Linux); [`FakeCamera`] replays frames from memory, so no test needs a camera.
//! * Frames live in memory only: nothing here writes one to disk, to a log or to the network.

pub mod convert;
mod fake;
mod system;

pub use fake::{FakeCamera, FakeStats};
pub use system::SystemCamera;

use std::time::Duration;

/// The widest picture a [`FrameSource`] gives.
pub const MAX_WIDTH: u32 = 640;
/// The tallest picture a [`FrameSource`] gives.
pub const MAX_HEIGHT: u32 = 480;

/// A camera, named by the system.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CameraId(pub String);

/// What the system says about a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraInfo {
    /// How to open it.
    pub id: CameraId,
    /// The name to show.
    pub name: String,
    /// More about it (the driver or the connection), possibly empty.
    pub description: String,
}

/// The picture asked of a camera. The camera gives the nearest it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WantedFormat {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
}

impl Default for WantedFormat {
    fn default() -> Self {
        Self {
            width: MAX_WIDTH,
            height: MAX_HEIGHT,
            fps: 30,
        }
    }
}

/// What came with a frame (the pixels are in the buffer the caller gave).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Time since the camera was opened.
    pub timestamp: Duration,
}

/// Why a camera could not be listed, opened or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CameraError {
    /// There is no such camera.
    #[error("no camera named {0}")]
    NotFound(String),
    /// The system did not let the program use the camera.
    #[error("the program may not use the camera; allow it in the system settings")]
    PermissionDenied,
    /// This system has no camera backend in this build.
    #[error("cameras are not supported on this system")]
    Unsupported,
    /// The camera could not be opened.
    #[error("cannot open the camera: {0}")]
    Open(String),
    /// A frame could not be read or decoded.
    #[error("cannot read a frame: {0}")]
    Frame(String),
    /// A fake camera ran out of frames.
    #[error("no more frames")]
    Ended,
}

/// An open camera. It is read on the thread that opened it (so it need not be `Send`), and the
/// device is released when it is dropped.
pub trait FrameSource {
    /// Waits for the next frame and puts its pixels in `buf` as RGB8 (`width * height * 3` bytes).
    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<FrameMeta, CameraError>;
}

/// The cameras of the machine.
pub trait Camera: Send + Sync {
    /// The cameras there are now.
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError>;

    /// Opens a camera. The device stays in use until the returned source is dropped.
    fn open(
        &self,
        id: &CameraId,
        wanted: WantedFormat,
    ) -> Result<Box<dyn FrameSource>, CameraError>;
}
