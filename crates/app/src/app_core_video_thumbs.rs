//! The video row of the arrangement in the app core: what the row needs from outside its view.
//!
//! The row ([`atomik_ui::ArrangementView`]) paints a clip bar for the song's film and decides
//! which frames its thumbnail slots show; the core supplies the rest, once per frame while the
//! song has a film and the row is open:
//!
//! * **State.** Whether the file is missing, how many frames the preview has (none: the row says
//!   why instead, from the same progress and FFmpeg messages the Video window uses) and the
//!   timecodes the row shows: at the playhead, of the first frame and just after the last one of
//!   the trimmed film, counted from the film's own start timecode.
//! * **Pictures.** The frames the row asks for ([`atomik_ui::ArrangementView::video_wanted`]:
//!   only the slots in view, the middle first) go to a [`FrameDecoder`] of their own (its own
//!   thread and its own handle on the preview, so the thumbnails never queue behind the Video
//!   window's frames and the other way round) that scales each picture down to thumbnail size on
//!   its thread. Finished pictures go into the row, which keeps them by frame number under a
//!   byte budget. The same request is not sent twice; a frame that cannot be decoded is not asked
//!   for again. A collapsed or hidden row, a missing film or no preview closes the worker.
//!
//! Nothing is read, decoded or scaled on the UI thread or the audio thread.

use super::*;
use atomik_ui::arrangement::{VIDEO_THUMB_HEIGHT, VideoFeed};
use atomik_video::timeline::{frame_number, locate};
use atomik_video::{FfmpegStatus, FrameDecoder, FrameEvent, Rational, Timecode};
use std::collections::BTreeSet;

/// What the row keeps between frames.
#[derive(Default)]
pub(super) struct ThumbState {
    /// The worker, with the preview it reads.
    worker: Option<(PathBuf, FrameDecoder)>,
    /// Frames that could not be decoded (not asked for again).
    bad: BTreeSet<u32>,
    /// The frames asked of the worker last.
    asked: Vec<u32>,
    /// The preview could not be opened: the worker's message.
    failure: Option<String>,
}

impl ThumbState {
    fn reset(&mut self) {
        self.worker = None;
        self.bad.clear();
        self.asked.clear();
        self.failure = None;
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ host API

    /// Whether a thumbnail worker runs (tests).
    #[cfg(test)]
    pub(super) fn thumbs_running(&self) -> bool {
        self.thumbs.worker.is_some()
    }

    // ------------------------------------------------------------------ per frame

    /// Per-frame: tells the row what it needs and keeps its pictures coming.
    pub(super) fn video_row_frame(&mut self) {
        let Some(clip) = self.video_clip().cloned() else {
            self.reset_thumbs();
            return;
        };
        if !self.arrangement.video_row_visible() {
            self.reset_thumbs();
            return;
        }
        let missing = self.video_missing();
        let prepared = if missing {
            None
        } else {
            self.video_prepared()
                .map(|p| (p.proxy.clone(), p.info.frames))
        };
        let mut feed = self.row_timecodes(&clip, prepared.as_ref().map(|(_, n)| *n));
        feed.missing = missing;
        feed.preview_frames = prepared.as_ref().map(|(_, n)| *n);
        if prepared.is_none() && !missing {
            feed.note = self.row_note();
        }
        let (Some((proxy, _)), false) = (prepared, self.arrangement.video_row_collapsed()) else {
            self.reset_thumbs();
            self.arrangement.set_video_feed(feed);
            return;
        };
        if self.thumbs.worker.as_ref().is_none_or(|(p, _)| *p != proxy) {
            self.reset_thumbs();
            match FrameDecoder::spawn_file_scaled(proxy.clone(), VIDEO_THUMB_HEIGHT) {
                Ok(d) => self.thumbs.worker = Some((proxy, d)),
                Err(e) => self.thumbs.failure = Some(format!("the decoder cannot start: {e}")),
            }
        }
        self.take_thumbs();
        if let Some(why) = &self.thumbs.failure {
            feed.preview_frames = None;
            feed.note = format!("Preview cannot be read: {why}");
        }
        self.arrangement.set_video_feed(feed);
        let list: Vec<u32> = self
            .arrangement
            .video_wanted()
            .into_iter()
            .filter(|n| !self.thumbs.bad.contains(n))
            .collect();
        if list != self.thumbs.asked {
            if let Some((_, worker)) = &self.thumbs.worker
                && !list.is_empty()
            {
                worker.want(list.clone());
            }
            self.thumbs.asked = list;
        }
    }

    /// Closes the worker and forgets the pictures (no film, the row closed, another preview).
    fn reset_thumbs(&mut self) {
        if self.thumbs.worker.is_some() || self.arrangement.video_thumbs().0 > 0 {
            self.arrangement.clear_video_thumbs();
        }
        self.thumbs.reset();
    }

    /// Takes what the worker finished: the pictures into the row, failures noted.
    fn take_thumbs(&mut self) {
        let Some((_, worker)) = &self.thumbs.worker else {
            return;
        };
        for event in worker.poll() {
            match event {
                FrameEvent::Opened(_) => {}
                FrameEvent::Frame(n, p) => {
                    self.arrangement
                        .add_video_thumb(n, p.width, p.height, p.rgba);
                }
                FrameEvent::Failed(None, why) => {
                    log::warn!("video thumbnails: the preview cannot be opened: {why}");
                    self.thumbs.failure = Some(why);
                }
                FrameEvent::Failed(Some(n), why) => {
                    log::warn!("video thumbnail {n} cannot be decoded: {why}");
                    self.thumbs.bad.insert(n);
                }
            }
        }
    }

    /// The line on the bar while there is no preview: the progress of the build, or why there is
    /// none.
    fn row_note(&self) -> String {
        if let Some((fraction, _)) = self
            .video
            .running
            .as_ref()
            .filter(|r| r.source.is_some() && r.source == self.video_file())
            .and_then(|_| self.video_progress())
        {
            return format!("Preparing the preview {:.0}%", fraction * 100.0);
        }
        match self.video_status().clone() {
            FfmpegStatus::Found(_) | FfmpegStatus::Unchecked if self.video.running.is_some() => {
                "Preparing the preview".to_owned()
            }
            FfmpegStatus::Found(_) | FfmpegStatus::Unchecked => {
                "No preview (File > Video > Prepare Video)".to_owned()
            }
            other => other.message().to_owned(),
        }
    }

    /// The three timecodes of the row (the rest of the feed is left at its defaults).
    fn row_timecodes(&self, clip: &atomik_model::VideoClip, frames: Option<u32>) -> VideoFeed {
        let rate = Rational {
            num: clip.facts.fps_num.max(1),
            den: clip.facts.fps_den.max(1),
        };
        let timecode = clip
            .facts
            .timecode
            .as_deref()
            .and_then(|t| Timecode::parse(t, rate))
            .unwrap_or_else(|| Timecode::zero(rate));
        let shot = locate(
            self.position_us(),
            clip.start_us,
            clip.in_us,
            clip.out_us,
            rate,
            frames.unwrap_or(u32::MAX),
        );
        VideoFeed {
            playhead_timecode: timecode.text(shot.number()),
            start_timecode: timecode
                .text(frame_number(clip.in_us.min(i64::MAX as u64) as i64, rate)),
            end_timecode: timecode
                .text(frame_number(clip.out_us.min(i64::MAX as u64) as i64, rate)),
            ..VideoFeed::default()
        }
    }
}
