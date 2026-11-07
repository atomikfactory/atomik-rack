//! The mix channel panel: the compact rack panel of a console channel or output bus. The master
//! section panel ([`crate::master_panel`]) shares its top row (fader, meter, the two insert
//! buttons and the output display) and adds the shared send rows under it.
//!
//! One rack unit. The front carries what a strip needs while mixing, and every control drives
//! the same param or command as the console strip, so both views stay in step:
//!
//! ```text
//! MIX          INSERT FX            LEVEL                          PAN   AUDIO OUTPUT
//! [MUTE][SOLO] [* BYPASS INSERT FX] ===o================ -3.2    (pan)  +----------------+
//!              [* SHOW INSERT FX]   L ||||||||||                         | Master Section |
//!                                   R ||||||||                           +----------------+
//! ```
//!
//! - MUTE, SOLO and PAN are the strip's `mute`, `solo` and `pan` params; the horizontal fader is
//!   its `fader` param (a drag along the track, a double-click resets it).
//! - BYPASS INSERT FX is one switch for the whole insert chain: it sets the `bypass` param of
//!   every insert effect (and the dynamics and EQ sections in use) in one undo step, the way the
//!   B button of the console strip does ([`atomik_model::bypass_inserts_commands`]).
//! - SHOW INSERT FX folds the insert area of the channel in the rack. The state is view state
//!   (see `RackView::inserts_shown`); the stacking of the insert devices under the panel builds
//!   on it.
//! - The meter shows the strip's output peaks, fed by the app from the console's meters.
//! - AUDIO OUTPUT says where the strip plays (the output display of the rack, derived from the
//!   cables by `console_links::displays_of`).
//!
//! The back carries four pairs of sockets and nothing else: the input (`in_l`/`in_r`, or the
//! summing `bus_l`/`bus_r` of a bus), the insert send TO DEVICE (`insert_send_l/r`), the insert
//! return FROM DEVICE (`insert_return_l/r`) and the output (`out_l`/`out_r`). The sends, the
//! parallel output, the key and CV inputs and the gain reduction output keep working in the
//! model; the rack panel does not draw them.

use crate::autolayout::{
    CONTENT_X, MARGIN_TOP, PlacedControl, PlacedSocket, SOCKET_CELL, Section, TITLE_H, cell_size,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::console_meter::{METER_FLOOR_DB, meter_norm};
use crate::paint::fit_label;
use crate::plugin_panel::led_button;
use crate::theme::{Theme, mix};
use atomik_device_sdk::{ControlKind, DeviceDescriptor, RACK_UNIT};
use vello::kurbo::{Line, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// Type id of the console channel.
pub const CHANNEL_TYPE: &str = atomik_model::console::CHANNEL_TYPE;
/// Type id of the output bus.
pub const BUS_TYPE: &str = atomik_model::console::BUS_TYPE;
/// Type id of the console master (the master section panel shares the top row).
pub const MASTER_TYPE: &str = crate::master_panel::MASTER_TYPE;
/// Height of the panel, in rack units.
pub const MIX_UNITS: u8 = 1;

/// The param keys of the controls the front places, in placement order.
pub const FRONT_PARAMS: [&str; 4] = ["mute", "solo", "fader", "pan"];
/// The socket pairs the back draws: section title, left key, right key. A bus takes the
/// summing input in place of the plain one.
pub const BACK_PAIRS: [(&str, [&str; 2], [&str; 2]); 4] = [
    ("INPUT", ["in_l", "in_r"], ["bus_l", "bus_r"]),
    (
        "INSERT FX TO DEVICE",
        ["insert_send_l", "insert_send_r"],
        ["insert_send_l", "insert_send_r"],
    ),
    (
        "INSERT FX FROM DEVICE",
        ["insert_return_l", "insert_return_r"],
        ["insert_return_l", "insert_return_r"],
    ),
    ("OUTPUT", ["out_l", "out_r"], ["out_l", "out_r"]),
];

/// Width of one back section (a pair of jack cells with room for its title).
const BACK_SECTION_W: f64 = 132.0;
/// Gap between back sections.
const BACK_GAP: f64 = 12.0;
/// Top of the controls, under the section titles.
const TOP: f64 = MARGIN_TOP + TITLE_H;

/// Whether `type_id` is drawn as a mix channel panel (a console channel or an output bus).
pub fn is_mix_type(type_id: &str) -> bool {
    type_id == CHANNEL_TYPE || type_id == BUS_TYPE
}

/// Whether `type_id` is drawn with the mix panel's top row (fader, meter, BYPASS INSERT FX and
/// SHOW INSERT FX) and has an insert area under it: a mix channel panel or the master section
/// panel.
pub fn has_mix_top(type_id: &str) -> bool {
    is_mix_type(type_id) || type_id == MASTER_TYPE
}

/// A button of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixButton {
    /// BYPASS INSERT FX: the whole insert chain at once.
    BypassInserts,
    /// SHOW INSERT FX: fold or unfold the insert area.
    ShowInserts,
}

/// Geometry of the panel's own parts (front, panel coordinates). The controls are ordinary
/// placed controls of the [`PanelSpec`](crate::autolayout::PanelSpec).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixPanel {
    /// BYPASS INSERT FX.
    pub bypass: Rect,
    /// SHOW INSERT FX.
    pub show: Rect,
    /// The fader: track, cap and the dB readout.
    pub fader: Rect,
    /// The fader's track and the cap's travel (left end, right end, y).
    pub travel: (f64, f64, f64),
    /// The left and right meter bars.
    pub meter: [Rect; 2],
    /// The AUDIO OUTPUT display.
    pub output: Rect,
}

