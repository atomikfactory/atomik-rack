//! The master section panel: the compact rack panel of the console master.
//!
//! Four rack units. The top row is the mix channel panel's (see [`crate::mix_panel`]): the
//! master fader and its meter, BYPASS INSERT FX, SHOW INSERT FX and the output display. Under
//! it come the eight shared sends, one row each:
//!
//! ```text
//! SEND EFFECTS
//! 1  Reverb               ==o=============================  -3.0 dB   [EDIT]
//! 2  free                 ===========o====================   0.0 dB   [EDIT]
//! ```
//!
//! - the name is the effect on the send's return slot (`Console::returns`), or "free";
//! - the slider is the return level (`return{k}.level`), driven like the console's return knob;
//! - EDIT selects the effect in the rack and scrolls to it (nothing for a free slot).
//!
//! The bus compressor, the control room and the key input stay on the console view. The insert
//! effects of the master sit stacked under the panel, then the shared send effects, each send
//! as its own group labelled "Send N" (see the rack's insert area module).
//!
//! The back carries the 8 FX sends (`fx_send{k}_l/r`) and the 8 FX returns (`return{k}_l/r`),
//! the insert loop TO and FROM DEVICE, the master out and the control room out; the other
//! master sockets (summing inputs, the key and the CV input) are not drawn, they keep working.
//!
//! This module holds the document-derived data ([`master_data`]), the geometry of what is drawn
//! on top of the generated controls ([`return_row`], [`edit_rects`]) and the painters, so the
//! rack view, the painter and the tests share one set of rectangles.

use crate::autolayout::{
    CONTENT_X, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section, TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::mix_panel;
use crate::paint::{edit_button, fit_label};
use crate::theme::Theme;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_model::{DeviceKey, FX_SLOTS, RackDoc};
use vello::kurbo::{Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the console master.
pub const MASTER_TYPE: &str = crate::console_layout::MASTER_TYPE;
/// Height of the panel, in rack units.
pub const MASTER_UNITS: u8 = 4;
/// Choice params with at least this many entries open a pick list instead of cycling.
pub const PICK_LIST_MIN: usize = 16;
/// A factory-sound or algorithm choice opens a pick list from this many entries: stepping
/// through six or more of them one click at a time is slow, and the list shows what is there.
pub const NAMED_PICK_LIST_MIN: usize = 6;
/// The grain sampler's play mode and zone mode, which open a pick list from two entries.
pub const GRAIN_PICK_KEYS: [&str; 2] = ["voice.mode", "zone.mode"];
/// Label of an unused slot.
pub const FREE_LABEL: &str = "free";

/// Top of the controls, under the section titles.
const TOP: f64 = MARGIN_TOP + TITLE_H;
/// Top of the title of the send rows.
const SENDS_TITLE_Y: f64 = TOP + 60.0;
/// Top of the first send row.
const ROWS_TOP: f64 = SENDS_TITLE_Y + TITLE_H + 6.0;
/// Height of one send row.
pub const ROW_H: f64 = 24.0;
/// Width of a back section of four jack pairs.
const PAIRS_W: f64 = 424.0;
/// Left of the single-pair back sections.
const SINGLE_X: f64 = 448.0;
/// Height of a back row (title and jack cells).
const BACK_ROW_H: f64 = TITLE_H + SOCKET_CELL.1 + 16.0;

/// Whether a click on `p` opens a pick list: a long choice such as the control-room source, or
/// a `sound`, `algorithm` or wavetable `table` choice of [`NAMED_PICK_LIST_MIN`] entries or
/// more, or the grain sampler's `voice.mode` and `zone.mode` ([`GRAIN_PICK_KEYS`]). A right-click still steps back through the choices.
pub fn is_pick_list(p: &ParamDescriptor) -> bool {
    match p.unit {
        atomik_core::Unit::Choice(names) => {
            names.len() >= PICK_LIST_MIN
                || (names.len() >= NAMED_PICK_LIST_MIN
                    && matches!(p.key, "sound" | "algorithm" | "osc1.table" | "osc2.table"))
                // The grain sampler's play mode and zone mode name what they do, so they open
                // a list too, whatever their length.
                || GRAIN_PICK_KEYS.contains(&p.key)
        }
        _ => false,
    }
}

/// What the master panel shows that the descriptor does not hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MasterData {
    /// The effect on each return slot: its device key and display name.
    pub returns: Vec<Option<(DeviceKey, String)>>,
}

/// What the painter reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MasterInfo<'a> {
    /// The effect on each return slot (`None` for a free slot or no data).
    pub returns: &'a [Option<(DeviceKey, String)>],
}

