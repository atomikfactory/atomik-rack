//! Panel geometry of the six effects: sections with explicit labels and
//! param lists, packed by the generic section packer, next to (or below) a visual.
//!
//! | Effect | Visual | Placement | Sections |
//! |---|---|---|---|
//! | reverb | decay sketch, output meters | left, 196 wide | SPACE, TONE, BALANCE, OUTPUT |
//! | compressor | transfer curve, level/GR meters, SC button | left, 172 wide | DETECTOR, GAIN, SIDECHAIN, OUTPUT |
//! | EQ | response curve, draggable handles, analyser | across the top, 170 high | LOW CUT, LOW SHELF, PEAK 1, PEAK 2, HIGH SHELF, HIGH CUT, OUTPUT |
//! | distortion | transfer curve, in/out meters | left, 172 wide | DRIVE, TONE, CUT, BODY, CRUSH, OUTPUT |
//! | guitar amp | the speakers of the cabinet with the mic, in/out and gate meters | left, 168 wide | SETUP, INPUT, PREAMP, TONE, POWER, CABINET, CHORUS, DELAY, REVERB |
//! | modulation | LFO sketch (left and right) | left, 196 wide | LFO, VOICE, OUTPUT |
//! | mastering | crossover and band GR, LUFS readouts, GR meters | across the top, 96 high | INPUT, MULTIBAND, BAND 1-3, STEREO, LIMITER, OUTPUT |
//! | character | preset matrix, screen, meters | its own 3 U layout, [`crate::character_panel`] | PRESET, SCREEN, METER, CONTROLS |
//! | spectrum | log-frequency plot or 1/3-octave bars, peak hold, loudest-bin readout, input peaks | across the top, 200 high | ANALYSIS, SMOOTHING, DISPLAY |
//! | audio track | the tuner display (needle, note, cents, Hz), only while the TUNER button of its name plate is on | across the top, 92 high | INPUT, TUNER (with the display) |
//! | phase meter | goniometer, correlation, balance and width bars, level bars, loudness readouts | across the top, 148 high | STEREO, LEVEL, SCOPE, LOUDNESS |
//!
//! The height is the smallest whole number of rack units where the front (visual and sections)
//! and the back (every socket) fit; the tests pin the units per effect. Every param is placed
//! exactly once; the back is the generic socket layout.

use crate::autolayout::{
    CONTENT_X, FxPanel, Group, Item, MARGIN_BOTTOM, MARGIN_TOP, MAX_UNITS, PanelSpec,
    PlacedControl, PlacedSocket, ROW_H, SECTION_GAP, SOCKET_CELL, Section, TITLE_H, back_groups,
    cell_size, choose_widget, fit_with,
};
use crate::fx::FxKind;
use atomik_device_sdk::{ControlKind, DeviceDescriptor, RACK_UNIT};
use vello::kurbo::Rect;

/// Width of the visual placed at the left of the sections.
const LEFT_REVERB: f64 = 196.0;
const LEFT_COMPRESSOR: f64 = 210.0;
const LEFT_DISTORTION: f64 = 172.0;
const LEFT_MODULATION: f64 = 196.0;
const LEFT_AMP: f64 = 168.0;
/// Height (title included) of the visual placed above the sections.
const TOP_EQ: f64 = 186.0;
const TOP_MASTERING: f64 = 112.0;
const TOP_SPECTRUM: f64 = 200.0;
const TOP_PHASE: f64 = 148.0;
const TOP_TUNER: f64 = 92.0;

/// Width of the guitar amp's setup selector.
const SETUP_CELL_W: f64 = 140.0;

type Def = &'static [(&'static str, &'static [&'static str])];

