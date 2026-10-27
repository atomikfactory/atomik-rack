//! The Mod Source's rack panel: two LFO rows and an envelope row of ordinary controls, a shape
//! display for each LFO and a live scrolling scope of LFO 1, LFO 2 and the envelope.
//!
//! ```text
//! LFO 1   [shape][morph][rate][sync][div][phase][amount][polarity][key sync][rate CV]
//! LFO 2   [shape][morph][rate][sync][div][phase][amount][polarity][key sync][rate CV]
//! ENVELOPE [A][D][S][R][amount][mode]          SHAPES  [LFO 1 curve] [LFO 2 curve]
//! SCOPE   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
//! ```
//!
//! The controls are the device's params, placed by the generic widgets. The shape displays draw
//! one cycle of the selected shape with its morph and amount and a marker at the live phase. The
//! scope is drawn from the device's meter values (see [`atomik_devices::mod_source`]): the device
//! publishes its latest outputs, the two LFO phases, the envelope stage and the last eight
//! history points of each signal with a push counter, and [`Scope::feed`] appends the points the
//! counter says are new to a fixed ring buffer (allocated at the first feed, never reallocated
//! afterwards).
//!
//! The shape curves are display code: they mirror `atomik_dsp::mod_source::mod_shape` with the
//! platform sine (a test compares them with the DSP).

use crate::autolayout::{
    CONTENT_X, MARGIN_TOP, PanelSpec, PlacedSocket, ROW_H, SOCKET_CELL, Section, TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::seq_panel::{PlayerInfo, place_controls};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Circle, Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the Mod Source (it has its own panel, [`modsrc_layout`]).
pub const MODSRC_TYPE: &str = "atomik.mod-source";
/// Height of the panel in rack units.
pub const MODSRC_UNITS: u8 = 4;
/// Number of meter values the device publishes.
pub const METER_COUNT: usize = 32;
/// History points per signal in one meter frame.
pub const HISTORY: usize = 8;
/// The device's push counter wraps at this value.
pub const COUNTER_MODULO: u32 = 4096;
/// Points the scope keeps per trace (one per pixel of the scope at 1x).
pub const SCOPE_CAPACITY: usize = 640;
/// Width of one control cell of the LFO rows.
const COL_W: f64 = 62.0;

/// The LFO params, in cell order (ten per row); `{}` is the LFO number.
const LFO_KEYS: [&str; 10] = [
    "shape",
    "morph",
    "rate",
    "sync",
    "div",
    "phase",
    "amount",
    "polarity",
    "key_sync",
    "rate_cv_trim",
];
/// The envelope params, in cell order.
const ENV_KEYS: [&str; 6] = [
    "env_attack",
    "env_decay",
    "env_sustain",
    "env_release",
    "env_amount",
    "env_mode",
];

/// Colour of the second LFO's trace.
const LFO2_COLOR: Color = Color::new([0.38, 0.78, 0.98, 1.0]);
/// Colour of the envelope's trace.
const ENV_COLOR: Color = Color::new([1.0, 0.69, 0.25, 1.0]);

/// Geometry of the panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModSourcePanel {
    /// The two shape displays.
    pub shapes: [Rect; 2],
    /// The scope.
    pub scope: Rect,
}

/// The short label of a param control (`None` for a key that is not this device's).
pub fn modsrc_label(key: &str) -> Option<String> {
    let k = key
        .strip_prefix("lfo1_")
        .or_else(|| key.strip_prefix("lfo2_"))
        .or_else(|| key.strip_prefix("env_"))?;
    Some(
        match k {
            "shape" => "Shape",
            "morph" => "Morph",
            "rate" => "Rate",
            "sync" => "Sync",
            "div" => "Division",
            "phase" => "Phase",
            "amount" => "Amount",
            "polarity" => "Polarity",
            "key_sync" => "Key sync",
            "rate_cv_trim" => "Rate CV",
            "attack" => "Attack",
            "decay" => "Decay",
            "sustain" => "Sustain",
            "release" => "Release",
            "mode" => "Mode",
            _ => return None,
        }
        .to_string(),
    )
}

/// The short label under a jack of the back panel (the sections name the LFO).
pub fn modsrc_socket_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "notes_in" => "Notes in",
        "gate_cv_in" => "Gate in",
        "lfo1_rate_cv_in" | "lfo2_rate_cv_in" => "Rate CV",
        "lfo1_amount_cv_in" | "lfo2_amount_cv_in" => "Amount CV",
        "lfo1_cv_out" | "lfo2_cv_out" | "env_cv_out" => "CV out",
        _ => return None,
    })
}

fn keys_of(prefix: &str) -> Vec<String> {
    LFO_KEYS.iter().map(|k| format!("{prefix}_{k}")).collect()
}