impl MixPanel {
    /// The rectangle of a button.
    pub fn button(&self, b: MixButton) -> Rect {
        match b {
            MixButton::BypassInserts => self.bypass,
            MixButton::ShowInserts => self.show,
        }
    }
}

/// The panel geometry: the same for every channel and bus.
pub fn geometry() -> MixPanel {
    let x = CONTENT_X.0;
    let fader = Rect::new(x + 236.0, TOP + 2.0, x + 416.0, TOP + 30.0);
    let meters_x = (fader.x0, fader.x1);
    MixPanel {
        bypass: Rect::new(x + 104.0, TOP + 2.0, x + 224.0, TOP + 24.0),
        show: Rect::new(x + 104.0, TOP + 28.0, x + 224.0, TOP + 50.0),
        fader,
        travel: (fader.x0 + 6.0, fader.x1 - 40.0, fader.y0 + 12.0),
        meter: [
            Rect::new(meters_x.0, TOP + 34.0, meters_x.1, TOP + 41.5),
            Rect::new(meters_x.0, TOP + 43.5, meters_x.1, TOP + 51.0),
        ],
        output: Rect::new(x + 488.0, TOP + 2.0, CONTENT_X.1, TOP + 26.0),
    }
}

/// The titled sections of the front, left to right.
fn front_sections() -> Vec<Section> {
    let x = CONTENT_X.0;
    let at = |label: &str, x0: f64, x1: f64| Section {
        label: label.to_string(),
        rect: Rect::new(x0, MARGIN_TOP, x1, MARGIN_TOP + TITLE_H),
    };
    vec![
        at("MIX", x, x + 92.0),
        at("INSERT FX", x + 104.0, x + 224.0),
        at("LEVEL", x + 236.0, x + 416.0),
        at("PAN", x + 428.0, x + 476.0),
        at("AUDIO OUTPUT", x + 488.0, CONTENT_X.1),
    ]
}

/// The controls of the front: MUTE and SOLO toggles, the fader (placed as a horizontal track
/// of the [`geometry`]) and the PAN knob.
fn front_controls(desc: &DeviceDescriptor) -> Vec<PlacedControl> {
    let g = geometry();
    let x = CONTENT_X.0;
    let knob_h = cell_size(ControlKind::Knob).1.unwrap_or(58.0);
    let mut out = Vec::new();
    for key in FRONT_PARAMS {
        let Some(param) = desc.params.iter().position(|p| p.key == key) else {
            continue;
        };
        let (kind, rect) = match key {
            "mute" => (
                ControlKind::Toggle,
                Rect::new(x, TOP + 2.0, x + 44.0, TOP + 30.0),
            ),
            "solo" => (
                ControlKind::Toggle,
                Rect::new(x + 48.0, TOP + 2.0, x + 92.0, TOP + 30.0),
            ),
            "fader" => (ControlKind::Fader, g.fader),
            _ => (
                ControlKind::Knob,
                Rect::new(x + 428.0, TOP, x + 476.0, TOP + knob_h),
            ),
        };
        out.push(PlacedControl { param, kind, rect });
    }
    out
}

/// The back: the four socket pairs of [`BACK_PAIRS`], each under its own title.
fn back_layout(desc: &DeviceDescriptor) -> (Vec<PlacedSocket>, Vec<Section>) {
    let summing = desc.type_id == BUS_TYPE;
    let mut cells = Vec::new();
    let mut sections = Vec::new();
    for (i, (title, plain, bus)) in BACK_PAIRS.iter().enumerate() {
        let keys = if summing { bus } else { plain };
        let x0 = CONTENT_X.0 + i as f64 * (BACK_SECTION_W + BACK_GAP);
        sections.push(Section {
            label: (*title).to_string(),
            rect: Rect::new(x0, MARGIN_TOP, x0 + BACK_SECTION_W, MARGIN_TOP + TITLE_H),
        });
        for (side, key) in keys.iter().enumerate() {
            let Some(socket) = desc.sockets.iter().position(|s| s.key == *key) else {
                continue;
            };
            let cx = x0 + 16.0 + side as f64 * SOCKET_CELL.0;
            cells.push(PlacedSocket {
                socket,
                rect: Rect::new(cx, TOP + 2.0, cx + SOCKET_CELL.0, TOP + 2.0 + SOCKET_CELL.1),
            });
        }
    }
    (cells, sections)
}

