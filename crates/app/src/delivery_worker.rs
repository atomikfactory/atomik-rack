//! The delivery worker: renders the master of the song on its own thread and measures its loudness
//! and true peak ([`atomik_render::delivery`]); see [`crate::AppCore::start_delivery`].
//!
//! Like the energy worker it owns what it needs (a copy of the document, its own device registry)
//! and talks to the UI thread only through [`Msg::Delivery`]; progress and cancel are two shared
//! atomics. The render is the stem render of the master, one thread, no engine workers: the live
//! engine has the cores, so the session keeps playing while the master is measured. The plugins
//! are built as for a freeze ([`crate::freeze_worker::plugin_host`]).

use crate::audio_import::Msg;
use crate::freeze_worker::{PluginEnv, plugin_host};
use atomik_device_sdk::RunMode;
use atomik_model::RackDoc;
use atomik_render::delivery::measure_master;
use atomik_render::loudness::LoudnessReport;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// One measurement.
pub(crate) struct Work {
    /// The document as it was when the measurement was asked for.
    pub doc: RackDoc,
    /// The folder the project's audio lives in.
    pub bundle_root: Option<PathBuf>,
    /// The plugin machinery.
    pub plugins: PluginEnv,
}

/// Renders and measures `work`. `None`: the control stopped it.
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<LoudnessReport>, String> {
    let Work {
        doc,
        bundle_root,
        plugins,
    } = work;
    if doc.song.tracks.is_empty() {
        return Err("the song is empty: there is nothing to measure".to_owned());
    }
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "the delivery targets")?;
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
    measure_master(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        control,
    )
    .map_err(|e| e.to_string())
}

/// Sends the result of a measurement, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<LoudnessReport>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Delivery {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the measurement stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that measures `work` and sends [`Msg::Delivery`]; `cancel` stops it early
/// and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-delivery".into())
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
            .unwrap_or_else(|_| Err("a device failed while the song was measured".to_owned()));
            guard.send(result);
        })
        .map(|_| ())
}
