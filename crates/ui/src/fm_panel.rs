//! The FM synth's rack panel: a band across the top with the algorithm drawn as a diagram and the
//! four operator envelopes drawn as shapes, then one row of controls per operator and the
//! global sections.
//!
//! ```text
//! ALGORITHM            ENVELOPES
//! +----------------+   +-------+ +-------+ +-------+ +-------+
//! |   [4]  fb      |   | OP 1  | | OP 2  | | OP 3  | | OP 4  |
//! |    |           |   |  /\__ | | ...                      |
//! |   [3]  [1]     |   +-------+ +-------+ +-------+ +-------+
//! +----------------+
//! SOUND  ...          OP 1  [fixed][coarse][fine][detune][level][vel][key][delay][A][D][S][R]
//! OP 2 ...
//! ```
//!
//! The diagram comes from [`atomik_dsp::fm::ALGORITHMS`], the same table the voice reads: a box
//! per operator (dim when its level is zero), a line from every modulator down to the operator it
//! modulates, a bar under each carrier and a small loop on the feedback operator. The envelope
//! shapes draw the delay, attack, decay, a sustain stretch and the release of each operator on a
//! square-root time scale, so a 5 ms attack and a 2 s decay are both visible. Both show the knob
//! positions, not the state of a sounding voice.

use crate::autolayout::{self, CONTENT_X, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_device_sdk::DeviceDescriptor;
use atomik_dsp::fm::{self, Algorithm, OPERATORS};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the FM synth (it has its own panel, [`fm_layout`]).
pub const FM_TYPE: &str = "atomik.fm-synth";
/// Height of the band across the top (the diagram and the envelopes), below the panel margin.
const BAND_H: f64 = 92.0;
/// Width of the algorithm diagram.
const DIAGRAM_W: f64 = 150.0;
/// Gap between the diagram and the envelopes, and between two envelope displays.
const GAP: f64 = 6.0;
/// Size of an operator box in the diagram.
const BOX: (f64, f64) = (22.0, 14.0);

/// The colours of the four operators (the first is the device accent).
const OP_COLORS: [Color; 3] = [
    Color::new([0.38, 0.78, 0.98, 1.0]),
    Color::new([0.62, 0.86, 0.46, 1.0]),
    Color::new([0.98, 0.72, 0.36, 1.0]),
];

/// Geometry of the panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FmPanel {
    /// The algorithm diagram.
    pub diagram: Rect,
    /// The four envelope displays, operator 1 first.
    pub envelopes: [Rect; OPERATORS],
}

/// The sections in layout order.
const SECTION_ORDER: [&str; 10] = [
    "fm", "op1", "op2", "op3", "op4", "pitch", "lfo", "play", "master", "cv",
];

/// The section of a param key: the single knobs gather in a few sections.
fn section_prefix(key: &str) -> &str {
    match key {
        "sound" | "algorithm" | "feedback" | "index" | "wheel.to_index" => "fm",
        "voices" | "spread" => "play",
        "pitch_cv_trim" | "pan_cv_trim" | "master.level_cv_trim" | "cv.index_amount" => "cv",
        _ => autolayout::group_prefix(key),
    }
}

/// The short label of a param control (the generated one for the others).
pub fn fm_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "index" => "Index",
        "wheel.to_index" => "Wheel",
        "pitch.env_amount" => "Amount",
        "pitch.env_decay" => "Decay",
        "lfo.to_pitch" => "Pitch",
        "lfo.to_amp" => "Amp",
        "lfo.to_index" => "Index",
        "play.mode" => "Mode",
        "play.glide" => "Glide",
        "voices" => "Voices",
        "spread" => "Spread",
        "pitch_cv_trim" => "Pitch",
        "cv.index_amount" => "Index",
        "master.level_cv_trim" => "Level",
        "pan_cv_trim" => "Pan",
        _ => {
            return match key.rsplit('.').next() {
                Some("key_scale") => Some("Key sc."),
                Some("velocity") if key.starts_with("op") => Some("Vel."),
                _ => None,
            };
        }
    })
}