/// The panel of a console channel or bus (see the module documentation).
pub fn mix_layout(desc: &DeviceDescriptor) -> crate::autolayout::PanelSpec {
    let (back, back_sections) = back_layout(desc);
    crate::autolayout::PanelSpec {
        height_units: MIX_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: None,
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
        front: front_controls(desc),
        front_sections: front_sections(),
        back,
        back_sections,
    }
}

/// What the panel shows that the device's params do not hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MixData {
    /// The insert chain is bypassed (`Some(true)`), plays (`Some(false)`), or has nothing to
    /// bypass (`None`).
    pub bypassed: Option<bool>,
    /// The insert area is shown (SHOW INSERT FX is on).
    pub shown: bool,
    /// Where the strip plays, as the AUDIO OUTPUT display words it, and whether the output was
    /// cabled by hand somewhere unusual.
    pub output: Option<(String, bool)>,
    /// The device whose name the plate shows and a rename changes: the source instrument of the
    /// channel (as the console strip does), with its name. `None` for a bus, a channel without
    /// a source and a second channel of one source: the strip's own name.
    pub follows: Option<(atomik_model::DeviceKey, String)>,
}

/// What the painter reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MixInfo<'a> {
    /// The insert chain bypass state (see [`MixData::bypassed`]).
    pub bypassed: Option<bool>,
    /// The insert area is shown.
    pub shown: bool,
    /// The AUDIO OUTPUT text and its warning flag.
    pub output: Option<(&'a str, bool)>,
    /// The output peaks in dB (left, right), empty while no meter feed runs.
    pub meters: &'a [f32],
}

impl MixData {
    /// The painter's view of this data with the meter feed `meters`.
    pub fn info<'a>(&'a self, meters: &'a [f32]) -> MixInfo<'a> {
        MixInfo {
            bypassed: self.bypassed,
            shown: self.shown,
            output: self.output.as_ref().map(|(t, w)| (t.as_str(), *w)),
            meters,
        }
    }
}

/// The short label under a jack of the back (the cell is 50 px wide: the section title says
/// where the pair goes, so the label only tells the side), `None` for a socket the back does
/// not draw.
pub fn socket_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "in_l" => "In L",
        "in_r" => "In R",
        "bus_l" => "Bus L",
        "bus_r" => "Bus R",
        "insert_send_l" => "Send L",
        "insert_send_r" => "Send R",
        "insert_return_l" => "Return L",
        "insert_return_r" => "Return R",
        "out_l" => "Out L",
        "out_r" => "Out R",
        _ => return None,
    })
}

/// The words of the AUDIO OUTPUT display for a strip whose output label is `text` (the label of
/// `console_links`, e.g. `Output: Master`): the destination without the lead
/// word, and the Master Section by its full name.
pub fn output_words(text: &str) -> String {
    let rest = text.strip_prefix("Output: ").unwrap_or(text);
    if rest == "Master" {
        "Master Section".to_string()
    } else {
        rest.to_string()
    }
}

/// The fader position (0..1) under the pointer at `x`, along the track of the [`geometry`].
pub fn fader_at(x: f64) -> f64 {
    let (x0, x1, _) = geometry().travel;
    ((x - x0) / (x1 - x0)).clamp(0.0, 1.0)
}

/// The fader readout: the gain in dB of a fader position, `-inf` at the bottom.
pub fn fader_text(pos: f32) -> String {
    let db = atomik_dsp::fader_db(pos);
    if db.is_finite() && db > -90.0 {
        format!("{db:+.1}")
    } else {
        "-inf".to_string()
    }
}

/// The meter level shown for a peak in dB, in steps of half a dB (so a quiet strip stops
/// repainting).
pub fn quantise_meter(db: f32) -> f32 {
    if db.is_nan() {
        return METER_FLOOR_DB;
    }
    ((db.max(METER_FLOOR_DB) * 2.0).round() / 2.0).max(METER_FLOOR_DB)
}

