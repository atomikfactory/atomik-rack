//! File > Export > Film with Mix in the app core: the song's mix over the span of the song's film,
//! written as a new film by the user's FFmpeg.
//!
//! The export is the audio export of `app_core_export.rs` with one more step. [`film_plan`] turns
//! where the film plays in the song (its start and trim, in microseconds) into the range of the
//! song the mix covers and the numbers of the cut ([`atomik_video::FilmCut`]); the export worker
//! renders that range to a temporary WAV file, probes the film, decides what happens to its
//! picture (copied when the cut points allow it, else re-encoded) and lets FFmpeg put the film
//! together ([`atomik_video::export_film`]), with progress and cancel and no partial file. The
//! mix's first sample sits at the first exported frame: the film's span starts the new film, and
//! a film that begins before the song's start gets silence in front of the mix.
//!
//! **Cover.** *The film's span* is exactly the trimmed film as it is placed in the song (the
//! range of the song under it, cut to it). *The whole song* renders from the start to the song End
//! and covers the film and the song both, with black pictures where only the mix runs (that
//! needs a re-encode: a copy is refused, and AUTO re-encodes).
//!
//! **Picture.** A copied picture is cut at the packets of the file, so a cut that is not at the
//! start of the film snaps to a keyframe: the picture can begin a little earlier than the trim
//! while the mix still starts at the trim. AUTO copies only a cut at the film's start; choose
//! re-encode (H.264 in MP4 or MOV, with a quality) for exact cuts.
//!
//! **Sound.** The mix goes in as AAC in an MP4 file (a bitrate choice) or as 24-bit PCM in a MOV
//! file. The film's own first audio track can stay as a second track.
//!
//! The export is refused, with the message the menu item and the toast carry, while another
//! export runs, when the song has no film, when the film's file is missing and when FFmpeg
//! cannot be used.

use super::*;
use atomik_render::export::BitDepth;
use atomik_ui::{
    ExportLoudness, ExportScope, FilmCover, FilmPictureChoice, FilmQualityChoice, FilmSoundChoice,
    FilmValues,
};
use atomik_video::{FilmCut, FilmPicture, FilmQuality, FilmSound, PictureChoice, Rational};

/// What a running film export was asked for, kept for its toast.
pub(crate) struct FilmRun {
    /// The panel's choices.
    pub values: FilmValues,
    /// The numbers of the cut.
    pub cut: FilmCut,
}

/// The range of the song the mix covers and the cut of the film, for one set of choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FilmPlan {
    /// The range to render.
    pub request: ExportRequest,
    /// The numbers of the cut.
    pub cut: FilmCut,
}

/// What an export of the song's film covers, or why there is nothing to export.
///
/// # Errors
/// The song has no film, the film ends before the song starts (for the film's span), or the range
/// of the song is empty.
pub(crate) fn film_plan(doc: &RackDoc, whole_song: bool) -> Result<FilmPlan, String> {
    let clip = doc
        .video
        .as_ref()
        .ok_or_else(|| "The song has no video".to_owned())?;
    let rate = u128::from(doc.sample_rate);
    let end = clip.end_us();
    let request = if whole_song {
        ExportRequest {
            span: ExportSpan::Song,
            tail_seconds: 0.0,
        }
    } else {
        if end <= 0 {
            return Err(
                "The film ends before the song starts: no part of the song plays under it"
                    .to_owned(),
            );
        }
        let from = if clip.start_us <= 0 {
            0
        } else {
            atomik_model::tick_at_us(&doc.song, clip.start_us)
        };
        ExportRequest {
            span: ExportSpan::Range(from, atomik_model::tick_at_us(&doc.song, end)),
            tail_seconds: 0.0,
        }
    };
    let (from, to) = span_frames(doc, request.span).map_err(|e| e.to_string())?;
    let us = |frames: u64| ((u128::from(frames) * 1_000_000 + rate / 2) / rate) as u64;
    let facts = &clip.facts;
    let frame = atomik_video::film::frame_us(Rational {
        num: facts.fps_num.max(1),
        den: facts.fps_den.max(1),
    });
    let cut = FilmCut::layout(
        (clip.start_us, clip.in_us, clip.out_us),
        (i64::try_from(us(from)).unwrap_or(i64::MAX), us(to - from)),
        whole_song,
        frame,
    );
    Ok(FilmPlan { request, cut })
}

