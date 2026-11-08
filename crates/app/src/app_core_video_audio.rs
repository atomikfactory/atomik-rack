//! The film's own sound in the song: Import Video Audio and "keep with video".
//!
//! **Import Video Audio** ([`AppCore::import_video_audio`]) runs on the video worker (see
//! `video_worker.rs`): `ffprobe` lists the film's audio tracks, the track is chosen, `ffmpeg`
//! writes it to a temporary WAV file with progress and cancel, and the song's own import takes the
//! file (decode, resample to the song's rate, store in the project's bundle with its hash and
//! peaks), so the asset is an ordinary one. Which track:
//!
//! * a film with one audio track uses it;
//! * a film with several asks: the job ends with the list ([`AppCore::take_video_audio_choice`]),
//!   the window shows it as a pop-up of one row per track (number, channels, codec, language,
//!   title) and the choice runs `video.import_audio_pick`, which starts the job again for that
//!   track;
//! * a film without audio, or a machine without FFmpeg, ends with a message that says so (and,
//!   for FFmpeg, how to set it up). Nothing is changed in the song then.
//!
//! When the file is in the bundle the song gets, **in one undo step**, a new audio track named
//! after the film, the clip on it and the film's link to the clip ([`atomik_model::AudioLink`]).
//! The clip is placed by [`atomik_model::video_audio_placement`]: its first sample sits on the
//! film's first frame, to the sample, whatever the tempo, the rate of the track, the start times
//! the container states for the picture and for the sound, the film's start and its trim.
//!
//! **Keep with video** is the link's `keep` flag (saved with the film, on after an import): while
//! it is on, moving, trimming or relinking the film moves and trims the clip in the same undo
//! step (the model does that, see [`atomik_model::VideoCommand`]). File > Video > Keep Audio with
//! Video turns it off and on; turning it on lines the clip up with the film again.

use super::video::{Purpose, Running};
use super::*;
use crate::audio_import::display_name;
use crate::video_worker::{AudioImported, AudioOutcome, Job};
use atomik_model::{AudioLink, FadeCurve, SongCommand, VideoCommand};
use atomik_video::VideoError;

