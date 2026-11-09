//! Exporting the song or the loop as an audio file in the app core.
//!
//! **File > Export Song as Audio File** renders from the start to the song End and keeps the
//! tail that is still audible; **Export Loop as Audio File** renders exactly the range between
//! the locators and **Export Selection as Audio File** the time range selected on the lanes of the
//! arrangement, exactly and with no tail (see [`atomik_render::export`] for the rules). The render
//! runs on a worker thread
//! ([`crate::export_worker`]) through the offline bounce path, on a copy of the document taken
//! when it starts, so the live engine is not touched: the song keeps playing, and editing goes on
//! (an edit after the start is not in the file). One export runs at a time.
//!
//! **Export Stems as Audio Files** ([`ExportScope::Stems`]) is the same export with one file per
//! strip of the console ([`atomik_render::stems`]): the path is the folder the files go into, and
//! the panel's values say the container, depth, dither, rate, tail, whether the channels are
//! taken before their strips and whether only the range between the locators (the same range as
//! Export Loop) or the time selection is exported, with no tail.
//!
//! A finished export says how loud it is: the toast (and the log) carry the integrated loudness,
//! the true peak and the loudness range of the audio that was written
//! ([`atomik_render::loudness`]); for stems the toast has the master's and the log every file's.
//!
//! **MP3, AAC and Opus** ([`ExportFile::Mp3`] and the like) are the same export with one more step:
//! the render is written to a temporary WAV file at the song's rate (or the rate chosen, brought to
//! one the format holds), and the FFmpeg the user installed encodes it on the export worker, with
//! progress and cancel ([`atomik_video::encode_audio`]). The song's title and author go into the
//! file's tags as arguments, never through a shell. Cancel or a failure removes the temporary
//! file and leaves no result file. The loudness in the toast is that of the audio that went into
//! the encoder; the encoder can raise the true peak a little, so leave headroom (docs/README.md).
//! Without FFmpeg the three formats are refused ([`AppCore::export_encoder`] says how to set it
//! up); stems are WAV, AIFF or FLAC only.
//!
//! The shell asks for the file, then for the options (its panel), then calls
//! [`AppCore::start_audio_export`]; progress and **Cancel** are [`AppCore::audio_export_progress`]
//! and [`AppCore::cancel_audio_export`]. A cancelled or failed export leaves no file (the worker
//! writes under a temporary name and renames it). Opening or creating a project cancels the
//! export that runs.

use super::*;
use crate::export_worker::{self, Done, Target, Work};
use crate::freeze_worker::PluginEnv;
use atomik_render::export::{
    BitDepth, DEFAULT_TAIL_SECONDS, ExportContainer, ExportFormat, ExportReport, ExportRequest,
    ExportSpan, span_frames,
};
use atomik_render::stems::StemStrip;
use atomik_ui::export_panel::rate_label;
use atomik_ui::{
    EncoderState, ExportDepth, ExportDither, ExportFile, ExportQuality, ExportScope, ExportValues,
};
use atomik_video::{FfmpegStatus, LossyCodec, LossyQuality, Tags};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The export that runs.
struct Running {
    job: u64,
    scope: ExportScope,
    file: String,
    values: ExportValues,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
}

/// The export state of an open project.
#[derive(Default)]
pub(super) struct ExportState {
    running: Option<Running>,
    next_job: u64,
}

impl ExportState {
    /// Stops the running export (the project changed).
    pub(super) fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The range the panel's choices mean; `selection` is the time selection in ticks, when there is
/// one (a selection that does not exist is an empty range, which the render refuses).
fn span_of(scope: ExportScope, values: &ExportValues, selection: Option<(i64, i64)>) -> ExportSpan {
    let range = || {
        let (start, end) = selection.unwrap_or((0, 0));
        ExportSpan::Range(start, end)
    };
    match scope {
        ExportScope::Song => ExportSpan::Song,
        ExportScope::Selection => range(),
        ExportScope::Stems if values.selection => range(),
        ExportScope::Stems if values.locators => ExportSpan::Loop,
        ExportScope::Stems => ExportSpan::Song,
        ExportScope::Loop => ExportSpan::Loop,
    }
}

/// The compressed format the panel's values ask for, if any.
pub fn lossy_codec(file: ExportFile) -> Option<LossyCodec> {
    match file {
        ExportFile::Mp3 => Some(LossyCodec::Mp3),
        ExportFile::Aac => Some(LossyCodec::Aac),
        ExportFile::Opus => Some(LossyCodec::Opus),
        ExportFile::Wav | ExportFile::Aiff | ExportFile::Flac => None,
    }
}

/// The quality the panel's values ask for, for the compressed format chosen.
pub fn lossy_quality(values: &ExportValues) -> Option<LossyQuality> {
    Some(match values.quality()? {
        ExportQuality::Cbr(kbps) => LossyQuality::Cbr(u32::from(kbps)),
        ExportQuality::Vbr(q) => LossyQuality::Vbr(q),
    })
}

/// The file extension of the format chosen, without the dot.
pub fn file_extension(values: &ExportValues) -> &'static str {
    lossy_codec(values.file).map_or_else(
        || export_format(values).container.extension(),
        LossyCodec::extension,
    )
}

