//! Waveform painting shared by the arrangement's audio clips, the slice editor and the sample
//! editor.
//!
//! A waveform is read from one of two sources. The peak pyramid answers every zoom down to its
//! finest bucket ([`atomik_audio_file::FINEST_PEAK_FRAMES`] frames). Below that, when the view
//! holds the samples of the asset (assets in RAM; the ones streamed from disk have none), each
//! pixel column reads the samples of its own frames ([`atomik_audio_file::raw_min_max`], at most
//! a finest bucket of frames per column). Below one frame per pixel the columns would repeat the
//! same sample, so the waveform becomes a line through the sample values, with a dot on every
//! sample once a frame is [`DOT_MIN_PX`] wide.
//!
//! Every pass reads only the visible columns (at most [`MAX_COLUMNS`] of them) or, for the line,
//! the frames under them ([`line_frames`]): the cost of a paint does not depend on the length of
//! the clip or of the file.

use crate::audio;
use crate::canvas::{Canvas, CanvasExt, pt};
use crate::seq_paint as sp;
use atomik_audio_file::{FINEST_PEAK_FRAMES, PeakPyramid, Peaks, raw_min_max};
use vello::kurbo::{BezPath, Circle};
use vello::peniko::Color;

/// The most pixel columns a waveform paints.
pub const MAX_COLUMNS: usize = 4096;
/// A frame this many pixels wide or wider gets a dot.
pub const DOT_MIN_PX: f64 = 6.0;
/// The most frames the line path reads (a guard: below one frame per pixel it needs one per
/// pixel at most, so [`MAX_COLUMNS`] plus the margin).
pub const MAX_LINE_FRAMES: usize = MAX_COLUMNS + 4;

/// The frames a line through the samples needs between the source frames `from` and `to`: one
/// frame beyond each side so the line leaves the view, clipped to `[0, limit)` and to
/// [`MAX_LINE_FRAMES`]. Empty when nothing of the range is left.
pub fn line_frames(from: f64, to: f64, limit: u64) -> std::ops::Range<usize> {
    if from.is_nan() || to.is_nan() || limit == 0 || to < from {
        return 0..0;
    }
    let first = (from.floor() - 1.0).max(0.0);
    let last = (to.ceil() + 2.0).min(limit as f64);
    if last <= first {
        return 0..0;
    }
    let first = first as usize;
    first..(last as usize).min(first + MAX_LINE_FRAMES)
}

/// Where a waveform's columns come from at one zoom.
#[derive(Clone, Copy)]
pub enum WaveDetail<'a> {
    /// A pyramid level.
    Peaks(&'a Peaks),
    /// The samples of the asset, one slice per channel.
    Samples(&'a [Vec<f32>]),
}

impl<'a> WaveDetail<'a> {
    /// The source for `frames_per_pixel`: the samples when the view has them and the zoom is
    /// finer than the finest pyramid level, else the pyramid level for that zoom.
    pub fn pick(
        peaks: &'a PeakPyramid,
        samples: Option<&'a [Vec<f32>]>,
        frames_per_pixel: f64,
    ) -> Self {
        match samples {
            Some(s) if !s.is_empty() && frames_per_pixel < f64::from(FINEST_PEAK_FRAMES) => {
                WaveDetail::Samples(s)
            }
            _ => WaveDetail::Peaks(peaks.level_for(frames_per_pixel.max(1.0))),
        }
    }

    /// Channels of the audio.
    pub fn channels(&self) -> usize {
        match self {
            WaveDetail::Peaks(p) => p.channels.len(),
            WaveDetail::Samples(s) => s.len(),
        }
    }

    /// Min and max of channel `ch` over the source frames `[from, to)`; frames at or after
    /// `limit` are not read. `None` when nothing is left (see [`audio::column_peak`]).
    pub fn column(&self, ch: usize, from: f64, to: f64, limit: u64) -> Option<(f32, f32)> {
        match self {
            WaveDetail::Peaks(p) => audio::column_peak(p, ch, from, to, limit),
            WaveDetail::Samples(s) => raw_min_max(s.get(ch)?, from, to.min(limit as f64)),
        }
    }

    /// Whether this source at `frames_per_pixel` is drawn as a line through the samples.
    pub fn is_line(&self, frames_per_pixel: f64) -> bool {
        matches!(self, WaveDetail::Samples(_)) && frames_per_pixel < 1.0
    }
}

/// One channel of a waveform to paint.
pub struct WaveChannel<'a> {
    /// The source.
    pub detail: WaveDetail<'a>,
    /// Channel index.
    pub ch: usize,
    /// Y of the channel's zero line.
    pub cy: f64,
    /// Half the height of the channel's band: full scale is `cy ± half`.
    pub half: f64,
    /// Linear gain of the clip (1 for the editors).
    pub gain: f64,
    /// Waveform colour.
    pub color: Color,
    /// Logical x of the first pixel column.
    pub x_start: f64,
    /// Pixel columns to paint.
    pub columns: usize,
    /// Horizontal extent of the zero line.
    pub line_x: (f64, f64),
    /// Frames of the asset: nothing at or after this frame is drawn.
    pub limit: u64,
    /// Source frames per pixel at the middle of the view.
    pub frames_per_pixel: f64,
}

