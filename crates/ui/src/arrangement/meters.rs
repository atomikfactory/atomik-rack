//! The level meter in the header of every audio track.
//!
//! * **What it shows.** The *input* level after the track's input gain while the track is armed
//!   (the same level the transport bar's recording meter reads), the *playback* level of the
//!   track otherwise (the output of its console channel, after the fader and the mute, so what
//!   you hear from it). The app decides which and hands both in with
//!   [`ArrangementView::feed_track_levels`] once per frame, with a monotonic time; the view has
//!   no clock of its own. A stereo track draws two bars, a mono input one.
//! * **Ballistics.** The console's: instant attack, 24 dB/s release, a hold tick that stays 1.5 s
//!   ([`crate::console_meter::Ballistic`]), on the same dB scale.
//! * **Readout.** The held peak in dB with one decimal (`-6.2`, `+1.4`, `-inf` below the floor),
//!   drawn when the header has a second line; a small (compact) header keeps the bars and the
//!   lamp.
//! * **Clip lamp.** Lights when a peak reaches 0 dBFS and stays lit until the meter is clicked.
//!   The latch is per track and per source: an input latch goes when the track is disarmed (as the
//!   transport bar's does), a playback latch survives arming. A click is reported
//!   ([`ArrangementView::take_meter_clears`]) so the app can clear the transport bar's latch of
//!   the same track, and the bar's clicks come back through
//!   [`ArrangementView::clear_meter_latch`]: the two always agree.
//! * **Repaints.** The view is marked for repaint only when what a *visible* meter draws
//!   changes (levels to a quarter of a dB, the readout, the lamp), so a quiet project or a
//!   meter scrolled out of view costs no frame.
//!
//! Note tracks have no meter: their header has no room for one next to the automation button,
//! and the console already meters their channel.

use super::{ArrangementView, COMPACT_BELOW, Hit, RowKind, TRACK_LIST_WIDTH};
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::console_meter::{
    Ballistic, METER_FLOOR_DB, PEAK_FALL_DB_PER_S, amp_to_meter_db, meter_norm,
};
use crate::seq_paint as sp;
use crate::tooltip::Help;
use atomik_model::DeviceKey;
use vello::kurbo::{Point, Rect, RoundedRect};

/// Width the name of an audio track keeps in a header with two lines; the meter sits after it.
const NAME_BASE: f64 = 80.0;
/// Width the name of an audio track keeps in a compact (one line) header.
const NAME_BASE_COMPACT: f64 = 54.0;
/// The largest frame step the ballistics take at once (a stalled app does not make them jump).
const MAX_DT: f64 = 0.25;
/// Length of a bar in a two-line header.
const BAR_LEN: f64 = 36.0;

/// What a track's meter reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeterSource {
    /// The input of an armed track, after its input gain.
    Input,
    /// The output of the track's console channel (after the fader and the mute).
    #[default]
    Playback,
}

impl MeterSource {
    fn index(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Playback => 1,
        }
    }
}

/// One audio track's level for this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackLevel {
    /// The audio track's device.
    pub device: DeviceKey,
    /// What the peaks are of.
    pub source: MeterSource,
    /// The peaks since the last frame, left and right (linear amplitude).
    pub peaks: [f32; 2],
    /// Two bars (the track is stereo here); else one, from the left peak.
    pub stereo: bool,
}

/// What a track's meter draws and reads, for tests and the app.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMeterInfo {
    /// What it reads now.
    pub source: MeterSource,
    /// Two bars.
    pub stereo: bool,
    /// The bars, dBFS (the floor when silent).
    pub level_db: [f32; 2],
    /// The held peak, dBFS.
    pub hold_db: f32,
    /// The clip lamp is lit.
    pub latched: bool,
    /// The readout text.
    pub readout: String,
}

/// The state of one track's meter.
#[derive(Debug, Clone, Copy)]
struct TrackMeter {
    source: MeterSource,
    stereo: bool,
    bars: [Ballistic; 2],
    /// The clip latch per source (see [`MeterSource::index`]).
    latched: [bool; 2],
}

impl Default for TrackMeter {
    fn default() -> Self {
        Self {
            source: MeterSource::Playback,
            stereo: true,
            bars: [Ballistic::new(); 2],
            latched: [false; 2],
        }
    }
}

