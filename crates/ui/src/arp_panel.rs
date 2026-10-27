//! The arpeggiator's rack panel: six mode buttons with direction glyphs,
//! a rate selector, an octave stepper, knobs and toggles for the other params, and a live
//! display of the held keys and the next notes.
//!
//! ```text
//! MODE                                     SETTINGS
//! [UP][DOWN][UP-DN][RANDOM][AS PLAYED][CHORD]    Gate  Swing  Fixed  Seed   Octaves
//! RATE                                           Hold  Run    Vel
//! [1/4][1/8][1/8T][1/16][1/16T][1/32][1/32T]
//! +--------------------------------------+
//! | HELD 3        next: C4 E4 G4 C5 ...  |
//! +--------------------------------------+
//! ```
//!
//! `mode`, `rate` and `octaves` are drawn and handled by the panel itself (they are still the
//! device's params: every button is one `SetParam`); the other seven params are ordinary
//! controls. The display shows the meter values the device publishes: the number of held keys
//! and the next eight keys it will play (0 = none), see [`arp_display`].

use crate::autolayout::{CONTENT_X, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_panel::{PlayerInfo, back_layout, place_controls};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use crate::zone_edit::note_name;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Circle, Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the arpeggiator (it has its own panel, [`arp_layout`]).
pub const ARP_TYPE: &str = "atomik.arpeggiator";
/// Height of the arpeggiator panel in rack units.
pub const ARP_UNITS: u8 = 2;
/// The mode buttons' labels, in `mode` param order.
pub const ARP_MODES: [&str; 6] = ["UP", "DOWN", "UP-DN", "RANDOM", "AS PLAYED", "CHORD"];
/// The rate buttons' labels, in `rate` param order.
pub const ARP_RATES: [&str; 7] = ["1/4", "1/8", "1/8T", "1/16", "1/16T", "1/32", "1/32T"];
/// Number of notes the device publishes after the held count.
pub const NEXT_NOTES: usize = 8;
const MODE_W: f64 = 48.0;
const GAP: f64 = 2.0;
const LEFT_W: f64 = 302.0;
const KNOBS_X: f64 = CONTENT_X.0 + LEFT_W + 8.0;
const COL_W: f64 = 63.0;
/// The controls, in cell order (five per row): the empty key is the octave stepper's cell.
const ARP_CONTROLS: [&str; 8] = [
    "gate",
    "swing",
    "velocity.fixed",
    "random.seed",
    "",
    "hold",
    "run",
    "velocity.mode",
];

/// Geometry of the arpeggiator panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArpPanel {
    /// Title strip of the mode row.
    pub mode_title: Rect,
    /// Title strip of the rate row.
    pub rate_title: Rect,
    /// The six mode buttons.
    pub modes: [Rect; 6],
    /// The seven rate buttons.
    pub rates: [Rect; 7],
    /// The octave stepper's minus button.
    pub octave_dec: Rect,
    /// The octave stepper's value window.
    pub octave_value: Rect,
    /// The octave stepper's plus button.
    pub octave_inc: Rect,
    /// The octave stepper's label cell (below the buttons).
    pub octave_label: Rect,
    /// The live display of the held keys and the next notes.
    pub display: Rect,
}