/// The tooltip of a control: what the knob or button does, one short line. `None` for a key the
/// synth does not have.
pub fn fm_help(key: &str) -> Option<&'static str> {
    if let Some(field) = key.strip_prefix("op").and_then(|r| r.split_once('.')) {
        return Some(match field.1 {
            "fixed" => {
                "Fixed frequency: the operator ignores the key. Coarse and fine set the Hz, a decade per eight steps"
            }
            "coarse" => "Frequency ratio of the played pitch, whole steps (0 is 0.5)",
            "fine" => "Adds a fraction to the ratio for inharmonic tones: 3 + 0.5 is 3.5",
            "detune" => "Detune in cents, for a slow beat against the other operators",
            "level" => "Volume of a carrier, modulation depth of a modulator",
            "velocity" => "How much a harder key raises this operator: brighter on a modulator",
            "key_scale" => "Lowers the level above middle C (right) or below it (left)",
            "delay" => "Time before this operator's envelope starts, to let it swell in late",
            "attack" => "Time for the operator to reach full level",
            "decay" => "Time to fall from full level to the sustain level",
            "sustain" => "Level held while the key is down",
            "release" => "Time to fade after the key is released",
            _ => return None,
        });
    }
    Some(match key {
        "sound" => "Factory sound: click for the next, right-click for the previous. One undo step",
        "algorithm" => "How the four operators are routed: who modulates whom, and who is heard",
        "feedback" => "Operator 4 modulates itself: a sine turns saw-like. Eased back on high keys",
        "index" => "Modulation index of every modulator at once: the brightness of the sound",
        "wheel.to_index" => "How far the mod wheel opens the modulation index",
        "pitch.env_amount" => "Semitones the pitch starts away from the key, falling back to it",
        "pitch.env_decay" => "Time the pitch envelope takes to fall back to the key",
        "lfo.wave" => "Shape of the LFO",
        "lfo.rate" => "LFO speed in Hz",
        "lfo.to_pitch" => "Vibrato depth in semitones",
        "lfo.to_amp" => "Tremolo depth",
        "lfo.to_index" => "How far the LFO sweeps the modulation index",
        "play.mode" => "Poly plays chords, mono one note, legato glides between overlapping notes",
        "play.glide" => "Time the pitch takes to slide to the next note in mono and legato",
        "voices" => "Most notes sounding at once in poly: the oldest is stolen past it",
        "spread" => "Pans the notes by key: low to the left, high to the right",
        "master.level" => "Output level",
        "pitch_cv_trim" => "How far the pitch CV moves the pitch (24 semitones at full)",
        "cv.index_amount" => "How far the index CV moves the modulation index",
        "master.level_cv_trim" => "How far the level CV moves the output level",
        "pan_cv_trim" => "How far the pan CV moves the sound across the stereo field",
        _ => return None,
    })
}

/// The panel of `atomik.fm-synth`: the generated layout with the band reserved above it; the back
/// is the generic socket layout.
pub fn fm_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec = autolayout::auto_layout_with_top(
        desc,
        BAND_H + TITLE_H + 4.0,
        section_prefix,
        &SECTION_ORDER,
        1,
    );
    let y0 = MARGIN_TOP;
    let x0 = CONTENT_X.0;
    let diagram = Rect::new(
        x0,
        y0 + TITLE_H + 2.0,
        x0 + DIAGRAM_W,
        y0 + TITLE_H + BAND_H,
    );
    let env_x0 = x0 + DIAGRAM_W + GAP + 4.0;
    let w = (CONTENT_X.1 - env_x0 - 3.0 * GAP) / OPERATORS as f64;
    let envelopes: [Rect; OPERATORS] = std::array::from_fn(|n| {
        let x = env_x0 + n as f64 * (w + GAP);
        Rect::new(x, diagram.y0, x + w, diagram.y1)
    });
    spec.front_sections.insert(
        0,
        Section {
            label: "ALGORITHM".to_string(),
            rect: Rect::new(x0, y0, x0 + DIAGRAM_W, y0 + TITLE_H),
        },
    );
    spec.front_sections.insert(
        1,
        Section {
            label: "ENVELOPES".to_string(),
            rect: Rect::new(env_x0, y0, CONTENT_X.1, y0 + TITLE_H),
        },
    );
    spec.fm = Some(FmPanel { diagram, envelopes });
    spec
}