impl MasterData {
    /// The painter's view of this data.
    pub fn info(&self) -> MasterInfo<'_> {
        MasterInfo {
            returns: &self.returns,
        }
    }
}

/// Reads the master panel data from `doc`: `console.returns` resolved to device names.
pub fn master_data(doc: &RackDoc) -> MasterData {
    let name_of = |key: &DeviceKey| {
        doc.devices
            .iter()
            .find(|d| d.key == *key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    };
    let returns = doc
        .console
        .returns
        .iter()
        .map(|slot| slot.as_ref().map(|k| (k.clone(), name_of(k))))
        .collect();
    MasterData { returns }
}

/// The 0-based slot of a `return{n}.*` or `send{n}.level` param key.
pub fn slot_of_param(key: &str) -> Option<usize> {
    let rest = key
        .strip_prefix("return")
        .or_else(|| key.strip_prefix("send"))?;
    let n: usize = rest.split('.').next()?.parse().ok()?;
    (1..=FX_SLOTS).contains(&n).then(|| n - 1)
}

/// The slot whose return level `key` is (`return{n}.level`), 0-based.
pub fn return_level_slot(key: &str) -> Option<usize> {
    key.starts_with("return")
        .then(|| key.strip_suffix(".level").and(slot_of_param(key)))
        .flatten()
}

/// The rectangles of the send row of slot `slot`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReturnRow {
    /// The whole row.
    pub row: Rect,
    /// The number and the effect name.
    pub name: Rect,
    /// The return level control (track and readout).
    pub slider: Rect,
    /// The slider's track: left end, right end, y.
    pub travel: (f64, f64, f64),
    /// The EDIT button.
    pub edit: Rect,
}

/// The row of send `slot` (0-based) in panel coordinates.
pub fn return_row(slot: usize) -> ReturnRow {
    let x = CONTENT_X.0;
    let y0 = ROWS_TOP + slot as f64 * ROW_H;
    let row = Rect::new(x, y0, CONTENT_X.1, y0 + ROW_H);
    let mid = y0 + ROW_H / 2.0;
    ReturnRow {
        row,
        name: Rect::new(x, y0, x + 200.0, y0 + ROW_H),
        slider: Rect::new(x + 212.0, y0 + 2.0, x + 548.0, y0 + ROW_H - 2.0),
        travel: (x + 220.0, x + 480.0, mid),
        edit: Rect::new(x + 572.0, y0 + 3.0, x + 618.0, y0 + ROW_H - 3.0),
    }
}

/// The EDIT button of each return, as `(slot, rectangle)` in panel coordinates.
pub fn edit_rects() -> Vec<(usize, Rect)> {
    (0..FX_SLOTS).map(|s| (s, return_row(s).edit)).collect()
}

/// The titled sections of the front, left to right (the top row is the mix panel's).
fn front_sections() -> Vec<Section> {
    let x = CONTENT_X.0;
    let at = |label: &str, x0: f64, x1: f64, y0: f64| Section {
        label: label.to_string(),
        rect: Rect::new(x0, y0, x1, y0 + TITLE_H),
    };
    vec![
        at("MASTER", x, x + 92.0, MARGIN_TOP),
        at("INSERT FX", x + 104.0, x + 224.0, MARGIN_TOP),
        at("LEVEL", x + 236.0, x + 416.0, MARGIN_TOP),
        at("AUDIO OUTPUT", x + 488.0, CONTENT_X.1, MARGIN_TOP),
        at("SEND EFFECTS", x, CONTENT_X.1, SENDS_TITLE_Y),
    ]
}