/// The panel of `atomik.arpeggiator` (two rack units, see the module documentation); the back is
/// the generic socket layout.
pub fn arp_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let top = MARGIN_TOP + TITLE_H;
    let x0 = CONTENT_X.0;
    let modes = std::array::from_fn(|i| {
        let x = x0 + i as f64 * (MODE_W + GAP);
        Rect::new(x, top + 2.0, x + MODE_W, top + 32.0)
    });
    let rate_title_y = top + 34.0;
    let rate_y = rate_title_y + TITLE_H + 2.0;
    let rates = std::array::from_fn(|i| {
        let x = x0 + i as f64 * (40.0 + GAP);
        Rect::new(x, rate_y, x + 40.0, rate_y + 18.0)
    });
    // The octave stepper takes the fifth cell of the first control row.
    let cx = KNOBS_X + 4.0 * COL_W + COL_W / 2.0;
    let stepper_y = top + 14.0;
    let panel = ArpPanel {
        mode_title: Rect::new(x0, MARGIN_TOP, x0 + LEFT_W, MARGIN_TOP + TITLE_H),
        rate_title: Rect::new(x0, rate_title_y, x0 + LEFT_W, rate_title_y + TITLE_H),
        modes,
        rates,
        octave_dec: Rect::new(cx - 28.0, stepper_y, cx - 11.0, stepper_y + 22.0),
        octave_value: Rect::new(cx - 9.0, stepper_y, cx + 9.0, stepper_y + 22.0),
        octave_inc: Rect::new(cx + 11.0, stepper_y, cx + 28.0, stepper_y + 22.0),
        octave_label: Rect::new(cx - 30.0, top + 44.0, cx + 30.0, top + 58.0),
        display: Rect::new(x0, rate_y + 24.0, x0 + LEFT_W, rate_y + 24.0 + 56.0),
    };
    let front = place_controls(desc, &ARP_CONTROLS, KNOBS_X, COL_W, 5, top);
    let front_sections = vec![
        Section {
            label: "MODE".to_string(),
            rect: panel.mode_title,
        },
        Section {
            label: "RATE".to_string(),
            rect: panel.rate_title,
        },
        Section {
            label: "SETTINGS".to_string(),
            rect: Rect::new(
                KNOBS_X,
                MARGIN_TOP,
                KNOBS_X + 5.0 * COL_W,
                MARGIN_TOP + TITLE_H,
            ),
        },
    ];
    let (units, back, back_sections) = back_layout(desc, ARP_UNITS);
    PanelSpec {
        height_units: units,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
        seq: None,
        drum_gen: None,
        arp: Some(panel),
        key_chord: None,
        modsrc: None,
        wavetable: None,
        fm: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

/// The held-key count and the next notes from the published meter values
/// `[held, next 1..8]` (zeros and values outside 1..=127 are no note); `None` before the first
/// feed.
pub fn arp_display(meters: &[f32]) -> Option<(u8, Vec<u8>)> {
    let (first, rest) = meters.split_first()?;
    let whole = |v: f32| {
        if v.is_finite() {
            v.round().max(0.0)
        } else {
            0.0
        }
    };
    let held = whole(*first).min(255.0) as u8;
    let notes = rest
        .iter()
        .take(NEXT_NOTES)
        .map(|v| whole(*v))
        .filter(|v| (1.0..=127.0).contains(v))
        .map(|v| v as u8)
        .collect();
    Some((held, notes))
}

/// The glyph of a mode: points in a unit box (x right, y up), and whether they are joined.
fn glyph(mode: usize) -> (&'static [(f64, f64)], bool) {
    match mode {
        0 => (&[(0.0, 0.0), (0.33, 0.33), (0.66, 0.66), (1.0, 1.0)], true),
        1 => (&[(0.0, 1.0), (0.33, 0.66), (0.66, 0.33), (1.0, 0.0)], true),
        2 => (
            &[(0.0, 0.0), (0.25, 0.5), (0.5, 1.0), (0.75, 0.5), (1.0, 0.0)],
            true,
        ),
        3 => (&[(0.0, 0.7), (0.3, 0.1), (0.6, 0.9), (1.0, 0.4)], false),
        4 => (&[(0.0, 0.2), (0.33, 0.9), (0.66, 0.5), (1.0, 1.0)], true),
        _ => (&[(0.5, 0.0), (0.5, 0.5), (0.5, 1.0)], false),
    }
}

fn param(device: &RackDevice, desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key)
        .map_or(0.0, |p| current_value(device, p))
}

