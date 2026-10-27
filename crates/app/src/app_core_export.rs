//! Exporting the song or the loop as an audio file in the app core.
//!
//! **File > Export Song as Audio File** renders from the start to the song End and keeps the
//! tail that is still audible; **Export Loop as Audio File** renders exactly the range between
//! the locators (see [`atomik_render::export`] for the rules). The render runs on a worker thread
//! ([`crate::export_worker`]) through the offline bounce path, on a copy of the document taken
//! when it starts, so the live engine is not touched: the song keeps playing, and editing goes on
//! (an edit after the start is not in the file). One export runs at a time.
//!
//! The shell asks for the file, then for the options (its panel), then calls
//! [`AppCore::start_audio_export`]; progress and **Cancel** are [`AppCore::audio_export_progress`]
//! and [`AppCore::cancel_audio_export`]. A cancelled or failed export leaves no file (the worker
//! writes under a temporary name and renames it). Opening or creating a project cancels the
//! export that runs.

use super::*;
use crate::export_worker::{self, Work};
use crate::freeze_worker::PluginEnv;
use atomik_render::export::{
    BitDepth, DEFAULT_TAIL_SECONDS, ExportFormat, ExportReport, ExportRequest, ExportSpan,
    span_frames,
};
use atomik_ui::export_panel::rate_label;
use atomik_ui::{ExportDepth, ExportDither, ExportScope, ExportValues};
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

fn span_of(scope: ExportScope) -> ExportSpan {
    match scope {
        ExportScope::Song => ExportSpan::Song,
        ExportScope::Loop => ExportSpan::Loop,
    }
}

/// The render format the panel's values mean: the dither is for 16-bit files only.
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
        bit_depth,
        dither,
        sample_rate: values.rate,
    }
}

/// The render request the panel's values mean: a loop has no tail.
pub fn export_request(scope: ExportScope, values: &ExportValues) -> ExportRequest {
    ExportRequest {
        span: span_of(scope),
        tail_seconds: if scope == ExportScope::Song && values.tail {
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

/// The toast of a finished export: "Exported song.wav: 24-bit, 48 kHz, 1:05.3".
fn describe(report: &ExportReport, values: &ExportValues, project_rate: u32) -> String {
    let file = report
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let depth = match values.depth {
        ExportDepth::Pcm16 => "16-bit",
        ExportDepth::Pcm24 => "24-bit",
        ExportDepth::Float32 => "32-bit float",
    };
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
        "Exported {file}: {depth}, {}, {}{tail}{warnings}",
        rate_label(report.sample_rate),
        clock(seconds)
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
        !self.audio_export_busy() && span_frames(self.doc(), span_of(scope)).is_ok()
    }

    /// Why `scope` cannot be exported, for the message of the menu item.
    pub fn export_audio_refusal(&self, scope: ExportScope) -> Option<String> {
        if self.audio_export_busy() {
            return Some("An export is running: wait for it, or cancel it".to_owned());
        }
        span_frames(self.doc(), span_of(scope))
            .err()
            .map(|e| e.to_string())
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
        if let Some(why) = self.export_audio_refusal(scope) {
            self.reject(why.clone());
            return Err(why);
        }
        let file = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.export.next_job;
        self.audio.export.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            request: export_request(scope, &values),
            format: export_format(&values),
            path,
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
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

    /// Stops the running export; nothing is written.
    pub fn cancel_audio_export(&mut self) {
        match &self.audio.export.running {
            Some(r) => r.cancel.store(true, Ordering::Relaxed),
            None => self.reject("No export is running".to_owned()),
        }
    }

    /// An export finished (or failed, or was stopped): says so.
    pub(super) fn apply_exported(
        &mut self,
        job: u64,
        result: Result<Option<ExportReport>, String>,
    ) {
        let Some(running) = self.audio.export.running.take_if(|r| r.job == job) else {
            return;
        };
        match result {
            Err(e) => self.reject(format!("Export of {} failed: {e}", running.file)),
            Ok(None) => self.reject(format!("Export of {} cancelled", running.file)),
            Ok(Some(report)) => {
                let message = describe(&report, &running.values, self.doc().sample_rate);
                self.reject(message);
            }
        }
    }
}