/// Draws the horizontal fader of the panel for position `pos` (0..1).
pub(crate) fn paint_fader<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &MixPanel,
    pos: f32,
    accent: Color,
) {
    let (x0, x1, y) = g.travel;
    // The scale: a tick every eighth, the unity mark longer.
    let unity = x0 + (x1 - x0) * f64::from(atomik_dsp::FADER_UNITY_POS);
    for i in 0..=8 {
        let x = x0 + (x1 - x0) * f64::from(i) / 8.0;
        let h = if i % 4 == 0 { 5.0 } else { 3.0 };
        c.stroke_with(
            Line::new(pt(x, y + 7.0), pt(x, y + 7.0 + h)),
            1.0,
            theme.text_muted.with_alpha(0.5),
        );
    }
    c.stroke_with(
        Line::new(pt(unity, y + 6.0), pt(unity, y + 14.0)),
        1.0,
        theme.text_muted.with_alpha(0.8),
    );
    c.fill_with(
        RoundedRect::new(x0 - 2.0, y - 2.5, x1 + 2.0, y + 2.5, 2.5),
        theme.knob_track,
    );
    let x = x0 + (x1 - x0) * f64::from(pos.clamp(0.0, 1.0));
    c.stroke_with(Line::new(pt(x0, y), pt(x, y)), 2.0, accent);
    let cap = Rect::new(x - 5.5, y - 9.0, x + 5.5, y + 9.0);
    c.fill_with(
        RoundedRect::from_rect(cap + Vec2::new(0.0, 1.5), 2.0),
        Color::BLACK.with_alpha(0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(cap, 2.0),
        crate::paint::vgrad(cap.y0, cap.y1, theme.knob_top, theme.knob_bottom),
    );
    c.stroke_with(
        Line::new(pt(x, cap.y0 + 3.0), pt(x, cap.y1 - 3.0)),
        1.2,
        theme.knob_pointer,
    );
    c.text(
        &TextRun::new(
            fader_text(pos),
            pt(g.fader.x1 - 2.0, y + 3.0),
            theme.label_size,
            theme.text,
        )
        .align(Align::End),
    );
}

/// Draws one meter bar filled to the peak `db`.
fn paint_meter_bar<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, db: f32, accent: Color) {
    c.fill_with(RoundedRect::from_rect(r, 2.0), theme.lcd);
    let n = meter_norm(db);
    if n <= 0.0 {
        return;
    }
    let color = if db >= 0.0 {
        theme.clip
    } else if db >= -6.0 {
        theme.warning
    } else {
        mix(accent, Color::WHITE, 0.1)
    };
    let w = r.width() * n;
    c.fill_with(RoundedRect::new(r.x0, r.y0, r.x0 + w, r.y1, 2.0), color);
}

/// Draws the panel's own parts: the horizontal fader, the meter, the two buttons and the AUDIO
/// OUTPUT display. The MUTE, SOLO and PAN controls are drawn by the generic control painter.
pub(crate) fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    desc: &DeviceDescriptor,
    values: &dyn Fn(&str) -> f32,
    info: &MixInfo<'_>,
    accent: Color,
) {
    let g = geometry();
    let fader = desc.param_by_key("fader").map_or(0.0, |p| values(p.key));
    paint_fader(c, theme, &g, fader, accent);
    for (i, bar) in g.meter.iter().enumerate() {
        let db = info.meters.get(i).copied().unwrap_or(METER_FLOOR_DB);
        paint_meter_bar(c, theme, *bar, db, accent);
        c.text(
            &TextRun::new(
                if i == 0 { "L" } else { "R" },
                pt(bar.x0 - 6.0, bar.y1 - 0.5),
                6.5,
                theme.text_muted,
            )
            .align(Align::Center),
        );
    }
    led_button(
        c,
        theme,
        g.bypass,
        "BYPASS INSERT FX",
        Some(info.bypassed == Some(true)),
        true,
        theme.warning,
    );
    led_button(
        c,
        theme,
        g.show,
        "SHOW INSERT FX",
        Some(info.shown),
        true,
        accent,
    );
    c.fill_with(RoundedRect::from_rect(g.output, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(g.output.inset(-0.5), 3.5),
        1.0,
        theme.ink.with_alpha(0.07),
    );
    if let Some((text, warn)) = info.output {
        let color = if warn {
            theme.warning
        } else if text == "none" {
            theme.text_muted
        } else {
            mix(accent, Color::WHITE, 0.3)
        };
        c.text(
            &TextRun::new(
                fit_label(text, g.output.width() - 12.0, 9.0),
                pt(g.output.center().x, g.output.y0 + 15.5),
                9.0,
                color,
            )
            .weight(600)
            .align(Align::Center),
        );
    }
}

/// The height of the panel in logical units.
pub fn height() -> f64 {
    f64::from(MIX_UNITS) * f64::from(RACK_UNIT)
}
