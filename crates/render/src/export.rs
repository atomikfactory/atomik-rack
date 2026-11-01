//! Exporting the song or the loop as an audio file (File > Export Song / Export Loop as Audio
//! File).
//!
//! An export is a project bounce ([`crate::project`]): the same engine, the same console (mute and
//! solo, the master section), automation and tempo ramps, plugins through the same host, with
//! the song played from tick 0 so every device is in the state it has when the song plays. What
//! differs is the range that is kept and what is done to it afterwards.
//!
//! * **Song.** From frame 0 (bar 1, tick 0) to the song End ([`atomik_model::song::Song::end_tick`])
//!   **plus a tail**: the render goes on for up to [`ExportRequest::tail_seconds`] past the End
//!   (by default [`DEFAULT_TAIL_SECONDS`]) and the tail is cut where the output has been at or
//!   below [`TAIL_FLOOR`] (about -90 dBFS) to the end of the render, so a dry song ends at its End
//!   and a song with a reverb rings out as long as it is audible. The file is never shorter than
//!   the End. This is the adaptive twin of the fixed two beats of tail the plain bounce
//!   ([`crate::project::TAIL_TICKS`], `atomik-render`, `session_bounce`) renders; the first
//!   End frames of an export are bit for bit those of the bounce.
//! * **Loop.** From the left to the right locator ([`atomik_model::song::Song::loop_range`]),
//!   **exactly** that many frames and no tail, so the file loops seamlessly on the grid. The song
//!   is rendered from tick 0 and the range is cut out of it, so a note held into the loop or an
//!   effect that has been running starts the file the way it sounds when the loop plays. Whether
//!   the loop is switched on does not matter.
//!
//! The rendered audio is converted to another sample rate if asked (the rendering itself is at
//! the project's rate: the sinc converter of [`atomik_audio_file::resample`], whose output has
//! `round(frames * rate / project rate)` frames), and only then written, quantised **once**:
//! 16-bit with the chosen [`DitherMode`] (TPDF by default), 24-bit without dither by default (its
//! quantisation noise is far below any recording's noise floor), or 32-bit float untouched. The
//! dither is seeded ([`DITHER_SEED`]), so the same project exports the same bits every time.
//!
//! The file is written under a temporary name next to the target and renamed when it is
//! complete: a cancelled or failed export leaves nothing behind, and an existing file is
//! replaced only by a complete one. The render can be watched and stopped through a
//! [`RenderControl`]; a stopped export returns `None`.

use crate::plugin_pass::RenderControl;
use crate::project::{Extra, PluginBounce, ProjectOptions, render_core};
use crate::wav::{SampleFormat, write_wav_file_with};
use crate::{RenderError, Warning};
use atomik_audio_file::AudioData;
use atomik_device_sdk::Registry;
use atomik_model::RackDoc;
use std::path::{Path, PathBuf};

pub use crate::freeze::{MAX_TAIL_SECONDS, TAIL_FLOOR};
pub use atomik_dsp::DitherMode;

/// The longest tail a song export keeps by default, in seconds.
pub const DEFAULT_TAIL_SECONDS: f64 = 10.0;

/// The seed of the dither: fixed, so an export is a function of the project and the format.
pub const DITHER_SEED: u64 = 0x4154_4F4D_4958_4458;

/// The sample rates an export can ask for, in Hz.
pub const SAMPLE_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

/// What range of the song is exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportSpan {
    /// From the start to the song End, plus the tail.
    Song,
    /// From the left to the right locator, exactly.
    Loop,
}

/// What to render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportRequest {
    /// The range.
    pub span: ExportSpan,
    /// For a song: seconds of tail to render past the End and keep while audible (`0.0`: the
    /// file ends at the End). Ignored for a loop, which has no tail.
    pub tail_seconds: f64,
}

/// The bit depth of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    /// 16-bit PCM.
    Pcm16,
    /// 24-bit PCM.
    Pcm24,
    /// 32-bit float.
    Float32,
}

impl BitDepth {
    /// The WAV sample format.
    pub fn sample_format(self) -> SampleFormat {
        match self {
            Self::Pcm16 => SampleFormat::Pcm16,
            Self::Pcm24 => SampleFormat::Pcm24,
            Self::Float32 => SampleFormat::F32,
        }
    }

