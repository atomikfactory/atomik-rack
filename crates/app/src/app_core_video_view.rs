//! The Video window in the app core: the picture of the song's film at the song position.
//!
//! The window ([`atomik_ui::video_view`]) paints what the core hands it once per frame from
//! [`AppCore::frame`]; the core works out *which* picture:
//!
//! * **Position to frame.** The song position (the ticks the UI already reads: the transport's
//!   while it plays, the locate, scrub or ruler click while it is stopped) goes through the
//!   tempo map to microseconds, and [`atomik_video::timeline::locate`] turns that, the film's
//!   start in the song, its trim and the proxy's rate into a frame, exactly (integer arithmetic,
//!   any rational rate). Before the film or after it the picture is black and the overlay keeps
//!   counting. A loop needs nothing special: the position jumps back and so does the frame;
//!   near the loop's end the first frames of the loop are decoded ahead.
//! * **Decoding.** A [`FrameDecoder`] (one worker thread, see `atomik_video::frames`) opens the
//!   proxy of the film and decodes what the core asks: the frame shown first, then the likeliest
//!   next ones ([`atomik_video::timeline::wanted`]: ahead while playing, around while stopped).
//!   A new request replaces the old one, so a slow decode drops frames instead of lagging; the
//!   picture shown meanwhile is the last one that was ready. Nothing is read or decoded on the
//!   UI thread or the audio thread; the window only holds the finished pixels.
//! * **Not ready.** The window says why instead of a picture: the song has no film, its file is
//!   missing, FFmpeg is missing, or the proxy is being built (with its progress). The proxy of
//!   the song's film is also built, quietly, when a project with a film is opened and when the
//!   window is opened, once per film (a cancelled or failed build is not started again until the
//!   user prepares the film or opens the project again).
//!
//! The picture, the cache and the worker exist only while the window is open.

use super::*;
use atomik_model::audio_trim::TempoMap;
use atomik_ui::video_view::{VideoFrame, VideoImage, VideoPrefs, VideoScreen, VideoView};
use atomik_video::timeline::{locate, wanted};
use atomik_video::{
    FfmpegStatus, FrameCache, FrameDecoder, FrameEvent, Picture, Prepared, Rational, Shot, Timecode,
};
use std::collections::BTreeSet;

/// What the window remembers between frames.
#[derive(Default)]
pub(super) struct ViewerState {
    /// The window is open.
    shown: bool,
    /// The worker, with the proxy it was started on.
    decoder: Option<(PathBuf, FrameDecoder)>,
    cache: FrameCache,
    /// Frames the worker could not decode (not asked for again).
    bad: BTreeSet<u32>,
    /// The proxy could not be opened: the worker's message.
    failure: Option<String>,
    /// The frames asked of the worker last (so the same list is not sent every frame).
    asked: Vec<u32>,
    /// The picture last handed to the window: its frame, and the image with its serial.
    shown_image: Option<(u32, VideoImage)>,
    next_serial: u64,
    /// The film a proxy build was started for by the core (not by the user): once per film.
    tried: Option<PathBuf>,
}

impl ViewerState {
    /// Forgets the worker and every picture (the window closed, or the film changed).
    fn reset(&mut self) {
        self.decoder = None;
        self.cache.clear();
        self.bad.clear();
        self.failure = None;
        self.asked.clear();
        self.shown_image = None;
    }
}

/// The frames and rate a shot is worked out with.
struct Proxy {
    path: PathBuf,
    rate: Rational,
    frames: u32,
}

