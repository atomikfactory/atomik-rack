//! Stems: the strips of the console rendered to audio of their own, in one deterministic pass.
//!
//! A stem is the output of one strip of the console over the same range an export keeps
//! ([`crate::export`]): a channel, an output bus, an FX return or the master
//! ([`atomik_model::stem_sources`]). The render is the project bounce ([`crate::project`]) with
//! one **tap** per strip on the flat rack: the strip's output sockets are cabled to two more
//! interface outputs each, exactly as a freeze taps a track ([`crate::freeze`]). A tap is one more
//! cable from an output and changes no sample of the song, and the engine renders the same
//! blocks whatever the taps are, so a stem is bit for bit the same audio in every pass, at every
//! host block size and with any number of engine workers.
//!
//! * **Strip.** [`StemStrip::Full`] takes each strip as it sounds (fader, pan, inserts and
//!   output applied; a muted or soloed-out strip is silent, as in the mix); [`StemStrip::Dry`]
//!   takes the channels' source before the strip, with no fader, pan, insert or send, and leaves
//!   the buses, returns and master out (they have no dry signal).
//! * **Alignment.** A strip's output comes out of the engine before the delay compensation the
//!   master applies, so a strip that is early compared with the slowest path to the outputs
//!   (a look-ahead device, a plugin, a channel's latency adjust) is delayed here by the
//!   difference. Every stem then has its audio where the master has it, and they all line up with
//!   each other and with the bounce of the song. The latencies are those the engine published
//!   when the render ended, summed along the cables like its own compensation does; with
//!   delay compensation off nothing is shifted.
//! * **Length.** All the stems have the length of the song export: the range, plus the tail of
//!   the document's own outputs while it is audible ([`crate::export::render_export`]).
//! * **Memory.** At most [`TAPS_PER_PASS`] strips are rendered per pass of the song, and each
//!   stem is handed on as soon as its pass ends, so a long song with many strips costs
//!   passes, not memory.
//!
//! Where the song has no send effect and the strips are routed to the master directly, the
//! full stems of the channels add up to the master within the rounding of the summation (the
//! engine sums the master's inputs in one order, this sums them in another).

use crate::export::{
    ExportFormat, ExportRender, ExportSpan, TAIL_FLOOR, span_frames, write_export,
};
use crate::plugin_pass::RenderControl;
use crate::project::{Core, Extra, PluginBounce, ProjectOptions, render_core};
use crate::{RenderError, Warning};
use atomik_audio_file::AudioData;
use atomik_device_sdk::Registry;
use atomik_model::{
    DeviceKey, ExpandedRack, INTERFACE_KEY, RackDoc, StemKind, StemSource, stem_file_names,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use crate::export::DEFAULT_TAIL_SECONDS;

/// The most strips one pass of the song reads (each is two more channels of the render).
pub const TAPS_PER_PASS: usize = 4;

/// Which signal of a channel is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StemStrip {
    /// The strip's output: everything the strip does is applied.
    #[default]
    Full,
    /// The source's output before the strip (channels only).
    Dry,
}

/// What to render.
#[derive(Debug, Clone, PartialEq)]
pub struct StemRequest {
    /// The range: the song with its tail, or the loop.
    pub span: ExportSpan,
    /// Seconds of tail to keep past the End while the outputs are audible (a song only).
    pub tail_seconds: f64,
    /// Which signal of the channels.
    pub strip: StemStrip,
    /// The strips to take, in the order they come out ([`atomik_model::stem_sources`]).
    pub sources: Vec<StemSource>,
}

/// One rendered stem.
#[derive(Debug, Clone, PartialEq)]
pub struct Stem {
    /// The strip.
    pub source: StemSource,
    /// Its place in the request (the order of the files).
    pub index: usize,
    /// The audio at the project's rate: two channels, as long as the export.
    pub audio: AudioData,
    /// Frames of delay the alignment added (0 when the strip is as late as the master).
    pub delay_frames: u64,
}