fn sections(kind: FxKind) -> Def {
    match kind {
        FxKind::Reverb => &[
            ("SPACE", &["algorithm", "predelay", "size", "decay"]),
            ("TONE", &["damping", "lowcut"]),
            ("BALANCE", &["early", "width", "modulation"]),
            ("OUTPUT", &["mix", "mix_cv_trim", "freeze", "output"]),
        ],
        FxKind::Compressor => &[
            (
                "DETECTOR",
                &[
                    "detector",
                    "stereo_link",
                    "attack",
                    "release",
                    "auto_release",
                    "lookahead",
                ],
            ),
            ("GAIN", &["threshold", "ratio", "limit", "knee"]),
            ("SIDECHAIN", &["sc_enable", "sc_hp", "sc_lp"]),
            ("OUTPUT", &["makeup", "auto_makeup", "mix"]),
        ],
        FxKind::Eq => &[
            (
                "LOW CUT",
                &["band1.on", "band1.freq", "band1.q", "lowcut.slope"],
            ),
            (
                "LOW SHELF",
                &[
                    "band2.on",
                    "band2.freq",
                    "band2.gain",
                    "band2.gain_cv_trim",
                    "band2.q",
                ],
            ),
            (
                "PEAK 1",
                &[
                    "band3.on",
                    "band3.freq",
                    "band3.gain",
                    "band3.gain_cv_trim",
                    "band3.q",
                ],
            ),
            (
                "PEAK 2",
                &[
                    "band4.on",
                    "band4.freq",
                    "band4.gain",
                    "band4.gain_cv_trim",
                    "band4.q",
                ],
            ),
            (
                "HIGH SHELF",
                &[
                    "band5.on",
                    "band5.freq",
                    "band5.gain",
                    "band5.gain_cv_trim",
                    "band5.q",
                ],
            ),
            (
                "HIGH CUT",
                &["band6.on", "band6.freq", "band6.q", "highcut.slope"],
            ),
            ("OUTPUT", &["output"]),
        ],
        FxKind::Distortion => &[
            ("DRIVE", &["curve", "drive", "drive_cv_trim", "bias"]),
            ("TONE", &["tone"]),
            ("CUT", &["cut.low", "cut.high"]),
            ("BODY", &["body.mode", "body.amount", "body.size"]),
            ("CRUSH", &["crush.bits", "crush.rate"]),
            ("OUTPUT", &["output", "mix", "mix_cv_trim", "oversample"]),
        ],
        FxKind::GuitarAmp => &[
            ("SETUP", &["setup"]),
            ("INPUT", &["input", "gate", "tight", "boost"]),
            ("PREAMP", &["gain"]),
            ("TONE", &["bass", "mid", "treble", "presence", "depth"]),
            ("POWER", &["sag", "master", "level"]),
            ("CABINET", &["cab", "mic"]),
            (
                "CHORUS",
                &["chorus.on", "chorus.mix", "chorus.rate", "chorus.depth"],
            ),
            (
                "DELAY",
                &[
                    "delay.on",
                    "delay.time",
                    "delay.sync",
                    "delay.feedback",
                    "delay.tone",
                    "delay.mix",
                ],
            ),
            (
                "REVERB",
                &["reverb.on", "reverb.size", "reverb.decay", "reverb.mix"],
            ),
        ],
        FxKind::Modulation => &[
            ("RATE", &["mode", "rate", "sync", "spread"]),
            ("VOICE", &["depth", "feedback", "delay", "stages"]),
            ("OUTPUT", &["mix", "mix_cv_trim", "output"]),
        ],
        // Laid out by `character_panel::character_layout`, not by these sections.
        FxKind::Character => &[],
        FxKind::Spectrum => &[
            ("ANALYSIS", &["size", "window", "channel", "mode"]),
            ("SMOOTHING", &["averaging", "hold", "decay"]),
            ("DISPLAY", &["range", "tilt"]),
        ],
        FxKind::PhaseMeter => &[
            ("STEREO", &["corr_time"]),
            ("LEVEL", &["hold", "meter.reset"]),
            ("SCOPE", &["scope", "scope_gain"]),
            ("LOUDNESS", &["loudness"]),
        ],
        FxKind::AudioTrack => &[
            ("INPUT", &["monitor", "rec_gain", "rec_clip_safe"]),
            ("TUNER", &["tuner_mute", "tuner_ref"]),
        ],
        FxKind::Mastering => &[
            ("INPUT", &["input"]),
            ("MULTIBAND", &["multiband.on", "xover.low", "xover.high"]),
            (
                "BAND 1",
                &[
                    "band1.threshold",
                    "band1.ratio",
                    "band1.attack",
                    "band1.release",
                    "band1.gain",
                    "band1.on",
                ],
            ),
            (
                "BAND 2",
                &[
                    "band2.threshold",
                    "band2.ratio",
                    "band2.attack",
                    "band2.release",
                    "band2.gain",
                    "band2.on",
                ],
            ),
            (
                "BAND 3",
                &[
                    "band3.threshold",
                    "band3.ratio",
                    "band3.attack",
                    "band3.release",
                    "band3.gain",
                    "band3.on",
                ],
            ),
            ("STEREO", &["width", "mono_below"]),
            (
                "SPLIT WIDTH",
                &[
                    "width.split",
                    "width.freq",
                    "width.lo",
                    "width.hi",
                    "width.solo",
                ],
            ),
            ("LIMITER", &["softclip", "lookahead", "ceiling", "release"]),
            ("OUTPUT", &["output", "meter.reset"]),
        ],
    }
}

