//! The mix check worker: renders the strips of the song as stems, measures them and builds the
//! findings ([`atomik_render::mixcheck`]); see [`crate::AppCore::start_mixcheck`].
//!
//! Like the masking worker it owns what it needs (a copy of the document, its own device
//! registry) and talks to the UI thread only through [`Msg::Checked`]; progress and cancel are two
//! shared atomics. The render is the stem render, one thread, no engine workers: the live engine
//! has the cores, so the session keeps playing while the song is checked. The plugins are built as
//! for a freeze ([`crate::freeze_worker::plugin_host`]).

use crate::audio_import::Msg;
use crate::freeze_worker::{PluginEnv, plugin_host};
use atomik_device_sdk::RunMode;
use atomik_model::RackDoc;
use atomik_render::export::ExportSpan;
use atomik_render::mixcheck::{Finding, findings, plan, render_check};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// What a check found.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckAnalysis {
    /// The first frame of the checked range, as a song frame.
    pub from_frame: u64,
    /// The sample rate of the project.
    pub rate: u32,
    /// How many strips were measured (channels, returns and the master).
    pub strips: usize,
    /// The findings, in the order of [`atomik_render::mixcheck::Kind`].
    pub findings: Vec<Finding>,
}

/// One check.
pub(crate) struct Work {
    /// The document as it was when the check was asked for.
    pub doc: RackDoc,
    /// The range.
    pub span: ExportSpan,
    /// The folder the project's audio lives in.
    pub bundle_root: Option<PathBuf>,
    /// The plugin machinery.
    pub plugins: PluginEnv,
}

/// Renders and checks `work`. `None`: the control stopped it.
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<CheckAnalysis>, String> {
    let Work {
        doc,
        span,
        bundle_root,
        plugins,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "the mix check")?;
    atomik_model::register_stand_ins(&doc, &mut registry).map_err(|e| e.to_string())?;
    let options = ProjectOptions {
        host_block: std::num::NonZeroUsize::new(512).unwrap_or(std::num::NonZeroUsize::MIN),
        workers: 0,
        loop_count: None,
    };
    let mut bounce = PluginBounce {
        host: host.as_mut(),
        mode: RunMode::Offline,
        missing: MissingPlugins::Error,
    };
    let plan = plan(&doc, &registry).map_err(|e| e.to_string())?;
    let (from_frame, _) =
        atomik_render::export::span_frames(&doc, span).map_err(|e| e.to_string())?;
    let Some(rendered) = render_check(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        (&plan, span),
        control,
    )
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    Ok(Some(CheckAnalysis {
        from_frame,
        rate: doc.sample_rate,
        strips: rendered.signals.len(),
        findings: findings(&doc, &rendered),
    }))
}

/// Sends the result of a check, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<CheckAnalysis>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Checked {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the check stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that checks `work` and sends [`Msg::Checked`]; `cancel` stops it early
/// and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-mixcheck".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                generation,
                job,
                sent: false,
            };
            let report = |p: f32| progress.store(p.to_bits(), Ordering::Relaxed);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    work,
                    RenderControl {
                        cancel: Some(&cancel),
                        progress: Some(&report),
                    },
                )
            }))
            .unwrap_or_else(|_| Err("a device failed while the song was checked".to_owned()));
            guard.send(result);
        })
        .map(|_| ())
}
