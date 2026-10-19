//! The Audio I/O panel: per-socket state lamps (connected, free,
//! unavailable on the host audio device), peak meters and clip lamps on the front of the
//! `interface` pseudo-device. The module holds the data types the host feeds, the pure state
//! and meter maths, the cell geometry shared by the painter and the hit tests, and the painter.
//!
//! Data flow: [`io_socket_states`] derives the lamp states from the document and the channel
//! counts of the active stream (the rack view calls it on every sync; the host only passes the
//! counts, `None` for the offline sink, which counts as "every channel exists");
//! [`update_levels`] turns the per-socket linear peaks the host feeds once per frame into the
//! displayed dB values and the latched clip flags. Both are pure and deterministic.

use crate::autolayout::CONTENT_X;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::console_meter::meter_norm;
use crate::theme::Theme;
use atomik_device_sdk::{DeviceDescriptor, Direction};
use atomik_model::{INTERFACE_KEY, RackDoc};
use vello::kurbo::{Circle, Rect, RoundedRect};

/// The state of one Audio I/O socket lamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IoSocketState {
    /// The socket has a cable and the host audio device has that channel (green).
    Connected,
    /// No cable (yellow).
    #[default]
    Free,
    /// A cable exists but the host audio device has fewer channels than the socket index (red;
    /// the audio still routes and is silent on the device).
    Unavailable,
}

impl IoSocketState {
    /// The words the tooltip uses.
    pub fn word(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Free => "free",
            Self::Unavailable => "cabled, but the audio device has no such channel",
        }
    }
}

/// Floor of the displayed level, dB.
pub const LEVEL_FLOOR_DB: f32 = -60.0;
/// How far a displayed level falls per feed, dB.
pub const LEVEL_FALL_DB: f32 = 3.0;
/// Quantisation of the displayed level, dB.
const LEVEL_STEP_DB: f32 = 0.5;

/// What the painter reads: lamp states, displayed levels (dB) and clip flags, indexed like the
/// interface descriptor's sockets (host inputs first, then host outputs). Missing entries read
/// as free, at the floor, not clipped.
#[derive(Debug, Clone, Copy, Default)]
pub struct IoInfo<'a> {
    /// Lamp state per socket.
    pub states: &'a [IoSocketState],
    /// Displayed level per socket, dB.
    pub levels: &'a [f32],
    /// Latched clip flag per socket.
    pub clips: &'a [bool],
}

/// The lamp states of the interface sockets, in descriptor order: `input_channels` host inputs
/// (`in_k`) then `output_channels` host outputs (`out_k`), each capped at the panel maximum.
///
/// A socket is [`IoSocketState::Connected`] when a cable touches it and the host device has the
/// channel, [`IoSocketState::Unavailable`] when a cable touches it and the host device has
/// fewer channels than the socket index, and [`IoSocketState::Free`] otherwise (an uncabled
/// socket beyond the channels of the device is free: nothing is lost). `host` is the
/// `(input, output)` channel count of the active stream, `None` for the offline sink (all
/// available).
pub fn io_socket_states(doc: &RackDoc, host: Option<(u16, u16)>) -> Vec<IoSocketState> {
    states_of(doc.input_channels, doc.output_channels, &doc.cables, host)
}

/// [`io_socket_states`] on the parts of a document it reads.
pub fn states_of(
    input_channels: u16,
    output_channels: u16,
    cables: &[atomik_model::Cable],
    host: Option<(u16, u16)>,
) -> Vec<IoSocketState> {
    let cap = crate::interface::MAX_OUTPUTS;
    let n_in = usize::from(input_channels).min(cap);
    let n_out = usize::from(output_channels).min(cap);
    let touches = |socket: &str, from: bool| {
        cables.iter().any(|c| {
            let end = if from { &c.from } else { &c.to };
            end.device.0 == INTERFACE_KEY && end.socket == socket
        })
    };
    let state = |cabled: bool, k: usize, have: Option<u16>| match (cabled, have) {
        (true, Some(h)) if k > usize::from(h) => IoSocketState::Unavailable,
        (true, _) => IoSocketState::Connected,
        (false, _) => IoSocketState::Free,
    };
    let mut out = Vec::with_capacity(n_in + n_out);
    for k in 1..=n_in {
        out.push(state(
            touches(&format!("in_{k}"), true),
            k,
            host.map(|h| h.0),
        ));
    }
    for k in 1..=n_out {
        out.push(state(
            touches(&format!("out_{k}"), false),
            k,
            host.map(|h| h.1),
        ));
    }
    out
}

