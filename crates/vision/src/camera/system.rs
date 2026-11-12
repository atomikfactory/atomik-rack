//! The real cameras. One small backend per system, chosen at build time:
//!
//! * Windows and macOS: the `nokhwa` capture crate (Media Foundation, AVFoundation), without its
//!   JPEG codec (it would need a C compiler and an assembler); the frames are converted by
//!   [`super::convert`].
//! * Linux: `linuxvideo`, a pure Rust Video4Linux client (no `bindgen`, no `libclang`, no
//!   system library).
//!
//! The capture crates use `unsafe` inside; this crate does not.

use super::{Camera, CameraError, CameraId, CameraInfo, FrameSource, WantedFormat};

/// The cameras of this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCamera;

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use super::*;
    use crate::camera::FrameMeta;
    use crate::camera::convert::{PixelFormat, shrink_to_fit, to_rgb};
    use crate::clock::now;
    use nokhwa::utils::{
        ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
        Resolution,
    };
    use std::time::Instant;

    /// Every format the program can convert.
    const ACCEPTED: &[FrameFormat] = &[
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::MJPEG,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::GRAY,
    ];
    /// The formats tried in turn: the ones that cost the least to convert first.
    const ORDER: &[FrameFormat] = &[
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::MJPEG,
        FrameFormat::RAWRGB,
    ];

    #[cfg(target_os = "macos")]
    fn allowed() -> Result<(), CameraError> {
        // The first use asks the user; wait for the answer for a while.
        if nokhwa::nokhwa_check() {
            return Ok(());
        }
        nokhwa::nokhwa_initialize(|_| {});
        for _ in 0..600 {
            if nokhwa::nokhwa_check() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err(CameraError::PermissionDenied)
    }

    #[cfg(not(target_os = "macos"))]
    fn allowed() -> Result<(), CameraError> {
        Ok(())
    }

    fn index_of(id: &CameraId) -> CameraIndex {
        match id.0.parse::<u32>() {
            Ok(i) => CameraIndex::Index(i),
            Err(_) => CameraIndex::String(id.0.clone()),
        }
    }

    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        allowed()?;
        let found =
            nokhwa::query(ApiBackend::Auto).map_err(|e| CameraError::Open(e.to_string()))?;
        Ok(found
            .into_iter()
            .map(|c| CameraInfo {
                id: CameraId(c.index().as_string()),
                name: c.human_name(),
                description: c.description().to_string(),
            })
            .collect())
    }

    struct Native {
        camera: nokhwa::Camera,
        started: Instant,
        scratch: Vec<u8>,
    }

    impl FrameSource for Native {
        fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<FrameMeta, CameraError> {
            let frame = self
                .camera
                .frame()
                .map_err(|e| CameraError::Frame(e.to_string()))?;
            let format = match frame.source_frame_format() {
                FrameFormat::MJPEG => PixelFormat::Mjpeg,
                FrameFormat::YUYV => PixelFormat::Yuyv,
                FrameFormat::NV12 => PixelFormat::Nv12,
                FrameFormat::GRAY => PixelFormat::Gray,
                FrameFormat::RAWRGB => PixelFormat::Rgb,
                FrameFormat::RAWBGR => PixelFormat::Bgr,
            };
            let res = frame.resolution();
            let (w, h) = to_rgb(
                format,
                res.width(),
                res.height(),
                frame.buffer(),
                &mut self.scratch,
            )?;
            std::mem::swap(buf, &mut self.scratch);
            let (w, h) = shrink_to_fit(buf, w, h);
            Ok(FrameMeta {
                width: w,
                height: h,
                timestamp: self.started.elapsed(),
            })
        }
    }

    impl Drop for Native {
        fn drop(&mut self) {
            // Release the device at once (the capture crate would do the same on drop, but
            // would panic on an error).
            let _ = self.camera.stop_stream();
        }
    }

    pub fn open(id: &CameraId, wanted: WantedFormat) -> Result<Box<dyn FrameSource>, CameraError> {
        allowed()?;
        let index = index_of(id);
        let mut last = String::from("no usable format");
        for &format in ORDER {
            let want = CameraFormat::new(
                Resolution::new(wanted.width, wanted.height),
                format,
                wanted.fps,
            );
            let request =
                RequestedFormat::with_formats(RequestedFormatType::Closest(want), ACCEPTED);
            match nokhwa::Camera::new(index.clone(), request) {
                Ok(mut camera) => match camera.open_stream() {
                    Ok(()) => {
                        return Ok(Box::new(Native {
                            camera,
                            started: now(),
                            scratch: Vec::new(),
                        }));
                    }
                    Err(e) => last = e.to_string(),
                },
                Err(e) => last = e.to_string(),
            }
        }
        Err(CameraError::Open(last))
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use crate::camera::FrameMeta;
    use crate::camera::convert::{PixelFormat, shrink_to_fit, to_rgb};
    use crate::clock::now;
    use linuxvideo::format::{PixFormat, PixelFormat as V4lFormat};
    use linuxvideo::stream::ReadStream;
    use linuxvideo::{BufType, CapabilityFlags, Device, Fract};
    use std::time::Instant;

    fn failed(e: std::io::Error) -> CameraError {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => CameraError::PermissionDenied,
            _ => CameraError::Open(e.to_string()),
        }
    }

    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        let devices = linuxvideo::list().map_err(failed)?;
        let mut out = Vec::new();
        for device in devices.flatten() {
            let Ok(caps) = device.capabilities() else {
                continue;
            };
            if !caps
                .device_capabilities()
                .contains(CapabilityFlags::VIDEO_CAPTURE)
            {
                continue;
            }
            let Ok(path) = device.path() else { continue };
            out.push(CameraInfo {
                id: CameraId(path.display().to_string()),
                name: caps.card().to_string(),
                description: caps.driver().to_string(),
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    struct Native {
        stream: ReadStream,
        format: PixelFormat,
        width: u32,
        height: u32,
        started: Instant,
        scratch: Vec<u8>,
    }

    impl FrameSource for Native {
        fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<FrameMeta, CameraError> {
            let (format, width, height) = (self.format, self.width, self.height);
            let scratch = &mut self.scratch;
            let converted = self
                .stream
                .dequeue(|view| Ok(to_rgb(format, width, height, &view, scratch)))
                .map_err(|e| CameraError::Frame(e.to_string()))?;
            let (w, h) = converted?;
            std::mem::swap(buf, &mut self.scratch);
            let (w, h) = shrink_to_fit(buf, w, h);
            Ok(FrameMeta {
                width: w,
                height: h,
                timestamp: self.started.elapsed(),
            })
        }
    }

    pub fn open(id: &CameraId, wanted: WantedFormat) -> Result<Box<dyn FrameSource>, CameraError> {
        let device = Device::open(&id.0).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CameraError::NotFound(id.0.clone()),
            _ => failed(e),
        })?;
        // The formats this camera offers, then the first one the program can convert, cheapest
        // first.
        let offered: Vec<V4lFormat> = device
            .formats(BufType::VIDEO_CAPTURE)
            .flatten()
            .map(|d| d.pixel_format())
            .collect();
        let choices = [
            (V4lFormat::YUYV, PixelFormat::Yuyv),
            (V4lFormat::MJPG, PixelFormat::Mjpeg),
            (V4lFormat::RGB3, PixelFormat::Rgb),
            (V4lFormat::BGR3, PixelFormat::Bgr),
        ];
        let (v4l, format) = choices
            .iter()
            .find(|(v, _)| offered.contains(v))
            .copied()
            .ok_or_else(|| {
                CameraError::Open("the camera offers no format the program can read".into())
            })?;
        let capture = device
            .video_capture(PixFormat::new(wanted.width, wanted.height, v4l))
            .map_err(failed)?;
        let (got_width, got_height) = (capture.format().width(), capture.format().height());
        // The frame rate is a request; a camera that refuses keeps its own.
        let _ = capture.set_frame_interval(Fract::new(1, wanted.fps.max(1)));
        let stream = capture.into_stream().map_err(failed)?;
        Ok(Box::new(Native {
            stream,
            format,
            width: got_width,
            height: got_height,
            started: now(),
            scratch: Vec::new(),
        }))
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod native {
    use super::*;

    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        Err(CameraError::Unsupported)
    }

    pub fn open(
        _id: &CameraId,
        _wanted: WantedFormat,
    ) -> Result<Box<dyn FrameSource>, CameraError> {
        Err(CameraError::Unsupported)
    }
}

impl Camera for SystemCamera {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError> {
        native::list()
    }

    fn open(
        &self,
        id: &CameraId,
        wanted: WantedFormat,
    ) -> Result<Box<dyn FrameSource>, CameraError> {
        native::open(id, wanted)
    }
}