/// What a render of stems produced besides the stems.
#[derive(Debug, Clone, PartialEq)]
pub struct StemsRendered {
    /// Frames of the range itself.
    pub span_frames: u64,
    /// Frames of the tail that were kept.
    pub tail_frames: u64,
    /// What the render reported.
    pub warnings: Vec<Warning>,
}

/// The latency in frames at the output of every device of `rack`: the longest path to it, each
/// device adding its own latency (`own`) to the latest of its inputs. A loop is cut where it
/// closes.
pub fn path_latencies(
    rack: &ExpandedRack,
    own: &BTreeMap<DeviceKey, u32>,
) -> BTreeMap<DeviceKey, u64> {
    let mut sources: BTreeMap<&DeviceKey, Vec<&DeviceKey>> = BTreeMap::new();
    for c in &rack.cables {
        if c.from.device.0 != INTERFACE_KEY && c.to.device.0 != INTERFACE_KEY {
            sources
                .entry(&c.to.device)
                .or_default()
                .push(&c.from.device);
        }
    }
    let mut done: BTreeMap<DeviceKey, u64> = BTreeMap::new();
    for d in &rack.devices {
        // An explicit stack: a rack can be a long chain.
        let mut stack: Vec<(&DeviceKey, bool)> = vec![(&d.key, false)];
        let mut open: Vec<&DeviceKey> = Vec::new();
        while let Some((key, ready)) = stack.pop() {
            if ready {
                open.retain(|k| *k != key);
                let input = sources
                    .get(key)
                    .into_iter()
                    .flatten()
                    .map(|s| done.get(*s).copied().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                done.insert(
                    key.clone(),
                    input + u64::from(own.get(key).copied().unwrap_or(0)),
                );
                continue;
            }
            if done.contains_key(key) || open.contains(&key) {
                continue;
            }
            open.push(key);
            stack.push((key, true));
            for s in sources.get(key).into_iter().flatten() {
                stack.push((s, false));
            }
        }
    }
    done
}

/// Renders the stems of `request` from `doc` (see the module docs) and hands each one to
/// `on_stem` as its pass ends, in the order of the request. `bundle_root` is the folder the
/// project's audio lives in, as for a bounce. A stopped render returns `None`.
///
/// # Errors
/// The request is empty or out of range, the bounce of the document fails, or `on_stem` does.
pub fn render_stems(
    doc: &RackDoc,
    registry: &Registry,
    (options, bundle_root): (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    request: &StemRequest,
    (control, on_stem): (
        RenderControl<'_>,
        &mut dyn FnMut(Stem) -> Result<(), RenderError>,
    ),
) -> Result<Option<StemsRendered>, RenderError> {
    let fail = |m: String| RenderError::Project(m);
    if !(0.0..=crate::freeze::MAX_TAIL_SECONDS).contains(&request.tail_seconds) {
        return Err(fail(format!(
            "the tail must be between 0 and {} seconds",
            crate::freeze::MAX_TAIL_SECONDS
        )));
    }
    // The strips the request can take, with the sockets each is read at.
    let taken: Vec<(usize, &StemSource, [atomik_model::SocketRef; 2])> = request
        .sources
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let tap = match request.strip {
                StemStrip::Full => Some(s.tap.clone()),
                StemStrip::Dry => s.dry.clone(),
            }?;
            Some((i, s, tap))
        })
        .collect();
    if taken.is_empty() {
        return Err(fail(match request.strip {
            StemStrip::Full => "there is no strip to export".to_owned(),
            StemStrip::Dry => "there is no channel with a source to export dry".to_owned(),
        }));
    }
    let (from, to) = span_frames(doc, request.span)?;
    let tail = match request.span {
        ExportSpan::Song => (request.tail_seconds * f64::from(doc.sample_rate)).round() as u64,
        ExportSpan::Loop => 0,
    };
    let frames = usize::try_from(to + tail).map_err(|e| fail(e.to_string()))?;
    let (from_f, to_f) = (from as usize, to as usize);
    let flat = doc.expand(registry);
    let probes: Vec<DeviceKey> = flat.devices.iter().map(|d| d.key.clone()).collect();
    let passes: Vec<&[(usize, &StemSource, [atomik_model::SocketRef; 2])]> =
        taken.chunks(TAPS_PER_PASS).collect();
    let base = usize::from(doc.output_channels);
    let mut delays: Option<BTreeMap<DeviceKey, u64>> = None;
    let mut summary: Option<StemsRendered> = None;
    for (n, pass) in passes.iter().enumerate() {
        let scaled = |p: f32| {
            if let Some(report) = control.progress {
                report((n as f32 + p.clamp(0.0, 1.0)) / passes.len() as f32);
            }
        };
        let extra = Extra {
            taps: pass.iter().map(|(_, _, tap)| tap.clone()).collect(),
            frames: Some(frames),
            // The latencies are the same in every pass: read them once.
            probes: if delays.is_none() {
                probes.clone()
            } else {
                Vec::new()
            },
            meter_probes: Vec::new(),
            control: RenderControl {
                cancel: control.cancel,
                progress: control.progress.map(|_| &scaled as &(dyn Fn(f32) + Sync)),
            },
        };
        let Some(Core {
            rendered,
            latencies,
            ..
        }) = render_core(doc, registry, (options, bundle_root), plugins, &extra)?
        else {
            return Ok(None);
        };
        let delay_of = delays.get_or_insert_with(|| {
            let own: BTreeMap<DeviceKey, u32> = probes.iter().cloned().zip(latencies).collect();
            alignment_delays(doc, &flat, &own, &taken)
        });
        // The outputs of the document decide how much of the tail is audible, as in an export.
        let keep = match request.span {
            ExportSpan::Loop => to_f,
            ExportSpan::Song => {
                let len = rendered.channels.first().map_or(0, Vec::len);
                let outputs = &rendered.channels[..base.min(rendered.channels.len())];
                let audible = (0..len)
                    .rev()
                    .find(|&i| outputs.iter().any(|c| c[i].abs() > TAIL_FLOOR))
                    .map_or(0, |i| i + 1);
                audible.clamp(to_f, frames)
            }
        };
        let span = to - from;
        summary.get_or_insert_with(|| StemsRendered {
            span_frames: span,
            tail_frames: (keep - to_f) as u64,
            warnings: rendered.warnings.clone(),
        });
        for (k, (index, source, tap)) in pass.iter().enumerate() {
            let delay = delay_of.get(&tap[0].device).copied().unwrap_or(0);
            let d = usize::try_from(delay).unwrap_or(usize::MAX);
            let shifted = |c: &[f32]| -> Vec<f32> {
                (from_f..keep)
                    .map(|i| {
                        i.checked_sub(d)
                            .and_then(|j| c.get(j).copied())
                            .unwrap_or(0.0)
                    })
                    .collect()
            };
            let channels = vec![
                shifted(&rendered.channels[base + 2 * k]),
                shifted(&rendered.channels[base + 2 * k + 1]),
            ];
            let audio =
                AudioData::new(doc.sample_rate, channels).map_err(|e| fail(e.to_string()))?;
            on_stem(Stem {
                source: (*source).clone(),
                index: *index,
                audio,
                delay_frames: delay,
            })?;
        }
        if control.cancelled() {
            return Ok(None);
        }
    }
    Ok(summary)
}

