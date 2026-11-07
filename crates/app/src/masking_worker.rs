//! The masking worker: renders the channels of the console as stems on its own thread, turns each
//! into a band map and ranks the pairs ([`atomik_dsp::masking`]); see
//! [`crate::AppCore::start_masking`].
//!
//! Like the export worker it owns what it needs (a copy of the document, its own device registry)
//! and talks to the UI thread only through [`Msg::Masked`]; progress and cancel are two shared
//! atomics. The render is the stem render ([`atomik_render::masking`]), one thread, no engine
//! workers: the live engine has the cores, so the session keeps playing while the song is
//! analysed. The plugins are built as for a freeze ([`crate::freeze_worker::plugin_host`]).

use crate::audio_import::Msg;
use crate::freeze_worker::{PluginEnv, plugin_host};
use atomik_device_sdk::RunMode;
use atomik_dsp::masking::{PairScore, rank_pairs};
use atomik_model::{DeviceKey, RackDoc, StemKind};
use atomik_render::export::ExportSpan;
use atomik_render::masking::{MaskRequest, render_maps};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// How many pairs the analysis keeps.
pub const MAX_PAIRS: usize = 24;

/// One analysed track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskTrack {
    /// The channel strip.
    pub channel: DeviceKey,
    /// The name of the track (or strip).
    pub name: String,
}

/// What an analysis found.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskAnalysis {
    /// The first tick that was analysed.
    pub from_tick: i64,
    /// The first frame of the analysis, as a song frame.
    pub from_frame: u64,
    /// The sample rate of the project.
    pub rate: u32,
    /// The tracks that were compared, in console order.
    pub tracks: Vec<MaskTrack>,
    /// The worst pairs, highest score first.
    pub pairs: Vec<PairScore>,
}

/// One analysis.
pub(crate) struct Work {
    /// The document as it was when the analysis was asked for.
    pub doc: RackDoc,
    /// The range.
    pub span: ExportSpan,
    /// The folder the project's audio lives in.
    pub bundle_root: Option<PathBuf>,
    /// The plugin machinery.
    pub plugins: PluginEnv,
}

/// Renders and analyses `work`. `None`: the control stopped it.
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<MaskAnalysis>, String> {
    let Work {
        doc,
        span,
        bundle_root,
        plugins,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "the masking analysis")?;
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
    let sources: Vec<_> = atomik_model::stem_sources(&doc, &registry)
        .into_iter()
        .filter(|s| s.kind == StemKind::Channel)
        .take(atomik_dsp::masking::MAX_TRACKS)
        .collect();
    if sources.len() < 2 {
        return Err("it needs at least two channels to compare".to_owned());
    }
    let (from_frame, _) =
        atomik_render::export::span_frames(&doc, span).map_err(|e| e.to_string())?;
    let from_tick = match span {
        ExportSpan::Range(start, _) => start,
        ExportSpan::Loop => doc.song.loop_range.0,
        ExportSpan::Song => 0,
    };
    let request = MaskRequest { span, sources };
    let Some(maps) = render_maps(
        &doc,
        &registry,
        (&options, bundle_root.as_deref()),
        &mut bounce,
        &request,
        control,
    )
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let refs: Vec<_> = maps.iter().map(|t| &t.map).collect();
    let pairs = rank_pairs(&refs, MAX_PAIRS);
    let tracks = maps
        .iter()
        .map(|t| MaskTrack {
            channel: t.source.key.clone(),
            name: t.source.name.clone(),
        })
        .collect();
    Ok(Some(MaskAnalysis {
        from_tick,
        from_frame,
        rate: doc.sample_rate,
        tracks,
        pairs,
    }))
}

/// Sends the result of an analysis, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Option<MaskAnalysis>, String>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Masked {
            generation: self.generation,
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err("the analysis stopped unexpectedly".to_owned()));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// Starts a worker thread that analyses `work` and sends [`Msg::Masked`]; `cancel` stops it early
/// and `progress` receives the fraction done as `f32` bits.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    (generation, job): (u64, u64),
    work: Work,
    (cancel, progress): (Arc<AtomicBool>, Arc<AtomicU32>),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-masking".into())
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
            .unwrap_or_else(|_| Err("a device failed while the song was analysed".to_owned()));
            guard.send(result);
        })
        .map(|_| ())
}