    /// The dither a format gets unless asked otherwise: TPDF at 16-bit, none above.
    pub fn default_dither(self) -> DitherMode {
        match self {
            Self::Pcm16 => DitherMode::Tpdf,
            Self::Pcm24 | Self::Float32 => DitherMode::None,
        }
    }
}

/// The container of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportContainer {
    /// A WAV file (RIFF).
    #[default]
    Wav,
    /// An AIFF file (AIFF-C for 32-bit float), see [`crate::aiff`].
    Aiff,
}

impl ExportContainer {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aif",
        }
    }
}

/// The file the audio is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportFormat {
    /// The container.
    pub container: ExportContainer,
    /// The bit depth.
    pub bit_depth: BitDepth,
    /// The dither of the requantisation (ignored for 32-bit float).
    pub dither: DitherMode,
    /// The sample rate in Hz; `None` keeps the project's. Anything from 8 kHz to 192 kHz.
    pub sample_rate: Option<u32>,
}

impl ExportFormat {
    /// A format with the default dither of the depth, at the project's rate.
    pub fn new(bit_depth: BitDepth) -> Self {
        Self {
            container: ExportContainer::Wav,
            bit_depth,
            dither: bit_depth.default_dither(),
            sample_rate: None,
        }
    }
}

/// A finished export.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    /// The file that was written.
    pub path: PathBuf,
    /// Its length in frames.
    pub frames: u64,
    /// Its sample rate in Hz.
    pub sample_rate: u32,
    /// Frames of the range itself (to the End, or the loop), at the project's rate.
    pub span_frames: u64,
    /// Frames of the song's tail that were kept, at the project's rate (0 for a loop).
    pub tail_frames: u64,
    /// What the render reported.
    pub warnings: Vec<Warning>,
}

/// The audio of an export, before it is written.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRender {
    /// The audio at the project's rate: one channel per output channel of the document.
    pub audio: AudioData,
    /// Frames of the range itself.
    pub span_frames: u64,
    /// Frames of the tail that were kept.
    pub tail_frames: u64,
    /// What the render reported.
    pub warnings: Vec<Warning>,
}

/// The first and one past the last frame of `span` in the song (at the project's rate).
///
/// # Errors
/// The range is empty (a loop whose locators coincide, a song End that rounds to frame 0).
pub fn span_frames(doc: &RackDoc, span: ExportSpan) -> Result<(u64, u64), RenderError> {
    let rate = doc.sample_rate;
    let (start, end) = match span {
        ExportSpan::Song => (0, doc.song.end_tick()),
        ExportSpan::Loop => doc.song.loop_range,
    };
    let (from, to) = (doc.song.frame_at(start, rate), doc.song.frame_at(end, rate));
    if to <= from {
        return Err(RenderError::Project(match span {
            ExportSpan::Song => "the song is empty: there is nothing to export".to_owned(),
            ExportSpan::Loop => "the loop is empty: move its locators apart".to_owned(),
        }));
    }
    Ok((from, to))
}

/// Renders the range of `request` from `doc` (see the module docs). `bundle_root` is the folder
/// the project's audio lives in, as for a bounce.
///
/// # Errors
/// The range is empty, the tail is out of range, or the bounce of the document fails.
pub fn render_export(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    request: &ExportRequest,
    control: RenderControl<'_>,
) -> Result<Option<ExportRender>, RenderError> {
    if !(0.0..=MAX_TAIL_SECONDS).contains(&request.tail_seconds) {
        return Err(RenderError::Project(format!(
            "the tail must be between 0 and {MAX_TAIL_SECONDS} seconds"
        )));
    }
    let (from, to) = span_frames(doc, request.span)?;
    let tail = match request.span {
        ExportSpan::Song => (request.tail_seconds * f64::from(doc.sample_rate)).round() as u64,
        ExportSpan::Loop => 0,
    };
    let frames = usize::try_from(to + tail).map_err(|e| RenderError::Project(e.to_string()))?;
    let extra = Extra {
        frames: Some(frames),
        control,
        ..Extra::default()
    };
    let Some(core) = render_core(doc, registry, (options, bundle_root), plugins, &extra)? else {
        return Ok(None);
    };
    let (rendered, from, to) = (core.rendered, from as usize, to as usize);
    let keep = match request.span {
        ExportSpan::Loop => to,
        ExportSpan::Song => {
            // Keep the tail as far as the output is above the floor, never less than the End.
            let rendered_len = rendered.channels.first().map_or(0, Vec::len);
            let audible = (0..rendered_len)
                .rev()
                .find(|&i| rendered.channels.iter().any(|c| c[i].abs() > TAIL_FLOOR))
                .map_or(0, |i| i + 1);
            audible.clamp(to, to + tail as usize)
        }
    };
    let channels = rendered
        .channels
        .into_iter()
        .map(|mut c| {
            c.resize(keep, 0.0);
            c.drain(..from);
            c
        })
        .collect();
    let audio = AudioData::new(doc.sample_rate, channels)
        .map_err(|e| RenderError::Project(e.to_string()))?;
    Ok(Some(ExportRender {
        span_frames: (to - from) as u64,
        tail_frames: (keep - to) as u64,
        audio,
        warnings: rendered.warnings,
    }))
}