/// Draws the arpeggiator panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &ArpPanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    info: &PlayerInfo<'_>,
    accent: Color,
) {
    let mode = param(device, desc, "mode").round().max(0.0) as usize;
    let rate = param(device, desc, "rate").round().max(0.0) as usize;
    let octaves = param(device, desc, "octaves").round().clamp(1.0, 4.0) as u8;
    let lit = |c: &mut C, r: Rect, on: bool| {
        let face = if on {
            mix(theme.button, accent, 0.4)
        } else {
            theme.button
        };
        c.fill_with(RoundedRect::from_rect(r, 3.0), face);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 3.5),
            if on { 2.0 } else { 1.0 },
            if on { accent } else { theme.bevel_dark },
        );
    };
    for (i, r) in panel.modes.iter().enumerate() {
        let on = i == mode;
        lit(c, *r, on);
        let col = if on { accent } else { theme.text_muted };
        let (pts, joined) = glyph(i);
        let (gw, gh) = (20.0, 10.0);
        let gx = r.center().x - gw / 2.0;
        let gy = r.y0 + 5.0;
        let at = |p: &(f64, f64)| pt(gx + p.0 * gw, gy + (1.0 - p.1) * gh);
        if joined {
            let mut path = BezPath::new();
            for (k, p) in pts.iter().enumerate() {
                if k == 0 {
                    path.move_to(at(p));
                } else {
                    path.line_to(at(p));
                }
            }
            c.stroke_with(path, 1.2, col);
        }
        for p in pts {
            c.fill_with(Circle::new(at(p), 1.5), col);
        }
        c.text(
            &TextRun::new(
                ARP_MODES[i],
                pt(r.center().x, r.y1 - 5.0),
                if i == 4 { 7.0 } else { 8.0 },
                if on { theme.text } else { theme.text_muted },
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    for (i, r) in panel.rates.iter().enumerate() {
        let on = i == rate;
        lit(c, *r, on);
        c.text(
            &TextRun::new(
                ARP_RATES[i],
                pt(r.center().x, r.center().y + 3.2),
                8.5,
                if on { theme.text } else { theme.text_muted },
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    // Octave stepper.
    for (r, label, enabled) in [
        (panel.octave_dec, "-", octaves > 1),
        (panel.octave_inc, "+", octaves < 4),
    ] {
        lit(c, r, false);
        let dim = if enabled { 1.0 } else { 0.35 };
        c.text(
            &TextRun::new(
                label,
                pt(r.center().x, r.center().y + 4.0),
                12.0,
                theme.text.with_alpha(dim),
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    c.fill_with(RoundedRect::from_rect(panel.octave_value, 3.0), theme.lcd);
    c.text(
        &TextRun::new(
            octaves.to_string(),
            pt(
                panel.octave_value.center().x,
                panel.octave_value.center().y + 4.0,
            ),
            11.0,
            accent,
        )
        .align(Align::Center)
        .weight(700),
    );
    c.text(
        &TextRun::new(
            "Octaves",
            pt(panel.octave_label.center().x, panel.octave_label.y1 - 2.0),
            theme.label_size,
            theme.text_muted,
        )
        .align(Align::Center),
    );
    paint_display(c, theme, panel.display, info.meters, accent);
}

fn paint_display<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    meters: &[f32],
    accent: Color,
) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        Color::WHITE.with_alpha(0.07),
    );
    let held = arp_display(meters).filter(|(held, _)| *held > 0);
    let Some((held, notes)) = held else {
        c.text(&TextRun::new(
            "No keys held",
            pt(r.x0 + 8.0, r.y0 + 15.0),
            10.0,
            theme.text_muted,
        ));
        return;
    };
    c.text(
        &TextRun::new(
            format!("HELD {held}"),
            pt(r.x0 + 8.0, r.y0 + 15.0),
            10.0,
            accent,
        )
        .weight(700),
    );
    c.text(
        &TextRun::new("next", pt(r.x1 - 8.0, r.y0 + 14.0), 8.0, theme.text_muted).align(Align::End),
    );
    let cells = NEXT_NOTES as f64;
    let cw = (r.width() - 16.0) / cells;
    let area = Rect::new(r.x0 + 8.0, r.y0 + 22.0, r.x1 - 8.0, r.y1 - 14.0);
    let (lo, hi) = notes
        .iter()
        .fold((127u8, 0u8), |(lo, hi), k| (lo.min(*k), hi.max(*k)));
    let span = f64::from(hi.saturating_sub(lo)).max(1.0);
    for (i, key) in notes.iter().enumerate() {
        let x = area.x0 + i as f64 * cw;
        let f = if hi == lo {
            0.5
        } else {
            f64::from(*key - lo) / span
        };
        let y = area.y1 - f * area.height();
        c.stroke_with(
            Line::new(pt(x + 3.0, y), pt(x + cw - 3.0, y)),
            2.5,
            accent.with_alpha(if i == 0 { 1.0 } else { 0.7 }),
        );
        c.text(
            &TextRun::new(
                fit_label(&note_name(*key), cw, 8.0),
                pt(x + cw / 2.0, r.y1 - 4.0),
                8.0,
                if i == 0 { theme.text } else { theme.text_muted },
            )
            .align(Align::Center),
        );
    }
}
