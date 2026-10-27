//! The export worker: renders the song or the loop to an audio file on its own thread (see
//! [`crate::AppCore::start_audio_export`]).
//!
//! Like the freeze worker it owns what it needs (a copy of the document, its own device registry)
//! and talks to the UI thread only through [`Msg::Exported`]; progress and cancel are two shared
//! atomics. The render is [`atomik_render::export::export_audio_file`], the project bounce, on
//! this one thread (no engine workers: the live engine has the cores), so the session keeps
//! playing and editing while the file is made. The plugins are built as for a freeze
//! ([`crate::freeze_worker::plugin_host`]): a project with plugins needs the plugin sandbox.

use crate::audio_import::Msg;
use crate::freeze_worker::{PluginEnv, plugin_host};
use atomik_device_sdk::RunMode;
use atomik_model::RackDoc;
use atomik_render::export::{ExportFormat, ExportReport, ExportRequest, export_audio_file};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// One export.
pub(crate) struct Work {
    /// The document as it was when the export was asked for.
    pub doc: RackDoc,
    /// The range and the tail.
    pub request: ExportRequest,
    /// The file format.
    pub format: ExportFormat,
    /// The file to write.
    pub path: PathBuf,
    /// The folder the project's audio lives in.
    pub bundle_root: Option<PathBuf>,
    /// The plugin machinery.
    pub plugins: PluginEnv,
}

/// Renders and writes `work`. `None`: the control stopped it (nothing was written).
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<ExportReport>, String> {
    let Work {
        doc,
        request,
        format,
        path,
        bundle_root,
        plugins,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "an export")?;
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
    export_audio_file(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        (&request, &format),
        (&path, control),
    )
    .map_err(|e| e.to_string())
}

/// Sends the result of an export, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<ExportReport>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Exported {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the export stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that renders `work` and sends [`Msg::Exported`]; `cancel` stops it
/// early and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-export".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                generation,
                job,
                sent: false,
            };
            let report = |p: f32| progress.store(p.to_bits(), Ordering::Relaxed);
            let result = run(
                work,
                RenderControl {
                    cancel: Some(&cancel),
                    progress: Some(&report),
                },
            );
            guard.send(result);
        })
        .map(|_| ())
}