/// The render format the panel's values mean: the dither is for 16-bit files only. A compressed
/// file renders to a float WAV file first (the encoder is the only step that loses anything); its
/// rate is the one asked for (else the project's `project_rate`), brought to one the format holds.
pub fn export_format_at(values: &ExportValues, project_rate: u32) -> ExportFormat {
    let Some(codec) = lossy_codec(values.file) else {
        return export_format(values);
    };
    ExportFormat {
        container: ExportContainer::Wav,
        bit_depth: BitDepth::Float32,
        dither: atomik_dsp::DitherMode::None,
        sample_rate: Some(codec.output_rate(values.rate.unwrap_or(project_rate))),
    }
}

/// The render format the panel's values mean: the dither is for 16-bit files only. A compressed
/// format is described by [`export_format_at`]; here it reads as the WAV file it renders to.
pub fn export_format(values: &ExportValues) -> ExportFormat {
    let bit_depth = match values.depth {
        ExportDepth::Pcm16 => BitDepth::Pcm16,
        ExportDepth::Pcm24 => BitDepth::Pcm24,
        ExportDepth::Float32 => BitDepth::Float32,
    };
    let dither = if bit_depth == BitDepth::Pcm16 {
        match values.dither {
            ExportDither::Off => atomik_dsp::DitherMode::None,
            ExportDither::Tpdf => atomik_dsp::DitherMode::Tpdf,
            ExportDither::Shaped => atomik_dsp::DitherMode::Shaped,
        }
    } else {
        atomik_dsp::DitherMode::None
    };
    ExportFormat {
        container: match values.file {
            ExportFile::Wav => ExportContainer::Wav,
            ExportFile::Aiff => ExportContainer::Aiff,
            ExportFile::Flac => ExportContainer::Flac,
            ExportFile::Mp3 | ExportFile::Aac | ExportFile::Opus => ExportContainer::Wav,
        },
        bit_depth,
        dither,
        sample_rate: values.rate,
    }
}

/// The render request the panel's values mean: a loop and a selection have no tail.
pub fn export_request(
    scope: ExportScope,
    values: &ExportValues,
    selection: Option<(i64, i64)>,
) -> ExportRequest {
    let span = span_of(scope, values, selection);
    ExportRequest {
        span,
        tail_seconds: if span == ExportSpan::Song && values.tail {
            DEFAULT_TAIL_SECONDS
        } else {
            0.0
        },
    }
}

/// `1:05.3` for 65.3 seconds.
fn clock(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:04.1}", minutes as u64, seconds - minutes * 60.0)
}

/// `24-bit`, or `MP3 192 kbps` for a compressed file.
fn depth_label(values: &ExportValues) -> String {
    if let (Some(codec), Some(quality)) = (lossy_codec(values.file), values.quality()) {
        return match quality {
            ExportQuality::Cbr(kbps) => format!("{} {kbps} kbps", codec.name()),
            ExportQuality::Vbr(q) => format!("{} VBR V{q}", codec.name()),
        };
    }
    match values.depth {
        ExportDepth::Pcm16 => "16-bit",
        ExportDepth::Pcm24 => "24-bit",
        ExportDepth::Float32 => "32-bit float",
    }
    .to_owned()
}

/// The toast of a finished export of stems: "Exported 5 stems to Bounced song: 24-bit, 48 kHz,
/// 1:05.3; Master -14.2 LUFS, -0.8 dBTP".
fn describe_stems(
    report: &atomik_render::stems::StemsReport,
    values: &ExportValues,
    project_rate: u32,
) -> String {
    let folder = report
        .folder
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let n = report.files.len();
    let seconds = report.frames as f64 / f64::from(report.sample_rate);
    let tail = if report.tail_frames > 0 {
        format!(
            ", with {:.1} s of tail",
            report.tail_frames as f64 / f64::from(project_rate)
        )
    } else {
        String::new()
    };
    let warnings = match report.warnings.len() {
        0 => String::new(),
        n => format!(" ({n} warning{})", if n == 1 { "" } else { "s" }),
    };
    // The master's loudness when there is one.
    let loud = report
        .files
        .iter()
        .zip(&report.loudness)
        .find(|((_, s), _)| s.kind == atomik_model::StemKind::Master)
        .map_or_else(String::new, |((_, s), l)| {
            format!("; {} {}", s.name, l.summary())
        });
    format!(
        "Exported {n} stem{} to {folder}: {}, {}, {}{tail}{warnings}{loud}",
        if n == 1 { "" } else { "s" },
        depth_label(values),
        rate_label(report.sample_rate),
        clock(seconds)
    )
}