impl TrackMeter {
    fn hold_db(&self) -> f32 {
        if self.stereo {
            self.bars[0].hold_db.max(self.bars[1].hold_db)
        } else {
            self.bars[0].hold_db
        }
    }

    fn lit(&self) -> bool {
        self.latched[self.source.index()]
    }

    /// What is drawn, quantised: a change too small to see does not repaint.
    fn fold(&self, mut h: u64) -> u64 {
        let mut put = |v: i64| h = (h ^ v as u64).wrapping_mul(0x0000_0100_0000_01b3);
        put(self.source.index() as i64);
        put(i64::from(self.stereo));
        for b in &self.bars {
            put((b.level_db * 4.0).round() as i64);
            put((b.hold_db * 4.0).round() as i64);
        }
        put((self.hold_db() * 10.0).round() as i64);
        put(i64::from(self.lit()));
        h
    }
}

/// The meters' state inside the view.
#[derive(Debug, Default)]
pub(super) struct MetersUi {
    meters: std::collections::BTreeMap<DeviceKey, TrackMeter>,
    last_time: Option<f64>,
    /// Clicks not yet taken by the app.
    cleared: Vec<(DeviceKey, MeterSource)>,
}

/// The readout of a held peak.
fn readout(db: f32) -> String {
    if db <= METER_FLOOR_DB + 0.05 {
        return "-inf".to_owned();
    }
    let tenths = (f64::from(db) * 10.0).round() / 10.0;
    if tenths == 0.0 {
        "0.0".to_owned()
    } else if tenths > 0.0 {
        format!("+{tenths:.1}")
    } else {
        format!("{tenths:.1}")
    }
}

/// The base width of the name of an audio track's header (the meter's room is taken off it).
pub(super) fn name_base(compact: bool) -> f64 {
    if compact {
        NAME_BASE_COMPACT
    } else {
        NAME_BASE
    }
}

impl ArrangementView {
    /// The levels of the audio tracks for this frame, at the monotonic time `now_seconds` (any
    /// clock; the first call only sets the origin). A track not listed loses its meter state,
    /// and a track that is not an audio track is ignored. See the module docs.
    pub fn feed_track_levels(&mut self, levels: &[TrackLevel], now_seconds: f64) {
        let dt = self
            .mt
            .last_time
            .map_or(0.0, |t| (now_seconds - t).clamp(0.0, MAX_DT)) as f32;
        self.mt.last_time = Some(now_seconds);
        let before = self.meters_shown();
        let song = &self.song;
        self.mt.meters.retain(|d, _| {
            levels
                .iter()
                .any(|l| &l.device == d && song.track(d).is_some_and(|t| t.is_audio()))
        });
        for l in levels {
            if !song.track(&l.device).is_some_and(|t| t.is_audio()) {
                continue;
            }
            let m = self.mt.meters.entry(l.device.clone()).or_default();
            if m.source != l.source {
                if m.source == MeterSource::Input {
                    // Disarmed: the input's lamp goes, as the transport bar's does.
                    m.latched[MeterSource::Input.index()] = false;
                }
                m.source = l.source;
                m.bars = [Ballistic::new(); 2];
            }
            m.stereo = l.stereo;
            let peak = |p: f32| if p.is_nan() { 0.0 } else { p.max(0.0) };
            let (left, right) = (peak(l.peaks[0]), peak(l.peaks[1]));
            m.bars[0].step(amp_to_meter_db(left), dt, PEAK_FALL_DB_PER_S);
            m.bars[1].step(
                amp_to_meter_db(if l.stereo { right } else { left }),
                dt,
                PEAK_FALL_DB_PER_S,
            );
            if left.max(if l.stereo { right } else { 0.0 }) >= 1.0 {
                m.latched[l.source.index()] = true;
            }
        }
        if self.meters_shown() != before {
            self.repaint = true;
        }
    }

    /// A hash of what the meters of the audio tracks in view draw.
    fn meters_shown(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325;
        for (row, _) in self.visible_rows() {
            let RowKind::Lane { track, lane: 0 } = row.kind else {
                continue;
            };
            let Some(t) = self.song.tracks.get(track).filter(|t| t.is_audio()) else {
                continue;
            };
            h = self
                .mt
                .meters
                .get(&t.device)
                .copied()
                .unwrap_or_default()
                .fold(h);
        }
        h
    }