/// The temporary name an export is written under: `<file name>.partial` next to the target.
pub fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

/// Writes `channels` to `path` in the container, depth and dither of `format` at `rate` (no
/// conversion), through a temporary file ([`partial_path`]) that is renamed when it is complete:
/// a failure removes it and leaves an existing file as it was.
///
/// # Errors
/// The file cannot be written.
pub fn write_audio_file(
    path: &Path,
    channels: &[Vec<f32>],
    rate: u32,
    format: &ExportFormat,
) -> Result<(), RenderError> {
    let partial = partial_path(path);
    let dither = (format.dither, DITHER_SEED);
    let sample_format = format.bit_depth.sample_format();
    let written = match format.container {
        ExportContainer::Wav => {
            write_wav_file_with(&partial, channels, rate, sample_format, dither)
        }
        ExportContainer::Aiff => {
            crate::aiff::write_aiff_file(&partial, channels, rate, sample_format, dither)
        }
    }
    .map_err(|source| RenderError::Write {
        path: path.to_path_buf(),
        source,
    });
    let renamed = written.and_then(|()| {
        std::fs::rename(&partial, path).map_err(|e| RenderError::Write {
            path: path.to_path_buf(),
            source: hound::Error::from(e).into(),
        })
    });
    if renamed.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    renamed
}

/// Converts `render` to the rate of `format` and writes it to `path` in the container of
/// `format`, through a temporary file that is renamed when it is complete (a failure removes it).
///
/// # Errors
/// The rate is out of range, the conversion fails, or the file cannot be written.
pub fn write_export(
    path: &Path,
    render: &ExportRender,
    format: &ExportFormat,
) -> Result<ExportReport, RenderError> {
    let project_rate = render.audio.sample_rate;
    let rate = format.sample_rate.unwrap_or(project_rate);
    if !(8_000..=192_000).contains(&rate) {
        return Err(RenderError::Project(format!(
            "the sample rate must be between 8000 and 192000 Hz, not {rate}"
        )));
    }
    let converted = if rate == project_rate {
        None
    } else {
        Some(
            atomik_audio_file::resample(&render.audio, rate)
                .map_err(|e| RenderError::Project(format!("sample rate conversion: {e}")))?,
        )
    };
    let audio = converted.as_ref().unwrap_or(&render.audio);
    write_audio_file(path, &audio.channels, rate, format)?;
    Ok(ExportReport {
        path: path.to_path_buf(),
        frames: audio.frames() as u64,
        sample_rate: rate,
        span_frames: render.span_frames,
        tail_frames: render.tail_frames,
        warnings: render.warnings.clone(),
    })
}

/// Renders and writes an export: [`render_export`], then [`write_export`]. A stopped render
/// writes nothing and returns `None`.
///
/// # Errors
/// As the two steps.
pub fn export_audio_file(
    doc: &RackDoc,
    registry: &Registry,
    project: (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    (request, format): (&ExportRequest, &ExportFormat),
    (path, control): (&Path, RenderControl<'_>),
) -> Result<Option<ExportReport>, RenderError> {
    let Some(render) = render_export(doc, registry, project, plugins, request, control)? else {
        return Ok(None);
    };
    if control.cancelled() {
        return Ok(None);
    }
    write_export(path, &render, format).map(Some)
}
