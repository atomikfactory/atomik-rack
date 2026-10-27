//! The granular sampler's rack panel: a band across the top with the sample strip (the drop
//! target, the EDIT button) and the waveform of the loaded sample with the live grains drawn
//! over it, then the generated sections of controls.
//!
//! ```text
//! SAMPLES          WAVEFORM
//! +------------+   +--------------------------------------------------------------+
//! | pad.wav    |   |  .||.   .|||||.        |  spray band   |   .|||.              |
//! | 3.00 s     |   | ~~||||~~|||||||~~~~~~~~~|~~~~~~~~~~~~~~~|~~~||||||~~~~~~~~~~~ |
//! | [  EDIT  ] |   +--------------------------------------------------------------+
//! +------------+   MAIN   GRAIN   POSITION   PITCH   AMP ENV   FILTER   MOD ENV   LFO   CV
//! ```
//!
//! The waveform is the window of the sample the grains read (the zone's start to end), from the
//! peak pyramid of its asset (the app feeds it with [`crate::RackView::set_sample_peaks`]); with
//! no sample the device plays its built-in vowel pad, drawn from the same arithmetic as the
//! device's own source ([`atomik_dsp::grain::default_source`]). Over it:
//!
//! * the **position** (a line with a triangle), and the **spray** band centred on it;
//! * the **grains** that sound: one tick each at its place in the window, bright when it has
//!   just started and fading as it nears its end.
//!
//! Position and spray are the published values of the newest voice, so the markers follow the
//! scan, the LFO, the mod envelope, the wheel and the CVs; with no voice sounding they show the
//! knobs. The grains are the last thirteen started, from the device's published values (the
//! source, the position, the spray, the grain and voice counts, then (position, life) pairs, as
//! `atomik_devices::granular` documents). The source says which waveform is shown: the device
//! plays its built-in pad while its sample is not loaded (or its file is missing), so the panel
//! shows the pad then too.

use crate::autolayout::{self, CONTENT_X, MARGIN_TOP, PanelSpec, SAMPLE_STRIP_W, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_audio_file::PeakPyramid;
use atomik_device_sdk::DeviceDescriptor;
use atomik_dsp::grain::default_source;
use atomik_model::{RackDevice, RackDoc, ZoneDoc};
use std::sync::OnceLock;
use vello::kurbo::{BezPath, Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the granular sampler (it has its own panel, [`granular_layout`]).
pub const GRANULAR_TYPE: &str = "atomik.granular";
/// Height of the band across the top (the sample strip and the waveform), below its titles.
const BAND_H: f64 = 92.0;
/// Gap between the sample strip and the waveform.
const GAP: f64 = 8.0;
/// Values the device publishes (see `atomik_devices::granular::METER_COUNT`).
pub const METER_COUNT: usize = 31;
/// Grains in the published values.
pub const TRAIL_GRAINS: usize = 13;
/// Columns of the built-in source's waveform.
const BUILTIN_COLUMNS: usize = 512;
/// Characters of the sample name the strip shows.
const NAME_CHARS: usize = 14;
/// Most rows of controls in a section.
const MAX_ROWS: usize = 2;

/// Geometry of the panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GranularPanel {
    /// The waveform display.
    pub wave: Rect,
}

/// What the strip says about the loaded sample (view state, from the document).
#[derive(Debug, Clone, PartialEq)]
pub struct SampleInfo {
    /// The imported file's name.
    pub name: String,
    /// Length of the window the grains read, in seconds.
    pub seconds: f64,
    /// Zones the device has (only the first matching one plays).
    pub zones: usize,
}

/// What a paint of the panel reads besides the device.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrainInfo<'a> {
    /// The values the device published (empty without a feed).
    pub meters: &'a [f32],
    /// The sample the strip names (`None` without a zone).
    pub sample: Option<&'a SampleInfo>,
    /// The peaks of the zone's asset, once the app has loaded it.
    pub peaks: Option<&'a PeakPyramid>,
}

