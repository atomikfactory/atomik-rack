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
use atomik_audio_file::{AudioData, Bundle};
use atomik_device_sdk::RunMode;
use atomik_dsp::rumble::{RumbleParams, reverse_swell, rumble_layer};
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

/// What the worker makes of the render before it is stored: the render is the source of a
/// rumble layer or of a reverse reverb swell instead of the result itself.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Post {
    /// A rumble layer built from the render (a kick, see `atomik_dsp::rumble`). `hits` are the
    /// frames of the kick hits from the start of the render; `None` finds them in the audio
    /// (an audio kick has no notes).
    Rumble {
        hits: Option<Vec<u64>>,
        params: RumbleParams,
    },
    /// The reverse reverb swell of the render, `seconds` long, that ends where the render starts:
    /// the result is put before the tick `anchor`, on whole ticks (see [`swell_place`]).
    ReverseReverb { seconds: f32, anchor: i64 },
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
    /// What to make of the render before it is stored.
    pub post: Option<Post>,
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

/// The plugin host a render of `doc` needs, with the device types it provides registered in
/// `registry` (`None`: the document has no plugin). `what` names the render in the message
/// that says the sandbox is needed.
pub(crate) fn plugin_host(
    doc: &RackDoc,
    plugins: PluginEnv,
    registry: &mut atomik_device_sdk::Registry,
    what: &str,
) -> Result<Option<PluginHost>, String> {
    if doc.plugin_devices().is_empty() {
        return Ok(None);
    }
    let isolation = plugins.isolation.ok_or_else(|| {
        format!(
            "this project uses plugins, which {what} can only render with the plugin sandbox: \
             turn it on in Audio > Plugins (it applies at the next start)"
        )
    })?;
    let mut host = PluginHost::with_isolation(isolation);
    host.set_catalog(&plugins.cache, &plugins.settings, registry);
    Ok(Some(host))
}

/// The rumble layer of the rendered kick `kick`: one piece per hit (`hits` as frames, else the
/// onsets found in the audio, with the start counted when the render begins loud).
pub(crate) fn rumble_of(
    kick: &AudioData,
    hits: Option<&[u64]>,
    params: &RumbleParams,
) -> Result<AudioData, String> {
    let rate = kick.sample_rate;
    let found: Vec<usize> = match hits {
        Some(frames) => frames.iter().map(|&f| f as usize).collect(),
        None => {
            let sides: Vec<&[f32]> = kick.channels.iter().map(Vec::as_slice).collect();
            let mut onsets: Vec<usize> = atomik_dsp::detect_onsets(&sides, rate, 0.5)
                .into_iter()
                .map(|f| f as usize)
                .collect();
            // The detector does not count what already sounds at the start.
            onsets.insert(0, 0);
            onsets
        }
    };
    if found.is_empty() {
        return Err("there is no kick hit to build a rumble from".to_owned());
    }
    let layer = rumble_layer(&kick.channels, rate, &found, params);
    if layer.iter().flatten().all(|s| s.abs() < 1.0e-6) {
        return Err("the kick is silent there: nothing to build a rumble from".to_owned());
    }
    AudioData::new(rate, layer).map_err(|e| format!("cannot make the rumble: {e}"))
}

/// Where a swell of `want` frames that ends at tick `anchor` goes: the start tick, the length in
/// ticks and the frames of the longest whole number of ticks that fits in `want` before the
/// anchor, so the swell ends exactly at the frame of the anchor (the audio is the last `frames`
/// frames of the swell).
///
/// # Errors
/// There is no room for a tick before the anchor (it is at the start of the song).
pub(crate) fn swell_place(
    doc: &RackDoc,
    want: u64,
    anchor: i64,
) -> Result<(i64, i64, usize), String> {
    let clock = doc.song.frame_clock(doc.sample_rate);
    let end = clock.at(anchor);
    let frames_of = |ticks: i64| end - clock.at(anchor - ticks);
    // The frames of `ticks` never decrease with the ticks: the largest that fits.
    let (mut lo, mut hi) = (0_i64, anchor.max(0));
    if frames_of(hi) <= want {
        lo = hi;
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if frames_of(mid) <= want {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return Err("there is no room before the start of the song for the swell:                     move the source later"
            .to_owned());
    }
    Ok((anchor - lo, lo, frames_of(lo) as usize))
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
        post,
    } = work;
    let mut registry = atomik_render::default_registry();
    let mut host = plugin_host(&doc, plugins, &mut registry, "a freeze")?;
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
    let Some(mut render) = rendered else {
        return Ok(None);
    };
    match post {
        None => {}
        Some(Post::Rumble { hits, params }) => {
            render.audio = rumble_of(&render.audio, hits.as_deref(), &params)?;
        }
        Some(Post::ReverseReverb { seconds, anchor }) => {
            let swell = reverse_swell(&render.audio.channels, render.audio.sample_rate, seconds);
            let (start, length, frames) = swell_place(&doc, swell[0].len() as u64, anchor)?;
            let from = swell[0].len() - frames;
            let trimmed = swell.into_iter().map(|c| c[from..].to_vec()).collect();
            render.audio = AudioData::new(render.audio.sample_rate, trimmed)
                .map_err(|e| format!("cannot make the swell: {e}"))?;
            (render.start, render.length, render.tail_frames) = (start, length, 0);
        }
    }
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