fn title(kind: FxKind) -> &'static str {
    match kind {
        FxKind::Reverb => "DECAY",
        FxKind::Compressor => "TRANSFER",
        FxKind::Eq => "RESPONSE",
        FxKind::Distortion => "CURVE",
        FxKind::GuitarAmp => "AMP",
        FxKind::Modulation => "LFO",
        FxKind::Mastering => "LOUDNESS AND BANDS",
        FxKind::Character => "SCREEN",
        FxKind::Spectrum => "SPECTRUM",
        FxKind::PhaseMeter => "STEREO IMAGE AND LEVELS",
        FxKind::AudioTrack => "TUNER",
    }
}

/// `(left width, top height)`: exactly one is non-zero.
fn placement(kind: FxKind) -> (f64, f64) {
    match kind {
        FxKind::Reverb => (LEFT_REVERB, 0.0),
        FxKind::Compressor => (LEFT_COMPRESSOR, 0.0),
        FxKind::Eq => (0.0, TOP_EQ),
        FxKind::Distortion => (LEFT_DISTORTION, 0.0),
        FxKind::GuitarAmp => (LEFT_AMP, 0.0),
        FxKind::Modulation => (LEFT_MODULATION, 0.0),
        FxKind::Mastering => (0.0, TOP_MASTERING),
        FxKind::Character => (0.0, 0.0),
        FxKind::Spectrum => (0.0, TOP_SPECTRUM),
        FxKind::PhaseMeter => (0.0, TOP_PHASE),
        FxKind::AudioTrack => (0.0, TOP_TUNER),
    }
}

fn knob_or_cell(desc: &DeviceDescriptor, param: usize) -> (ControlKind, f64, Option<f64>) {
    let kind = match choose_widget(&desc.params[param]) {
        ControlKind::Fader => ControlKind::Knob,
        k => k,
    };
    let (w, h) = cell_size(kind);
    // The setup selector shows names such as "Industrial Rhythm".
    let w = if desc.params[param].key == "setup" {
        SETUP_CELL_W
    } else {
        w
    };
    (kind, w, h)
}

/// The panel of one of the six effects.
pub fn fx_layout(kind: FxKind, desc: &DeviceDescriptor) -> PanelSpec {
    layout(kind, desc, true)
}

/// The panel of an audio track: with the tuner (the display and the TUNER section) or, with it
/// hidden, only the input settings, so the panel is shorter. The TUNER button is on the name
/// plate in both ([`crate::paint::tuner_button_rect`]).
pub fn audio_track_layout(desc: &DeviceDescriptor, tuner_on: bool) -> PanelSpec {
    layout(FxKind::AudioTrack, desc, tuner_on)
}

fn layout(kind: FxKind, desc: &DeviceDescriptor, tuner_on: bool) -> PanelSpec {
    if kind == FxKind::Character {
        return crate::character_panel::character_layout(desc);
    }
    // A hidden tuner leaves no display and no settings of its own.
    let collapsed = kind == FxKind::AudioTrack && !tuner_on;
    let (left, top) = if collapsed {
        (0.0, 0.0)
    } else {
        placement(kind)
    };
    let reserve = if left > 0.0 { left + SECTION_GAP } else { 0.0 };
    let groups: Vec<Group> = sections(kind)
        .iter()
        .filter(|(label, _)| !(collapsed && *label == "TUNER"))
        .map(|(label, keys)| Group {
            label: (*label).to_string(),
            min_w: label.chars().count() as f64 * 5.7 + 8.0,
            items: keys
                .iter()
                .filter_map(|k| desc.params.iter().position(|p| p.key == *k))
                .map(|i| {
                    let (_, w, h) = knob_or_cell(desc, i);
                    (i, w, h)
                })
                .collect::<Vec<Item>>(),
        })
        .collect();
    let back = back_groups(desc);
    let min_units = 1;
    let units = (min_units..=MAX_UNITS)
        .find(|&u| {
            fit_with(&groups, u, ROW_H, reserve, top).is_some()
                && fit_with(&back, u, SOCKET_CELL.1, 0.0, 0.0).is_some()
        })
        .unwrap_or(MAX_UNITS);
    let (cells, mut front_sections) =
        fit_with(&groups, units, ROW_H, reserve, top).unwrap_or_default();
    let (back_cells, back_sections) =
        fit_with(&back, units, SOCKET_CELL.1, 0.0, 0.0).unwrap_or_default();
    let height = f64::from(units) * f64::from(RACK_UNIT);

    // The visual cell.
    let (x0, x1, y1) = if left > 0.0 {
        (CONTENT_X.0, CONTENT_X.0 + left, height - MARGIN_BOTTOM)
    } else {
        (CONTENT_X.0, CONTENT_X.1, MARGIN_TOP + top - 4.0)
    };
    let fx = (!collapsed).then(|| {
        front_sections.insert(
            0,
            Section {
                label: title(kind).to_string(),
                rect: Rect::new(x0, MARGIN_TOP, x1, MARGIN_TOP + TITLE_H),
            },
        );
        fx_rects(kind, Rect::new(x0, MARGIN_TOP + TITLE_H, x1, y1))
    });

    let front = cells
        .into_iter()
        .map(|(param, rect)| PlacedControl {
            param,
            kind: knob_or_cell(desc, param).0,
            rect,
        })
        .collect();
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        wavetable: None,
        fm: None,
        granular: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back: back_cells
            .into_iter()
            .map(|(socket, rect)| PlacedSocket { socket, rect })
            .collect(),
        back_sections,
    }
}