/// The delay in frames to give the stem read at each strip's tap device so it lands where the
/// master has its audio: the latency of the slowest path to the document's outputs minus the
/// latency of the path to the tap. Empty (no delay) when delay compensation is off.
fn alignment_delays(
    doc: &RackDoc,
    flat: &ExpandedRack,
    own: &BTreeMap<DeviceKey, u32>,
    taken: &[(usize, &StemSource, [atomik_model::SocketRef; 2])],
) -> BTreeMap<DeviceKey, u64> {
    if !doc.console.compensation {
        return BTreeMap::new();
    }
    let latency = path_latencies(flat, own);
    let reference = flat
        .cables
        .iter()
        .filter(|c| c.to.device.0 == INTERFACE_KEY)
        .filter_map(|c| latency.get(&c.from.device))
        .max()
        .copied()
        .unwrap_or(0);
    taken
        .iter()
        .map(|(_, _, tap)| {
            let at = latency.get(&tap[0].device).copied().unwrap_or(0);
            (tap[0].device.clone(), reference.saturating_sub(at))
        })
        .collect()
}

/// A finished export of stems.
#[derive(Debug, Clone, PartialEq)]
pub struct StemsReport {
    /// The folder the files were written to.
    pub folder: PathBuf,
    /// The files, in the order of the request, with the strip each holds.
    pub files: Vec<(PathBuf, StemSource)>,
    /// Frames of each file at its sample rate.
    pub frames: u64,
    /// The sample rate of the files in Hz.
    pub sample_rate: u32,
    /// Frames of the range itself, at the project's rate.
    pub span_frames: u64,
    /// Frames of the tail that were kept, at the project's rate.
    pub tail_frames: u64,
    /// What the render reported.
    pub warnings: Vec<Warning>,
}