/// The audio tracks of a film to choose from, for the pop-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioChoice {
    /// The film.
    pub film: PathBuf,
    /// Each track: its stream number in the file and the line the pop-up shows.
    pub tracks: Vec<(u32, String)>,
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ queries

    /// Whether the song's film has audio imported that follows it: `Some(keep)`.
    pub fn video_audio_kept(&self) -> Option<bool> {
        self.video_clip()?.audio_link.map(|l| l.keep)
    }

    /// Whether Import Video also imports the film's audio.
    pub fn import_audio_with_video(&self) -> bool {
        self.video.import_audio
    }

    /// The audio tracks to choose from, when an import found several (kept until taken).
    pub fn video_audio_choice(&self) -> Option<&AudioChoice> {
        self.video.choice.as_ref()
    }

    /// Takes the choice to show it (the window opens the pop-up).
    pub fn take_video_audio_choice(&mut self) -> Option<AudioChoice> {
        self.video.choice.take()
    }

    // ------------------------------------------------------------------ settings

    /// Sets whether Import Video also imports the film's audio.
    pub fn set_import_audio_with_video(&mut self, on: bool) {
        self.video.import_audio = on;
    }

    // ------------------------------------------------------------------ import

    /// Starts Import Video Audio for the song's film: `stream` is the stream number of the track
    /// (`None`: the only one, or the list to choose from; see the module docs).
    ///
    /// # Errors
    /// The song has no film or its file is missing, the film has no audio track (as far as the
    /// import said), a video job runs already, or the worker cannot start; the message is also
    /// the toast.
    pub fn import_video_audio(&mut self, stream: Option<u32>) -> Result<(), String> {
        let Some(clip) = self.video_clip().cloned() else {
            return self.video_refused("The song has no video".to_owned());
        };
        if self.video.running.is_some() {
            return self.video_refused("A video job is already running".to_owned());
        }
        let Some(film) = self.video_file().filter(|f| f.is_file()) else {
            return self.video_refused(format!(
                "The video {} is missing: relink it in File > Video",
                clip.name()
            ));
        };
        if !clip.facts.has_audio {
            return self.video_refused(format!("{} has no audio track to import", clip.name()));
        }
        self.video.choice = None;
        let bundle = match self.import_bundle() {
            Ok(b) => b,
            Err(e) => return self.video_refused(e),
        };
        let job = Job::Audio {
            folder: self.video_folder().map(Path::to_path_buf),
            source: film.clone(),
            stream,
            bundle: Box::new(bundle),
            rate: self.doc().sample_rate,
            threshold: self.stream_threshold(),
        };
        let name = display_name(&film);
        self.spawn_video(job, Some((name, film, Purpose::Audio)), false)
    }

    /// The audio job ended (see the module docs): offers the tracks, puts the sound in the song,
    /// or says why not.
    pub(super) fn finish_video_audio(
        &mut self,
        running: Running,
        result: Result<AudioOutcome, VideoError>,
    ) {
        let film = running.film.clone().unwrap_or_default();
        match result {
            Ok(AudioOutcome::Choose(tracks)) => {
                let tracks: Vec<(u32, String)> = tracks
                    .iter()
                    .enumerate()
                    .map(|(n, t)| (t.index, t.label(n + 1)))
                    .collect();
                self.reject(format!(
                    "{film} has {} audio tracks: choose the one to import",
                    tracks.len()
                ));
                self.video.choice = Some(AudioChoice {
                    film: running.source.unwrap_or_default(),
                    tracks,
                });
            }
            Ok(AudioOutcome::Imported(got)) => self.place_video_audio(&running, *got),
            Err(VideoError::Cancelled) => {
                self.reject(format!("Importing the audio of {film} cancelled"));
            }
            Err(VideoError::NoAudio) => {
                self.reject(format!("{film} has no audio track to import"));
            }
            Err(VideoError::NoSuchAudio(n)) => {
                self.reject(format!("{film} has no audio track {n}"));
            }
            Err(VideoError::NotUsable(message)) => self.reject(message),
            Err(e) => self.reject(format!("Importing the audio of {film} failed: {e}")),
        }
    }

    /// Puts the imported track in the song: a track named after the film, the clip aligned with
    /// the film's first frame and the link, as one undo step.
    fn place_video_audio(&mut self, running: &Running, got: AudioImported) {
        let film = running.film.clone().unwrap_or_default();
        if running.generation != self.audio.generation {
            return self.reject(format!(
                "The project changed: the audio of {film} was not put in it"
            ));
        }
        let (Some(clip), Some(info)) = (self.video_clip().cloned(), got.loaded.info.clone()) else {
            return self.reject(format!(
                "The video was removed from the song: the audio of {film} was not imported"
            ));
        };
        if self.video_file() != running.source {
            return self.reject(format!(
                "The song's video changed: the audio of {film} was not imported"
            ));
        }
        self.insert_loaded(&got.loaded);
        let rate = self.doc().sample_rate;
        if let Err(e) = self.apply_command(Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: atomik_model::AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        }) {
            return self.reject(format!("Cannot add the audio of {film}: {e}"));
        }
        let Some(at) = atomik_model::video_audio_placement(
            &self.doc().song,
            rate,
            &clip,
            got.lead_us,
            info.frames,
        ) else {
            return self.reject(format!(
                "No part of the audio of {film} is inside the part of the video that plays"
            ));
        };
        let stem = Path::new(clip.name())
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.trim().is_empty());
        let placed = self.add_video_audio_clip(stem, &info.hash, at, got.lead_us);
        match placed {
            Ok(()) => self.reject(format!(
                "Imported the audio of {film} ({}) on a new track, lined up with the first frame",
                got.stream.describe()
            )),
            Err(e) => self.reject(format!("Cannot add the audio of {film}: {e}")),
        }
    }

    /// Adds the track, the clip and the link (one undo step).
    fn add_video_audio_clip(
        &mut self,
        name: Option<String>,
        asset: &str,
        at: atomik_model::AudioPlacement,
        lead_us: i64,
    ) -> Result<(), String> {
        let run = |core: &mut Self, command: Command, joined: bool| {
            core.join_next = joined;
            let done = core.apply_command(command).map_err(|e| e.to_string());
            core.join_next = false;
            done
        };
        run(
            self,
            Command::Song(SongCommand::AddAudioTrack { name }),
            false,
        )?;
        let device = self
            .doc()
            .song
            .tracks
            .last()
            .map(|t| t.device.clone())
            .ok_or("the audio track was not created")?;
        run(
            self,
            Command::Song(SongCommand::AddAudioClip {
                device,
                start: at.start,
                length: at.length,
                asset: asset.to_owned(),
                source_offset: at.source_offset,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: FadeCurve::Linear,
            }),
            true,
        )?;
        let clip = self
            .document
            .last_clip_id()
            .ok_or("the audio clip was not created")?;
        run(
            self,
            Command::Video(VideoCommand::LinkAudio(Some(AudioLink {
                clip,
                lead_us,
                keep: true,
            }))),
            true,
        )
    }

    // ------------------------------------------------------------------ keep with video

    /// File > Video > Keep Audio with Video: the clip of the film's sound follows the film (and is
    /// lined up with it now) or stays where it is. One undo step.
    pub fn toggle_keep_video_audio(&mut self) {
        let Some(keep) = self.video_audio_kept() else {
            return self.reject("The song has no imported video audio".to_owned());
        };
        match self.apply_command(Command::Video(VideoCommand::KeepAudio(!keep))) {
            Ok(()) if keep => self.reject("The audio no longer follows the video".to_owned()),
            Ok(()) => self.reject("The audio follows the video again".to_owned()),
            Err(e) => self.reject(format!("Video: {e}")),
        }
    }
}
