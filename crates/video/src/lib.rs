//! Atomik Rack video: the foundation of video tracks.
//!
//! Video is decoded by the user's own FFmpeg (found on the system path or in a folder set in
//! Settings; nothing is bundled and no codec ships with the program). This crate holds
//! everything about it that does not need a window:
//!
//! - [`discover`] / [`FfmpegStatus`]: where FFmpeg is, which version, and a message for the
//!   user when it is missing, too old or broken (video is then off, nothing fails);
//! - [`VideoTool`]: the one trait every `ffmpeg` and `ffprobe` call goes through, with the real
//!   [`SystemTool`] (no shell, a list of arguments, no console window, killed when stopped,
//!   bounded error output), [`NoVideoTools`] for a headless run and the [`fake`] of the tests;
//! - [`parse_probe`] / [`ProbeInfo`]: the film's length, frame rate, size, rotation, start time
//!   and timecode, and its audio tracks;
//! - the proxy ([`proxy`], [`jpeg`]): every frame as a small independent JPEG in one indexed
//!   file, so any frame is one seek; [`prepare`] builds it from ffmpeg's piped output with
//!   progress and cancel and never leaves a partial file;
//! - [`audio`]: choosing a film's audio track and writing it to a WAV file (with progress and
//!   cancel) for the song to import;
//! - [`encode`]: compressed audio export, a WAV file to MP3, AAC or Opus through the same tool,
//!   with the song's title and author as tags, progress and cancel, and no partial file;
//! - the viewer's side ([`decode`], [`frames`], [`timeline`]): a pure-Rust JPEG decoder, the
//!   [`FrameDecoder`] worker with its [`FrameCache`], and the exact arithmetic from a song time to
//!   a frame and a timecode ([`timeline::locate`], [`Timecode`]);
//! - [`ProxyCache`]: the per-user folder of proxies, keyed by [`SourceIdentity`], versioned and
//!   pruned to a size.
//!
//! No test in this crate needs FFmpeg. One `#[ignore]`d test (in `tests/real_ffmpeg.rs`) runs the
//! real programs when they are installed.

pub mod args;
pub mod audio;
pub mod cache;
pub mod decode;
pub mod discover;
pub mod encode;
mod error;
pub mod fake;
pub mod frames;
pub mod jpeg;
pub mod prepare;
pub mod probe;
pub mod proxy;
pub mod runner;
pub mod samples;
pub mod timeline;
pub mod version;

pub use args::ProxySpec;
pub use audio::{MAX_LEAD_US, Pick, extract_audio, lead_us, pick_stream};
pub use cache::{DEFAULT_MAX_BYTES, ProxyCache, SourceIdentity};
pub use decode::{Picture, decode_jpeg};
pub use discover::{Ffmpeg, FfmpegStatus, Platform, Search, discover};
pub use encode::{
    EncodeJob, EncodeSpec, LossyCodec, LossyQuality, Tags, encode_args, encode_audio,
};
pub use error::VideoError;
pub use frames::{Event as FrameEvent, FrameCache, FrameDecoder};
pub use prepare::{Control, Prepared, prepare, probe as probe_file};
pub use probe::{AudioStream, ProbeInfo, Rational, VideoStream, parse_probe};
pub use proxy::{PROXY_VERSION, ProxyInfo, ProxyReader, ProxyWriter};
pub use runner::{NoVideoTools, SystemTool, VideoTool};
pub use timeline::{Shot, Timecode};
pub use version::{FfmpegVersion, MIN_MAJOR, parse_version};