/// The controls of the front: the master fader (the horizontal track of the mix panel) and the
/// eight return levels (a horizontal track per row).
fn front_controls(desc: &DeviceDescriptor) -> Vec<PlacedControl> {
    let mut out = Vec::new();
    if let Some(param) = desc.params.iter().position(|p| p.key == "fader") {
        out.push(PlacedControl {
            param,
            kind: ControlKind::Fader,
            rect: mix_panel::geometry().fader,
        });
    }
    for slot in 0..FX_SLOTS {
        let key = format!("return{}.level", slot + 1);
        if let Some(param) = desc.params.iter().position(|p| p.key == key) {
            out.push(PlacedControl {
                param,
                kind: ControlKind::Fader,
                rect: return_row(slot).slider,
            });
        }
    }
    out
}

/// The back sockets: the keys of the pairs of each section, in drawing order. `(title, row,
/// column x, [left key, right key] per pair)`.
fn back_sections_keys() -> Vec<(String, usize, f64, Vec<[String; 2]>)> {
    let pairs = |prefix: &str, range: std::ops::RangeInclusive<usize>| -> Vec<[String; 2]> {
        range
            .map(|k| [format!("{prefix}{k}_l"), format!("{prefix}{k}_r")])
            .collect()
    };
    let one = |l: &str, r: &str| vec![[l.to_string(), r.to_string()]];
    let x = CONTENT_X.0;
    vec![
        ("FX SEND 1-4".into(), 0, x, pairs("fx_send", 1..=4)),
        ("FX SEND 5-8".into(), 1, x, pairs("fx_send", 5..=8)),
        ("FX RETURN 1-4".into(), 2, x, pairs("return", 1..=4)),
        ("FX RETURN 5-8".into(), 3, x, pairs("return", 5..=8)),
        (
            "INSERT FX TO DEVICE".into(),
            0,
            x + SINGLE_X,
            one("insert_send_l", "insert_send_r"),
        ),
        (
            "INSERT FX FROM DEVICE".into(),
            1,
            x + SINGLE_X,
            one("insert_return_l", "insert_return_r"),
        ),
        (
            "MASTER OUT".into(),
            2,
            x + SINGLE_X,
            one("main_l", "main_r"),
        ),
        (
            "CONTROL ROOM OUT".into(),
            3,
            x + SINGLE_X,
            one("monitor_l", "monitor_r"),
        ),
    ]
}

/// The back: the sockets and sections of [`back_sections_keys`].
fn back_layout(desc: &DeviceDescriptor) -> (Vec<PlacedSocket>, Vec<Section>) {
    let mut cells = Vec::new();
    let mut sections = Vec::new();
    for (title, row, x0, pairs) in back_sections_keys() {
        let y0 = MARGIN_TOP + row as f64 * BACK_ROW_H;
        let width = if pairs.len() > 1 { PAIRS_W } else { 124.0 };
        sections.push(Section {
            label: title,
            rect: Rect::new(x0, y0, x0 + width, y0 + TITLE_H),
        });
        for (n, keys) in pairs.iter().enumerate() {
            for (side, key) in keys.iter().enumerate() {
                let Some(socket) = desc.sockets.iter().position(|s| s.key == key.as_str()) else {
                    continue;
                };
                let cx = x0 + 6.0 + (n * 2 + side) as f64 * SOCKET_CELL.0 + n as f64 * 8.0;
                cells.push(PlacedSocket {
                    socket,
                    rect: Rect::new(
                        cx,
                        y0 + TITLE_H + 2.0,
                        cx + SOCKET_CELL.0,
                        y0 + TITLE_H + 2.0 + SOCKET_CELL.1,
                    ),
                });
            }
        }
    }
    (cells, sections)
}