impl Proxy {
    fn of(p: &Prepared) -> Self {
        Self {
            path: p.proxy.clone(),
            rate: p.info.rate,
            frames: p.info.frames,
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ host API

    /// The Video window's view.
    pub fn video_window(&self) -> &VideoView {
        &self.video_win
    }

    /// The Video window's view, for the shell.
    pub fn video_window_mut(&mut self) -> &mut VideoView {
        &mut self.video_win
    }

    /// Whether the window is open.
    pub fn video_shown(&self) -> bool {
        self.viewer.shown
    }

    /// The window opened or closed (the shell). Opening builds the proxy of the song's film when
    /// it has none yet; closing frees the worker and the pictures.
    pub fn set_video_shown(&mut self, shown: bool) {
        if self.viewer.shown == shown {
            return;
        }
        self.viewer.shown = shown;
        if shown {
            self.warm_video_proxy();
            self.feed_video_view();
        } else {
            self.viewer.reset();
        }
    }

    /// The window's saved choices (size, overlay), set by the shell at start.
    pub fn set_video_prefs(&mut self, prefs: VideoPrefs) {
        self.video_win.set_prefs(prefs);
    }

    /// The command: switches the overlay (timecode and frame number) off or back on.
    pub fn toggle_video_overlay(&mut self) {
        self.video_win.toggle_overlay();
    }

    /// The frames held decoded, in order (tests).
    #[cfg(test)]
    pub(super) fn viewer_cached(&self) -> Vec<u32> {
        self.viewer.cache.held()
    }

    // ------------------------------------------------------------------ proxy

    /// Builds the proxy of the song's film when nothing has: a quiet job (no toast: the window
    /// shows the progress and says what is wrong). Once per film.
    pub(super) fn warm_video_proxy(&mut self) {
        let Some(film) = self.video_file() else {
            return;
        };
        if self.video_missing()
            || self.video_prepared().is_some()
            || self.viewer.tried.as_ref() == Some(&film)
            || self.video.running.is_some()
        {
            return;
        }
        self.viewer.tried = Some(film.clone());
        let _ = self.start_film_job(film, video::Purpose::Open);
    }

    /// The project was replaced or another film imported: the next film is tried afresh.
    pub(super) fn forget_viewer_film(&mut self) {
        self.viewer.tried = None;
        self.viewer.reset();
    }

    /// Per-frame: starts the proxy of a freshly opened project's film once the platform has given
    /// the core its tools and cache folder (a headless core never starts a program).
    pub(super) fn video_frame(&mut self) {
        if self.video.warm && self.video.ready() && self.video.running.is_none() {
            self.video.warm = false;
            self.warm_video_proxy();
        }
        if self.viewer.shown {
            self.feed_video_view();
        }
    }

    // ------------------------------------------------------------------ the picture

    /// The song position in microseconds through the tempo map.
    fn position_us(&self) -> i64 {
        let seconds = TempoMap::new(&self.doc().song.tempo).seconds_at_tick(self.position as f64);
        (seconds * 1e6).round() as i64
    }

    /// The song's loop as the frames of its end and start, while it is on and both lie in the
    /// film.
    fn loop_frames(&self, clip: &atomik_model::VideoClip, p: &Proxy) -> Option<(u32, u32)> {
        let song = &self.doc().song;
        if !song.loop_enabled {
            return None;
        }
        let map = TempoMap::new(&song.tempo);
        let frame_at = |tick: i64| {
            let us = (map.seconds_at_tick(tick as f64) * 1e6).round() as i64;
            locate(us, clip.start_us, clip.in_us, clip.out_us, p.rate, p.frames).frame()
        };
        Some((frame_at(song.loop_range.1)?, frame_at(song.loop_range.0)?))
    }

    /// Hands the window what to show now and asks the worker for the frames it will need.
    pub(super) fn feed_video_view(&mut self) {
        let frame = self.video_picture();
        self.video_win.set_frame(frame);
    }

    fn video_picture(&mut self) -> VideoFrame {
        let message = |text: String| VideoFrame {
            screen: VideoScreen::Message(text),
            ..VideoFrame::default()
        };
        let Some(clip) = self.video_clip().cloned() else {
            self.viewer.reset();
            return message(
                "The song has no video\nAdd one with File > Video > Import Video".to_owned(),
            );
        };
        let film_size = (clip.facts.width.max(1), clip.facts.height.max(1));
        let with_film = |mut f: VideoFrame| {
            f.film = film_size;
            f
        };
        if self.video_missing() {
            self.viewer.reset();
            return with_film(message(format!(
                "The video {} is missing\nRelink it with File > Video > Relink Video",
                clip.name()
            )));
        }
        let Some(prepared) = self.video_prepared().map(Proxy::of) else {
            self.viewer.reset();
            return with_film(self.video_not_ready(&clip));
        };
        self.open_viewer_worker(&prepared);
        self.take_viewer_events();
        if let Some(why) = self.viewer.failure.clone() {
            return with_film(message(format!("The preview cannot be shown\n{why}")));
        }
        let shot = locate(
            self.position_us(),
            clip.start_us,
            clip.in_us,
            clip.out_us,
            prepared.rate,
            prepared.frames,
        );
        let rate = prepared.rate;
        let timecode = clip
            .facts
            .timecode
            .as_deref()
            .and_then(|t| Timecode::parse(t, rate))
            .unwrap_or_else(|| Timecode::zero(rate));
        let mut out = VideoFrame {
            screen: VideoScreen::Black,
            film: film_size,
            timecode: timecode.text(shot.number()),
            frame_number: Some(shot.number()),
        };
        if let Shot::Frame { n, .. } = shot {
            self.ask_for_frames(n, self.loop_frames(&clip, &prepared), prepared.frames);
            out.screen = self.picture_of(n);
        }
        out
    }

    /// What the window shows in place of a picture while the proxy is not there.
    fn video_not_ready(&self, clip: &atomik_model::VideoClip) -> VideoFrame {
        let screen = if let Some((fraction, text)) = self
            .video
            .running
            .as_ref()
            .filter(|r| r.source.is_some() && r.source == self.video_file())
            .and_then(|_| self.video_progress())
        {
            VideoScreen::Progress(fraction, text)
        } else {
            match self.video_status().clone() {
                FfmpegStatus::Found(_) | FfmpegStatus::Unchecked
                    if self.video.running.is_some() =>
                {
                    VideoScreen::Message(format!("Preparing the preview of {}...", clip.name()))
                }
                FfmpegStatus::Found(_) | FfmpegStatus::Unchecked => VideoScreen::Message(
                    "The preview is not built\nUse File > Video > Prepare Video".to_owned(),
                ),
                other => VideoScreen::Message(other.message().replace(". ", ".\n")),
            }
        };
        VideoFrame {
            screen,
            ..VideoFrame::default()
        }
    }

    /// Starts the worker on `proxy` unless it already runs on it.
    fn open_viewer_worker(&mut self, proxy: &Proxy) {
        if self
            .viewer
            .decoder
            .as_ref()
            .is_some_and(|(p, _)| *p == proxy.path)
        {
            return;
        }
        self.viewer.reset();
        match FrameDecoder::spawn_file(proxy.path.clone()) {
            Ok(decoder) => self.viewer.decoder = Some((proxy.path.clone(), decoder)),
            Err(e) => self.viewer.failure = Some(format!("the decoder cannot start: {e}")),
        }
    }

    /// Takes what the worker finished: pictures into the cache, failures noted.
    fn take_viewer_events(&mut self) {
        let Some((_, decoder)) = &self.viewer.decoder else {
            return;
        };
        for event in decoder.poll() {
            match event {
                FrameEvent::Opened(_) => {}
                FrameEvent::Frame(n, picture) => self.viewer.cache.insert(n, picture),
                FrameEvent::Failed(None, why) => {
                    log::warn!("video proxy cannot be opened: {why}");
                    self.viewer.failure = Some(why);
                }
                FrameEvent::Failed(Some(n), why) => {
                    log::warn!("video frame {n} cannot be decoded: {why}");
                    self.viewer.bad.insert(n);
                }
            }
        }
    }

    /// Asks the worker for the frames around `current` that are not decoded yet (the same list is
    /// not sent twice).
    fn ask_for_frames(&mut self, current: u32, wrap: Option<(u32, u32)>, frames: u32) {
        let playing = self.playing;
        let list: Vec<u32> = wanted(current, frames, playing, wrap)
            .into_iter()
            .filter(|n| !self.viewer.cache.contains(*n) && !self.viewer.bad.contains(n))
            .collect();
        if list == self.viewer.asked {
            return;
        }
        if let Some((_, decoder)) = &self.viewer.decoder
            && !list.is_empty()
        {
            decoder.want(list.clone());
        }
        self.viewer.asked = list;
    }

    /// The window's picture for frame `n`: the decoded one, else the last one that was shown
    /// (frames are dropped, not waited for), else black.
    fn picture_of(&mut self, n: u32) -> VideoScreen {
        if self.viewer.bad.contains(&n) {
            return VideoScreen::Message(format!("Frame {n} of the preview cannot be decoded"));
        }
        let picture: Option<Picture> = self.viewer.cache.get(n).cloned();
        match picture {
            Some(p) => {
                let same = self
                    .viewer
                    .shown_image
                    .as_ref()
                    .is_some_and(|(k, _)| *k == n);
                if !same {
                    self.viewer.next_serial += 1;
                    let image = VideoImage {
                        serial: self.viewer.next_serial,
                        width: p.width,
                        height: p.height,
                        rgba: p.rgba,
                    };
                    self.viewer.shown_image = Some((n, image));
                }
            }
            None if self.viewer.shown_image.is_none() => return VideoScreen::Black,
            None => {}
        }
        match &self.viewer.shown_image {
            Some((_, image)) => VideoScreen::Picture(image.clone()),
            None => VideoScreen::Black,
        }
    }
}
