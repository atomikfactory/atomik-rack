//! A [`VideoTool`] for tests: no FFmpeg is needed and no process starts.
//!
//! The fake looks at the arguments only to tell the three calls apart (`-version`, `-show_streams`
//! for the probe, anything else for the frame stream) and answers with canned text and canned
//! frames. It records every call, so a test can check the argument lists. It is a normal module
//! (not `cfg(test)`) so the tests of the app crate can use it too.

use crate::VideoError;
use crate::runner::{Exit, VideoTool};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// One call the fake received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The program.
    pub exe: PathBuf,
    /// The arguments, one per entry.
    pub args: Vec<OsString>,
}

impl Call {
    /// Whether `arg` is one of the arguments.
    pub fn has(&self, arg: &str) -> bool {
        self.args.iter().any(|a| a == arg)
    }

    /// The argument after `flag`.
    pub fn after(&self, flag: &str) -> Option<&OsString> {
        let at = self.args.iter().position(|a| a == flag)?;
        self.args.get(at + 1)
    }
}

/// Called with the number of frames delivered so far.
type FrameHook = Arc<dyn Fn(usize) + Send + Sync>;

#[derive(Default)]
struct State {
    version: Option<String>,
    version_fails: bool,
    probe: Option<String>,
    frames: Vec<Vec<u8>>,
    chunk: usize,
    stream_fails: Option<String>,
    on_frame: Option<FrameHook>,
    audio: Option<Vec<u8>>,
    calls: Vec<Call>,
}

/// The fake tool; configure it with the builder methods, share it as an `Arc`.
pub struct FakeTool {
    state: Mutex<State>,
    refuse_all: bool,
}

impl Default for FakeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeTool {
    /// A fake that says it is FFmpeg 7.1.1, probes as [`crate::samples::MP4_H264`] and streams no
    /// frames until [`FakeTool::frames`] is called.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                version: Some(
                    "ffmpeg version 7.1.1-full_build Copyright (c) 2000-2025 the FFmpeg developers"
                        .to_owned(),
                ),
                probe: Some(crate::samples::MP4_H264.to_owned()),
                chunk: 4096,
                ..State::default()
            }),
            refuse_all: false,
        }
    }

    /// A fake that refuses every call like [`crate::NoVideoTools`] (a program that cannot start).
    pub fn none() -> Self {
        Self {
            refuse_all: true,
            ..Self::new()
        }
    }

    fn with(self, f: impl FnOnce(&mut State)) -> Self {
        f(&mut self.lock());
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The text `-version` prints.
    pub fn version(self, text: &str) -> Self {
        self.with(|s| s.version = Some(text.to_owned()))
    }

    /// `-version` ends with a failure code.
    pub fn version_fails(self) -> Self {
        self.with(|s| s.version_fails = true)
    }

    /// The JSON the probe prints.
    pub fn probe(self, json: &str) -> Self {
        self.with(|s| s.probe = Some(json.to_owned()))
    }

    /// The probe ends with a failure code (the file cannot be read).
    pub fn probe_fails(self) -> Self {
        self.with(|s| s.probe = None)
    }

    /// The frames the frame stream writes, whole JPEG files.
    pub fn frames(self, frames: Vec<Vec<u8>>) -> Self {
        self.with(|s| s.frames = frames)
    }

    /// The size of the chunks the frame stream is delivered in.
    pub fn chunk(self, bytes: usize) -> Self {
        self.with(|s| s.chunk = bytes.max(1))
    }

    /// The frame stream ends with a failure code and this error output, after its frames.
    pub fn stream_fails(self, stderr: &str) -> Self {
        self.with(|s| s.stream_fails = Some(stderr.to_owned()))
    }

    /// Runs `hook(frames delivered so far)` after each frame of the stream (a test sets a
    /// cancel flag from it).
    pub fn on_frame(self, hook: impl Fn(usize) + Send + Sync + 'static) -> Self {
        self.with(|s| s.on_frame = Some(Arc::new(hook)))
    }

    /// The WAV file an audio extraction (a call with `-vn`) writes to its output path, and the
    /// progress it prints: four steps, each a call of the [`FakeTool::on_frame`] hook.
    pub fn audio(self, wav: Vec<u8>) -> Self {
        self.with(|s| s.audio = Some(wav))
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }
}