/// The sections in layout order.
const SECTION_ORDER: [&str; 9] = [
    "main", "grain", "position", "pitch", "amp.env", "filter", "mod.env", "lfo", "cv",
];

/// The section of a param key.
fn section_prefix(key: &str) -> &str {
    match key {
        "sound" | "voices" | "master.level" => "main",
        "velocity.amp" => "amp.env",
        "master.level_cv_trim" => "cv",
        _ if key.starts_with("pos.") || key.starts_with("wheel.") => "position",
        _ if key.starts_with("env.amp.") => "amp.env",
        _ if key.starts_with("env.mod.") => "mod.env",
        _ => autolayout::group_prefix(key),
    }
}

/// The short label of a param control (the generated one for the others).
pub fn granular_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "grain.density" => "Density",
        "grain.size" => "Size",
        "grain.window" => "Window",
        "grain.timing" => "Timing",
        "grain.reverse" => "Reverse",
        "grain.spread" => "Spread",
        "grain.limit" => "Limit",
        "pos.position" => "Position",
        "pos.spray" => "Spray",
        "pos.scan" => "Scan",
        "wheel.to_position" => "Wheel pos",
        "wheel.to_spray" => "Wheel spray",
        "pitch.transpose" => "Transpose",
        "pitch.fine" => "Fine",
        "pitch.random" => "Random",
        "pitch.key_track" => "Key track",
        "velocity.amp" => "Velocity",
        "filter.enabled" => "On",
        "filter.mode" => "Mode",
        "filter.resonance" => "Reson.",
        "filter.key_track" => "Key track",
        "filter.env_amount" => "Env amount",
        "env.mod.to_position" | "lfo.to_position" | "cv.position_amount" => "Position",
        "env.mod.to_size" | "lfo.to_size" => "Size",
        "env.mod.to_density" | "lfo.to_density" | "cv.density_amount" => "Density",
        "env.mod.to_pitch" | "lfo.to_pitch" | "cv.pitch_amount" => "Pitch",
        "lfo.to_cutoff" | "cv.cutoff_amount" => "Cutoff",
        "master.level_cv_trim" => "Level",
        _ => return None,
    })
}

