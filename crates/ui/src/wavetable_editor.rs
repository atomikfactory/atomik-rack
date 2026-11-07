//! The wavetable editor: a modal panel opened from the Wavetable Synth's table displays, with a
//! list of the table's key frames, a box to draw the selected frame in, a bar for each of its
//! first harmonics, whole-frame processes and the cleanup of a table made from a sample.
//!
//! ```text
//! WAVETABLE EDITOR  Wavetable Synth                       [ PHASE-SAFE ]  [ CLOSE ]
//! [+][=][x][^][v]   +------------------------------------------------------+
//! 1  ~~~~           |  draw here: the pointer sets the samples it crosses   |
//! 2  ~~~~  <-       +------------------------------------------------------+
//! 3  ~~~~           HARMONICS 1-64: drag the bars (level), the dots (phase)
//! ...               [NORM] [DC] [INV] [REV] [SMOOTH] [ALL]
//!                   CYCLE 441.0 samples (100 Hz)  [CUT] [DEDUPE] [EDGE] [128]  [USE OSC 1]
//!                   the morph warning or the level dip
//! ```
//!
//! The panel holds a working copy of the key frames ([`WavetableData::keys`], at most
//! [`MAX_KEYS`]) and reports every finished gesture as one [`WavetableAction::Commit`] with the
//! whole list: a stroke of the pointer in the box, a drag of the bars, a click on a frame or
//! process button. The host stores the table (one undo step per commit) and feeds the stored
//! table back with [`WavetableView::set_data`]; while a gesture runs the panel ignores that.
//! Escape during a gesture puts the frames of the press back. The maths is in
//! [`atomik_dsp::wavetable::edit`]:
//!
//! * **Frame list.** `ADD` inserts the blend of the selected frame and the next one (a copy at
//!   the end) after the selected frame, `DUP` a copy, `DEL` removes it (the last one stays),
//!   `UP`/`DOWN` move it. The list never goes past [`MAX_KEYS`] frames.
//! * **Draw.** The pointer sets the samples it crosses to its height (the box runs from -1 to 1)
//!   along straight lines between the pointer positions.
//! * **Harmonics.** A bar is a harmonic's level (0 to [`LEVEL_FULL`]), the dot under it its phase
//!   (a full height is half a cycle either way); a press and a drag across several bars sets
//!   each one. Every other harmonic of the frame stays as it was.
//! * **Phase-safe.** The toggle aligns the fundamentals of the frames when the table is stored,
//!   so a morph keeps its level; the line under the controls names the pairs of frames whose
//!   fundamental would cancel, or reports the level dip of the morph.
//! * **Cleanup.** The readout shows the cycle length found in the sample the table was made
//!   from; `CUT` cuts the sample again with one cycle per frame, `DEDUPE` drops silent and
//!   near-identical frames, `EDGE` makes each cycle's end meet its start (the number cycles the
//!   overlap), `USE OSC 1` makes oscillator 1 play the table.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_dsp::wavetable::edit::{
    self, BINS, Bin, DUPLICATE_TOLERANCE, FRAME_LEN, Frame, MAX_KEYS, MorphWarning, Warning,
};
use vello::kurbo::{Affine, BezPath, Point, Rect};
use vello::peniko::Color;

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const LIST_W: f64 = 148.0;
const ROW_H: f64 = 24.0;
const TOOL_H: f64 = 22.0;
const CANVAS_H: f64 = 170.0;
const LEVEL_H: f64 = 86.0;
const PHASE_H: f64 = 34.0;
const BTN_H: f64 = 22.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 760.0;
const FONT: f32 = 10.5;
/// The level at the top of the harmonic bars.
pub const LEVEL_FULL: f32 = 1.5;
/// The overlaps `EDGE` offers, in samples.
pub const EDGE_STEPS: [usize; 5] = [32, 64, 128, 256, 512];
/// Points of the selected frame's curve.
const CURVE_POINTS: usize = 512;
/// Points of a thumbnail's curve.
const THUMB_POINTS: usize = 48;

/// A painter of one button: canvas, box, label, lit colour, enabled.
type Button<'a> = dyn Fn(&mut dyn Canvas, Rect, &str, Option<Color>, bool) + 'a;

