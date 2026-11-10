//! The export worker: renders the song or the loop to an audio file, or the strips of the console
//! to a folder of stem files ([`atomik_render::stems`]), on its own thread (see
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
use atomik_render::RenderError;
use atomik_render::export::{
    BitDepth, ExportContainer, ExportFormat, ExportRender, ExportReport, ExportRequest,
    export_audio_file, render_export, write_export,
};
use atomik_render::normalise::{Finish, NormaliseReport, finish_render};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::plugins::MissingPlugins;
use atomik_render::project::{PluginBounce, ProjectOptions};
use atomik_render::stems::{StemRequest, StemStrip, StemsReport, export_stems};
use atomik_video::{
    EncodeJob, EncodeSpec, FilmCut, FilmJob, FilmPicture, FilmSound, FilmSpec, LossyCodec,
    LossyQuality, PictureChoice, Tags, VideoError, VideoTool, encode_audio, export_film,
    plan_picture, probe_file,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

/// What an export writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// One file: the mix of the document's outputs.
    File(PathBuf),
    /// One file per strip of the console, in this folder.
    Stems {
        /// The folder.
        folder: PathBuf,
        /// Which signal of the channels.
        strip: StemStrip,
    },
}

/// A compressed export: the render goes to a temporary WAV file that FFmpeg encodes.
pub(crate) struct Lossy {
    /// What runs FFmpeg.
    pub tool: Arc<dyn VideoTool>,
    /// The folder set in Settings > Video (`None`: the system path).
    pub folder: Option<PathBuf>,
    /// The format.
    pub codec: LossyCodec,
    /// The quality.
    pub quality: LossyQuality,
    /// The song's title and author for the file's tags.
    pub tags: Tags,
}

/// A film export: the render goes to a temporary WAV file that FFmpeg puts together with the
/// picture of the song's film.
pub(crate) struct Film {
    /// What runs FFmpeg.
    pub tool: Arc<dyn VideoTool>,
    /// The folder set in Settings > Video (`None`: the system path).
    pub folder: Option<PathBuf>,
    /// The song's film.
    pub source: PathBuf,
    /// The numbers of the cut.
    pub cut: FilmCut,
    /// What was asked for the picture.
    pub picture: PictureChoice,
    /// The sound of the mix (and so the container).
    pub sound: FilmSound,
    /// Keep the film's own sound as a second track.
    pub own_audio: bool,
    /// The song's title and author for the file's tags.
    pub tags: Tags,
}

/// What a finished export wrote.
#[derive(Debug, Clone, PartialEq)]
pub enum Done {
    /// The file of [`Target::File`].
    File(ExportReport),
    /// The film of a film export, with what was done to its picture.
    Film(ExportReport, FilmPicture),
    /// The files of [`Target::Stems`].
    Stems(StemsReport),
}

/// One export.
pub(crate) struct Work {
    /// The document as it was when the export was asked for.
    pub doc: RackDoc,
    /// The range and the tail.
    pub request: ExportRequest,
    /// The file format.
    pub format: ExportFormat,
    /// What to write.
    pub target: Target,
    /// The folder the project's audio lives in.
    pub bundle_root: Option<PathBuf>,
    /// The plugin machinery.
    pub plugins: PluginEnv,
    /// Encode the file with FFmpeg after the render (the format is then the temporary WAV file,
    /// at the rate of the result); `None` for a file the program writes itself.
    pub lossy: Option<Lossy>,
    /// Make a film with FFmpeg after the render (the format is then the temporary WAV file);
    /// `None` for an audio export.
    pub film: Option<Film>,
    /// The mono mix-down and the loudness target applied to the render before it is written
    /// (not to stems).
    pub finish: Finish,
    /// Also encode the finished render to a second file next to the first (a file the program
    /// writes itself plus an MP3): the encoding and the path of that file.
    pub also: Option<(Lossy, PathBuf)>,
}

/// The share of an export's progress that is the render; the encoding is the rest.
const RENDER_SHARE: f32 = 0.8;
/// The same for a film: writing a picture takes longer than writing a sound.
const FILM_RENDER_SHARE: f32 = 0.4;

/// Removes the temporary folder of a compressed export (and the WAV file in it) when dropped.
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The folder a compressed export writes its WAV file to: one per process and export.
fn temp_folder() -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "atomik-export-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The mix as a temporary WAV file, and what it is made of.
struct MixFile {
    /// The folder holding the file: removed when this is dropped.
    _folder: TempFolder,
    /// The WAV file.
    wav: PathBuf,
    /// What the write reported (its path is the temporary file).
    report: ExportReport,
    /// The channels of the audio.
    channels: usize,
}