fn fx_rects(kind: FxKind, v: Rect) -> FxPanel {
    let empty = Rect::ZERO;
    let mut aux = empty;
    let (plot, meters, readout, button) = match kind {
        FxKind::Reverb => (
            Rect::new(v.x0, v.y0 + 1.0, v.x1 - 52.0, v.y1 - 1.0),
            Rect::new(v.x1 - 44.0, v.y0 + 1.0, v.x1, v.y1 - 1.0),
            empty,
            None,
        ),
        FxKind::Compressor => {
            let s = (v.height() - 24.0).clamp(40.0, 150.0);
            let plot = Rect::new(v.x0, v.y0 + 1.0, v.x0 + s, v.y0 + 1.0 + s);
            (
                plot,
                Rect::new(plot.x1 + 8.0, plot.y0, v.x1, plot.y1),
                empty,
                Some(Rect::new(v.x0, v.y1 - 19.0, v.x1, v.y1 - 1.0)),
            )
        }
        FxKind::Distortion => {
            let s = (v.height() - 2.0).clamp(40.0, 118.0);
            let plot = Rect::new(v.x0, v.y0 + 1.0, v.x0 + s, v.y0 + 1.0 + s);
            (
                plot,
                Rect::new(plot.x1 + 8.0, plot.y0, v.x1, plot.y1),
                empty,
                None,
            )
        }
        FxKind::Modulation => (
            Rect::new(v.x0, v.y0 + 1.0, v.x1, v.y1 - 1.0),
            empty,
            empty,
            None,
        ),
        FxKind::GuitarAmp => (
            Rect::new(v.x0, v.y0 + 1.0, v.x1 - 74.0, v.y1 - 1.0),
            Rect::new(v.x1 - 66.0, v.y0 + 1.0, v.x1, v.y1 - 1.0),
            empty,
            None,
        ),
        FxKind::Eq => (
            Rect::new(v.x0 + 26.0, v.y0, v.x1 - 40.0, v.y1 - 12.0),
            Rect::new(v.x1 - 32.0, v.y0, v.x1, v.y1 - 12.0),
            empty,
            None,
        ),
        FxKind::Mastering => (
            Rect::new(v.x0, v.y0, v.x0 + 250.0, v.y1),
            Rect::new(v.x0 + 474.0, v.y0, v.x1, v.y1),
            Rect::new(v.x0 + 262.0, v.y0, v.x0 + 462.0, v.y1),
            None,
        ),
        // `character_layout` builds its own geometry.
        FxKind::Character => (v, v, empty, None),
        FxKind::Spectrum => (
            Rect::new(v.x0 + 26.0, v.y0 + 1.0, v.x1 - 40.0, v.y1 - 12.0),
            Rect::new(v.x1 - 32.0, v.y0 + 1.0, v.x1, v.y1 - 12.0),
            Rect::new(v.x0 + 32.0, v.y0 + 1.0, v.x1 - 46.0, v.y0 + 14.0),
            None,
        ),
        // The display fills the visual; the TUNER button is on the name plate.
        FxKind::AudioTrack => (
            Rect::new(v.x0, v.y0 + 1.0, v.x1, v.y1 - 1.0),
            empty,
            empty,
            None,
        ),
        FxKind::PhaseMeter => {
            // Goniometer square, stereo bars, level bars with their scale, loudness readouts.
            let s = (v.height() - 2.0).clamp(80.0, 140.0);
            let plot = Rect::new(v.x0, v.y0 + 1.0, v.x0 + s, v.y0 + 1.0 + s);
            let stereo = Rect::new(plot.x1 + 12.0, plot.y0, plot.x1 + 12.0 + 214.0, plot.y1);
            let levels = Rect::new(stereo.x1 + 12.0, plot.y0, stereo.x1 + 12.0 + 76.0, plot.y1);
            aux = Rect::new(levels.x1 + 12.0, plot.y0, v.x1, plot.y1);
            (plot, levels, stereo, None)
        }
    };
    FxPanel {
        kind,
        visual: v,
        plot,
        meters,
        readout,
        aux,
        button,
        character: None,
    }
}