/// Paints one channel: the zero line, then the min/max polygon of the columns, or at deep zoom
/// the line through the samples. `source(x)` is the source frame under logical x and
/// `frame_x(frame)` the logical x of a source frame (the line path places every sample with it).
pub fn paint_channel(
    c: &mut dyn Canvas,
    w: &WaveChannel<'_>,
    source: &dyn Fn(f64) -> f64,
    frame_x: &dyn Fn(f64) -> f64,
) {
    let y_of = |v: f32| w.cy - (f64::from(v) * w.gain).clamp(-1.0, 1.0) * w.half;
    let zero = w.cy.round() + 0.5;
    if let (true, WaveDetail::Samples(s)) = (w.detail.is_line(w.frames_per_pixel), w.detail) {
        let Some(samples) = s.get(w.ch) else { return };
        let end = w.x_start + w.columns as f64;
        let frames = line_frames(source(w.x_start), source(end), w.limit);
        let Some(points) = samples.get(frames.clone()) else {
            return;
        };
        if points.is_empty() {
            return;
        }
        sp::line(
            c,
            w.line_x.0,
            zero,
            w.line_x.1,
            zero,
            w.color.with_alpha(0.22),
        );
        let at = |k: usize, v: f32| pt(frame_x((frames.start + k) as f64), y_of(v));
        let mut path = BezPath::new();
        for (k, v) in points.iter().enumerate() {
            let p = at(k, if v.is_nan() { 0.0 } else { *v });
            if k == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        c.stroke_with(path, 1.25, w.color);
        if 1.0 / w.frames_per_pixel >= DOT_MIN_PX {
            for (k, v) in points.iter().enumerate() {
                let v = if v.is_nan() { 0.0 } else { *v };
                c.fill_with(Circle::new(at(k, v), 2.0), w.color);
            }
        }
        return;
    }
    let peak = |i: usize| {
        let x = w.x_start + i as f64;
        w.detail.column(w.ch, source(x), source(x + 1.0), w.limit)
    };
    let n = (0..w.columns.min(MAX_COLUMNS))
        .take_while(|i| peak(*i).is_some())
        .count();
    if n == 0 {
        return;
    }
    sp::line(
        c,
        w.line_x.0,
        zero,
        w.line_x.1,
        zero,
        w.color.with_alpha(0.22),
    );
    let mut path = BezPath::new();
    for i in 0..n {
        let (_, hi) = peak(i).unwrap_or((0.0, 0.0));
        let p = pt(w.x_start + i as f64 + 0.5, y_of(hi));
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    for i in (0..n).rev() {
        let (lo, _) = peak(i).unwrap_or((0.0, 0.0));
        path.line_to(pt(w.x_start + i as f64 + 0.5, y_of(lo)));
    }
    path.close_path();
    c.fill_with(path, w.color);
}

/// The most the wave zoom of the sample and slice editors may reach for an asset of `frames`
/// frames shown `width` pixels wide: the zoom that gives [`MIN_FRAMES_PER_PIXEL`], at least 1
/// (a short sample is never zoomed out below the fit).
pub fn max_wave_zoom(frames: f64, width: f64) -> f64 {
    if frames.is_nan() || width.is_nan() || width <= 0.0 {
        return 1.0;
    }
    (frames / (width * MIN_FRAMES_PER_PIXEL)).max(1.0)
}

/// Deepest waveform zoom: a frame is 32 pixels wide.
pub const MIN_FRAMES_PER_PIXEL: f64 = 1.0 / 32.0;

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_audio_file::AudioData;

    fn pyramid(frames: usize) -> PeakPyramid {
        let ch: Vec<f32> = (0..frames).map(|i| ((i % 17) as f32 - 8.0) / 8.0).collect();
        PeakPyramid::build(&AudioData::new(48_000, vec![ch]).unwrap()).unwrap()
    }

    fn samples(frames: usize) -> Vec<Vec<f32>> {
        vec![(0..frames).map(|i| ((i % 17) as f32 - 8.0) / 8.0).collect()]
    }

    #[test]
    fn the_source_is_the_pyramid_down_to_the_finest_bucket_and_the_samples_below() {
        let (p, s) = (pyramid(10_000), samples(10_000));
        for fpp in [256.0, 300.0, 1024.0, 1e6, f64::NAN] {
            assert!(
                matches!(WaveDetail::pick(&p, Some(&s), fpp), WaveDetail::Peaks(_)),
                "{fpp}"
            );
        }
        for fpp in [255.9, 64.0, 1.0, 0.5, 0.01] {
            assert!(
                matches!(WaveDetail::pick(&p, Some(&s), fpp), WaveDetail::Samples(_)),
                "{fpp}"
            );
        }
        // No samples (a streamed asset): the finest level whatever the zoom.
        let d = WaveDetail::pick(&p, None, 0.1);
        assert!(matches!(d, WaveDetail::Peaks(l) if l.frames_per_bucket == 256));
        assert!(matches!(
            WaveDetail::pick(&p, Some(&[]), 0.1),
            WaveDetail::Peaks(_)
        ));
    }

    #[test]
    fn sample_columns_agree_with_the_pyramid_where_both_are_exact() {
        let (p, s) = (pyramid(5000), samples(5000));
        let level = p.level_for(256.0);
        let (peaks, raw) = (WaveDetail::Peaks(level), WaveDetail::Samples(&s));
        for b in 0..19 {
            let (from, to) = (b as f64 * 256.0, b as f64 * 256.0 + 256.0);
            assert_eq!(
                peaks.column(0, from, to, 5000),
                raw.column(0, from, to, 5000),
                "bucket {b}"
            );
        }
        assert_eq!(
            raw.column(0, 4990.0, 5100.0, 5000),
            raw.column(0, 4990.0, 5000.0, 5000)
        );
        assert_eq!(raw.column(0, 5000.0, 5100.0, 5000), None, "past the end");
        assert_eq!(raw.column(3, 0.0, 10.0, 5000), None, "no such channel");
        assert_eq!(raw.channels(), 1);
        assert_eq!(peaks.channels(), 1);
    }

    #[test]
    fn the_line_mode_starts_below_one_frame_per_pixel_and_only_for_samples() {
        let (p, s) = (pyramid(100), samples(100));
        let raw = WaveDetail::Samples(&s);
        assert!(raw.is_line(0.99));
        assert!(!raw.is_line(1.0));
        assert!(!WaveDetail::Peaks(p.level_for(1.0)).is_line(0.01));
    }

    #[test]
    fn line_frames_cover_the_view_plus_a_margin_and_stay_inside_the_asset() {
        assert_eq!(line_frames(10.2, 20.7, 1000), 9..23);
        assert_eq!(line_frames(-50.0, 3.0, 1000), 0..5);
        assert_eq!(line_frames(995.0, 1100.0, 1000), 994..1000);
        assert_eq!(line_frames(2000.0, 3000.0, 1000), 0..0);
        assert_eq!(line_frames(-9.0, -3.0, 1000), 0..0);
        assert_eq!(line_frames(f64::NAN, 3.0, 1000), 0..0);
        assert_eq!(line_frames(5.0, 3.0, 1000), 0..0);
        assert_eq!(line_frames(0.0, 10.0, 0), 0..0);
        // A view that is far too wide for the line is cut, not read.
        let r = line_frames(0.0, 1e9, u64::MAX);
        assert_eq!(r.len(), MAX_LINE_FRAMES);
    }

    #[test]
    fn the_deepest_wave_zoom_reaches_a_frame_of_32_pixels() {
        // 10 s at 48 kHz in 1000 px: 480 frames/px fit; 32 px per frame is 15360 times more.
        let z = max_wave_zoom(480_000.0, 1000.0);
        assert!((480_000.0 / (1000.0 * z) - MIN_FRAMES_PER_PIXEL).abs() < 1e-12);
        assert!(z > 4096.0, "beyond the old limit: {z}");
        assert_eq!(
            max_wave_zoom(10.0, 1000.0),
            1.0,
            "a tiny sample stays fitted"
        );
        assert_eq!(max_wave_zoom(0.0, 1000.0), 1.0);
        assert_eq!(max_wave_zoom(1e6, 0.0), 1.0);
        assert_eq!(max_wave_zoom(f64::NAN, 100.0), 1.0);
    }
}