impl MixFile {
    /// The length of the audio in seconds.
    fn seconds(&self) -> f64 {
        self.report.frames as f64 / f64::from(self.report.sample_rate)
    }
}

/// Renders the export and finishes it: the mono mix-down and the loudness target of `finish`.
/// The render's progress is `share` of `control`'s. `None`: the control stopped it.
fn render_finished(
    (doc, registry): (&RackDoc, &atomik_device_sdk::Registry),
    project: (&ProjectOptions, Option<&Path>),
    bounce: &mut PluginBounce<'_>,
    (request, finish): (&ExportRequest, &Finish),
    control: RenderControl<'_>,
    share: f32,
) -> Result<Option<(ExportRender, Option<NormaliseReport>)>, RenderError> {
    let outer = control.progress;
    let render_progress = |p: f32| {
        if let Some(outer) = outer {
            outer(p * share);
        }
    };
    let rendering = RenderControl {
        cancel: control.cancel,
        progress: Some(&render_progress),
    };
    let render = render_export(doc, registry, project, bounce, request, rendering)?;
    let Some(mut render) = render.filter(|_| !control.cancelled()) else {
        return Ok(None);
    };
    let normalise = finish_render(&mut render, finish);
    Ok(Some((render, normalise)))
}

/// Writes a finished render to a temporary WAV file (float, at the rate of `format`: the encoder
/// is the only step that loses anything). `None`: the control stopped it.
fn write_mix(
    render: &ExportRender,
    format: &ExportFormat,
    control: RenderControl<'_>,
) -> Result<Option<MixFile>, RenderError> {
    let failed = |e: &dyn std::fmt::Display| RenderError::Project(e.to_string());
    let folder = TempFolder(temp_folder());
    std::fs::create_dir_all(&folder.0).map_err(|e| failed(&e))?;
    let wav = folder.0.join("mix.wav");
    let report = write_export(&wav, render, format)?;
    if control.cancelled() {
        return Ok(None);
    }
    Ok(Some(MixFile {
        channels: render.audio.channels.len(),
        _folder: folder,
        wav,
        report,
    }))
}

/// [`render_finished`] and [`write_mix`]: the finished mix as a temporary WAV file. The report
/// of the file says what finishing did.
fn render_mix(
    docs: (&RackDoc, &atomik_device_sdk::Registry),
    project: (&ProjectOptions, Option<&Path>),
    bounce: &mut PluginBounce<'_>,
    (request, format, finish): (&ExportRequest, &ExportFormat, &Finish),
    control: RenderControl<'_>,
    share: f32,
) -> Result<Option<MixFile>, RenderError> {
    let Some((render, normalise)) =
        render_finished(docs, project, bounce, (request, finish), control, share)?
    else {
        return Ok(None);
    };
    let Some(mut mix) = write_mix(&render, format, control)? else {
        return Ok(None);
    };
    mix.report.normalise = normalise;
    Ok(Some(mix))
}

/// Encodes the temporary WAV file of `mix` to `path` with `lossy`'s codec, its progress running
/// from `from` to 1 of `control`'s. `None`: the control stopped it.
fn encode_mix(
    ffmpeg: &atomik_video::Ffmpeg,
    lossy: &Lossy,
    (mix, path): (&MixFile, &Path),
    control: RenderControl<'_>,
    from: f32,
) -> Result<Option<()>, RenderError> {
    let failed = |e: &dyn std::fmt::Display| RenderError::Project(e.to_string());
    let outer = control.progress;
    let job = EncodeJob {
        source: &mix.wav,
        out: path,
        spec: EncodeSpec {
            codec: lossy.codec,
            quality: lossy.quality,
            rate: mix.report.sample_rate,
            channels: mix.channels,
        },
        tags: &lossy.tags,
        duration_s: mix.seconds(),
    };
    let never = AtomicBool::new(false);
    let cancel = control.cancel.unwrap_or(&never);
    let encoded = encode_audio(&*lossy.tool, ffmpeg, &job, cancel, &|p| {
        if let Some(outer) = outer {
            outer(from + p * (1.0 - from));
        }
    });
    match encoded {
        Ok(()) => Ok(Some(())),
        Err(VideoError::Cancelled) => Ok(None),
        Err(e) => Err(failed(&e)),
    }
}