/// The Audio I/O state a [`crate::RackView`] keeps: the channel counts of the host device, the
/// lamp states derived from them, and the displayed levels and latched clip flags.
#[derive(Debug, Default, Clone)]
pub struct IoView {
    /// `(input, output)` channels of the active stream; `None` is the offline sink.
    pub host: Option<(u16, u16)>,
    /// Lamp state per socket (descriptor order).
    pub states: Vec<IoSocketState>,
    /// Displayed level per socket, dB.
    pub levels: Vec<f32>,
    /// Latched clip flag per socket.
    pub clips: Vec<bool>,
}

impl IoView {
    /// What the painter reads.
    pub fn info(&self) -> IoInfo<'_> {
        IoInfo {
            states: &self.states,
            levels: &self.levels,
            clips: &self.clips,
        }
    }
}

fn db_of(peak: f32) -> f32 {
    if peak.is_finite() && peak > 0.0 {
        (20.0 * peak.log10()).max(-120.0)
    } else {
        -120.0
    }
}

/// Folds one frame of linear per-socket peaks into the displayed `levels` (dB, quantised, with
/// a per-feed release of [`LEVEL_FALL_DB`], floor [`LEVEL_FLOOR_DB`]) and the latched `clips`
/// (a peak above 0 dBFS sets the flag; only a reset clears it). Both vectors are resized to
/// `n` on a size change and reused afterwards. Returns whether anything displayed changed, so
/// a quiet panel stops repainting. Allocation-free after the first call.
pub fn update_levels(
    peaks: &[f32],
    n: usize,
    levels: &mut Vec<f32>,
    clips: &mut Vec<bool>,
) -> bool {
    let mut changed = false;
    if levels.len() != n {
        levels.clear();
        levels.resize(n, LEVEL_FLOOR_DB);
        changed = true;
    }
    if clips.len() != n {
        clips.clear();
        clips.resize(n, false);
        changed = true;
    }
    for i in 0..n {
        let peak = peaks.get(i).copied().unwrap_or(0.0);
        let new = db_of(peak);
        let old = levels[i];
        let v = if new >= old {
            new
        } else {
            (old - LEVEL_FALL_DB).max(new)
        };
        let v = ((v / LEVEL_STEP_DB).round() * LEVEL_STEP_DB).max(LEVEL_FLOOR_DB);
        if v.to_bits() != old.to_bits() {
            levels[i] = v;
            changed = true;
        }
        if peak.is_finite() && peak > 1.0 && !clips[i] {
            clips[i] = true;
            changed = true;
        }
    }
    changed
}

// ------------------------------------------------------------------ geometry

/// Top of each row (inputs, outputs) in panel coordinates.
const ROW_Y: [f64; 2] = [4.0, 40.0];
/// Width reserved at the left of a row for its "IN" / "OUT" tag.
const TAG_W: f64 = 30.0;
/// Widest cell.
const CELL_W: f64 = 36.0;
/// Cell height (lamp, meter, clip lamp).
const CELL_H: f64 = 24.0;

/// One socket cell of the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IoCell {
    /// Index in the descriptor's sockets.
    pub socket: usize,
    /// The cell (lamp, meter and clip lamp), panel coordinates.
    pub rect: Rect,
    /// The click target of the clip lamp.
    pub clip: Rect,
    /// Row: 0 inputs, 1 outputs.
    pub row: usize,
}

/// The cells of `desc` (an interface descriptor): host inputs in the first row, host outputs
/// in the second; a missing row leaves the other one at the top.
pub fn cells(desc: &DeviceDescriptor) -> Vec<IoCell> {
    let rows: [Vec<usize>; 2] = [Direction::Output, Direction::Input].map(|dir| {
        desc.sockets
            .iter()
            .enumerate()
            .filter(|(_, s)| s.direction == dir)
            .map(|(i, _)| i)
            .collect()
    });
    let longest = rows[0].len().max(rows[1].len()).max(1);
    let x0 = CONTENT_X.0 + TAG_W;
    let cw = ((CONTENT_X.1 - x0) / longest as f64).floor().min(CELL_W);
    let mut out = Vec::new();
    let mut slot = 0;
    for (row, sockets) in rows.iter().enumerate() {
        if sockets.is_empty() {
            continue;
        }
        let y = ROW_Y[slot];
        slot += 1;
        for (k, &socket) in sockets.iter().enumerate() {
            let x = x0 + k as f64 * cw;
            let rect = Rect::new(x, y, x + cw - 4.0, y + CELL_H);
            let clip = Rect::new(rect.x0, y + 14.0, rect.x1, y + CELL_H);
            out.push(IoCell {
                socket,
                rect,
                clip,
                row,
            });
        }
    }
    out
}