/// The name of the folder an export of stems goes into: `Bounced <song name>`.
pub fn stems_folder_name(song_name: &str) -> String {
    format!("Bounced {}", atomik_model::safe_file_name(song_name))
}

/// Renders the stems of `request` and writes each to its own file in `folder` (created when it is
/// missing) in the container, depth, dither and rate of `format`: `01 Kick.wav`, `02 Bass.wav`,
/// and so on ([`atomik_model::stem_file_names`]). Every file is written under a temporary name
/// and renamed when it is complete, so one that fails or is stopped leaves nothing of its own;
/// a render that fails or is stopped part way removes the files it had written (a folder it
/// created too), leaving what was in the folder before.
///
/// # Errors
/// As [`render_stems`], or a file cannot be written.
pub fn export_stems(
    doc: &RackDoc,
    registry: &Registry,
    project: (&ProjectOptions, Option<&Path>),
    plugins: &mut PluginBounce<'_>,
    (request, format): (&StemRequest, &ExportFormat),
    (folder, control): (&Path, RenderControl<'_>),
) -> Result<Option<StemsReport>, RenderError> {
    let created = !folder.exists();
    std::fs::create_dir_all(folder).map_err(|e| RenderError::Write {
        path: folder.to_path_buf(),
        source: hound::Error::from(e).into(),
    })?;
    let names = stem_file_names(&request.sources);
    let extension = format.container.extension();
    let mut files: Vec<(PathBuf, StemSource)> = Vec::new();
    let mut last: Option<(u64, u32)> = None;
    let mut write = |stem: Stem| -> Result<(), RenderError> {
        let path = folder.join(format!("{}.{extension}", names[stem.index]));
        let render = ExportRender {
            audio: stem.audio,
            span_frames: 0,
            tail_frames: 0,
            warnings: Vec::new(),
        };
        let report = write_export(&path, &render, format)?;
        last = Some((report.frames, report.sample_rate));
        files.push((path, stem.source));
        Ok(())
    };
    let rendered = render_stems(
        doc,
        registry,
        project,
        plugins,
        request,
        (control, &mut write),
    );
    let cleanup = |files: &[(PathBuf, StemSource)]| {
        for (path, _) in files {
            let _ = std::fs::remove_file(path);
        }
        if created {
            let _ = std::fs::remove_dir(folder);
        }
    };
    let rendered = match rendered {
        Ok(Some(r)) if !control.cancelled() => r,
        Ok(_) => {
            cleanup(&files);
            return Ok(None);
        }
        Err(e) => {
            cleanup(&files);
            return Err(e);
        }
    };
    let (frames, sample_rate) = last.unwrap_or((0, doc.sample_rate));
    // The stems of a dry request skip the strips without a source: the files keep the order.
    files.sort_by_key(|(_, s)| {
        request
            .sources
            .iter()
            .position(|o| o == s)
            .unwrap_or(usize::MAX)
    });
    Ok(Some(StemsReport {
        folder: folder.to_path_buf(),
        files,
        frames,
        sample_rate,
        span_frames: rendered.span_frames,
        tail_frames: rendered.tail_frames,
        warnings: rendered.warnings,
    }))
}

/// True for the strips a dry stem can be taken from.
pub fn has_dry(source: &StemSource) -> bool {
    source.kind == StemKind::Channel && source.dry.is_some()
}