/// The compressed export: render, write the audio to a temporary WAV file, encode it, and report
/// the loudness of the audio that went in. Cancelling at any step removes the temporary file and
/// leaves no result file.
fn export_lossy(
    docs: (&RackDoc, &atomik_device_sdk::Registry),
    project: (&ProjectOptions, Option<&Path>),
    bounce: &mut PluginBounce<'_>,
    (request, format, finish): (&ExportRequest, &ExportFormat, &Finish),
    (path, lossy): (&Path, &Lossy),
    control: RenderControl<'_>,
) -> Result<Option<ExportReport>, RenderError> {
    let status = crate::video_worker::search(&*lossy.tool, lossy.folder.as_deref());
    let Some(ffmpeg) = status.tools() else {
        return Err(RenderError::Project(status.message()));
    };
    let Some(mix) = render_mix(
        docs,
        project,
        bounce,
        (request, format, finish),
        control,
        RENDER_SHARE,
    )?
    else {
        return Ok(None);
    };
    if encode_mix(ffmpeg, lossy, (&mix, path), control, RENDER_SHARE)?.is_none() {
        return Ok(None);
    }
    let mut report = mix.report.clone();
    path.clone_into(&mut report.path);
    Ok(Some(report))
}

/// The export of a file the program writes itself, finished as `finish` says, and, with
/// `also`, an MP3 encoded from the same finished render. Cancelling at any step removes the
/// temporary files and leaves no new file.
fn export_plain(
    docs: (&RackDoc, &atomik_device_sdk::Registry),
    project: (&ProjectOptions, Option<&Path>),
    bounce: &mut PluginBounce<'_>,
    (request, format, finish): (&ExportRequest, &ExportFormat, &Finish),
    (path, also): (&Path, Option<&(Lossy, PathBuf)>),
    control: RenderControl<'_>,
) -> Result<Option<ExportReport>, RenderError> {
    let ffmpeg_status =
        also.map(|(lossy, _)| crate::video_worker::search(&*lossy.tool, lossy.folder.as_deref()));
    let ffmpeg = match &ffmpeg_status {
        Some(status) => Some(
            status
                .tools()
                .ok_or_else(|| RenderError::Project(status.message()))?,
        ),
        None => None,
    };
    let share = if also.is_some() { RENDER_SHARE } else { 1.0 };
    let Some((render, normalise)) =
        render_finished(docs, project, bounce, (request, finish), control, share)?
    else {
        return Ok(None);
    };
    if control.cancelled() {
        return Ok(None);
    }
    let mut report = write_export(path, &render, format)?;
    report.normalise = normalise;
    if let (Some((lossy, extra)), Some(ffmpeg)) = (also, ffmpeg) {
        let rate = format.sample_rate.unwrap_or(render.audio.sample_rate);
        let wav = ExportFormat {
            container: ExportContainer::Wav,
            bit_depth: BitDepth::Float32,
            dither: atomik_dsp::DitherMode::None,
            sample_rate: Some(lossy.codec.output_rate(rate)),
        };
        let Some(mix) = write_mix(&render, &wav, control)? else {
            let _ = std::fs::remove_file(path);
            return Ok(None);
        };
        if encode_mix(ffmpeg, lossy, (&mix, extra), control, RENDER_SHARE)?.is_none() {
            let _ = std::fs::remove_file(path);
            return Ok(None);
        }
    }
    Ok(Some(report))
}