/// What the editor shows: the stored table and what the host knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct WavetableData {
    /// The device's name.
    pub device: String,
    /// The key frames, [`FRAME_LEN`] samples each. Empty while the sample is still loading.
    pub keys: Vec<Frame>,
    /// The table is stored aligned (phase-safe).
    pub phase_safe: bool,
    /// The device has a sample: false while the table shown is only a starting point that the
    /// first edit stores.
    pub stored: bool,
    /// The table is a dropped sample cut into frames (the cleanup works on it).
    pub imported: bool,
    /// The cycle length found in that sample, in samples, and its pitch in Hz.
    pub cycle: Option<(f32, f32)>,
    /// An oscillator plays the imported table.
    pub plays: bool,
}

impl Default for WavetableData {
    fn default() -> Self {
        Self {
            device: String::new(),
            keys: Vec::new(),
            phase_safe: true,
            stored: false,
            imported: false,
            cycle: None,
            plays: false,
        }
    }
}

/// What the editor asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum WavetableAction {
    /// Store this table: the key frames and whether to align them. One undo step.
    Commit {
        /// The key frames.
        keys: Vec<Frame>,
        /// Align the frames when storing.
        phase_safe: bool,
    },
    /// Cut the sample the table was made from with one cycle per frame.
    CutCycles,
    /// Make oscillator 1 play the table.
    UseOnOsc1,
    /// Close the panel.
    Close,
}

/// A gesture that changes the frames.
#[derive(Debug, Clone)]
enum Drag {
    /// Drawing in the box; the last pointer position as (sample, value).
    Draw((f32, f32)),
    /// Setting levels on the bars.
    Level,
    /// Setting phases on the dots.
    Phase,
}

/// What the line under the controls says.
#[derive(Debug, Clone, Default, PartialEq)]
struct Report {
    warnings: Vec<MorphWarning>,
    /// The worst level dip of the morph between neighbouring frames, dB.
    dip_db: f32,
}

/// The wavetable editor. See the module documentation.
#[derive(Debug)]
pub struct WavetableView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    data: WavetableData,
    keys: Vec<Frame>,
    sel: usize,
    phase_safe: bool,
    all: bool,
    edge: usize,
    bins: Vec<Bin>,
    report: Report,
    drag: Option<(Drag, Vec<Frame>)>,
    hover: Option<Point>,
    repaint: bool,
}

impl WavetableView {
    /// An editor with no table yet; call [`Self::set_bounds`] and [`Self::set_data`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 960.0, 600.0),
            scale: 1.0,
            data: WavetableData::default(),
            keys: Vec::new(),
            sel: 0,
            phase_safe: true,
            all: false,
            edge: 2,
            bins: Vec::new(),
            report: Report::default(),
            drag: None,
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Shows the stored table. Ignored while a gesture runs.
    pub fn set_data(&mut self, data: WavetableData) {
        // Equal data still puts the stored frames back when an edit was refused.
        let same =
            self.data == data && self.keys == data.keys && self.phase_safe == data.phase_safe;
        if self.drag.is_some() || same {
            return;
        }
        self.keys = data.keys.clone();
        self.phase_safe = data.phase_safe;
        self.data = data;
        self.sel = self.sel.min(self.keys.len().saturating_sub(1));
        self.refresh();
        self.repaint = true;
    }

    /// What the panel shows (as the host last set it).
    pub fn data(&self) -> &WavetableData {
        &self.data
    }

    /// The key frames as the panel holds them now (during a gesture they are ahead of the
    /// stored table).
    pub fn keys(&self) -> &[Frame] {
        &self.keys
    }

    /// The selected key frame.
    pub fn selected(&self) -> usize {
        self.sel
    }

    /// Selects key frame `n` (clamped).
    pub fn select(&mut self, n: usize) {
        let n = n.min(self.keys.len().saturating_sub(1));
        if n != self.sel {
            self.sel = n;
            self.refresh_bins();
            self.repaint = true;
        }
    }

    /// Whether the frames are aligned when stored.
    pub fn phase_safe(&self) -> bool {
        self.phase_safe
    }

    /// The overlap `EDGE` uses, in samples.
    pub fn edge_overlap(&self) -> usize {
        EDGE_STEPS[self.edge]
    }

    /// True while a gesture runs (Escape then cancels it instead of closing the panel).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The harmonics of the selected frame as the bars show them.
    pub fn bins(&self) -> &[Bin] {
        &self.bins
    }

    /// The warnings about the morph, as the line under the controls reads them.
    pub fn warnings(&self) -> &[MorphWarning] {
        &self.report.warnings
    }

