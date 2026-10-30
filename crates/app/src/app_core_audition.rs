//! The speaker tool: auditioning a span of an audio file, unprocessed.
//!
//! `UiAction::AuditionSpan` (the speaker pressed on an audio clip, in the arrangement or in the
//! audio clip editor) plays the frames `start_frame..end_frame` of an asset the way they are in
//! the file: at the file's own rate (the preview voice resamples it to the interface), with no
//! warp, fades, envelope or crossfade, from the end down when `reverse`, at the clip's gain.
//! `UiAction::StopAudition` (the release) ends it.
//!
//! It is the **preview voice** of the file browser (`atomik_engine::preview_channel`): the stream
//! goes to the engine at once and a worker thread (`atomik-audition`) reads the span from the
//! engine's copy of the asset (shared memory, or a read through the stream source in chunks for
//! an asset that streams from disk) into the feed, waiting when the ring is full. A new press, a
//! browser preview and a stop all replace or end the stream; the worker sees that the feed was
//! abandoned and ends. Nothing is allocated or locked on the audio thread, the audition is live
//! only (offline renders never receive a preview) and no bounce or export can hear it.

use super::*;
use atomik_engine::{PreviewFeed, PreviewMonitor, SongAsset, preview_channel};
use std::time::Duration;

/// Frames read from the asset at a time.
const CHUNK: u64 = 1 << 14;
/// How long the worker waits for room in a full ring.
const WAIT: Duration = Duration::from_millis(2);

/// The linear gain of `db` (0 dB for a value that is not a finite number).
fn linear(db: f32) -> f32 {
    if db.is_finite() {
        10f32.powf(db.clamp(-120.0, 24.0) / 20.0)
    } else {
        1.0
    }
}

/// The chunks of the frames `[start, end)` of `asset` in play order, as planar vectors: ascending
/// for a forward span, from the end down (each chunk reversed) when `reverse`.
pub(super) fn span_chunks(
    asset: &SongAsset,
    start: u64,
    end: u64,
    reverse: bool,
    mut each: impl FnMut(Vec<Vec<f32>>) -> bool,
) {
    let end = end.min(asset.frames());
    if start >= end {
        return;
    }
    let read = |at: u64, n: u64| -> Vec<Vec<f32>> {
        match asset {
            SongAsset::Memory { data, .. } => data
                .iter()
                .map(|c| c[at as usize..(at + n) as usize].to_vec())
                .collect(),
            SongAsset::Stream {
                channels, source, ..
            } => {
                let mut buffers = vec![vec![0.0f32; n as usize]; usize::from(*channels)];
                let got = source.read(at, &mut buffers).min(n as usize);
                for b in &mut buffers {
                    b.truncate(got);
                }
                buffers
            }
        }
    };
    if reverse {
        let mut top = end;
        while top > start {
            let n = (top - start).min(CHUNK);
            let mut planes = read(top - n, n);
            for p in &mut planes {
                p.reverse();
            }
            if planes.first().is_none_or(Vec::is_empty) || !each(planes) {
                return;
            }
            top -= n;
        }
    } else {
        let mut at = start;
        while at < end {
            let n = (end - at).min(CHUNK);
            let planes = read(at, n);
            if planes.first().is_none_or(Vec::is_empty) || !each(planes) {
                return;
            }
            at += n;
        }
    }
}

/// The worker: feeds the span into the ring until it is all in or the stream is let go of.
pub(super) fn feed_span(
    mut feed: PreviewFeed,
    asset: &SongAsset,
    start: u64,
    end: u64,
    reverse: bool,
) {
    span_chunks(asset, start, end, reverse, |planes| {
        let mut done = 0;
        let len = planes.first().map_or(0, Vec::len);
        while done < len {
            if feed.is_abandoned() {
                return false;
            }
            let n = feed.push(&planes, done);
            if n == 0 {
                std::thread::sleep(WAIT);
            }
            done += n;
        }
        true
    });
    feed.finish();
}

/// The audition that is playing (or has just finished).
#[derive(Default)]
pub(super) struct AuditionState {
    monitor: Option<PreviewMonitor>,
}

impl<S: EngineSink> AppCore<S> {
    /// `UiAction::AuditionSpan`: see the module docs.
    pub fn audition_span(
        &mut self,
        asset: &str,
        (start, end): (u64, u64),
        reverse: bool,
        gain_db: f32,
    ) {
        // A press replaces what plays, a browser preview included.
        self.stop_audition();
        let Some(source) = self.audio.assets.get(asset).cloned() else {
            return self.reject("The audio file of the clip is missing".to_owned());
        };
        let Some(rate) = self.doc().assets.get(asset).map(|m| m.sample_rate) else {
            return self.reject("The audio file of the clip is missing".to_owned());
        };
        let end = end.min(source.frames());
        if start >= end {
            return;
        }
        self.stop_preview();
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(rate);
        let monitor = feed.monitor();
        if let Err(e) = self.sink.preview_start(stream, linear(gain_db)) {
            return self.report(format!("audition: {e}"));
        }
        let spawned = std::thread::Builder::new()
            .name("atomik-audition".into())
            .spawn(move || feed_span(feed, &source, start, end, reverse));
        match spawned {
            Ok(_) => self.audio.audition.monitor = Some(monitor),
            Err(e) => {
                let _ = self.sink.preview_stop();
                self.report(format!("audition: cannot start the reader: {e}"));
            }
        }
    }

    /// `UiAction::StopAudition`: ends the audition (and only that: a browser preview started
    /// since keeps playing).
    pub fn stop_audition(&mut self) {
        if let Some(monitor) = self.audio.audition.monitor.take()
            && !monitor.is_abandoned()
            && !monitor.is_done()
        {
            let _ = self.sink.preview_stop();
        }
    }

    /// Whether an audition is playing (tests).
    #[cfg(test)]
    pub(crate) fn auditioning(&self) -> bool {
        self.audio
            .audition
            .monitor
            .as_ref()
            .is_some_and(|m| !m.is_abandoned() && !m.is_done())
    }
}
