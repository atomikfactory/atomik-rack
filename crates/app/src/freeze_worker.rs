//! The freeze worker: renders a track's chain on its own thread and stores the audio in the
//! project's bundle (see [`crate::AppCore::freeze_tracks`]).
//!
//! The worker owns what it needs (a copy of the document, a clone of the bundle, its own device
//! registry) and talks to the UI thread only through [`Msg::Frozen`]; progress and cancel are two
//! shared atomics. The render is [`atomik_render::freeze::render_freeze`], the same bounce path
//! the command line uses, one engine on this thread (no workers: the live engine has the cores).
//!
//! **Plugins.** A document with plugin devices needs a plugin host on the worker's thread. The
//! host is made here, from the app's scan cache and settings, with the sandbox of the app (each
//! instance in a child process, waited for every batch like a bounce): plugin instances may only
//! live on the thread that made the host, and the worker is that thread. Without the sandbox the
//! app's plugins live in the app process and cannot be built here, so the freeze is refused with
//! a message that says so.

use crate::audio_import::{Loaded, Msg, load_asset};
use atomik_audio_file::Bundle;
use atomik_device_sdk::RunMode;
use atomik_model::RackDoc;
use atomik_plugin_host::{PluginCache, PluginSettings};
use atomik_render::freeze::{FreezeRequest, render_freeze};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::{MissingPlugins, PluginHost, PluginIsolation};
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// What the worker needs to build the plugins of the document.
#[derive(Debug, Clone)]
pub(crate) struct PluginEnv {
    /// The app's scan cache.
    pub cache: PluginCache,
    /// The app's plugin settings.
    pub settings: PluginSettings,
    /// The sandbox of the app, for a bounce (`None`: the app's plugins are in its own process).
    pub isolation: Option<PluginIsolation>,
}

/// One render.
pub(crate) struct Work {
    /// The document as it was when the render was asked for.
    pub doc: RackDoc,
    /// What to render.
    pub request: FreezeRequest,
    /// The project bundle (or the session scratch bundle): its audio is read and the render is
    /// stored in it.
    pub bundle: Bundle,
    /// The decoded size above which an asset streams (the render is loaded the usual way).
    pub threshold: u64,
    /// The original name the render gets in the pool.
    pub name: String,
    /// The plugin machinery.
    pub plugins: PluginEnv,
}

/// A finished render, stored and loaded.
pub(crate) struct Done {
    /// The stored asset, as an import gives it.
    pub loaded: Loaded,
    /// Song tick where the render starts.
    pub start: i64,
    /// Length in ticks of the clip that plays it.
    pub length: i64,
    /// Frames past the end of the span that the tail kept.
    pub tail_frames: u64,
    /// What the render reported.
    pub warnings: Vec<String>,
}

/// Renders `work`. `None`: the control stopped it.
fn render(work: Work, control: RenderControl<'_>) -> Result<Option<Done>, String> {
    let Work {
        doc,
        request,
        mut bundle,
        threshold,
        name,
        plugins,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host = if doc.plugin_devices().is_empty() {
        None
    } else {
        let isolation = plugins.isolation.ok_or_else(|| {
            "this project uses plugins, which a freeze can only render with the plugin sandbox: \
             turn it on in Audio > Plugins (it applies at the next start)"
                .to_owned()
        })?;
        let mut host = PluginHost::with_isolation(isolation);
        host.set_catalog(&plugins.cache, &plugins.settings, &mut registry);
        Some(host)
    };
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
    let rendered = render_freeze(
        &doc,
        &registry,
        (&options, Some(bundle.root())),
        &mut bounce,
        &request,
        control,
    )
    .map_err(|e| e.to_string())?;
    let Some(render) = rendered else {
        return Ok(None);
    };
    let info = bundle
        .add_recording(&render.audio, &name)
        .map_err(|e| format!("cannot store the render: {e}"))?;
    let mut loaded = load_asset(&bundle, &info.hash, threshold)?;
    loaded.info = Some(info);
    Ok(Some(Done {
        loaded,
        start: render.start,
        length: render.length,
        tail_frames: render.tail_frames,
        warnings: render.warnings.iter().map(ToString::to_string).collect(),
    }))
}

/// Sends the result of a render, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<Done>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Frozen {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the render stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that renders `work` and sends [`Msg::Frozen`]; `cancel` stops it early
/// and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-freeze".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                generation,
                job,
                sent: false,
            };
            let report = |p: f32| progress.store(p.to_bits(), Ordering::Relaxed);
            let result = render(
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