impl VideoTool for FakeTool {
    fn run(
        &self,
        exe: &Path,
        args: &[OsString],
        stdout: &mut dyn FnMut(&[u8]) -> bool,
        cancel: &AtomicBool,
    ) -> Result<Exit, VideoError> {
        if self.refuse_all {
            return Err(VideoError::Unavailable);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(VideoError::Cancelled);
        }
        let call = Call {
            exe: exe.to_path_buf(),
            args: args.to_vec(),
        };
        let (is_version, is_probe) = (call.has("-version"), call.has("-show_streams"));
        let is_audio = call.has("-vn");
        let out_path = call.args.last().cloned();
        let (version, version_fails, probe, frames, chunk, stream_fails, on_frame, audio) = {
            let mut s = self.lock();
            s.calls.push(call);
            (
                s.version.clone(),
                s.version_fails,
                s.probe.clone(),
                s.frames.clone(),
                s.chunk,
                s.stream_fails.clone(),
                s.on_frame.clone(),
                s.audio.clone(),
            )
        };
        let failure = |stderr: &str| Exit {
            success: false,
            code: Some(1),
            stderr: stderr.to_owned(),
            stopped: false,
        };
        if is_version {
            if version_fails {
                return Ok(failure("boom"));
            }
            let _ = stdout(version.unwrap_or_default().as_bytes());
            return Ok(Exit::ok());
        }
        if is_probe {
            return Ok(match probe {
                Some(json) => {
                    let _ = stdout(json.as_bytes());
                    Exit::ok()
                }
                None => failure("No such file or directory"),
            });
        }
        if is_audio {
            // The output is the last argument, `file:` and the path.
            if let (Some(wav), Some(out)) = (audio, out_path) {
                let text = out.to_string_lossy().into_owned();
                let path = text.strip_prefix("file:").unwrap_or(&text);
                if std::fs::write(path, wav).is_err() {
                    return Ok(failure("cannot write the output"));
                }
            }
            for step in 1..=4usize {
                if cancel.load(Ordering::Relaxed) {
                    return Err(VideoError::Cancelled);
                }
                // The sample probe is 12.021 s long: the progress counts quarters of it.
                let line = format!(
                    "out_time_us={}
progress=continue
",
                    step * 3_005_333
                );
                if !stdout(line.as_bytes()) {
                    return Ok(Exit {
                        stopped: true,
                        ..Exit::ok()
                    });
                }
                if let Some(hook) = &on_frame {
                    hook(step);
                }
            }
            if let Some(stderr) = stream_fails {
                return Ok(failure(&stderr));
            }
            let line = "out_time_us=12021333
progress=end
";
            let _ = stdout(line.as_bytes());
            return Ok(Exit::ok());
        }
        for (n, frame) in frames.iter().enumerate() {
            for piece in frame.chunks(chunk) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(VideoError::Cancelled);
                }
                if !stdout(piece) {
                    return Ok(Exit {
                        stopped: true,
                        ..Exit::ok()
                    });
                }
            }
            if let Some(hook) = &on_frame {
                hook(n + 1);
            }
        }
        Ok(match stream_fails {
            Some(stderr) => failure(&stderr),
            None => Exit::ok(),
        })
    }
}

/// A WAV file of 16 bit PCM: `samples` are the interleaved samples of `channels` channels.
pub fn wav_pcm16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// A small JPEG-shaped file of `width` by `height` (not decodable: the right markers around a
/// few data bytes). `tag` is written into the data so frames differ. The quantisation table holds
/// the bytes `FF D9` and the data holds `FF 00`, as a real frame may.
pub fn tiny_jpeg(width: u32, height: u32, tag: u8) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    // APP0, "JFIF".
    out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    // DQT: a 64-entry table that contains the end marker bytes.
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
    let mut table = vec![8u8; 64];
    table[10] = 0xFF;
    table[11] = 0xD9;
    table[40] = 0xFF;
    table[41] = 0xD9;
    out.extend_from_slice(&table);
    // SOF0: 8 bits, height, width, three components.
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    // SOS, then data with a stuffed FF and a restart marker, then the end marker.
    out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x0C, 3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0]);
    out.extend_from_slice(&[tag, 0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, tag]);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// The colour of a [`solid_jpeg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Solid {
    /// Pure red.
    Red,
    /// Pure green.
    Green,
    /// Pure blue.
    Blue,
    /// White.
    White,
}

/// A real, decodable 16 by 16 JPEG of one colour (for tests of the decoder and the viewer).
pub fn solid_jpeg(colour: Solid) -> &'static [u8] {
    match colour {
        Solid::Red => include_bytes!("../fixtures/solid_red.jpg"),
        Solid::Green => include_bytes!("../fixtures/solid_green.jpg"),
        Solid::Blue => include_bytes!("../fixtures/solid_blue.jpg"),
        Solid::White => include_bytes!("../fixtures/solid_white.jpg"),
    }
}

/// A real, decodable 32 by 16 JPEG, red on the left half and blue on the right.
pub fn split_jpeg() -> &'static [u8] {
    include_bytes!("../fixtures/split_red_blue.jpg")
}