/// The tooltip of a control: what the knob or button does, one short line. `None` for a key the
/// device does not have.
pub fn granular_help(key: &str) -> Option<&'static str> {
    Some(match key {
        "sound" => "Factory sound: click for the next, right-click for the previous. One undo step",
        "voices" => "Most notes sounding at once: the oldest is stolen past it",
        "master.level" => "Output level",
        "grain.density" => "Grains started a second: few are separate slices, many blur to a cloud",
        "grain.size" => {
            "Length of each grain: short is grainy and noisy, long keeps the sample's character"
        }
        "grain.window" => {
            "Shape of a grain: Hann is smooth, Tukey has a flat top, Percussive plucks"
        }
        "grain.timing" => {
            "Randomness of the time between grains, from a steady stream to a scatter"
        }
        "grain.reverse" => {
            "Share of the grains that play backwards: all or none at the ends, mixed between"
        }
        "grain.spread" => "Pans each grain at random up to this far to either side",
        "grain.limit" => {
            "Most grains one note holds at once: past it the one nearest its end is replaced"
        }
        "pos.position" => "Where in the sample the grains start, from its beginning to its end",
        "pos.spray" => {
            "Random scatter of the start around the position: the band over the waveform"
        }
        "pos.scan" => {
            "Speed the position moves through the sample: 1 is real time, 0 freezes, negative runs back"
        }
        "wheel.to_position" => "How far the mod wheel moves the position",
        "wheel.to_spray" => "How far the mod wheel opens the spray",
        "pitch.transpose" => "Transposes the grains in semitones",
        "pitch.fine" => "Fine tune in cents",
        "pitch.random" => {
            "Each grain's pitch is moved at random by up to this many cents either way"
        }
        "pitch.key_track" => {
            "How far the pitch follows the key: 100 % plays the keyboard, 0 plays every key as recorded"
        }
        "env.amp.attack" => "Time for a note to fade in",
        "env.amp.decay" => "Time to fall from full level to the sustain level",
        "env.amp.sustain" => "Level held while the key is down",
        "env.amp.release" => "Time to fade after the key is released",
        "velocity.amp" => "How much a harder key plays louder",
        "filter.enabled" => "Switches the filter on",
        "filter.mode" => "Low-pass, band-pass or high-pass",
        "filter.cutoff" => "Cutoff frequency of the filter",
        "filter.resonance" => "Emphasis at the cutoff",
        "filter.key_track" => "How far the cutoff follows the key",
        "filter.env_amount" => "How far the mod envelope opens the cutoff",
        "env.mod.attack" => "Time for the mod envelope to rise",
        "env.mod.decay" => "Time for the mod envelope to fall to its sustain level",
        "env.mod.sustain" => "Level the mod envelope holds while the key is down",
        "env.mod.release" => "Time for the mod envelope to fall after the key is released",
        "env.mod.to_position" => "How far the mod envelope moves the position",
        "env.mod.to_size" => "How far the mod envelope changes the grain size, in octaves",
        "env.mod.to_density" => "How far the mod envelope changes the density, in octaves",
        "env.mod.to_pitch" => "How far the mod envelope moves the pitch, in semitones",
        "lfo.wave" => "Shape of the LFO",
        "lfo.rate" => "LFO speed in Hz",
        "lfo.to_position" => "How far the LFO sweeps the position",
        "lfo.to_size" => "How far the LFO changes the grain size, in octaves",
        "lfo.to_density" => "How far the LFO changes the density, in octaves",
        "lfo.to_pitch" => "Vibrato depth in semitones",
        "lfo.to_cutoff" => "How far the LFO sweeps the cutoff, in octaves",
        "cv.pitch_amount" => "How far the pitch CV moves the pitch (24 semitones at most)",
        "cv.position_amount" => "How far the position CV moves the position",
        "cv.density_amount" => "How far the density CV changes the density, in octaves",
        "cv.cutoff_amount" => "How far the cutoff CV moves the cutoff, in octaves",
        "master.level_cv_trim" => "How far the level CV moves the output level",
        _ => return None,
    })
}

/// The panel of `atomik.granular`: the generated layout with the band reserved above it, the
/// sample strip and the waveform in the band; the back is the generic socket layout.
pub fn granular_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec = autolayout::auto_layout_with_top(
        desc,
        BAND_H + TITLE_H + 4.0,
        section_prefix,
        &SECTION_ORDER,
        MAX_ROWS,
    );
    let (x0, y0) = (CONTENT_X.0, MARGIN_TOP);
    let strip = Rect::new(x0, y0, x0 + SAMPLE_STRIP_W, y0 + TITLE_H + 2.0 + BAND_H);
    let wave = Rect::new(
        strip.x1 + GAP,
        y0 + TITLE_H + 2.0,
        CONTENT_X.1,
        y0 + TITLE_H + BAND_H,
    );
    spec.front_sections.insert(
        0,
        Section {
            label: "WAVEFORM".to_string(),
            rect: Rect::new(wave.x0, y0, wave.x1, y0 + TITLE_H),
        },
    );
    spec.samples = Some(strip);
    spec.granular = Some(GranularPanel { wave });
    spec
}

/// The zone the panel draws: the one the device says it plays (published source `k` is zone
/// `k - 1`; 0 is the built-in source), else, without a feed, the first; `None` for the built-in
/// source.
pub fn shown_zone<'a>(device: &'a RackDevice, meters: &[f32]) -> Option<&'a ZoneDoc> {
    match meters.first() {
        Some(s) if s.is_finite() => {
            if *s >= 1.0 {
                device.zones.get(*s as usize - 1)
            } else {
                None
            }
        }
        _ => device.zones.first(),
    }
}