fn param(device: &RackDevice, desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key)
        .map_or(0.0, |p| current_value(device, p))
}

fn op_color(n: usize, accent: Color) -> Color {
    if n == 0 {
        accent
    } else {
        OP_COLORS[(n - 1) % OP_COLORS.len()]
    }
}

/// The rows of the diagram: each operator's rank, 0 for one that modulates nothing and 1 more
/// than the highest of its targets otherwise.
pub fn ranks(alg: &Algorithm) -> [usize; OPERATORS] {
    let mut r = [0usize; OPERATORS];
    for op in 0..OPERATORS {
        for to in 0..op {
            if alg.modulates(op, to) {
                r[op] = r[op].max(r[to] + 1);
            }
        }
    }
    r
}

/// The box of each operator in the diagram `area`: the rows by rank (rank 0 at the bottom), the
/// operators of a row centred side by side in operator order.
pub fn diagram_boxes(alg: &Algorithm, area: Rect) -> [Rect; OPERATORS] {
    let rank = ranks(alg);
    let rows = rank.iter().max().copied().unwrap_or(0) + 1;
    let (pad_top, pad_bottom) = (14.0, 8.0);
    let usable = (area.height() - pad_top - pad_bottom - BOX.1).max(0.0);
    let step = if rows > 1 {
        usable / (rows - 1) as f64
    } else {
        0.0
    };
    let spacing = area.width() / OPERATORS as f64;
    let mut out = [Rect::ZERO; OPERATORS];
    for r in 0..rows {
        let ops: Vec<usize> = (0..OPERATORS).filter(|o| rank[*o] == r).collect();
        for (i, op) in ops.iter().enumerate() {
            let cx = area.center().x + (i as f64 - (ops.len() as f64 - 1.0) / 2.0) * spacing;
            let y1 = area.y1 - pad_bottom - r as f64 * step;
            out[*op] = Rect::new(cx - BOX.0 / 2.0, y1 - BOX.1, cx + BOX.0 / 2.0, y1);
        }
    }
    out
}

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &FmPanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    accent: Color,
) {
    let alg = fm::algorithm(param(device, desc, "algorithm").round().max(0.0) as usize);
    let levels: [f32; OPERATORS] =
        std::array::from_fn(|n| param(device, desc, &format!("op{}.level", n + 1)));
    paint_diagram(c, theme, panel.diagram, alg, &levels, accent);
    for (n, r) in panel.envelopes.iter().enumerate() {
        let p = |f: &str| param(device, desc, &format!("op{}.{f}", n + 1));
        let shape = EnvShape {
            delay: p("delay"),
            attack: p("attack"),
            decay: p("decay"),
            sustain: p("sustain"),
            release: p("release"),
        };
        paint_envelope(
            c,
            theme,
            *r,
            n,
            &shape,
            levels[n] > 0.0,
            op_color(n, accent),
        );
    }
}

fn lcd<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
}

