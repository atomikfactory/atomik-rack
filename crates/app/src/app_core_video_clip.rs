//! The film of the song in the app core: Import Video, Relink Video, Remove, Move and Trim.
//!
//! The model keeps one linked film per song ([`atomik_model::VideoClip`]: the path, the probe
//! facts, a start in microseconds and a trim). This file is the app's side of it:
//!
//! * **Import Video** ([`AppCore::import_video`]) is a prepare job that reports its probe first
//!   (see `app_core_video.rs`): when the probe is in, the film is in the song at the song's start
//!   (one undo step, replacing the film the song had) and the proxy builds on in the background.
//!   Without FFmpeg nothing can be probed: the toast says how to set it up.
//! * **Relink Video** ([`AppCore::relink_video`]) probes another file the same way and points
//!   the song's film at it (one undo step, start and trim kept). When the file's length or frame
//!   rate differs from what the project knows, the toast says so: the sync may be off.
//! * **Remove**, **Move to Playhead**, **Trim Start to Playhead** and **Trim End to Playhead**
//!   are plain commands with undo; the playhead is read through the tempo map into the film's
//!   own time, so the film stays where it was set when the tempo changes.
//!
//! A song whose film file is gone opens as usual; [`AppCore::video_missing`] says so (the menu
//! and the toast at open) and Relink Video points it at the file again.

use super::video::Purpose;
use super::*;
use atomik_model::audio_trim::TempoMap;
use atomik_model::{VideoClip, VideoCommand, VideoFacts};
use atomik_video::{Prepared, ProbeInfo};

/// What the probe found, as the facts a project keeps.
pub(crate) fn facts_of(probe: &ProbeInfo) -> VideoFacts {
    let (width, height) = probe.video.display_size();
    VideoFacts {
        duration_us: (probe.duration_s.max(0.0) * 1e6).round() as u64,
        fps_num: probe.video.frame_rate.num,
        fps_den: probe.video.frame_rate.den,
        width,
        height,
        timecode: probe.timecode.clone(),
        has_audio: !probe.audio.is_empty(),
    }
}