/// The tag baseline ("IN" or "OUT") of `row` given which rows exist.
fn tag_y(desc: &DeviceDescriptor, row: usize) -> Option<f64> {
    let has = |dir| desc.sockets.iter().any(|s| s.direction == dir);
    let (ins, outs) = (has(Direction::Output), has(Direction::Input));
    match (row, ins, outs) {
        (0, true, _) => Some(ROW_Y[0] + 12.0),
        (1, true, true) => Some(ROW_Y[1] + 12.0),
        (1, false, true) => Some(ROW_Y[0] + 12.0),
        _ => None,
    }
}

// ------------------------------------------------------------------ painter

/// Paints the front of the Audio I/O panel: row tags, and per socket the state lamp, the peak
/// meter, the clip lamp and the socket number.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    desc: &DeviceDescriptor,
    info: &IoInfo<'_>,
) {
    for (row, tag) in [(0, "IN"), (1, "OUT")] {
        if let Some(y) = tag_y(desc, row) {
            c.text(
                &TextRun::new(
                    tag,
                    pt(CONTENT_X.0, y),
                    theme.section_size,
                    theme.text_muted,
                )
                .weight(600)
                .spacing(1.0),
            );
        }
    }
    for cell in cells(desc) {
        let s = &desc.sockets[cell.socket];
        let state = info.states.get(cell.socket).copied().unwrap_or_default();
        let level = info
            .levels
            .get(cell.socket)
            .copied()
            .unwrap_or(LEVEL_FLOOR_DB);
        let clipped = info.clips.get(cell.socket).copied().unwrap_or(false);
        let r = cell.rect;
        c.fill_with(RoundedRect::from_rect(r, 2.0), theme.lcd);
        c.fill_with(
            Circle::new(pt(r.center().x, r.y0 + 5.5), 2.8),
            theme.io_state_color(state),
        );
        // Peak meter.
        let track = Rect::new(r.x0 + 3.0, r.y0 + 10.0, r.x1 - 3.0, r.y0 + 13.0);
        c.fill_with(track, theme.knob_track);
        let fill = meter_norm(level);
        if fill > 0.0 {
            let colour = if level > -6.0 {
                theme.warning
            } else {
                theme.io_connected
            };
            c.fill_with(
                Rect::new(
                    track.x0,
                    track.y0,
                    track.x0 + track.width() * fill,
                    track.y1,
                ),
                colour,
            );
        }
        // Clip lamp.
        let lamp = Rect::new(r.x0 + 3.0, r.y0 + 16.0, r.x1 - 3.0, r.y0 + 20.0);
        c.fill_with(
            RoundedRect::from_rect(lamp, 1.0),
            if clipped { theme.clip } else { theme.led_off },
        );
        let number = s.name.rsplit(' ').next().unwrap_or(s.name);
        c.text(
            &TextRun::new(
                number,
                pt(r.center().x, r.y1 + 8.0),
                theme.label_size,
                theme.text_muted,
            )
            .align(Align::Center),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::interface_descriptor;
    use atomik_model::{Cable, DeviceKey, SocketRef, empty_doc};

    fn cable(from: (&str, &str), to: (&str, &str)) -> Cable {
        let sref = |(d, s): (&str, &str)| SocketRef {
            device: DeviceKey(d.to_string()),
            socket: s.to_string(),
        };
        Cable {
            from: sref(from),
            to: sref(to),
        }
    }

    fn doc(inputs: u16, outputs: u16, cables: &[Cable]) -> RackDoc {
        let mut d = empty_doc();
        d.input_channels = inputs;
        d.output_channels = outputs;
        d.cables = cables.to_vec();
        d
    }

    use IoSocketState::{Connected, Free, Unavailable};

    #[test]
    fn states_are_connected_free_or_unavailable() {
        let d = doc(
            2,
            4,
            &[
                cable(("interface", "in_1"), ("t", "in_l")),
                cable(("m", "main_l"), ("interface", "out_1")),
                cable(("m", "main_r"), ("interface", "out_4")),
            ],
        );
        // Host with 2 inputs and 2 outputs: out_4 is cabled but missing, out_3 is free.
        assert_eq!(
            io_socket_states(&d, Some((2, 2))),
            [Connected, Free, Connected, Free, Free, Unavailable]
        );
        // The offline sink has every channel.
        assert_eq!(
            io_socket_states(&d, None),
            [Connected, Free, Connected, Free, Free, Connected]
        );
        // A host with 4 outputs has them all.
        assert_eq!(io_socket_states(&d, Some((2, 4)))[5], Connected);
    }

    #[test]
    fn a_cabled_input_beyond_the_device_is_unavailable_and_an_uncabled_one_is_free() {
        let d = doc(
            3,
            0,
            &[
                cable(("interface", "in_3"), ("t", "in_l")),
                cable(("interface", "in_2"), ("t", "in_r")),
            ],
        );
        assert_eq!(
            io_socket_states(&d, Some((2, 0))),
            [Free, Connected, Unavailable]
        );
        // An uncabled socket beyond the device is free, not red.
        assert_eq!(io_socket_states(&doc(3, 0, &[]), Some((1, 0)))[2], Free);
    }

    #[test]
    fn states_follow_the_panel_cap_and_order() {
        let d = doc(40, 40, &[]);
        let s = io_socket_states(&d, None);
        assert_eq!(s.len(), 2 * crate::interface::MAX_OUTPUTS);
        assert!(s.iter().all(|s| *s == Free));
        assert!(io_socket_states(&doc(0, 0, &[]), None).is_empty());
    }

    #[test]
    fn a_sine_lights_its_meter_and_clips_above_zero_dbfs() {
        let (mut levels, mut clips) = (Vec::new(), Vec::new());
        // Socket 0 carries a -6 dBFS sine peak, socket 1 a +3 dBFS one, 2 and 3 are silent.
        let peaks = [0.5, 1.41, 0.0, 0.0];
        assert!(update_levels(&peaks, 4, &mut levels, &mut clips));
        assert!((levels[0] + 6.0).abs() < 0.3, "{}", levels[0]);
        assert!(levels[1] > 2.5);
        assert_eq!(levels[2], LEVEL_FLOOR_DB);
        assert_eq!(levels[3], LEVEL_FLOOR_DB);
        assert_eq!(
            clips,
            [false, true, false, false],
            "only the hot socket clips"
        );
        // The level releases, the clip stays latched.
        let silent = [0.0; 4];
        for _ in 0..60 {
            update_levels(&silent, 4, &mut levels, &mut clips);
        }
        assert!(levels.iter().all(|l| *l == LEVEL_FLOOR_DB));
        assert_eq!(clips, [false, true, false, false]);
        // A settled panel reports no change.
        assert!(!update_levels(&silent, 4, &mut levels, &mut clips));
    }

    #[test]
    fn exactly_zero_dbfs_is_not_a_clip_and_junk_is_ignored() {
        let (mut levels, mut clips) = (Vec::new(), Vec::new());
        update_levels(&[1.0, f32::NAN, f32::INFINITY], 3, &mut levels, &mut clips);
        assert_eq!(clips, [false, false, false]);
        assert!(levels[0] > -0.6);
        assert_eq!(levels[1], LEVEL_FLOOR_DB);
    }

    #[test]
    fn the_cells_put_inputs_above_outputs() {
        let d = interface_descriptor(2, 3);
        let cells = cells(&d);
        assert_eq!(cells.len(), 5);
        assert!(cells[..2].iter().all(|c| c.row == 0));
        assert!(cells[2..].iter().all(|c| c.row == 1));
        assert!(cells[2].rect.y0 > cells[0].rect.y1);
        for w in cells.windows(2) {
            if w[0].row == w[1].row {
                assert!(w[0].rect.x1 <= w[1].rect.x0);
            }
        }
        assert!(cells.iter().all(|c| c.rect.x1 <= CONTENT_X.1));
        // Without inputs the outputs take the first row.
        let only_out = super::cells(&interface_descriptor(0, 2));
        assert_eq!(only_out[0].rect.y0, ROW_Y[0]);
        // 32 sockets per row still fit the width.
        let wide = super::cells(&interface_descriptor(32, 32));
        assert!(wide.iter().all(|c| c.rect.x1 <= CONTENT_X.1 + 1e-9));
        assert!(wide[0].rect.width() >= 12.0);
    }
}