/// The panel of `atomik.mod-source` (four rack units, see the module documentation). The back
/// groups the nine sockets by LFO and envelope.
pub fn modsrc_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let mut front = Vec::new();
    let mut sections = Vec::new();
    let mut y = MARGIN_TOP;
    for (n, prefix) in ["lfo1", "lfo2"].into_iter().enumerate() {
        let keys = keys_of(prefix);
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        front.extend(place_controls(desc, &refs, x0, COL_W, 10, y + TITLE_H));
        sections.push(Section {
            label: format!("LFO {}", n + 1),
            rect: Rect::new(x0, y, x0 + 10.0 * COL_W, y + TITLE_H),
        });
        y += TITLE_H + ROW_H + 2.0;
    }
    front.extend(place_controls(desc, &ENV_KEYS, x0, COL_W, 6, y + TITLE_H));
    let env_w = 6.0 * COL_W;
    sections.push(Section {
        label: "ENVELOPE".to_string(),
        rect: Rect::new(x0, y, x0 + env_w, y + TITLE_H),
    });
    let shapes_x = x0 + env_w + 8.0;
    sections.push(Section {
        label: "SHAPES".to_string(),
        rect: Rect::new(shapes_x, y, x1, y + TITLE_H),
    });
    let box_w = (x1 - shapes_x - 4.0) / 2.0;
    let box_y = y + TITLE_H + 3.0;
    let shapes = [0.0, 1.0].map(|i| {
        let bx = shapes_x + i * (box_w + 4.0);
        Rect::new(bx, box_y, bx + box_w, box_y + ROW_H - 6.0)
    });
    y += TITLE_H + ROW_H + 2.0;
    sections.push(Section {
        label: "SCOPE".to_string(),
        rect: Rect::new(x0, y, x1, y + TITLE_H),
    });
    let scope = Rect::new(
        x0,
        y + TITLE_H + 2.0,
        x1,
        f64::from(MODSRC_UNITS) * f64::from(atomik_device_sdk::RACK_UNIT) - 4.0,
    );

    // Back: one row of cells per section.
    let (back, back_sections) = back_cells(desc);
    PanelSpec {
        height_units: MODSRC_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: Some(ModSourcePanel { shapes, scope }),
        wavetable: None,
        fm: None,
        container: None,
        plugin: None,
        front,
        front_sections: sections,
        back,
        back_sections,
    }
}

fn back_cells(desc: &DeviceDescriptor) -> (Vec<PlacedSocket>, Vec<Section>) {
    let groups: [(&str, [&str; 3]); 3] = [
        (
            "LFO 1",
            ["lfo1_rate_cv_in", "lfo1_amount_cv_in", "lfo1_cv_out"],
        ),
        (
            "LFO 2",
            ["lfo2_rate_cv_in", "lfo2_amount_cv_in", "lfo2_cv_out"],
        ),
        ("ENVELOPE", ["notes_in", "gate_cv_in", "env_cv_out"]),
    ];
    let mut back = Vec::new();
    let mut sections = Vec::new();
    let mut x = CONTENT_X.0;
    let w = SOCKET_CELL.0 + 4.0;
    for (label, keys) in groups {
        let top = MARGIN_TOP + TITLE_H;
        sections.push(Section {
            label: label.to_string(),
            rect: Rect::new(x, MARGIN_TOP, x + 3.0 * w, MARGIN_TOP + TITLE_H),
        });
        for (i, key) in keys.iter().enumerate() {
            let Some(socket) = desc.sockets.iter().position(|s| s.key == *key) else {
                continue;
            };
            let cx = x + i as f64 * w;
            back.push(PlacedSocket {
                socket,
                rect: Rect::new(cx, top, cx + SOCKET_CELL.0, top + SOCKET_CELL.1),
            });
        }
        x += 3.0 * w + 16.0;
    }
    (back, sections)
}

// ------------------------------------------------------------------ the shapes