    /// The worst level dip of the morph, in dB (0 for none).
    pub fn dip_db(&self) -> f32 {
        self.report.dip_db
    }

    fn refresh(&mut self) {
        self.refresh_bins();
        self.refresh_report();
    }

    fn refresh_bins(&mut self) {
        self.bins = self
            .keys
            .get(self.sel)
            .map_or_else(Vec::new, |f| edit::analyse(f, BINS));
    }

    fn refresh_report(&mut self) {
        let warnings = edit::morph_warnings(&self.keys, self.phase_safe);
        let dip_db = self
            .keys
            .windows(2)
            .map(|p| edit::morph_dip_db(&p[0], &p[1], self.phase_safe, 12))
            .fold(0.0f32, f32::max);
        self.report = Report { warnings, dip_db };
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H
            + PAD
            + CANVAS_H
            + 8.0
            + 14.0
            + LEVEL_H
            + PHASE_H
            + 10.0
            + BTN_H
            + 6.0
            + BTN_H
            + 8.0
            + 32.0
            + PAD
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The PHASE-SAFE toggle.
    pub fn safe_rect(&self) -> Rect {
        let c = self.close_rect();
        Rect::new(c.x0 - 10.0 - 110.0, c.y0, c.x0 - 10.0, c.y1)
    }

    fn top(&self) -> f64 {
        self.panel().y0 + HEADER_H + PAD
    }

    fn center_x(&self) -> f64 {
        self.panel().x0 + PAD + LIST_W + 12.0
    }

    /// The box the frame is drawn in.
    pub fn canvas_rect(&self) -> Rect {
        let y = self.top();
        Rect::new(self.center_x(), y, self.panel().x1 - PAD, y + CANVAS_H)
    }

    /// The area of the harmonic level bars.
    pub fn level_rect(&self) -> Rect {
        let c = self.canvas_rect();
        let y = c.y1 + 8.0 + 14.0;
        Rect::new(c.x0, y, c.x1, y + LEVEL_H)
    }

    /// The area of the harmonic phase dots.
    pub fn phase_rect(&self) -> Rect {
        let l = self.level_rect();
        Rect::new(l.x0, l.y1, l.x1, l.y1 + PHASE_H)
    }

    fn row1_y(&self) -> f64 {
        self.phase_rect().y1 + 10.0
    }

    fn row2_y(&self) -> f64 {
        self.row1_y() + BTN_H + 6.0
    }

    fn tool_button(&self, n: usize) -> Rect {
        let x = self.panel().x0 + PAD + n as f64 * 29.0;
        let y = self.top();
        Rect::new(x, y, x + 27.0, y + TOOL_H)
    }

    /// The ADD button (a blend after the selected frame).
    pub fn add_rect(&self) -> Rect {
        self.tool_button(0)
    }

    /// The DUP button.
    pub fn dup_rect(&self) -> Rect {
        self.tool_button(1)
    }

    /// The DEL button.
    pub fn del_rect(&self) -> Rect {
        self.tool_button(2)
    }

    /// The UP button (move the frame earlier).
    pub fn up_rect(&self) -> Rect {
        self.tool_button(3)
    }

    /// The DOWN button (move the frame later).
    pub fn down_rect(&self) -> Rect {
        self.tool_button(4)
    }

    /// The row of key frame `n` in the list.
    pub fn row_rect(&self, n: usize) -> Rect {
        let x = self.panel().x0 + PAD;
        let y = self.top() + TOOL_H + 4.0 + n as f64 * ROW_H;
        Rect::new(x, y, x + LIST_W, y + ROW_H)
    }

    fn process_rect(&self, n: usize) -> Rect {
        let x = self.center_x() + n as f64 * 60.0;
        let y = self.row1_y();
        Rect::new(x, y, x + 54.0, y + BTN_H)
    }

    /// The NORM button (peak at 1).
    pub fn norm_rect(&self) -> Rect {
        self.process_rect(0)
    }

    /// The DC button (take the mean out).
    pub fn dc_rect(&self) -> Rect {
        self.process_rect(1)
    }

    /// The INV button (flip the polarity).
    pub fn invert_rect(&self) -> Rect {
        self.process_rect(2)
    }

    /// The REV button (play the cycle backwards).
    pub fn reverse_rect(&self) -> Rect {
        self.process_rect(3)
    }

    /// The SMOOTH button.
    pub fn smooth_rect(&self) -> Rect {
        self.process_rect(4)
    }

    /// The ALL toggle (processes act on every frame).
    pub fn all_rect(&self) -> Rect {
        self.process_rect(5)
    }

    fn cleanup_rect(&self, x: f64, w: f64) -> Rect {
        let y = self.row2_y();
        Rect::new(x, y, x + w, y + BTN_H)
    }

    fn readout_w() -> f64 {
        190.0
    }

    /// The CUT button (cut the sample by its cycle).
    pub fn cut_rect(&self) -> Rect {
        self.cleanup_rect(self.center_x() + Self::readout_w() + 6.0, 46.0)
    }

    /// The DEDUPE button.
    pub fn dedupe_rect(&self) -> Rect {
        let c = self.cut_rect();
        self.cleanup_rect(c.x1 + 6.0, 62.0)
    }

    /// The EDGE button (cross-fade the cycle ends).
    pub fn edge_rect(&self) -> Rect {
        let c = self.dedupe_rect();
        self.cleanup_rect(c.x1 + 6.0, 46.0)
    }

    /// The overlap of EDGE (a click steps it).
    pub fn edge_amount_rect(&self) -> Rect {
        let c = self.edge_rect();
        self.cleanup_rect(c.x1 + 4.0, 40.0)
    }

    /// The USE OSC 1 button.
    pub fn use_rect(&self) -> Rect {
        let r = self.panel().x1 - PAD;
        self.cleanup_rect(r - 96.0, 96.0)
    }

    fn usable_rows(&self) -> usize {
        self.keys.len().min(MAX_KEYS)
    }

    // ------------------------------------------------------------------ editing

    fn can_cut(&self) -> bool {
        self.data.imported && self.data.cycle.is_some()
    }

    fn commit(&self) -> UiAction {
        UiAction::Wavetable(WavetableAction::Commit {
            keys: self.keys.clone(),
            phase_safe: self.phase_safe,
        })
    }

    /// Runs `f` on the selected frame, or on every frame with ALL on.
    fn process(&mut self, f: impl Fn(&mut Frame)) -> bool {
        if self.keys.is_empty() {
            return false;
        }
        if self.all {
            self.keys.iter_mut().for_each(f);
        } else if let Some(frame) = self.keys.get_mut(self.sel) {
            f(frame);
        }
        true
    }

    fn changed(&mut self, out: &mut Vec<UiAction>) {
        self.refresh();
        self.repaint = true;
        out.push(self.commit());
    }

    fn add_frame(&mut self) -> bool {
        if self.keys.is_empty() || self.keys.len() >= MAX_KEYS {
            return false;
        }
        let at = self.sel;
        let blend: Frame = match self.keys.get(at + 1) {
            Some(next) => self.keys[at]
                .iter()
                .zip(next)
                .map(|(a, b)| 0.5 * (a + b))
                .collect(),
            None => self.keys[at].clone(),
        };
        self.keys.insert(at + 1, blend);
        self.sel = at + 1;
        true
    }

    fn dup_frame(&mut self) -> bool {
        if self.keys.is_empty() || self.keys.len() >= MAX_KEYS {
            return false;
        }
        let copy = self.keys[self.sel].clone();
        self.keys.insert(self.sel + 1, copy);
        self.sel += 1;
        true
    }

    fn del_frame(&mut self) -> bool {
        if self.keys.len() <= 1 {
            return false;
        }
        self.keys.remove(self.sel);
        self.sel = self.sel.min(self.keys.len() - 1);
        true
    }

    fn move_frame(&mut self, later: bool) -> bool {
        let to = if later {
            self.sel + 1
        } else {
            match self.sel.checked_sub(1) {
                Some(n) => n,
                None => return false,
            }
        };
        if to >= self.keys.len() {
            return false;
        }
        self.keys.swap(self.sel, to);
        self.sel = to;
        true
    }

    fn dedupe(&mut self) -> bool {
        let kept = edit::drop_near_duplicates(&self.keys, DUPLICATE_TOLERANCE);
        if kept.is_empty() || kept == self.keys {
            return false;
        }
        self.keys = kept;
        self.sel = self.sel.min(self.keys.len() - 1);
        true
    }

    fn sample_at(&self, pos: Point) -> (f32, f32) {
        let c = self.canvas_rect();
        let x = ((pos.x - c.x0) / c.width()).clamp(0.0, 1.0) * (FRAME_LEN - 1) as f64;
        let y = (1.0 - 2.0 * ((pos.y - c.y0) / c.height())).clamp(-1.0, 1.0);
        (x as f32, y as f32)
    }

    fn bin_at(&self, x: f64) -> usize {
        let l = self.level_rect();
        let n = ((x - l.x0) / l.width() * BINS as f64).floor();
        (n.max(0.0) as usize).min(BINS - 1) + 1
    }

    fn set_bin(&mut self, pos: Point, level: bool) {
        let h = self.bin_at(pos.x);
        let Some(current) = self.bins.get(h - 1).copied() else {
            return;
        };
        let bin = if level {
            let r = self.level_rect();
            let v = ((r.y1 - pos.y) / r.height()).clamp(0.0, 1.0) as f32 * LEVEL_FULL;
            Bin {
                level: v,
                phase: current.phase,
            }
        } else {
            let r = self.phase_rect();
            let v = 0.5 - ((pos.y - r.y0) / r.height()).clamp(0.0, 1.0) as f32;
            Bin {
                level: current.level,
                phase: v,
            }
        };
        if let Some(frame) = self.keys.get_mut(self.sel) {
            edit::set_harmonic(frame, h, bin);
        }
        self.refresh_bins();
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let tips: [(Rect, &'static str); 21] = [
            (self.close_rect(), "Close the editor"),
            (
                self.safe_rect(),
                "Align the fundamental of every frame when the table is stored, so a morph keeps its level",
            ),
            (
                self.add_rect(),
                "Add a frame after this one: the blend of this frame and the next",
            ),
            (self.dup_rect(), "Duplicate this frame"),
            (self.del_rect(), "Delete this frame (the last one stays)"),
            (self.up_rect(), "Move this frame earlier in the table"),
            (self.down_rect(), "Move this frame later in the table"),
            (
                self.canvas_rect(),
                "Draw the frame: the pointer sets the samples it crosses",
            ),
            (
                self.level_rect(),
                "The level of each harmonic of this frame: drag across the bars",
            ),
            (
                self.phase_rect(),
                "The phase of each harmonic: a full height is half a cycle either way",
            ),
            (self.norm_rect(), "Scale the frame so it peaks at 1"),
            (self.dc_rect(), "Take the offset out of the frame"),
            (self.invert_rect(), "Flip the polarity of the frame"),
            (self.reverse_rect(), "Play the cycle backwards"),
            (self.smooth_rect(), "Round off the corners of the frame"),
            (
                self.all_rect(),
                "Apply NORM, DC, INV, REV and SMOOTH to every frame",
            ),
            (
                self.cut_rect(),
                "Cut the sample again with one cycle per frame, at the length found",
            ),
            (
                self.dedupe_rect(),
                "Drop silent frames and frames that are near copies of the one before",
            ),
            (
                self.edge_rect(),
                "Make the end of each cycle meet its start by cross-fading the overlap",
            ),
            (
                self.edge_amount_rect(),
                "The overlap of EDGE in samples: click for the next",
            ),
            (
                self.use_rect(),
                "Make oscillator 1 play this table (its table choice becomes imported)",
            ),
        ];
        tips.into_iter()
            .find(|(r, _)| r.contains(pos))
            .map(|(r, t)| Help::new(r, t))
            .or_else(|| {
                let rows = (0..self.usable_rows()).map(|n| self.row_rect(n));
                rows.into_iter()
                    .find(|r| r.contains(pos))
                    .map(|r| Help::new(r, "A key frame of the table: click to select it"))
            })
    }

    fn press(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        let act = UiAction::Wavetable;
        if self.close_rect().contains(pos) {
            out.push(act(WavetableAction::Close));
        } else if self.safe_rect().contains(pos) {
            self.phase_safe = !self.phase_safe;
            self.changed(out);
        } else if self.add_rect().contains(pos) {
            if self.add_frame() {
                self.changed(out);
            }
        } else if self.dup_rect().contains(pos) {
            if self.dup_frame() {
                self.changed(out);
            }
        } else if self.del_rect().contains(pos) {
            if self.del_frame() {
                self.changed(out);
            }
        } else if self.up_rect().contains(pos) {
            if self.move_frame(false) {
                self.changed(out);
            }
        } else if self.down_rect().contains(pos) {
            if self.move_frame(true) {
                self.changed(out);
            }
        } else if self.norm_rect().contains(pos) {
            if self.process(edit::normalise) {
                self.changed(out);
            }
        } else if self.dc_rect().contains(pos) {
            if self.process(edit::remove_dc) {
                self.changed(out);
            }
        } else if self.invert_rect().contains(pos) {
            if self.process(edit::invert) {
                self.changed(out);
            }
        } else if self.reverse_rect().contains(pos) {
            if self.process(edit::reverse) {
                self.changed(out);
            }
        } else if self.smooth_rect().contains(pos) {
            if self.process(edit::smooth) {
                self.changed(out);
            }
        } else if self.all_rect().contains(pos) {
            self.all = !self.all;
        } else if self.cut_rect().contains(pos) {
            if self.can_cut() {
                out.push(act(WavetableAction::CutCycles));
            }
        } else if self.dedupe_rect().contains(pos) {
            if self.dedupe() {
                self.changed(out);
            }
        } else if self.edge_rect().contains(pos) {
            let overlap = self.edge_overlap();
            if self.process(|f| *f = edit::crossfade_edges(f, overlap)) {
                self.changed(out);
            }
        } else if self.edge_amount_rect().contains(pos) {
            self.edge = (self.edge + 1) % EDGE_STEPS.len();
        } else if self.use_rect().contains(pos) {
            if !self.data.plays {
                out.push(act(WavetableAction::UseOnOsc1));
            }
        } else if let Some(n) = (0..self.usable_rows()).find(|n| self.row_rect(*n).contains(pos)) {
            self.select(n);
        } else if self.canvas_rect().contains(pos) && !self.keys.is_empty() {
            let at = self.sample_at(pos);
            self.drag = Some((Drag::Draw(at), self.keys.clone()));
            if let Some(frame) = self.keys.get_mut(self.sel) {
                edit::paint_line(frame, at, at);
            }
            self.refresh_bins();
        } else if self.level_rect().contains(pos) && !self.keys.is_empty() {
            self.drag = Some((Drag::Level, self.keys.clone()));
            self.set_bin(pos, true);
        } else if self.phase_rect().contains(pos) && !self.keys.is_empty() {
            self.drag = Some((Drag::Phase, self.keys.clone()));
            self.set_bin(pos, false);
        }
        self.repaint = true;
    }

    fn drag_to(&mut self, pos: Point) {
        let Some((drag, _)) = &self.drag else {
            return;
        };
        match drag.clone() {
            Drag::Draw(last) => {
                let at = self.sample_at(pos);
                if let Some(frame) = self.keys.get_mut(self.sel) {
                    edit::paint_line(frame, last, at);
                }
                if let Some((d, _)) = &mut self.drag {
                    *d = Drag::Draw(at);
                }
                self.refresh_bins();
            }
            Drag::Level => self.set_bin(pos, true),
            Drag::Phase => self.set_bin(pos, false),
        }
        self.repaint = true;
    }

    fn release(&mut self, out: &mut Vec<UiAction>) {
        let Some((_, before)) = self.drag.take() else {
            return;
        };
        if before != self.keys {
            self.changed(out);
        } else {
            self.repaint = true;
        }
    }

    fn cancel(&mut self) {
        if let Some((_, before)) = self.drag.take() {
            self.keys = before;
            self.refresh();
            self.repaint = true;
        }
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.drag_to(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => self.press(*pos, &mut out),
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.release(&mut out),
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => self.cancel(),
                Key::Up if self.drag.is_none() && self.sel > 0 => self.select(self.sel - 1),
                Key::Down if self.drag.is_none() => self.select(self.sel + 1),
                _ => {}
            },
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    fn curve(frame: &[f32], r: Rect, points: usize) -> BezPath {
        let mut path = BezPath::new();
        let mid = r.center().y;
        let step = (frame.len() / points).max(1);
        for i in 0..=points {
            let v =
                f64::from(frame[(i * step).min(frame.len() - 1) % frame.len()]).clamp(-1.0, 1.0);
            let p = Point::new(
                r.x0 + i as f64 / points as f64 * r.width(),
                mid - v * r.height() / 2.0,
            );
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path
    }

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "WAVETABLE EDITOR",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        sp::label(
            c,
            &fit_label(&self.data.device, 180.0, FONT),
            p.x0 + PAD + 150.0,
            p.y0 + 22.0,
            FONT,
            t.text_muted,
            Align::Start,
        );
        let lit = |on: bool| on.then_some(t.focus);
        let btn = |c: &mut dyn Canvas, r: Rect, text: &str, lit: Option<Color>, on: bool| {
            let hover = on && hovered(r);
            sp::button(c, st, r, text, lit, hover);
            if !on {
                c.fill_with(r, st.background.with_alpha(0.55));
            }
        };
        btn(c, self.close_rect(), "CLOSE", None, true);
        btn(
            c,
            self.safe_rect(),
            "PHASE-SAFE",
            lit(self.phase_safe),
            true,
        );
        let editable = !self.keys.is_empty();
        let room = self.keys.len() < MAX_KEYS && editable;
        btn(c, self.add_rect(), "ADD", None, room);
        btn(c, self.dup_rect(), "DUP", None, room);
        btn(c, self.del_rect(), "DEL", None, self.keys.len() > 1);
        btn(c, self.up_rect(), "UP", None, self.sel > 0);
        btn(
            c,
            self.down_rect(),
            "DOWN",
            None,
            self.sel + 1 < self.keys.len(),
        );
        self.paint_list(c);
        self.paint_canvas(c);
        self.paint_bins(c);
        for (r, text) in [
            (self.norm_rect(), "NORM"),
            (self.dc_rect(), "DC"),
            (self.invert_rect(), "INV"),
            (self.reverse_rect(), "REV"),
            (self.smooth_rect(), "SMOOTH"),
        ] {
            btn(c, r, text, None, editable);
        }
        btn(c, self.all_rect(), "ALL", lit(self.all), true);
        self.paint_cleanup(c, &btn);
        self.paint_lines(c);
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_list(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let first = self.row_rect(0);
        let area = Rect::new(
            first.x0,
            first.y0,
            first.x1,
            first.y0 + MAX_KEYS as f64 * ROW_H,
        );
        sp::lcd(c, st, area.inflate(2.0, 2.0), None);
        for (n, frame) in self.keys.iter().enumerate().take(MAX_KEYS) {
            let r = self.row_rect(n);
            if n == self.sel {
                sp::rect(c, r, mix(t.lcd, t.focus, 0.45));
            } else if self.hover.is_some_and(|h| r.contains(h)) {
                sp::rect(c, r, mix(t.lcd, t.text, 0.08));
            }
            sp::label(
                c,
                &(n + 1).to_string(),
                r.x0 + 6.0,
                r.y0 + 16.0,
                FONT,
                t.text,
                Align::Start,
            );
            let wave = Rect::new(r.x0 + 30.0, r.y0 + 3.0, r.x1 - 6.0, r.y1 - 3.0);
            c.stroke_with(
                Self::curve(frame, wave, THUMB_POINTS),
                1.0,
                mix(t.lcd, t.text, 0.8),
            );
        }
    }

    fn paint_canvas(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let r = self.canvas_rect();
        sp::lcd(c, st, r.inflate(2.0, 2.0), None);
        let mid = r.center().y;
        sp::line(c, r.x0, mid, r.x1, mid, t.section_line);
        for q in 1..4 {
            let x = r.x0 + r.width() * f64::from(q) / 4.0;
            sp::line(c, x, r.y0, x, r.y1, mix(t.lcd, t.section_line, 0.5));
        }
        let Some(frame) = self.keys.get(self.sel) else {
            let text = if self.data.stored {
                "Loading the sample"
            } else {
                "No table"
            };
            sp::label(
                c,
                text,
                r.center().x,
                mid + 4.0,
                FONT,
                t.text_muted,
                Align::Center,
            );
            return;
        };
        // The neighbours, faint, to draw against.
        for n in [self.sel.checked_sub(1), Some(self.sel + 1)]
            .into_iter()
            .flatten()
        {
            if let Some(f) = self.keys.get(n) {
                c.stroke_with(
                    Self::curve(f, r, CURVE_POINTS),
                    1.0,
                    mix(t.lcd, t.text, 0.18),
                );
            }
        }
        c.stroke_with(Self::curve(frame, r, CURVE_POINTS), 1.6, t.focus);
        sp::label(
            c,
            &format!("FRAME {} OF {}", self.sel + 1, self.keys.len()),
            r.x0 + 6.0,
            r.y0 + 12.0,
            9.0,
            t.text_muted,
            Align::Start,
        );
    }

    fn paint_bins(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let l = self.level_rect();
        let ph = self.phase_rect();
        sp::label(
            c,
            &format!("HARMONICS 1-{BINS}"),
            l.x0,
            l.y0 - 5.0,
            9.0,
            t.text_muted,
            Align::Start,
        );
        sp::lcd(c, st, l.inflate(2.0, 2.0), None);
        sp::lcd(c, st, ph.inflate(2.0, 2.0), None);
        sp::line(
            c,
            ph.x0,
            ph.center().y,
            ph.x1,
            ph.center().y,
            t.section_line,
        );
        let pitch = l.width() / BINS as f64;
        for (n, bin) in self.bins.iter().enumerate() {
            let x0 = l.x0 + n as f64 * pitch;
            let h = f64::from((bin.level / LEVEL_FULL).clamp(0.0, 1.0)) * l.height();
            let bar = Rect::new(x0 + 1.0, l.y1 - h, x0 + pitch - 1.0, l.y1);
            sp::rect(c, bar, t.focus);
            if bin.level > 1.0e-4 {
                let y = ph.center().y - f64::from(bin.phase) * ph.height();
                let dot = Rect::new(x0 + 1.0, y - 1.5, x0 + pitch - 1.0, y + 1.5);
                sp::rect(c, dot, mix(t.lcd, t.text, 0.85));
            }
        }
    }

    fn paint_cleanup(&self, c: &mut dyn Canvas, btn: &Button) {
        let t = &self.st.base;
        let readout = Rect::new(
            self.center_x(),
            self.row2_y(),
            self.center_x() + Self::readout_w(),
            self.row2_y() + BTN_H,
        );
        let text = match (self.data.imported, self.data.cycle) {
            (true, Some((len, hz))) => format!("CYCLE {len:.1} samples ({hz:.0} Hz)"),
            (true, None) => "CYCLE not found".to_owned(),
            (false, _) => "Drawn table".to_owned(),
        };
        sp::label(
            c,
            &fit_label(&text, readout.width(), FONT),
            readout.x0,
            readout.y0 + 15.0,
            FONT,
            t.text,
            Align::Start,
        );
        btn(c, self.cut_rect(), "CUT", None, self.can_cut());
        btn(c, self.dedupe_rect(), "DEDUPE", None, self.keys.len() > 1);
        btn(c, self.edge_rect(), "EDGE", None, !self.keys.is_empty());
        btn(
            c,
            self.edge_amount_rect(),
            &self.edge_overlap().to_string(),
            None,
            true,
        );
        let label = if self.data.plays {
            "OSC 1 PLAYS IT"
        } else {
            "USE OSC 1"
        };
        let on = !self.data.plays;
        btn(c, self.use_rect(), label, None, on);
    }

    fn paint_lines(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let p = self.panel();
        let width = p.width() - 2.0 * PAD;
        let y = self.row2_y() + BTN_H + 8.0;
        let (text, color) = match self.report.warnings.first() {
            Some(w) => {
                let more = self.report.warnings.len() - 1;
                let mut s = Self::warning_text(w);
                if more > 0 {
                    s.push_str(&format!(" (+{more} more)"));
                }
                (s, t.warning)
            }
            None if self.keys.len() > 1 => (
                format!("Morph level dip: {:.1} dB", self.report.dip_db),
                t.text,
            ),
            None => (
                "One frame: the table does not morph".to_owned(),
                t.text_muted,
            ),
        };
        sp::label(
            c,
            &fit_label(&text, width, FONT),
            p.x0 + PAD,
            y + 8.0,
            FONT,
            color,
            Align::Start,
        );
        let hint = if self.data.stored {
            "Every release is one undo step. Each frame plays at full level: the table is scaled per frame."
        } else {
            "Not stored yet: the first edit stores the table as this device's sample."
        };
        sp::label(
            c,
            &fit_label(hint, width, 9.5),
            p.x0 + PAD,
            y + 26.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }

    /// The sentence for one warning.
    pub fn warning_text(w: &MorphWarning) -> String {
        match w.kind {
            Warning::Cancels { dip_db } => format!(
                "Frames {} and {}: the fundamental cancels by {dip_db:.0} dB half way: turn on phase-safe",
                w.frame + 1,
                w.frame + 2
            ),
            Warning::NoFundamental => format!(
                "Frame {} has no fundamental: phase-safe cannot align it",
                w.frame + 1
            ),
        }
    }
}