/// The picture choice of the panel as FFmpeg's planner reads it.
fn picture_choice(values: &FilmValues) -> PictureChoice {
    let quality = match values.quality {
        FilmQualityChoice::High => FilmQuality::High,
        FilmQualityChoice::Standard => FilmQuality::Standard,
        FilmQualityChoice::Small => FilmQuality::Small,
    };
    match values.picture {
        FilmPictureChoice::Auto => PictureChoice::Auto(quality),
        FilmPictureChoice::Copy => PictureChoice::Copy,
        FilmPictureChoice::Encode => PictureChoice::Encode(quality),
    }
}

/// The sound of the panel as the film export reads it.
fn film_sound(values: &FilmValues) -> FilmSound {
    match values.sound {
        FilmSoundChoice::Aac => FilmSound::Aac(u32::from(values.aac)),
        FilmSoundChoice::Pcm => FilmSound::Pcm,
    }
}

/// The toast of a finished film export: "Exported film.mp4: 0:12.0, picture copied, AAC 192 kbps;
/// -14.2 LUFS, -0.8 dBTP, range 6.3 LU".
pub(crate) fn describe_film(
    report: &ExportReport,
    picture: FilmPicture,
    run: Option<&FilmRun>,
) -> String {
    let file = report
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let seconds = run.map_or_else(
        || report.frames as f64 / f64::from(report.sample_rate),
        |r| r.cut.total_us as f64 / 1e6,
    );
    let picture = match picture {
        FilmPicture::Copy if run.is_some_and(|r| r.cut.in_us > 0) => {
            "picture copied, the start snapped to a keyframe".to_owned()
        }
        FilmPicture::Copy => "picture copied".to_owned(),
        FilmPicture::Encode(q) => format!("picture re-encoded to H.264, {}", q.name()),
    };
    let sound = match run.map(|r| r.values.sound) {
        Some(FilmSoundChoice::Pcm) => "24-bit PCM".to_owned(),
        Some(FilmSoundChoice::Aac) => format!("AAC {} kbps", run.map_or(0, |r| r.values.aac)),
        None => "AAC".to_owned(),
    };
    let own = if run.is_some_and(|r| r.values.own_audio) {
        ", film audio as track 2"
    } else {
        ""
    };
    let warnings = match report.warnings.len() {
        0 => String::new(),
        n => format!(" ({n} warning{})", if n == 1 { "" } else { "s" }),
    };
    format!(
        "Exported {file}: {}, {picture}, {sound}{own}{warnings}; {}",
        clock(seconds),
        report.loudness.summary()
    )
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// Why the film cannot be exported now, for the message of the menu item: another export
    /// runs, the song has no film, the film's file is missing, or FFmpeg cannot be used. `None`
    /// when it can (FFmpeg that is still being looked for does not refuse).
    pub fn film_export_refusal(&self) -> Option<String> {
        if self.audio_export_busy() {
            return Some("An export is running: wait for it, or cancel it".to_owned());
        }
        if self.video_clip().is_none() {
            return Some(
                "The song has no video: import one (File > Video > Import Video) to export a film"
                    .to_owned(),
            );
        }
        if self.video_missing() {
            return Some(
                "The film's file is missing: relink it (File > Video > Relink Video) to export a film"
                    .to_owned(),
            );
        }
        if self.video_tool_and_folder().is_none() {
            return Some("Exporting a film needs FFmpeg, which is not available here".to_owned());
        }
        match self.film_encoder() {
            EncoderState::Missing(why) => Some(why),
            EncoderState::Ready | EncoderState::Checking => None,
        }
    }

    /// Whether the film can be exported now (see [`Self::film_export_refusal`]).
    pub fn can_export_film(&self) -> bool {
        self.film_export_refusal().is_none()
    }

    /// The song's film has sound of its own the export can keep as a second track.
    pub fn film_has_audio(&self) -> bool {
        self.video_clip().is_some_and(|c| c.facts.has_audio)
    }

    /// Whether FFmpeg, which makes the film, can be used: found, being looked for, or missing
    /// with the line that says how to set it up (the film panel waits or says so).
    pub fn film_encoder(&self) -> EncoderState {
        match self.video_status() {
            FfmpegStatus::Found(_) => EncoderState::Ready,
            FfmpegStatus::Unchecked => EncoderState::Checking,
            FfmpegStatus::Missing { .. } => EncoderState::Missing(
                "Exporting a film needs FFmpeg: set its folder in Settings > Video or put it on your PATH."
                    .to_owned(),
            ),
            FfmpegStatus::TooOld { .. } => EncoderState::Missing(format!(
                "Exporting a film needs FFmpeg {} or later: update it, or set a folder in Settings > Video.",
                atomik_video::MIN_MAJOR
            )),
            FfmpegStatus::Broken { .. } => EncoderState::Missing(
                "The FFmpeg that was found does not run: set a working folder in Settings > Video."
                    .to_owned(),
            ),
        }
    }

    /// The file a film export writes when the dialog picked `path`: the same name with the
    /// extension of the container the sound needs (MP4 for AAC, MOV for PCM).
    pub fn film_file_for(&self, path: &Path, values: &FilmValues) -> PathBuf {
        path.with_extension(values.sound.extension())
    }

    // ------------------------------------------------------------------ commands

    /// Starts the export of the song's film with the mix to `path` with the panel's `values` (see
    /// the module docs).
    ///
    /// # Errors
    /// The export is refused (see [`Self::film_export_refusal`]), nothing of the song plays under
    /// the film, or the worker cannot start; the message is also shown as the toast.
    pub fn start_film_export(&mut self, path: PathBuf, values: FilmValues) -> Result<(), String> {
        self.start_film_export_with(path, values, ExportLoudness::Off)
    }

    /// [`Self::start_film_export`], with the loudness of the mix set first (an export preset's
    /// target, with its limiter under the ceiling).
    ///
    /// # Errors
    /// As [`Self::start_film_export`].
    pub fn start_film_export_with(
        &mut self,
        path: PathBuf,
        values: FilmValues,
        loudness: ExportLoudness,
    ) -> Result<(), String> {
        let plan = match self.film_export_refusal() {
            Some(why) => Err(why),
            None if values.own_audio && !self.film_has_audio() => {
                Err("The film has no sound of its own to keep".to_owned())
            }
            None => film_plan(self.doc(), values.cover == FilmCover::Song),
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(why) => {
                self.reject(why.clone());
                return Err(why);
            }
        };
        let (Some((tool, folder)), Some(source)) =
            (self.video_tool_and_folder(), self.video_file())
        else {
            let why = "Exporting a film needs FFmpeg, which is not available here".to_owned();
            self.reject(why.clone());
            return Err(why);
        };
        let path = self.film_file_for(&path, &values);
        let file = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let sound = film_sound(&values);
        let rate = self.doc().sample_rate;
        let info = &self.doc().info;
        let film = export_worker::Film {
            tool,
            folder,
            source,
            cut: plan.cut,
            picture: picture_choice(&values),
            sound,
            own_audio: values.own_audio,
            tags: Tags {
                title: info.title.clone(),
                author: info.author.clone(),
            },
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU32::new(0.0f32.to_bits()));
        let job = self.audio.export.next_job;
        self.audio.export.next_job += 1;
        let work = Work {
            doc: self.doc().clone(),
            request: plan.request,
            // The mix goes to FFmpeg as a float WAV file at the rate the sound codec holds.
            format: ExportFormat {
                container: ExportContainer::Wav,
                bit_depth: BitDepth::Float32,
                dither: atomik_dsp::DitherMode::None,
                sample_rate: Some(match sound {
                    FilmSound::Aac(_) => LossyCodec::Aac.output_rate(rate),
                    FilmSound::Pcm => rate,
                }),
            },
            target: Target::File(path),
            bundle_root: self.bundle_root().map(Path::to_path_buf),
            plugins: PluginEnv {
                cache: self.plugins.cache.clone(),
                settings: self.plugins.settings.clone(),
                isolation: self.plugins.freeze_isolation.clone(),
            },
            lossy: None,
            film: Some(film),
            finish: finish_for(false, loudness),
            also: None,
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
                    scope: ExportScope::Song,
                    file,
                    values: ExportValues::default(),
                    film: Some(FilmRun {
                        values,
                        cut: plan.cut,
                    }),
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
}