    /// What the meter of the audio track `device` shows (`None` for another kind of track).
    pub fn track_meter(&self, device: &DeviceKey) -> Option<TrackMeterInfo> {
        self.song.track(device).filter(|t| t.is_audio())?;
        let m = self.mt.meters.get(device).copied().unwrap_or_default();
        Some(TrackMeterInfo {
            source: m.source,
            stereo: m.stereo,
            level_db: [m.bars[0].level_db, m.bars[1].level_db],
            hold_db: m.hold_db(),
            latched: m.lit(),
            readout: readout(m.hold_db()),
        })
    }

    /// The rectangle of the meter in the header of `device`'s track (where a click clears the
    /// lamp), `None` for a track that is not an audio track or is not shown.
    pub fn track_meter_rect(&self, device: &DeviceKey) -> Option<Rect> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let row = self.row_rect(self.row_of(track, 0)?);
        self.meter_rect(row, track)
    }

    /// A point on the meter of `device`'s track.
    pub fn track_meter_point(&self, device: &DeviceKey) -> Option<Point> {
        self.track_meter_rect(device).map(|r| r.center())
    }

    /// The meters clicked since the last call: the track and the source whose latch the click
    /// cleared. The app clears the same latch of the transport bar.
    pub fn take_meter_clears(&mut self) -> Vec<(DeviceKey, MeterSource)> {
        std::mem::take(&mut self.mt.cleared)
    }

    /// Clears the clip latch of `device`'s `source` (the transport bar's meter was clicked):
    /// nothing is reported back. A latch of the source not shown is left alone.
    pub fn clear_meter_latch(&mut self, device: &DeviceKey, source: MeterSource) {
        if let Some(m) = self.mt.meters.get_mut(device)
            && m.source == source
            && m.lit()
        {
            m.latched[source.index()] = false;
            m.bars = [Ballistic::new(); 2];
            self.repaint = true;
        }
    }

    /// The meter's rectangle in the header row `hr` of audio track `track`: after the name on
    /// the first line of a two-line header, between the name and the buttons of a compact one.
    pub(super) fn meter_rect(&self, hr: Rect, track: usize) -> Option<Rect> {
        self.song.tracks.get(track).filter(|t| t.is_audio())?;
        let x = hr.x0;
        Some(if hr.height() < COMPACT_BELOW {
            Rect::new(x + 88.0, hr.y0 + 3.0, x + 110.0, hr.y1 - 3.0)
        } else {
            Rect::new(
                x + 116.0,
                hr.y0 + 8.0,
                x + TRACK_LIST_WIDTH - 20.0,
                hr.y0 + 26.0,
            )
        })
    }

    /// The meter of track `track` when `pos` is on it.
    pub(super) fn meter_hit(&self, hr: Rect, track: usize, pos: Point) -> Option<Hit> {
        self.meter_rect(hr, track)
            .is_some_and(|r| r.contains(pos))
            .then_some(Hit::TrackMeter(track))
    }

    /// A primary press on the meter of track `track`: clears the lamp and the hold.
    pub(super) fn meter_click(&mut self, track: usize) {
        let Some(device) = self.song.tracks.get(track).map(|t| t.device.clone()) else {
            return;
        };
        let Some(m) = self.mt.meters.get_mut(&device) else {
            return;
        };
        m.latched[m.source.index()] = false;
        m.bars = [Ballistic::new(); 2];
        let source = m.source;
        self.mt.cleared.push((device, source));
        self.repaint = true;
    }

    /// The tooltip of the meter of track `track`.
    pub(super) fn meter_help(&self, hr: Rect, track: usize) -> Option<Help> {
        let r = self.meter_rect(hr, track)?;
        let device = &self.song.tracks.get(track)?.device;
        let source = self
            .mt
            .meters
            .get(device)
            .map_or(MeterSource::Playback, |m| m.source);
        Some(Help::new(
            r,
            match source {
                MeterSource::Input => {
                    "Level of this track's input after the gain, with the peak in dB; the lamp lights at 0 dBFS and stays lit until you click here"
                }
                MeterSource::Playback => {
                    "Level of this track's output after its fader (what you hear from it), with the peak in dB; the lamp lights at 0 dBFS and stays lit until you click here"
                }
            },
        ))
    }

    /// Paints the meter in the header row `hr` of audio track `track` (nothing for another
    /// kind of track).
    pub(super) fn paint_track_meter(&self, c: &mut dyn Canvas, hr: Rect, track: usize) {
        let Some(r) = self.meter_rect(hr, track) else {
            return;
        };
        let st = &self.st;
        let t = &st.base;
        let device = &self.song.tracks[track].device;
        let m = self.mt.meters.get(device).copied().unwrap_or_default();
        let compact = hr.height() < COMPACT_BELOW;
        let n = if m.stereo { 2 } else { 1 };
        let bars: Vec<Rect> = if compact {
            let w = if m.stereo { 5.0 } else { 8.0 };
            (0..n)
                .map(|i| {
                    let x = r.x0 + 1.0 + f64::from(i) * (w + 2.0);
                    Rect::new(x, r.y0, x + w, r.y1)
                })
                .collect()
        } else {
            let (h, gap) = if m.stereo { (4.0, 1.0) } else { (6.0, 0.0) };
            let top = r.center().y - (h * f64::from(n) + gap * f64::from(n - 1)) / 2.0;
            (0..n)
                .map(|i| {
                    let y = top + f64::from(i) * (h + gap);
                    Rect::new(r.x0 + 13.0, y, r.x0 + 13.0 + BAR_LEN, y + h)
                })
                .collect()
        };
        for (bar, ballistic) in bars.iter().zip(&m.bars) {
            if m.source == MeterSource::Input {
                // An armed track's meter reads the input: a red outline says so.
                c.stroke_with(
                    RoundedRect::from_rect(bar.inset(-1.0), 1.5),
                    1.0,
                    st.record.with_alpha(0.55),
                );
            }
            c.fill_with(
                RoundedRect::from_rect(*bar, 1.0),
                t.bevel_dark.with_alpha(0.55),
            );
            let pos_of = |db: f32| {
                let n = meter_norm(db);
                if compact {
                    bar.y1 - bar.height() * n
                } else {
                    bar.x0 + bar.width() * n
                }
            };
            for (lo, hi, color) in [
                (METER_FLOOR_DB, -9.0, st.solo),
                (-9.0, -0.5, st.mute),
                (-0.5, 6.0, st.record),
            ] {
                let top = ballistic.level_db.min(hi);
                if top > lo {
                    let (a, b) = (pos_of(lo), pos_of(top));
                    let fill = if compact {
                        Rect::new(bar.x0, b, bar.x1, a)
                    } else {
                        Rect::new(a, bar.y0, b, bar.y1)
                    };
                    c.fill_with(fill, color);
                }
            }
            let mark = |c: &mut dyn Canvas, db: f32, color, grow: f64| {
                let p = pos_of(db);
                if compact {
                    sp::line(c, bar.x0 - grow, p, bar.x1 + grow, p, color);
                } else {
                    sp::line(c, p, bar.y0 - grow, p, bar.y1 + grow, color);
                }
            };
            if ballistic.hold_db > METER_FLOOR_DB + 0.5 {
                mark(c, ballistic.hold_db, t.highlight, 0.0);
            }
            mark(c, 0.0, t.text_muted, 1.0);
        }
        // The clip lamp.
        // A rounded square, not a circle: the tests of the deep zoom count circles as sample dots.
        let (centre, radius) = if compact {
            (Point::new(r.x1 - 3.5, r.center().y), 2.5)
        } else {
            (Point::new(r.x0 + 5.0, r.center().y), 3.5)
        };
        let lamp = RoundedRect::from_rect(
            Rect::from_center_size(centre, (2.0 * radius, 2.0 * radius)),
            radius,
        );
        c.fill_with(
            lamp,
            if m.lit() {
                st.record
            } else {
                t.bevel_dark.with_alpha(0.7)
            },
        );
        if !compact {
            sp::label(
                c,
                &readout(m.hold_db()),
                r.x1,
                r.center().y + 3.0,
                8.0,
                if m.lit() { st.record } else { t.text_muted },
                Align::End,
            );
        }
    }
}