/// What the strip says about `device`'s sample: its first zone's asset name and the length of the
/// zone window (`None` without a zone, or without the asset in the pool).
pub fn sample_info(device: &RackDevice, doc: &RackDoc) -> Option<SampleInfo> {
    let zone = device.zones.first()?;
    let asset = doc.assets.get(&zone.asset)?;
    let frames = zone.end.saturating_sub(zone.start);
    Some(SampleInfo {
        name: asset.original_name.clone(),
        seconds: frames as f64 / f64::from(asset.sample_rate.max(1)),
        zones: device.zones.len(),
    })
}

/// The text of the sample strip: the name and the length, or the built-in source.
pub fn strip_lines(sample: Option<&SampleInfo>) -> (String, String) {
    match sample {
        None => ("Built-in".to_string(), "drop a sample here".to_string()),
        Some(s) => {
            let name: String = if s.name.chars().count() > NAME_CHARS {
                let head: String = s.name.chars().take(NAME_CHARS - 1).collect();
                format!("{head}\u{2026}")
            } else {
                s.name.clone()
            };
            let secs = if s.seconds < 10.0 {
                format!("{:.2} s", s.seconds)
            } else {
                format!("{:.1} s", s.seconds)
            };
            let l2 = if s.zones > 1 {
                format!("{secs} \u{b7} {} zones", s.zones)
            } else {
                secs
            };
            (name, l2)
        }
    }
}

/// The peak columns of the built-in source, made once from the device's own arithmetic.
fn builtin_columns() -> &'static [(f32, f32)] {
    static COLUMNS: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    COLUMNS.get_or_init(|| {
        let x = default_source(48_000.0);
        let per = x.len().div_ceil(BUILTIN_COLUMNS).max(1);
        x.chunks(per)
            .map(|c| {
                c.iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(*v), hi.max(*v))
                    })
            })
            .collect()
    })
}

/// Where the position and spray markers are: the published values, else the knobs.
pub fn markers(device: &RackDevice, desc: &DeviceDescriptor, meters: &[f32]) -> (f32, f32) {
    let knob = |key: &str| {
        desc.param_by_key(key)
            .map_or(0.0, |p| current_value(device, p))
    };
    let published = |i: usize, fallback: f32| match meters.get(i) {
        Some(v) if v.is_finite() && *v >= 0.0 => v.min(1.0),
        _ => fallback,
    };
    (
        published(1, knob("pos.position")),
        published(2, knob("pos.spray")),
    )
}

