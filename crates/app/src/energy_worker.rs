//! The energy worker: renders the master of the song on its own thread, measures it per section
//! and builds the energy map ([`atomik_model::energy_map`]); see [`crate::AppCore::start_energy`].
//!
//! Like the masking worker it owns what it needs (a copy of the document, its own device
//! registry) and talks to the UI thread only through [`Msg::Energy`]; progress and cancel are two
//! shared atomics. The render is the stem render of the master ([`atomik_render::energy`]), one
//! thread, no engine workers: the live engine has the cores, so the session keeps playing while
//! the song is measured. The plugins are built as for a freeze
//! ([`crate::freeze_worker::plugin_host`]).

use crate::audio_import::Msg;
use crate::freeze_worker::{PluginEnv, plugin_host};
use atomik_device_sdk::RunMode;
use atomik_model::RackDoc;
use atomik_model::energy_map::{EnergyMap, Measure, active_counts, sections};
use atomik_render::energy::render_blocks;
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
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<EnergyMap>, String> {
    let Work {
        doc,
        bundle_root,
        plugins,
    } = work;
    let parts = sections(&doc.song);
    if parts.is_empty() {
        return Err("the song is empty: there is nothing to measure".to_owned());
    }
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "the energy map")?;
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
    let Some(blocks) = render_blocks(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        control,
    )
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let (counts, total) = active_counts(&doc.song, &parts);
    let clock = doc.song.frame_clock(doc.sample_rate);
    let measures: Vec<Measure> = parts
        .iter()
        .zip(&counts)
        .map(|(s, &tracks)| {
            let f = blocks.features(clock.at(s.start), clock.at(s.end));
            Measure {
                loudness_db: f.loudness_db,
                brightness_hz: f.brightness_hz,
                width: f.width,
                tracks,
            }
        })
        .collect();
    Ok(Some(EnergyMap::new(&doc.song, parts, &measures, total)))
}

/// Sends the result of a measurement, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<EnergyMap>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Energy {
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

/// Starts a worker thread that measures `work` and sends [`Msg::Energy`]; `cancel` stops it early
/// and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-energy".into())
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