fn paint_diagram<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    area: Rect,
    alg: &Algorithm,
    levels: &[f32; OPERATORS],
    accent: Color,
) {
    lcd(c, theme, area);
    c.text(&TextRun::new(
        alg.name,
        pt(area.x0 + 6.0, area.y0 + 10.0),
        8.5,
        theme.text_muted,
    ));
    let boxes = diagram_boxes(alg, area);
    let line = mix(theme.lcd, theme.text_muted, 0.7);
    for from in 0..OPERATORS {
        for to in 0..OPERATORS {
            if alg.modulates(from, to) {
                let (a, b) = (boxes[from], boxes[to]);
                c.stroke_with(
                    Line::new(pt(a.center().x, a.y1), pt(b.center().x, b.y0)),
                    1.2,
                    line,
                );
            }
        }
    }
    for (n, b) in boxes.iter().enumerate() {
        let col = op_color(n, accent);
        let on = levels[n] > 0.0;
        let fill = if on {
            mix(theme.lcd, col, 0.35)
        } else {
            mix(theme.lcd, col, 0.08)
        };
        c.fill_with(RoundedRect::from_rect(*b, 3.0), fill);
        c.stroke_with(
            RoundedRect::from_rect(*b, 3.0),
            1.0,
            if on { col } else { mix(theme.lcd, col, 0.3) },
        );
        c.text(
            &TextRun::new(
                (n + 1).to_string(),
                pt(b.center().x, b.center().y + 3.2),
                9.0,
                theme.text,
            )
            .align(Align::Center),
        );
        if alg.is_carrier(n) {
            // A bar under a carrier: it reaches the output.
            c.stroke_with(
                Line::new(pt(b.x0 + 2.0, b.y1 + 3.0), pt(b.x1 - 2.0, b.y1 + 3.0)),
                2.0,
                col,
            );
        }
        if n == usize::from(alg.feedback) {
            // The feedback loop: out of the right side, up and back in.
            let (x, y) = (b.x1, b.center().y);
            let mut path = BezPath::new();
            path.move_to(pt(x, y));
            path.line_to(pt(x + 7.0, y));
            path.line_to(pt(x + 7.0, b.y0 - 3.0));
            path.line_to(pt(b.center().x + 4.0, b.y0 - 3.0));
            path.line_to(pt(b.center().x + 4.0, b.y0));
            c.stroke_with(path, 1.0, mix(theme.lcd, col, 0.8));
        }
    }
}

/// The envelope settings of one operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvShape {
    /// Delay in ms.
    pub delay: f32,
    /// Attack in ms.
    pub attack: f32,
    /// Decay in ms.
    pub decay: f32,
    /// Sustain level `0..=1`.
    pub sustain: f32,
    /// Release in ms.
    pub release: f32,
}

/// The corner points of an envelope in `0..=1` of the display's width and height (height 1 is
/// the top): the delay, attack, decay, a sustain stretch and the release. The times use a
/// square-root scale.
pub fn envelope_points(e: &EnvShape) -> [(f64, f64); 6] {
    let t = |ms: f32| f64::from(ms.max(0.0)).sqrt();
    let (d, a, dc, r) = (t(e.delay), t(e.attack), t(e.decay), t(e.release));
    let hold = 0.3 * (d + a + dc + r).max(10.0);
    let total = d + a + dc + hold + r;
    let s = f64::from(e.sustain.clamp(0.0, 1.0));
    let mut x = 0.0;
    let mut p = |w: f64, y: f64| {
        x += w / total;
        (x, y)
    };
    [
        (0.0, 0.0),
        p(d, 0.0),
        p(a, 1.0),
        p(dc, s),
        p(hold, s),
        p(r, 0.0),
    ]
}

fn paint_envelope<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    n: usize,
    e: &EnvShape,
    on: bool,
    color: Color,
) {
    lcd(c, theme, r);
    let col = if on {
        color
    } else {
        mix(theme.lcd, color, 0.3)
    };
    c.text(&TextRun::new(
        format!("OP {}", n + 1),
        pt(r.x0 + 5.0, r.y0 + 10.0),
        8.5,
        mix(col, Color::WHITE, 0.2),
    ));
    let inner = Rect::new(r.x0 + 5.0, r.y0 + 16.0, r.x1 - 5.0, r.y1 - 6.0);
    c.stroke_with(
        Line::new(pt(inner.x0, inner.y1), pt(inner.x1, inner.y1)),
        0.5,
        theme.section_line,
    );
    let pts = envelope_points(e);
    let at = |(x, y): (f64, f64)| pt(inner.x0 + x * inner.width(), inner.y1 - y * inner.height());
    let mut path = BezPath::new();
    path.move_to(at(pts[0]));
    for q in &pts[1..] {
        path.line_to(at(*q));
    }
    let mut fill = path.clone();
    fill.line_to(pt(inner.x1, inner.y1));
    fill.close_path();
    c.fill_with(fill, col.with_alpha(0.18));
    c.stroke_with(path, 1.4, col);
}