/// The toast of a finished export: "Exported song.wav: 24-bit, 48 kHz, 1:05.3; -14.2 LUFS,
/// -0.8 dBTP, range 6.3 LU".
fn describe(report: &ExportReport, values: &ExportValues, project_rate: u32) -> String {
    let file = report
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let depth = depth_label(values);
    let seconds = report.frames as f64 / f64::from(report.sample_rate);
    let tail = if report.tail_frames > 0 {
        format!(
            ", with {:.1} s of tail",
            report.tail_frames as f64 / f64::from(project_rate)
        )
    } else {
        String::new()
    };
    let warnings = match report.warnings.len() {
        0 => String::new(),
        n => format!(" ({n} warning{})", if n == 1 { "" } else { "s" }),
    };
    format!(
        "Exported {file}: {depth}, {}, {}{tail}{warnings}; {}",
        rate_label(report.sample_rate),
        clock(seconds),
        report.loudness.summary()
    )
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// True while an export renders.
    pub fn audio_export_busy(&self) -> bool {
        self.audio.export.running.is_some()
    }

    /// Whether `scope` can be exported now: no export runs and its range is not empty.
    pub fn can_export_audio(&self, scope: ExportScope) -> bool {
        self.export_audio_refusal(scope).is_none()
    }

    /// Why `scope` cannot be exported, for the message of the menu item.
    pub fn export_audio_refusal(&self, scope: ExportScope) -> Option<String> {
        if self.audio_export_busy() {
            return Some("An export is running: wait for it, or cancel it".to_owned());
        }
        if scope == ExportScope::Stems && self.doc().console.channels.is_empty() {
            return Some("There are no mixer strips to export as stems".to_owned());
        }
        if scope == ExportScope::Selection && self.export_selection().is_none() {
            return Some("Select a time range on a lane to export it".to_owned());
        }
        let span = span_of(scope, &ExportValues::default(), self.export_selection());
        span_frames(self.doc(), span).err().map(|e| e.to_string())
    }

    /// The time selection an export can take: the range selected on the lanes of the
    /// arrangement, as `(start, end)` ticks, when it is not empty.
    pub fn export_selection(&self) -> Option<(i64, i64)> {
        self.arrangement
            .lane_ranges()
            .map(|(_, start, end)| (start, end))
            .filter(|(start, end)| start < end)
    }

    /// Whether FFmpeg, which encodes MP3, AAC and Opus, can be used: found, being looked for, or
    /// missing with the line that says how to set it up (the export panel greys the three formats
    /// then).
    pub fn export_encoder(&self) -> EncoderState {
        match self.video_status() {
            FfmpegStatus::Found(_) => EncoderState::Ready,
            FfmpegStatus::Unchecked => EncoderState::Checking,
            FfmpegStatus::Missing { .. } => EncoderState::Missing(
                "MP3, AAC and Opus need FFmpeg: set its folder in Settings > Video or put it on your PATH."
                    .to_owned(),
            ),
            FfmpegStatus::TooOld { .. } => EncoderState::Missing(format!(
                "MP3, AAC and Opus need FFmpeg {} or later: update it, or set a folder in Settings > Video.",
                atomik_video::MIN_MAJOR
            )),
            FfmpegStatus::Broken { .. } => EncoderState::Missing(
                "The FFmpeg that was found does not run: set a working folder in Settings > Video."
                    .to_owned(),
            ),
        }
    }

    /// The sample rate of the project in Hz (the rate of an export that does not convert).
    pub fn project_sample_rate(&self) -> u32 {
        self.doc().sample_rate
    }

    /// The fraction done, `0.0..=1.0`, and a line saying what runs, while an export renders.
    pub fn audio_export_progress(&self) -> Option<(f32, String)> {
        let running = self.audio.export.running.as_ref()?;
        let fraction = f32::from_bits(running.progress.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let what = match running.scope {
            ExportScope::Song => "Rendering the song",
            ExportScope::Loop => "Rendering the loop",
            ExportScope::Selection => "Rendering the selection",
            ExportScope::Stems => "Rendering the stems",
        };
        Some((fraction, what.to_owned()))
    }

    // ------------------------------------------------------------------ commands

    /// Starts the export of `scope` to `path` with the panel's `values` (see the module docs).
    ///
    /// # Errors
    /// An export runs already, the range is empty, or the worker cannot start; the message is
    /// also shown as the toast.
    pub fn start_audio_export(
        &mut self,
        scope: ExportScope,
        path: PathBuf,
        values: ExportValues,
    ) -> Result<(), String> {
        let refusal = self.export_audio_refusal(scope).or_else(|| {
            if values.file == ExportFile::Flac && values.depth == ExportDepth::Float32 {
                return Some("FLAC holds 16-bit and 24-bit audio, not 32-bit float".to_owned());
            }
            if values.file.is_lossy() {
                if scope == ExportScope::Stems {
                    return Some("Stems are written as WAV, AIFF or FLAC files".to_owned());
                }
                if self.video_tool_and_folder().is_none() {
                    return Some(
                        "MP3, AAC and Opus need FFmpeg, which is not available here".to_owned(),
                    );
                }
                if let EncoderState::Missing(why) = self.export_encoder() {
                    return Some(why);
                }
            }
            if scope == ExportScope::Stems && values.selection && self.export_selection().is_none()
            {
                return Some("Select a time range on a lane to export it".to_owned());
            }
            // Stems between the locators or the selection need a range of their own.
            let span = span_of(scope, &values, self.export_selection());
            span_frames(self.doc(), span).err().map(|e| e.to_string())
        });
        if let Some(why) = refusal {
            self.reject(why.clone());
            return Err(why);
        }
        // The file keeps the name the dialog gave it, with the extension of the container; the
        // stems go to a folder of that name.
        let path = match scope {
            ExportScope::Stems => path,
            _ => path.with_extension(file_extension(&values)),
        };
        let file = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.export.next_job;
        self.audio.export.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            request: export_request(scope, &values, self.export_selection()),
            format: export_format_at(&values, self.doc().sample_rate),
            target: match scope {
                ExportScope::Stems => Target::Stems {
                    folder: path,
                    strip: if values.dry {
                        StemStrip::Dry
                    } else {
                        StemStrip::Full
                    },
                },
                _ => Target::File(path),
            },
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
            lossy: self.lossy_job(&values),
        };
        let started = export_worker::spawn(
            self.audio.tx.clone(),
            (self.audio.generation, job),
            work,
            (Arc::clone(&cancel), Arc::clone(&progress)),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                self.audio.export.running = Some(Running {
                    job,
                    scope,
                    file,
                    values,
                    cancel,
                    progress,
                });

                Ok(())
            }
            Err(e) => {
                let why = format!("cannot start the export: {e}");
                self.reject(why.clone());
                Err(why)
            }
        }
    }

    /// The encoding step of a compressed export, with the song's title and author as tags.
    fn lossy_job(&self, values: &ExportValues) -> Option<export_worker::Lossy> {
        let codec = lossy_codec(values.file)?;
        let (tool, folder) = self.video_tool_and_folder()?;
        let info = &self.doc().info;
        Some(export_worker::Lossy {
            tool,
            folder,
            codec,
            quality: lossy_quality(values)?,
            tags: Tags {
                title: info.title.clone(),
                author: info.author.clone(),
            },
        })
    }

    /// Stops the running export; nothing is written.
    pub fn cancel_audio_export(&mut self) {
        match &self.audio.export.running {
            Some(r) => r.cancel.store(true, Ordering::Relaxed),
            None => self.reject("No export is running".to_owned()),
        }
    }

    /// An export finished (or failed, or was stopped): says so.
    pub(super) fn apply_exported(&mut self, job: u64, result: Result<Option<Done>, String>) {
        let Some(running) = self.audio.export.running.take_if(|r| r.job == job) else {
            return;
        };
        match result {
            Err(e) => self.reject(format!("Export of {} failed: {e}", running.file)),
            Ok(None) => self.reject(format!("Export of {} cancelled", running.file)),
            Ok(Some(done)) => {
                let rate = self.doc().sample_rate;
                // The report says how the export sits against the project's delivery target.
                let target = self.doc().delivery.clone();
                let against = |l: &atomik_render::loudness::LoudnessReport| {
                    target.as_ref().map_or_else(String::new, |t| {
                        format!("; {}", t.compare(l.integrated_lufs, l.true_peak_dbtp))
                    })
                };
                let message = match done {
                    Done::File(report) => {
                        log::info!("exported {}: {}", running.file, report.loudness.summary());
                        describe(&report, &running.values, rate) + &against(&report.loudness)
                    }
                    Done::Stems(report) => {
                        for ((path, _), loudness) in report.files.iter().zip(&report.loudness) {
                            let name = path
                                .file_name()
                                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                            log::info!("exported stem {name}: {}", loudness.summary());
                        }
                        let master = report
                            .files
                            .iter()
                            .zip(&report.loudness)
                            .find(|((_, s), _)| s.kind == atomik_model::StemKind::Master)
                            .map(|(_, l)| against(l))
                            .unwrap_or_default();
                        describe_stems(&report, &running.values, rate) + &master
                    }
                };
                self.reject(message);
            }
        }
    }
}