// ------------------------------------------------------------------ EQ plot geometry

use crate::fx::{EQ_PLOT_DB, EqBand, eq_freq_at, eq_pos_of};
use vello::kurbo::Point;

/// Radius of an EQ handle.
pub const EQ_HANDLE_R: f64 = 5.5;
/// Distance within which a press or wheel picks an EQ handle.
pub const EQ_PICK_R: f64 = 11.0;

/// Plot x of a frequency.
pub fn eq_x_of(plot: Rect, freq: f64) -> f64 {
    plot.x0 + eq_pos_of(freq) * plot.width()
}

/// Frequency at a plot x (clamped to the plot).
pub fn eq_freq_of_x(plot: Rect, x: f64) -> f64 {
    eq_freq_at((x - plot.x0) / plot.width())
}

/// Plot y of a gain in dB (clamped to the plot's range).
pub fn eq_y_of(plot: Rect, db: f64) -> f64 {
    let t = (db.clamp(-EQ_PLOT_DB, EQ_PLOT_DB) + EQ_PLOT_DB) / (2.0 * EQ_PLOT_DB);
    plot.y1 - t * plot.height()
}

/// Gain (dB) at a plot y (clamped to the plot's range).
pub fn eq_gain_of_y(plot: Rect, y: f64) -> f64 {
    let t = ((plot.y1 - y) / plot.height()).clamp(0.0, 1.0);
    t * 2.0 * EQ_PLOT_DB - EQ_PLOT_DB
}

/// Where a band's handle sits: its frequency and gain (cuts sit on the 0 dB line).
pub fn eq_handle_pos(plot: Rect, b: &EqBand) -> Point {
    let gain = if b.shape.is_cut() { 0.0 } else { b.gain };
    Point::new(eq_x_of(plot, b.freq), eq_y_of(plot, gain))
}

/// The band whose handle is nearest to `p` within [`EQ_PICK_R`] (later bands win ties, so the
/// topmost drawn handle is picked).
pub fn eq_pick(plot: Rect, bands: &[EqBand; 6], p: Point) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, b) in bands.iter().enumerate() {
        let d = eq_handle_pos(plot, b).distance(p);
        if d <= EQ_PICK_R && best.is_none_or(|(_, bd)| d <= bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The control label on an effect panel: the generated label with the parenthesised unit
/// dropped and a few short names so every label fits its cell.
pub fn fx_label(p: &atomik_core::ParamDescriptor) -> String {
    match p.key {
        "auto_release" | "auto_makeup" => "Auto".to_string(),
        "limit" => "Limit".to_string(),
        "sc_enable" => "On".to_string(),
        "sc_hp" => "HP".to_string(),
        "sc_lp" => "LP".to_string(),
        "stereo_link" => "Link".to_string(),
        "spread" => "Spread".to_string(),
        "release" => "Release".to_string(),
        "early" => "Early/late".to_string(),
        "meter.reset" => "Reset".to_string(),
        "rec_gain" => "Gain".to_string(),
        "rec_clip_safe" => "Clip safe".to_string(),
        "tuner_mute" => "Mute".to_string(),
        "tuner_ref" => "A4 Hz".to_string(),
        "corr_time" => "Corr. time".to_string(),
        "mic" => "Mic".to_string(),
        "scope_gain" => "Gain".to_string(),
        "transform_cv_trim" | "mix_cv_trim" | "drive_cv_trim" | "feedback_cv_trim"
        | "band2.gain_cv_trim" | "band3.gain_cv_trim" | "band4.gain_cv_trim"
        | "band5.gain_cv_trim" => "CV trim".to_string(),
        _ => {
            let label = crate::autolayout::control_label(p);
            match label.find(" (") {
                Some(i) => label[..i].to_string(),
                None => label,
            }
        }
    }
}