/// The grains the device published as (position, life) pairs, for the ones that sound.
pub fn live_grains(meters: &[f32]) -> Vec<(f32, f32)> {
    (0..TRAIL_GRAINS)
        .filter_map(|i| {
            let (p, l) = (*meters.get(5 + 2 * i)?, *meters.get(6 + 2 * i)?);
            ((0.0..=1.0).contains(&p) && (0.0..=1.0).contains(&l)).then_some((p, l))
        })
        .collect()
}

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &GranularPanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    accent: Color,
    grain: &GrainInfo<'_>,
) {
    let r = panel.wave;
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let inner = Rect::new(r.x0 + 6.0, r.y0 + 6.0, r.x1 - 6.0, r.y1 - 6.0);
    let (position, spray) = markers(device, desc, grain.meters);
    let x_of = |f: f32| inner.x0 + f64::from(f.clamp(0.0, 1.0)) * inner.width();
    // The spray band, then the waveform over it.
    let (lo, hi) = (position - spray * 0.5, position + spray * 0.5);
    if spray > 0.0 {
        c.fill_with(
            Rect::new(x_of(lo), inner.y0, x_of(hi).max(x_of(lo) + 1.0), inner.y1),
            accent.with_alpha(0.16),
        );
    }
    paint_wave(c, theme, inner, device, grain, accent);
    // The grains that sound.
    for (pos, life) in live_grains(grain.meters) {
        let x = x_of(pos).round() + 0.5;
        let a = 0.95 - 0.8 * life;
        c.stroke_with(
            Line::new(pt(x, inner.y0 + 3.0), pt(x, inner.y1 - 3.0)),
            1.5,
            mix(accent, Color::WHITE, 0.55).with_alpha(a),
        );
    }
    // The position: a line with a triangle on top.
    let x = x_of(position).round() + 0.5;
    c.stroke_with(
        Line::new(pt(x, inner.y0 + 4.0), pt(x, inner.y1)),
        1.5,
        mix(accent, Color::WHITE, 0.3),
    );
    let mut tri = BezPath::new();
    tri.move_to(pt(x - 4.0, inner.y0));
    tri.line_to(pt(x + 4.0, inner.y0));
    tri.line_to(pt(x, inner.y0 + 5.0));
    tri.close_path();
    c.fill_with(tri, mix(accent, Color::WHITE, 0.3));
    // The source.
    let label = match (shown_zone(device, grain.meters), grain.sample) {
        (Some(_), Some(s)) => strip_lines(Some(s)).0,
        _ => "Built-in source".to_string(),
    };
    c.text(&TextRun::new(
        label,
        pt(r.x0 + 8.0, r.y1 - 5.0),
        8.5,
        theme.text_muted,
    ));
    c.text(
        &TextRun::new(
            format!("{} %", (position * 100.0).round() as i32),
            pt(r.x1 - 8.0, r.y1 - 5.0),
            8.5,
            theme.text_muted,
        )
        .align(Align::End),
    );
}

/// The waveform of the zone window, or of the built-in source, as a min/max polygon.
fn paint_wave<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    area: Rect,
    device: &RackDevice,
    grain: &GrainInfo<'_>,
    accent: Color,
) {
    let cy = area.center().y;
    let half = area.height() * 0.5 - 2.0;
    c.stroke_with(
        Line::new(pt(area.x0, cy.round() + 0.5), pt(area.x1, cy.round() + 0.5)),
        0.5,
        theme.section_line,
    );
    let columns = area.width().floor().max(1.0) as usize;
    let column: Box<dyn Fn(usize) -> Option<(f32, f32)>> = match shown_zone(device, grain.meters) {
        None => {
            let cols = builtin_columns();
            Box::new(move |i| cols.get(i * cols.len() / columns).copied())
        }
        Some(z) => {
            let Some(peaks) = grain.peaks else {
                return;
            };
            let window = z.end.saturating_sub(z.start).max(1) as f64;
            let per_pixel = window / columns as f64;
            let level = peaks.level_for(per_pixel.max(1.0));
            let (start, limit) = (z.start as f64, z.end);
            Box::new(move |i| {
                let from = start + i as f64 * per_pixel;
                let mut merged: Option<(f32, f32)> = None;
                for ch in 0..level.channels.len() {
                    if let Some((a, b)) =
                        crate::audio::column_peak(level, ch, from, from + per_pixel, limit)
                    {
                        merged = Some(merged.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b))));
                    }
                }
                merged
            })
        }
    };
    let y_of = |v: f32| cy - f64::from(v).clamp(-1.0, 1.0) * half;
    let first = (0..columns).find(|i| column(*i).is_some());
    let Some(first) = first else { return };
    let last = (first..columns)
        .take_while(|i| column(*i).is_some())
        .count()
        + first;
    let mut path = BezPath::new();
    for i in first..last {
        let (_, hi) = column(i).unwrap_or((0.0, 0.0));
        let p = pt(area.x0 + i as f64 + 0.5, y_of(hi));
        if i == first {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    for i in (first..last).rev() {
        let (lo, _) = column(i).unwrap_or((0.0, 0.0));
        path.line_to(pt(area.x0 + i as f64 + 0.5, y_of(lo)));
    }
    path.close_path();
    c.fill_with(path, accent.with_alpha(0.62));
}