/// `m:ss.mmm` (or `h:mm:ss.mmm`) of a time in microseconds, for messages.
pub(crate) fn time_text(us: i64) -> String {
    let sign = if us < 0 { "-" } else { "" };
    let ms = us.unsigned_abs() / 1_000;
    let (h, m, s, f) = (
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1_000 % 60,
        ms % 1_000,
    );
    if h > 0 {
        format!("{sign}{h}:{m:02}:{s:02}.{f:03}")
    } else {
        format!("{sign}{m}:{s:02}.{f:03}")
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// The film of the song.
    pub fn video_clip(&self) -> Option<&VideoClip> {
        self.doc().video.as_ref()
    }

    /// The file of the song's film: the saved path, or, for a relative one, the path from the
    /// project's folder.
    pub fn video_file(&self) -> Option<PathBuf> {
        let clip = self.video_clip()?;
        let path = PathBuf::from(&clip.path);
        if path.is_relative()
            && let Some(dir) = self.file.as_deref().and_then(Path::parent)
        {
            return Some(dir.join(path));
        }
        Some(path)
    }

    /// The song has a film and its file is not there. The answer is kept per path, so it is
    /// cheap to ask on every menu refresh; Import, Relink and opening a project look again.
    pub fn video_missing(&self) -> bool {
        let Some(file) = self.video_file() else {
            return false;
        };
        let mut known = self.video.file.borrow_mut();
        match &*known {
            Some((path, missing)) if *path == file => *missing,
            _ => {
                let missing = !file.is_file();
                *known = Some((file, missing));
                missing
            }
        }
    }

    /// What this session prepared for the song's film (its proxy), when it did.
    pub fn video_prepared(&self) -> Option<&Prepared> {
        self.prepared_video(&self.video_file()?)
    }

    // ------------------------------------------------------------------ import and relink

    /// Starts Import Video for `film`: probes it, puts it in the song at its start and builds the
    /// proxy (see the module docs).
    ///
    /// # Errors
    /// A video job runs already or `film` is not a file; the message is also the toast.
    pub fn import_video(&mut self, film: PathBuf) -> Result<(), String> {
        self.forget_video_file();
        self.forget_viewer_film();
        self.start_film_job(film, Purpose::Import)
    }

    /// Starts Relink Video for `film`: probes it and points the song's film at it.
    ///
    /// # Errors
    /// The song has no film, a video job runs already or `film` is not a file; the message is
    /// also the toast.
    pub fn relink_video(&mut self, film: PathBuf) -> Result<(), String> {
        if self.video_clip().is_none() {
            return self.video_refused("The song has no video".to_owned());
        }
        self.forget_video_file();
        self.start_film_job(film, Purpose::Relink)
    }

    /// Forgets whether the film's file was there (it is looked at again).
    pub(super) fn forget_video_file(&self) {
        *self.video.file.borrow_mut() = None;
    }

    /// The probe of an import or a relink is in: the song takes the film (see the module docs).
    pub(super) fn apply_video_probed(&mut self, job: u64, probe: ProbeInfo) {
        let Some(running) = self.video.running.as_ref().filter(|r| r.job == job) else {
            return;
        };
        let (purpose, generation) = (running.purpose, running.generation);
        let (Some(source), Some(film)) = (running.source.clone(), running.film.clone()) else {
            return;
        };
        if !matches!(purpose, Purpose::Import | Purpose::Relink) {
            return;
        }
        if generation != self.audio.generation {
            self.reject(format!("The project changed: {film} was not put in it"));
            return;
        }
        let facts = facts_of(&probe);
        let path = source.to_string_lossy().into_owned();
        let note = match purpose {
            Purpose::Relink => self.video_clip().map(|c| c.facts.differences(&facts)),
            _ => None,
        };
        let command = match purpose {
            Purpose::Relink => VideoCommand::Relink {
                path,
                facts: facts.clone(),
            },
            _ => VideoCommand::Set(VideoClip::new(path, facts.clone())),
        };
        if let Err(e) = self.apply_command(Command::Video(command)) {
            self.reject(format!("Cannot use {film}: {e}"));
            return;
        }
        self.forget_video_file();
        let length = time_text(i64::try_from(facts.duration_us).unwrap_or(i64::MAX));
        let message = match (purpose, note) {
            (Purpose::Relink, Some(differences)) if !differences.is_empty() => {
                self.video.warned = true;
                format!(
                    "Relinked the video to {film}, but {}: the picture may not match the song",
                    differences.join(" and ")
                )
            }
            (Purpose::Relink, _) => format!("Relinked the video to {film}"),
            _ => format!(
                "Video {film} added at the start of the song ({length}); building its preview"
            ),
        };
        self.reject(message);
    }

    // ------------------------------------------------------------------ edits

    /// Takes the film out of the song (the file is untouched). One undo step.
    pub fn remove_video(&mut self) {
        let Some(name) = self.video_clip().map(|c| c.name().to_owned()) else {
            return self.reject("The song has no video".to_owned());
        };
        if self.run_video_command(VideoCommand::Remove) {
            self.reject(format!("Video {name} removed from the song"));
        }
    }

    /// Starts the film at the playhead. One undo step.
    pub fn move_video_to_playhead(&mut self) {
        if self.video_clip().is_none() {
            return self.reject("The song has no video".to_owned());
        }
        let at = self.playhead_us();
        if self.run_video_command(VideoCommand::Move {
            start_us: at,
            merge: None,
        }) {
            self.reject(format!("The video starts at {}", time_text(at)));
        }
    }

    /// Cuts the film's beginning up to the playhead: the picture that stays keeps its place.
    /// One undo step.
    pub fn trim_video_start_to_playhead(&mut self) {
        let Some((clip, at)) = self.video_clip().cloned().zip(Some(self.playhead_us())) else {
            return self.reject("The song has no video".to_owned());
        };
        let Some(source) = clip.source_at(at) else {
            return self.reject("The playhead is outside the video".to_owned());
        };
        if self.run_video_command(VideoCommand::Trim {
            in_us: source,
            out_us: clip.out_us,
            merge: None,
        }) {
            self.reject(format!("The video now starts at {}", time_text(at)));
        }
    }

    /// Cuts the film's end from the playhead on. One undo step.
    pub fn trim_video_end_to_playhead(&mut self) {
        let Some((clip, at)) = self.video_clip().cloned().zip(Some(self.playhead_us())) else {
            return self.reject("The song has no video".to_owned());
        };
        let Some(source) = clip.source_at(at) else {
            return self.reject("The playhead is outside the video".to_owned());
        };
        if self.run_video_command(VideoCommand::Trim {
            in_us: clip.in_us,
            out_us: source,
            merge: None,
        }) {
            self.reject(format!("The video now ends at {}", time_text(at)));
        }
    }

    /// Runs a video command; a refusal is the toast. True when it was done.
    fn run_video_command(&mut self, command: VideoCommand) -> bool {
        match self.apply_command(Command::Video(command)) {
            Ok(()) => true,
            Err(e) => {
                self.reject(format!("Video: {e}"));
                false
            }
        }
    }

    /// The song position in microseconds, through the tempo map: the film's own time.
    pub(super) fn playhead_us(&self) -> i64 {
        let seconds = TempoMap::new(&self.doc().song.tempo).seconds_at_tick(self.position as f64);
        (seconds * 1e6).round() as i64
    }

    // ------------------------------------------------------------------ open

    /// After a project was opened: looks at the film's file again and says when it is missing.
    pub(super) fn video_opened(&mut self) {
        self.forget_video_file();
        self.forget_viewer_film();
        if !self.video_missing() {
            // The picture of a film that is there is prepared in the background, quietly.
            self.video.warm = self.video_clip().is_some();
            return;
        }
        let Some(name) = self.video_clip().map(|c| c.name().to_owned()) else {
            return;
        };
        let sentence = format!("The video {name} is missing: relink it in File > Video");
        let message = match self.rejection.clone() {
            // The missing audio is already on screen: the video joins it.
            Some(audio) if !self.audio.missing.is_empty() => format!("{audio}. {sentence}"),
            _ => sentence,
        };
        self.reject(message);
    }
}