/// The film export: find FFmpeg, read the film and decide what happens to its picture (a copy
/// that cannot be made is refused here, before the render), render the mix, write it to a
/// temporary WAV file and let FFmpeg put the film together. Cancelling at any step removes the
/// temporary file and leaves no film.
fn export_film_file(
    (doc, registry): (&RackDoc, &atomik_device_sdk::Registry),
    project: (&ProjectOptions, Option<&Path>),
    bounce: &mut PluginBounce<'_>,
    (request, format, finish): (&ExportRequest, &ExportFormat, &Finish),
    (path, film): (&Path, &Film),
    control: RenderControl<'_>,
) -> Result<Option<(ExportReport, FilmPicture)>, RenderError> {
    let failed = |e: &dyn std::fmt::Display| RenderError::Project(e.to_string());
    let status = crate::video_worker::search(&*film.tool, film.folder.as_deref());
    let Some(ffmpeg) = status.tools() else {
        return Err(failed(&status.message()));
    };
    let never = AtomicBool::new(false);
    let cancel = control.cancel.unwrap_or(&never);
    let info = match probe_file(&*film.tool, ffmpeg, &film.source, cancel) {
        Ok(info) => info,
        Err(VideoError::Cancelled) => return Ok(None),
        Err(e) => return Err(failed(&format!("the film cannot be read: {e}"))),
    };
    if film.own_audio && info.audio.is_empty() {
        return Err(failed(&"the film has no sound of its own to keep"));
    }
    let picture = plan_picture(film.picture, &info, film.sound.container(), &film.cut)
        .map_err(|e| failed(&e))?;
    let outer = control.progress;
    let Some(mix) = render_mix(
        (doc, registry),
        project,
        bounce,
        (request, format, finish),
        control,
        FILM_RENDER_SHARE,
    )?
    else {
        return Ok(None);
    };
    let job = FilmJob {
        film: &film.source,
        mix: &mix.wav,
        out: path,
        info: &info,
        spec: FilmSpec {
            picture,
            sound: film.sound,
            own_audio: film.own_audio,
            rate: mix.report.sample_rate,
            channels: mix.channels,
        },
        cut: film.cut,
        tags: &film.tags,
    };
    let made = export_film(&*film.tool, ffmpeg, &job, cancel, &|p| {
        if let Some(outer) = outer {
            outer(FILM_RENDER_SHARE + p * (1.0 - FILM_RENDER_SHARE));
        }
    });
    match made {
        Ok(()) => {}
        Err(VideoError::Cancelled) => return Ok(None),
        Err(e) => return Err(failed(&e)),
    }
    let mut report = mix.report.clone();
    path.clone_into(&mut report.path);
    Ok(Some((report, picture)))
}

/// Renders and writes `work`. `None`: the control stopped it (nothing was written).
fn run(work: Work, control: RenderControl<'_>) -> Result<Option<Done>, String> {
    let Work {
        doc,
        request,
        format,
        target,
        bundle_root,
        plugins,
        lossy,
        film,
        finish,
        also,
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
    match target {
        Target::File(path) => match (&lossy, &film) {
            (_, Some(film)) => export_film_file(
                (&doc, &registry),
                (&options, bundle_root.as_deref()),
                &mut bounce,
                (&request, &format, &finish),
                (&path, film),
                control,
            )
            .map(|r| r.map(|(report, picture)| Done::Film(report, picture))),
            (Some(lossy), None) => export_lossy(
                (&doc, &registry),
                (&options, bundle_root.as_deref()),
                &mut bounce,
                (&request, &format, &finish),
                (&path, lossy),
                control,
            )
            .map(|r| r.map(Done::File)),
            (None, None) if finish.is_noop() && also.is_none() => export_audio_file(
                &doc,
                &registry,
                (&options, bundle_root.as_deref()),
                &mut bounce,
                (&request, &format),
                (&path, control),
            )
            .map(|r| r.map(Done::File)),
            (None, None) => export_plain(
                (&doc, &registry),
                (&options, bundle_root.as_deref()),
                &mut bounce,
                (&request, &format, &finish),
                (&path, also.as_ref()),
                control,
            )
            .map(|r| r.map(Done::File)),
        },
        Target::Stems { folder, strip } => {
            let stems = StemRequest {
                span: request.span,
                tail_seconds: request.tail_seconds,
                strip,
                sources: atomik_model::stem_sources(&doc, &registry),
            };
            export_stems(
                &doc,
                &registry,
                (&options, bundle_root.as_deref()),
                &mut bounce,
                (&stems, &format),
                (&folder, control),
            )
            .map(|r| r.map(Done::Stems))
        }
    }
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
    fn send(&mut self, result: Result<Option<Done>, String>) {
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
    #[cfg(test)]
    let hold = hold::take();
    std::thread::Builder::new()
        .name("atomik-export".into())
        .spawn(move || {
            #[cfg(test)]
            hold::wait(hold);
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

/// A test seam: the export the calling thread starts next waits for a release before it renders,
/// so a test can press CANCEL while the worker is certainly still at its start.
#[cfg(test)]
pub(crate) mod hold {
    use std::cell::RefCell;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    thread_local! {
        static NEXT: RefCell<Option<Receiver<()>>> = const { RefCell::new(None) };
    }

    /// Holds the next export started on this thread; send (or drop the sender) to let it run.
    pub(crate) fn next_export() -> Sender<()> {
        let (tx, rx) = channel();
        NEXT.with(|n| *n.borrow_mut() = Some(rx));
        tx
    }

    pub(super) fn take() -> Option<Receiver<()>> {
        NEXT.with(|n| n.borrow_mut().take())
    }

    pub(super) fn wait(hold: Option<Receiver<()>>) {
        if let Some(rx) = hold {
            let _ = rx.recv_timeout(Duration::from_secs(60));
        }
    }
}