/// The panel of the console master (see the module documentation).
pub fn master_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let (back, back_sections) = back_layout(desc);
    PanelSpec {
        height_units: MASTER_UNITS,
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

/// The label under a jack of the back (the cell is 50 px wide), `None` for a socket the back
/// does not draw.
pub fn socket_label(key: &str) -> Option<String> {
    let numbered = |prefix: &str, word: &str| -> Option<String> {
        let rest = key.strip_prefix(prefix)?;
        let (n, side) = rest.split_once('_')?;
        let side = match side {
            "l" => "L",
            "r" => "R",
            _ => return None,
        };
        n.parse::<usize>()
            .ok()
            .filter(|n| (1..=FX_SLOTS).contains(n))
            .map(|n| format!("{word} {n} {side}"))
    };
    numbered("fx_send", "Send")
        .or_else(|| numbered("return", "Ret"))
        .or_else(|| {
            Some(
                match key {
                    "insert_send_l" => "Send L",
                    "insert_send_r" => "Send R",
                    "insert_return_l" => "Return L",
                    "insert_return_r" => "Return R",
                    "main_l" => "Main L",
                    "main_r" => "Main R",
                    "monitor_l" => "Mon L",
                    "monitor_r" => "Mon R",
                    _ => return None,
                }
                .to_string(),
            )
        })
}

/// The tooltip of the return level of `slot`: the effect it brings back.
pub fn level_tip(info: &MasterInfo<'_>, slot: usize) -> String {
    match info.returns.get(slot).and_then(Option::as_ref) {
        Some((_, name)) => format!("Return level of send {}: {name}", slot + 1),
        None => format!("Return level of send {} (no effect on it yet)", slot + 1),
    }
}

/// Draws one return level slider for the normalised position `pos` (0..1).
fn paint_slider<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    row: &ReturnRow,
    pos: f32,
    readout: &str,
    used: bool,
    accent: Color,
) {
    let (x0, x1, y) = row.travel;
    let alpha = if used { 1.0 } else { 0.55 };
    c.fill_with(
        RoundedRect::new(x0 - 2.0, y - 2.0, x1 + 2.0, y + 2.0, 2.0),
        theme.knob_track,
    );
    let x = x0 + (x1 - x0) * f64::from(pos.clamp(0.0, 1.0));
    c.stroke_with(
        Line::new(pt(x0, y), pt(x, y)),
        2.0,
        accent.with_alpha(alpha),
    );
    let cap = Rect::new(x - 4.0, y - 7.0, x + 4.0, y + 7.0);
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
            readout,
            pt(row.slider.x1 - 2.0, y + 3.5),
            theme.label_size + 1.0,
            if used { theme.text } else { theme.text_muted },
        )
        .align(Align::End),
    );
}