/// Value of shape `shape` (the device's `shape` choice index) at phase `p` in cycles with `morph`
/// in -1..=1; `rand` are the previous, current and next random values (random shapes only). It
/// mirrors `atomik_dsp::mod_source::mod_shape` (see its table), with the platform sine.
pub fn shape_value(shape: usize, p: f64, morph: f64, rand: [f64; 3]) -> f64 {
    let p = p - p.floor();
    let m = morph.clamp(-1.0, 1.0);
    let skew = |p: f64| {
        let s = (0.5 + 0.48 * m).clamp(0.02, 0.98);
        if p < s {
            0.5 * p / s
        } else {
            0.5 + 0.5 * (p - s) / (1.0 - s)
        }
    };
    let bend = |p: f64| {
        let k = 0.95 * m;
        p * (1.0 - k) / (1.0 - k * p)
    };
    let v = match shape {
        0 => (std::f64::consts::TAU * skew(p)).sin(),
        1 => {
            let u = skew(p);
            if u < 0.25 {
                4.0 * u
            } else if u < 0.75 {
                2.0 - 4.0 * u
            } else {
                4.0 * u - 4.0
            }
        }
        2 => 2.0 * bend(p) - 1.0,
        3 => 1.0 - 2.0 * bend(p),
        4 => {
            if p < 0.5 + 0.45 * m {
                1.0
            } else {
                -1.0
            }
        }
        5 => {
            let g = m.max(0.0);
            if p < g {
                let t = p / g;
                rand[0] * (1.0 - t) + rand[1] * t
            } else {
                rand[1]
            }
        }
        _ => {
            let ss = p * p * (3.0 - 2.0 * p);
            let t = if m < 0.0 {
                ss * (1.0 + m) - m * p
            } else {
                let sss = p * p * p * (p * (6.0 * p - 15.0) + 10.0);
                ss * (1.0 - m) + sss * m
            };
            rand[1] * (1.0 - t) + rand[2] * t
        }
    };
    v.clamp(-1.0, 1.0)
}

/// The fixed example values the random shapes are drawn with.
const PREVIEW_RAND: [f64; 3] = [-0.45, 0.6, -0.35];

// ------------------------------------------------------------------ the scope buffer

/// The scope's history: three fixed-size ring buffers of recent output points (LFO 1, LFO 2,
/// envelope), the latest LFO phases and the envelope stage, fed from the device's meter values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    rings: [Vec<f32>; 3],
    head: usize,
    len: usize,
    last_counter: Option<u32>,
    spacing_ms: f32,
    phases: [f32; 2],
    stage: u8,
}

impl Scope {
    /// An empty scope (no memory until the first feed).
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one meter frame into the buffers and returns whether anything changed. The rings
    /// are allocated by the first feed; later feeds allocate nothing. The history points that are
    /// new since the previous feed (by the push counter) are appended oldest first; a gap longer
    /// than the eight points a frame carries appends all eight.
    pub fn feed(&mut self, values: &[f32]) -> bool {
        if values.len() < METER_COUNT || !values.iter().all(|v| v.is_finite()) {
            return false;
        }
        if self.rings[0].is_empty() {
            for r in &mut self.rings {
                r.resize(SCOPE_CAPACITY, 0.0);
            }
        }
        let counter = (values[6].round().max(0.0) as u32) % COUNTER_MODULO;
        let new = match self.last_counter {
            None => HISTORY,
            Some(last) => (counter + COUNTER_MODULO - last) as usize % COUNTER_MODULO as usize,
        };
        // A counter that went backwards by a small step is a restart: take the whole frame.
        let new = if new > COUNTER_MODULO as usize / 2 {
            HISTORY
        } else {
            new.min(HISTORY)
        };
        let phases = [values[3].clamp(0.0, 1.0), values[4].clamp(0.0, 1.0)];
        let stage = values[5].round().clamp(0.0, 4.0) as u8;
        let mut changed = false;
        if new > 0 {
            for i in HISTORY - new..HISTORY {
                for lane in 0..3 {
                    self.rings[lane][self.head] = values[7 + lane * HISTORY + i];
                }
                self.head = (self.head + 1) % SCOPE_CAPACITY;
                self.len = (self.len + 1).min(SCOPE_CAPACITY);
            }
            changed = true;
        }
        if phases.map(f32::to_bits) != self.phases.map(f32::to_bits) || stage != self.stage {
            changed = true;
        }
        self.last_counter = Some(counter);
        self.spacing_ms = values[31].max(0.0);
        self.phases = phases;
        self.stage = stage;
        changed
    }

    /// Number of points stored per trace.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing was fed yet.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The point `age` steps before the newest of trace `lane` (0 LFO 1, 1 LFO 2, 2 envelope);
    /// `None` beyond what is stored.
    pub fn point(&self, lane: usize, age: usize) -> Option<f32> {
        if lane >= 3 || age >= self.len {
            return None;
        }
        let i = (self.head + SCOPE_CAPACITY - 1 - age) % SCOPE_CAPACITY;
        Some(self.rings[lane][i])
    }

    /// Spacing of the points in milliseconds (0 before the first feed).
    pub fn spacing_ms(&self) -> f32 {
        self.spacing_ms
    }

    /// The live phases of the two LFOs (0..1).
    pub fn phases(&self) -> [f32; 2] {
        self.phases
    }

    /// The envelope stage (0 idle, 1 attack, 2 decay, 3 sustain, 4 release).
    pub fn stage(&self) -> u8 {
        self.stage
    }
}

// ------------------------------------------------------------------ painting

fn param(device: &RackDevice, desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key)
        .map_or(0.0, |p| current_value(device, p))
}

fn window<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
}

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ModSourcePanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    let colors = [accent, LFO2_COLOR];
    let phases = info.scope.map_or([0.0; 2], Scope::phases);
    for (n, r) in panel.shapes.iter().enumerate() {
        let key = |k: &str| format!("lfo{}_{k}", n + 1);
        let shape = param(device, desc, &key("shape")).round().max(0.0) as usize;
        let morph = f64::from(param(device, desc, &key("morph")));
        let amount = f64::from(param(device, desc, &key("amount")));
        let unipolar = param(device, desc, &key("polarity")) >= 0.5;
        paint_shape(
            c,
            theme,
            *r,
            shape,
            morph,
            amount,
            unipolar,
            colors[n],
            info.scope.map(|_| f64::from(phases[n])),
        );
    }
    paint_scope(c, theme, panel.scope, info.scope, accent);
}

#[allow(clippy::too_many_arguments)]
fn paint_shape<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    shape: usize,
    morph: f64,
    amount: f64,
    unipolar: bool,
    color: Color,
    marker: Option<f64>,
) {
    window(c, theme, r);
    let inner = r.inset(-5.0);
    let mid = inner.center().y;
    c.stroke_with(
        Line::new(pt(inner.x0, mid), pt(inner.x1, mid)),
        0.5,
        theme.section_line,
    );
    let y_of = |v: f64| {
        let v = if unipolar { 0.5 * (v + 1.0) } else { v } * amount;
        mid - v * inner.height() / 2.0
    };
    let steps = (inner.width() as usize).max(8);
    let mut path = BezPath::new();
    for i in 0..=steps {
        let p = i as f64 / steps as f64;
        // The last sample sits just below 1 so a saw does not draw its flyback.
        let q = if i == steps { p - 1e-9 } else { p };
        let v = shape_value(shape, q, morph, PREVIEW_RAND);
        let pt_ = pt(inner.x0 + p * inner.width(), y_of(v));
        if i == 0 {
            path.move_to(pt_);
        } else {
            path.line_to(pt_);
        }
    }
    c.stroke_with(path, 1.4, color);
    if let Some(p) = marker {
        let v = shape_value(shape, p, morph, PREVIEW_RAND);
        let at = pt(inner.x0 + p.clamp(0.0, 1.0) * inner.width(), y_of(v));
        c.stroke_with(
            Line::new(pt(at.x, inner.y0), pt(at.x, inner.y1)),
            0.6,
            color.with_alpha(0.35),
        );
        c.fill_with(Circle::new(at, 2.6), theme.text);
    }
}

fn paint_scope<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    scope: Option<&Scope>,
    accent: Color,
) {
    window(c, theme, r);
    let inner = r.inset(-4.0);
    let mid = inner.center().y;
    for v in [-1.0, 0.0, 1.0] {
        let y = mid - v * inner.height() / 2.0;
        c.stroke_with(
            Line::new(pt(inner.x0, y), pt(inner.x1, y)),
            0.5,
            theme
                .section_line
                .with_alpha(if v == 0.0 { 0.9 } else { 0.4 }),
        );
    }
    let colors = [accent, LFO2_COLOR, ENV_COLOR];
    for (i, name) in ["LFO 1", "LFO 2", "ENV"].into_iter().enumerate() {
        c.text(&TextRun::new(
            name,
            pt(inner.x0 + 4.0 + i as f64 * 38.0, inner.y0 + 8.0),
            8.0,
            colors[i],
        ));
    }
    let Some(scope) = scope.filter(|s| !s.is_empty()) else {
        c.text(
            &TextRun::new(
                "waiting for audio",
                pt(inner.center().x, mid + 3.0),
                9.0,
                theme.text_muted,
            )
            .align(Align::Center),
        );
        return;
    };
    // One point per pixel, the newest at the right edge.
    let n = scope.len().min(inner.width().floor() as usize).max(2);
    let px = inner.width() / (n - 1) as f64;
    for lane in (0..3).rev() {
        let mut path = BezPath::new();
        for k in 0..n {
            let age = n - 1 - k;
            let Some(v) = scope.point(lane, age) else {
                continue;
            };
            let v = if lane == 2 { v * 2.0 - 1.0 } else { v };
            let p = pt(
                inner.x0 + k as f64 * px,
                mid - f64::from(v.clamp(-1.0, 1.0)) * inner.height() / 2.0,
            );
            if path.elements().is_empty() {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        c.stroke_with(path, 1.3, mix(colors[lane], theme.lcd, 0.1));
    }
    if scope.spacing_ms() > 0.0 {
        let secs = n as f32 * scope.spacing_ms() / 1000.0;
        c.text(
            &TextRun::new(
                format!("{secs:.1} s"),
                pt(inner.x1 - 4.0, inner.y1 - 3.0),
                7.5,
                theme.text_muted,
            )
            .align(Align::End),
        );
    }
}