/// Paints the send rows: the number and name of the effect, the return level slider and the
/// EDIT button of each. `value` reads a param of the master by key.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    desc: &DeviceDescriptor,
    values: &dyn Fn(&str) -> f32,
    info: &MasterInfo<'_>,
    accent: Color,
) {
    for slot in 0..FX_SLOTS {
        let row = return_row(slot);
        let used = info.returns.get(slot).is_some_and(Option::is_some);
        if slot % 2 == 1 {
            c.fill_with(
                RoundedRect::from_rect(row.row.inflate(4.0, 0.0), 3.0),
                theme.ink.with_alpha(0.04),
            );
        }
        let size = theme.label_size + 2.0;
        let mid = row.row.center().y + 3.5;
        c.text(
            &TextRun::new(
                (slot + 1).to_string(),
                pt(row.name.x0 + 4.0, mid),
                size,
                accent,
            )
            .weight(700),
        );
        let (name, color) = match info.returns.get(slot).and_then(Option::as_ref) {
            Some((_, name)) => (name.as_str(), theme.text),
            None => (FREE_LABEL, theme.text_muted),
        };
        c.text(
            &TextRun::new(
                fit_label(name, row.name.width() - 28.0, size),
                pt(row.name.x0 + 22.0, mid),
                size,
                color,
            )
            .weight(600),
        );
        if let Some(p) = desc.param_by_key(&format!("return{}.level", slot + 1)) {
            let v = values(p.key);
            paint_slider(
                c,
                theme,
                &row,
                p.to_normalized(v),
                &crate::widget::format_value(p, v),
                used,
                accent,
            );
        }
        edit_button(c, theme, row.edit, "EDIT", used);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::{RackDevice, empty_doc};

    fn dev(key: &str, name: &str) -> RackDevice {
        RackDevice {
            key: DeviceKey(key.into()),
            type_id: "x".into(),
            name: name.into(),
            params: Default::default(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            curves: None,
            drums: None,
            inner: None,
            plugin: None,
            column: 0,
            patch_name: String::new(),
            midi_out: None,
            mods: Vec::new(),
        }
    }

    #[test]
    fn the_return_names_come_from_the_console_returns() {
        let mut d = empty_doc();
        d.devices.push(dev("verb", "Reverb"));
        d.devices.push(dev("echo", "Delay 2"));
        d.console.returns[0] = Some(DeviceKey("verb".into()));
        d.console.returns[2] = Some(DeviceKey("echo".into()));
        let data = master_data(&d);
        assert_eq!(data.returns.len(), FX_SLOTS);
        assert_eq!(data.returns[0].as_ref().unwrap().1, "Reverb");
        assert!(data.returns[1].is_none());
        assert_eq!(data.returns[2].as_ref().unwrap().1, "Delay 2");
        let tip = level_tip(&data.info(), 2);
        assert!(tip.contains("Delay 2") && tip.contains('3'), "{tip}");
        assert!(level_tip(&data.info(), 1).contains("no effect"));
    }

    #[test]
    fn a_dangling_return_shows_its_key() {
        let mut d = empty_doc();
        d.console.returns[0] = Some(DeviceKey("gone".into()));
        let data = master_data(&d);
        assert_eq!(data.returns[0].as_ref().unwrap().1, "gone");
    }

    #[test]
    fn slots_come_from_param_keys() {
        assert_eq!(slot_of_param("return1.level"), Some(0));
        assert_eq!(slot_of_param("send8.level"), Some(7));
        assert_eq!(slot_of_param("send9.level"), None);
        assert_eq!(slot_of_param("fader"), None);
        assert_eq!(return_level_slot("return3.level"), Some(2));
        assert_eq!(return_level_slot("return3.mute"), None);
        assert_eq!(return_level_slot("send3.level"), None);
    }

    #[test]
    fn the_rows_stack_without_overlap_inside_the_panel() {
        let h = f64::from(MASTER_UNITS) * f64::from(atomik_device_sdk::RACK_UNIT);
        let mut last = 0.0;
        for slot in 0..FX_SLOTS {
            let r = return_row(slot);
            assert!(r.row.y0 >= last, "row {slot} starts below the previous");
            last = r.row.y1;
            assert!(r.edit.x1 <= CONTENT_X.1 && r.slider.x1 < r.edit.x0);
            assert!(r.name.x1 < r.slider.x0);
        }
        assert!(last < h, "the rows end inside the panel");
    }

    #[test]
    fn jack_labels_name_the_side_and_the_number() {
        assert_eq!(socket_label("fx_send3_l").as_deref(), Some("Send 3 L"));
        assert_eq!(socket_label("return8_r").as_deref(), Some("Ret 8 R"));
        assert_eq!(socket_label("main_l").as_deref(), Some("Main L"));
        assert_eq!(socket_label("monitor_r").as_deref(), Some("Mon R"));
        assert_eq!(socket_label("bus_l"), None);
        assert_eq!(socket_label("key_in"), None);
        assert_eq!(socket_label("send_bus1_l"), None);
    }
}
