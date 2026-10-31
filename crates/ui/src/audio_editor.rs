//! The audio clip editor: a pane that replaces the arrangement and edits one audio clip.
//!
//! Hosting follows [`crate::PianoRollView`]: on [`UiAction::OpenAudioEditor`] call
//! [`AudioEditorView::open`] and show the view in place of the arrangement; on
//! [`UiAction::CloseAudioEditor`] show the arrangement again (also when
//! [`AudioEditorView::is_present`] turns false because the clip was deleted). Peaks, samples
//! and the missing-asset set come from the app like for the slice editor.
//!
//! # Layout (logical pixels, inside the bounds)
//!
//! ```text
//! top bar    AUDIO CLIP   <track> · <asset name>                                  [x]
//! toolbar    MODE [Overview][Slice][Vocal][Comp]   DRAG [Stretch][Slide]  [Revert] [Split] [To slicer]
//!            SENS [-] 50 [+] [Add all]                                        [Speaker]
//! ruler      bars and beats of the song under the clip
//! waveform   the clip in clip time: the audio is drawn where it plays, so a stretched span is
//!            drawn wider or narrower; warp markers are the vertical lines
//! footer     the hint or the last refusal, and the marker count and clip length
//! ```
//!
//! # Modes
//!
//! * **Overview** shows the clip and its markers; nothing is edited by the pointer.
//! * **Slice** edits the warp markers. A marker pins a source frame to a tick of the clip: a new
//!   marker is pinned where the audio already plays, so adding markers changes no sound. Entering
//!   Slice mode with warp off switches warp on at the song tempo at the clip start (one undo step,
//!   like Quantize Audio); the first marker added to such a clip does both in one step. A reversed
//!   clip cannot be warped and cannot be sliced.
//! * **Vocal** and **Comp** are listed and disabled.
//!
//! The mode is view state ([`AudioEditorView::mode`], [`AudioEditorView::set_mode`]); the app
//! persists it. Opening another clip keeps it.
//!
//! # Interaction in Slice mode
//!
//! * **Add.** Double-click on the waveform, or the `editor.audio_add_marker` command at the
//!   pointer. The marker goes on the detected transient within a few pixels of the pointer when
//!   there is one (the sound does not change either way), else where the clip plays at the pointer.
//! * **Transients.** Entering Slice mode, opening a clip in it and the SENS buttons emit
//!   [`UiAction::DetectTransients`]; the host answers with [`AudioEditorView::set_transients`]
//!   and they are drawn under the ruler. **Add all** (`editor.audio_add_transients`) puts a
//!   marker on every one, as one undo step.
//! * **Speaker.** The SPEAKER toggle (`tool.speaker`) makes a press on the waveform play the clip
//!   from there to its end as it is in its file ([`UiAction::AuditionSpan`]), unprocessed;
//!   the release (or Esc) emits [`UiAction::StopAudition`]. It edits nothing.
//! * **Move (stretch).** Drag a marker: it follows the pointer in time with its audio frame, the
//!   audio on both sides stretches. The clip's start and end are pinned (the first and last
//!   marker are drawn dimmed and cannot be moved).
//! * **Move (slide).** With the alternate-tool role held (or the **Slide** toggle on), the
//!   marker slides along the audio: marker and frame move together, the audio keeps its place.
//! * **Group.** With the constrain role held, the markers after the dragged one move with it in
//!   proportion (an accordion up to the clip end).
//! * **Nudge.** `editor.nudge_*` move the selected marker by a sixteenth, a beat or a tick.
//! * **Delete** (`edit.delete`) removes the selected marker. **Revert** removes every marker.
//!   **Split** cuts the clip at each marker as one `Batch` of `SplitClip` (one undo step).
//!   **To slicer** emits [`UiAction::ClipToSlicer`].
//! * Every drag is one undo step: its `SetWarpMarkers` share a merge id and the gesture ends
//!   with [`UiAction::EndMerge`]. Esc during a drag restores the markers.
//! * Left/Right (`editor.select_prev_note` / `next`) select the previous or next marker; Up and
//!   Down open the previous or next audio clip of the song in the same mode. Esc closes.
//!
//! The ctrl/cmd wheel zooms around the pointer, the wheel scrolls. Double-clicking the title
//! renames the asset (`Command::RenameAsset`).

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::{CommandOutcome, GestureClass, Os, Role, Roles};
use crate::sample_editor::TOP_BAR_HEIGHT;
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::timeline::{Bars, MAX_PX_PER_TICK, TimeAxis};
use crate::tooltip::Help;
use atomik_audio_file::PeakPyramid;
use atomik_device_sdk::Registry;
use atomik_model::audition_span;
use atomik_model::slice_edit::{self, Side};
use atomik_model::{
    AssetMeta, AudioClipContent, ClipContent, ClipId, Command, DeviceKey, RackDoc, Song,
    SongCommand, WarpMarker, source_at_tick, warped_at_song_tempo,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// Height of the toolbar band.
pub const TOOLBAR_HEIGHT: f64 = 32.0;
/// Height of the ruler.
pub const RULER_HEIGHT: f64 = 20.0;
/// Height of the footer band.
pub const FOOTER_HEIGHT: f64 = 20.0;

const PAD: f64 = 6.0;
const EDGE_PX: f64 = 5.0;
const DRAG_THRESHOLD: f64 = 2.0;
const SCROLL_STEP: f64 = 36.0;
const ZOOM_STEP: f64 = 1.15;
const MERGE_BASE: u64 = 13 << 40;
const NAME_MAX_CHARS: usize = 200;
/// How close, in logical pixels, a double-click must be to a detected transient to snap to it.
const SNAP_PX: f64 = 8.0;
/// The transient detector's sensitivity at start, in percent, and the step of its buttons.
const DEFAULT_SENSITIVITY: u8 = 50;
const SENSITIVITY_STEP: u8 = 10;

/// The editing mode of the audio clip editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioMode {
    /// The clip and its markers, read-only.
    #[default]
    Overview,
    /// Edit the warp markers.
    Slice,
    /// Listed, not available yet.
    Vocal,
    /// Listed, not available yet.
    Comp,
}

impl AudioMode {
    /// Every mode, in toolbar order.
    pub const ALL: [AudioMode; 4] = [
        AudioMode::Overview,
        AudioMode::Slice,
        AudioMode::Vocal,
        AudioMode::Comp,
    ];

    /// The toolbar label.
    pub fn label(self) -> &'static str {
        match self {
            AudioMode::Overview => "OVERVIEW",
            AudioMode::Slice => "SLICE",
            AudioMode::Vocal => "VOCAL",
            AudioMode::Comp => "COMP",
        }
    }

    /// The id stored in `ui-state.json`.
    pub fn id(self) -> &'static str {
        match self {
            AudioMode::Overview => "overview",
            AudioMode::Slice => "slice",
            AudioMode::Vocal => "vocal",
            AudioMode::Comp => "comp",
        }
    }

    /// The mode with this id.
    pub fn from_id(id: &str) -> Option<AudioMode> {
        AudioMode::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Whether the mode can be chosen.
    pub fn is_enabled(self) -> bool {
        matches!(self, AudioMode::Overview | AudioMode::Slice)
    }
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    Title,
    Mode(AudioMode),
    Stretch,
    Slide,
    Revert,
    Split,
    ToSlicer,
    SensDown,
    SensUp,
    AddAll,
    Speaker,
    /// An interior marker, by its index in the framed list.
    Marker(usize),
    Wave,
}

/// How a marker drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveKind {
    Stretch,
    Slide,
    Group,
}

#[derive(Debug, Clone)]
struct MarkerDrag {
    /// Index in the framed list.
    index: usize,
    /// Pointer x minus marker x at the press.
    grab: f64,
    start: Point,
    /// The warp settings at the press: every move is computed from them.
    orig: atomik_model::WarpSettings,
    /// The markers last sent (or the clip's at the press).
    last: Vec<WarpMarker>,
    merge: u64,
    moved: bool,
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    Marker(Box<MarkerDrag>),
    /// The speaker is held on the waveform: the audition stops on the release.
    Audition,
}

/// The clip being edited, as the document holds it.
#[derive(Debug, Clone, PartialEq)]
struct ClipInfo {
    track: DeviceKey,
    start: i64,
    length: i64,
    muted: bool,
    content: AudioClipContent,
}

/// The rectangles of the view, from its bounds.
#[derive(Debug, Clone, Copy)]
struct Layout {
    top: Rect,
    close: Rect,
    title: Rect,
    bar: Rect,
    modes: [Rect; 4],
    stretch: Rect,
    slide: Rect,
    revert: Rect,
    split: Rect,
    to_slicer: Rect,
    sens_down: Rect,
    sens_up: Rect,
    add_all: Rect,
    speaker: Rect,
    ruler: Rect,
    wave: Rect,
    footer: Rect,
}

/// The audio clip editor. See the module documentation.
#[derive(Debug)]
pub struct AudioEditorView {
    st: SeqTheme,
    roles: Roles,
    bounds: Rect,
    scale: f64,
    clip: Option<ClipId>,
    present: bool,
    info: Option<ClipInfo>,
    track_name: String,
    accent: Color,
    meta: Option<AssetMeta>,
    rate: u32,
    bars: Bars,
    /// `(ticks from the clip start, source frames from its offset)`, ascending in tick: where
    /// the audio of the clip plays. A reversed clip's frames descend.
    curve: Vec<(f64, f64)>,
    /// The clip's markers with its two ends (see [`slice_edit::framed`]); empty without warp.
    framed: Vec<WarpMarker>,
    mode: AudioMode,
    selected: Option<usize>,
    slide: bool,
    zoom: f64,
    scroll: f64,
    peaks: BTreeMap<String, Arc<PeakPyramid>>,
    samples: BTreeMap<String, Arc<[Vec<f32>]>>,
    missing: BTreeSet<String>,
    /// Detected transients as asset frames, drawn on the ruler when the host supplies them.
    transients: Vec<u64>,
    /// The transient detector's sensitivity in percent.
    sensitivity: u8,
    /// The speaker is on: a press on the waveform plays it instead of editing.
    speaker: bool,
    drag: Drag,
    hover: Hit,
    pointer: Option<Point>,
    rename: Option<String>,
    status: String,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl AudioEditorView {
    /// Switches to `theme`: the colours are derived again and the view repaints (the accent
    /// follows at the next sync).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.accent = self.st.base.highlight;
        self.repaint = true;
    }

    /// A closed editor; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            roles: Roles::defaults(Os::host()),
            bounds: Rect::new(0.0, 0.0, 1000.0, 560.0),
            scale: 1.0,
            clip: None,
            present: false,
            info: None,
            track_name: String::new(),
            accent,
            meta: None,
            rate: 48_000,
            bars: Bars::default(),
            curve: Vec::new(),
            framed: Vec::new(),
            mode: AudioMode::Overview,
            selected: None,
            slide: false,
            zoom: 1.0,
            scroll: 0.0,
            peaks: BTreeMap::new(),
            samples: BTreeMap::new(),
            missing: BTreeSet::new(),
            transients: Vec::new(),
            sensitivity: DEFAULT_SENSITIVITY,
            speaker: false,
            drag: Drag::None,
            hover: Hit::None,
            pointer: None,
            rename: None,
            status: String::new(),
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on an audio clip. The mode, the peaks and the missing-asset set stay.
    pub fn open(&mut self, clip: ClipId) {
        if self.clip != Some(clip) {
            self.selected = None;
            self.info = None;
            self.curve.clear();
            self.framed.clear();
            self.transients.clear();
            self.rename = None;
            self.zoom = 1.0;
            self.scroll = 0.0;
        }
        self.clip = Some(clip);
        self.present = false;
        self.drag = Drag::None;
        self.status.clear();
        self.repaint = true;
    }

    /// Closes the editor (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.clip = None;
        self.present = false;
        self.info = None;
        self.curve.clear();
        self.framed.clear();
        self.transients.clear();
        self.meta = None;
        self.selected = None;
        self.drag = Drag::None;
        self.rename = None;
        self.repaint = true;
    }

    /// The clip being edited.
    pub fn clip(&self) -> Option<ClipId> {
        self.clip
    }

    /// True while the open clip exists in the document and is an audio clip. When it turns
    /// false (the clip was deleted), the host shows the arrangement again.
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// The editing mode.
    pub fn mode(&self) -> AudioMode {
        self.mode
    }

    /// Sets the mode without touching the document (the persisted choice on start-up). A mode
    /// that is not available is ignored.
    pub fn set_mode(&mut self, mode: AudioMode) {
        if mode.is_enabled() && mode != self.mode {
            self.mode = mode;
            self.drag = Drag::None;
            self.repaint = true;
        }
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_view();
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the view as focused (thin top line).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// Gives the view the modifier role table of the keymap.
    pub fn set_roles(&mut self, roles: Roles) {
        self.roles = roles;
    }

    /// True while a pointer gesture is in progress (keep routing pointer events here).
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// True while the asset name is being typed: the view is the text owner.
    pub fn is_text_owner(&self) -> bool {
        self.rename.is_some()
    }

    /// The asset name being typed.
    pub fn rename_text(&self) -> Option<&str> {
        self.rename.as_deref()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Sets the waveform peaks of an asset (its content hash), replacing earlier ones.
    pub fn set_peaks(&mut self, asset: &str, peaks: Arc<PeakPyramid>) {
        self.peaks.insert(asset.to_string(), peaks);
        self.repaint = true;
    }

    /// Sets the samples of an asset held in RAM, which the waveform reads below the finest peak
    /// level. Assets without samples (streamed from disk) keep the finest peak level.
    pub fn set_samples(&mut self, asset: &str, samples: Arc<[Vec<f32>]>) {
        self.samples.insert(asset.to_string(), samples);
        self.repaint = true;
    }

    /// Forgets the peaks and samples of an asset.
    pub fn clear_peaks(&mut self, asset: &str) {
        self.peaks.remove(asset);
        self.samples.remove(asset);
        self.repaint = true;
    }

    /// Sets the assets whose file is missing.
    pub fn set_missing_assets(&mut self, assets: impl IntoIterator<Item = String>) {
        let missing: BTreeSet<String> = assets.into_iter().collect();
        if missing != self.missing {
            self.missing = missing;
            self.repaint = true;
        }
    }

    /// Supplies the detected transients of the open clip, as asset frames, to draw on the ruler
    /// (the place where Add at Transient will work from). An empty list draws none.
    pub fn set_transients(&mut self, frames: Vec<u64>) {
        if frames != self.transients {
            self.transients = frames;
            self.repaint = true;
        }
    }

    /// The detected transients of the open clip, as asset frames.
    pub fn transients(&self) -> &[u64] {
        &self.transients
    }

    /// The transient detector's sensitivity in percent (0..=100).
    pub fn sensitivity(&self) -> u8 {
        self.sensitivity
    }

    /// Sets the sensitivity (the persisted choice on start-up) without asking for a detection.
    pub fn set_sensitivity(&mut self, percent: u8) {
        let percent = percent.min(100);
        if percent != self.sensitivity {
            self.sensitivity = percent;
            self.repaint = true;
        }
    }

    /// Whether the speaker is on: a press on the waveform plays the audio under it.
    pub fn speaker(&self) -> bool {
        self.speaker
    }

    /// Switches the speaker on or off.
    pub fn set_speaker(&mut self, on: bool) {
        if on != self.speaker {
            self.speaker = on;
            self.repaint = true;
        }
    }

    /// The marker the pointer is over or has selected, for tests and the host: the framed list.
    pub fn markers(&self) -> &[WarpMarker] {
        &self.framed
    }

    /// The selected marker, as an index in [`Self::markers`].
    pub fn selected_marker(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a marker by its index in [`Self::markers`] (an end of the clip cannot be selected).
    pub fn set_selected_marker(&mut self, index: Option<usize>) {
        let n = self.framed.len();
        self.selected = index.filter(|i| *i >= 1 && *i + 1 < n);
        self.repaint = true;
    }

    /// The text of the last refusal or hint shown in the footer.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The waveform's rectangle.
    pub fn wave_rect(&self) -> Rect {
        self.layout().wave
    }

    /// The logical x of a tick from the clip start.
    pub fn tick_x(&self, tick: i64) -> f64 {
        self.x_of(tick as f64)
    }

    /// Centre of the mode button.
    pub fn mode_button_center(&self, mode: AudioMode) -> Point {
        let i = AudioMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
        self.layout().modes[i].center()
    }

    /// Centre of the Revert button.
    pub fn revert_button_center(&self) -> Point {
        self.layout().revert.center()
    }

    /// Centre of the Split button.
    pub fn split_button_center(&self) -> Point {
        self.layout().split.center()
    }

    /// Centre of the To Slicer button.
    pub fn to_slicer_button_center(&self) -> Point {
        self.layout().to_slicer.center()
    }

    /// Centre of the sensitivity "less" button.
    pub fn sensitivity_down_center(&self) -> Point {
        self.layout().sens_down.center()
    }

    /// Centre of the sensitivity "more" button.
    pub fn sensitivity_up_center(&self) -> Point {
        self.layout().sens_up.center()
    }

    /// Centre of the Add All button (a marker at every detected transient).
    pub fn add_all_button_center(&self) -> Point {
        self.layout().add_all.center()
    }

    /// Centre of the speaker toggle.
    pub fn speaker_button_center(&self) -> Point {
        self.layout().speaker.center()
    }

    /// Centre of the title (double-click renames).
    pub fn title_center(&self) -> Point {
        self.layout().title.center()
    }

    // ------------------------------------------------------------------ geometry

    fn layout(&self) -> Layout {
        let b = self.bounds;
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Rect::new(x0, y0, x1.max(x0), y1.max(y0));
        let top = rect(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        let close = Rect::new(b.x1 - 30.0, b.y0 + 5.0, b.x1 - 8.0, b.y0 + 23.0);
        let title = rect(b.x0 + 100.0, b.y0 + 3.0, close.x0 - 8.0, b.y0 + 25.0);
        let bar = rect(b.x0, top.y1, b.x1, top.y1 + TOOLBAR_HEIGHT);
        let (y0, y1) = (bar.y0 + 5.0, bar.y1 - 5.0);
        let mut x = b.x0 + PAD + 34.0;
        let mut modes = [Rect::ZERO; 4];
        for (i, m) in AudioMode::ALL.iter().enumerate() {
            let w = if *m == AudioMode::Overview {
                78.0
            } else {
                56.0
            };
            modes[i] = Rect::new(x, y0, x + w, y1);
            x += w + 4.0;
        }
        x += 14.0 + 34.0;
        let stretch = Rect::new(x, y0, x + 66.0, y1);
        let slide = Rect::new(stretch.x1 + 4.0, y0, stretch.x1 + 56.0, y1);
        x = slide.x1 + 18.0;
        let revert = Rect::new(x, y0, x + 58.0, y1);
        let split = Rect::new(revert.x1 + 4.0, y0, revert.x1 + 50.0, y1);
        let to_slicer = Rect::new(split.x1 + 4.0, y0, split.x1 + 86.0, y1);
        let sens_down = Rect::new(to_slicer.x1 + 52.0, y0, to_slicer.x1 + 72.0, y1);
        let sens_up = Rect::new(sens_down.x1 + 36.0, y0, sens_down.x1 + 56.0, y1);
        let add_all = Rect::new(sens_up.x1 + 12.0, y0, sens_up.x1 + 76.0, y1);
        let speaker = Rect::new(b.x1 - 78.0, y0, b.x1 - 8.0, y1);
        let footer = rect(b.x0, b.y1 - FOOTER_HEIGHT, b.x1, b.y1);
        let ruler = rect(
            b.x0 + PAD,
            bar.y1 + 2.0,
            b.x1 - PAD,
            bar.y1 + 2.0 + RULER_HEIGHT,
        );
        let wave = rect(ruler.x0, ruler.y1, ruler.x1, footer.y0 - 2.0);
        Layout {
            top,
            close,
            title,
            bar,
            modes,
            stretch,
            slide,
            revert,
            split,
            to_slicer,
            sens_down,
            sens_up,
            add_all,
            speaker,
            ruler,
            wave,
            footer,
        }
    }

    fn length(&self) -> i64 {
        self.info.as_ref().map_or(0, |i| i.length)
    }

    fn start(&self) -> i64 {
        self.info.as_ref().map_or(0, |i| i.start)
    }

    /// Logical pixels per tick at zoom 1: the clip fills the waveform.
    fn fit(&self) -> f64 {
        self.layout().wave.width().max(1.0) / self.length().max(1) as f64
    }

    /// The time axis in song ticks: `x = wave.x0 + (tick - clip start - scroll) · px_per_tick`.
    fn axis(&self) -> TimeAxis {
        TimeAxis {
            x0: self.layout().wave.x0,
            px_per_tick: self.fit() * self.zoom,
            scroll: self.start() as f64 + self.scroll,
        }
    }

    /// Logical x of a tick from the clip start.
    fn x_of(&self, tick: f64) -> f64 {
        let a = self.axis();
        a.x0 + (tick - self.scroll) * a.px_per_tick
    }

    /// The tick from the clip start at logical x.
    fn tick_of(&self, x: f64) -> f64 {
        let a = self.axis();
        self.scroll + (x - a.x0) / a.px_per_tick
    }

    fn max_zoom(&self) -> f64 {
        (MAX_PX_PER_TICK / self.fit()).max(1.0)
    }

    fn clamp_view(&mut self) {
        self.zoom = self.zoom.clamp(1.0, self.max_zoom());
        let span = self.length() as f64 / self.zoom;
        let max = (self.length() as f64 - span).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// Whether markers can be edited: the clip is open, has its asset and is not reversed.
    fn editable(&self) -> bool {
        self.present
            && self.meta.is_some()
            && self.info.as_ref().is_some_and(|i| !i.content.reverse)
            && self.length() >= 2
    }

    fn slicing(&self) -> bool {
        self.mode == AudioMode::Slice && self.editable()
    }

    /// The tick from the clip start at which the clip plays the source frame `source` (from its
    /// offset), by the time curve; `None` when the clip never plays it.
    fn tick_of_source(&self, source: f64) -> Option<f64> {
        self.curve.windows(2).find_map(|p| {
            let ((t0, s0), (t1, s1)) = (p[0], p[1]);
            let (lo, hi) = (s0.min(s1), s0.max(s1));
            (source >= lo && source <= hi && (s1 - s0).abs() > f64::EPSILON)
                .then(|| t0 + (source - s0) / (s1 - s0) * (t1 - t0))
        })
    }

    /// The line of help the footer shows when there is no refusal to show.
    fn hint(&self) -> String {
        let keys = |role: Role| {
            self.roles
                .binding(role)
                .map_or_else(String::new, |b| b.mods.menu_text(Os::host()))
        };
        match self.mode {
            AudioMode::Slice if self.editable() => format!(
                "Double-click adds a marker  ·  drag stretches  ·  {} slides  ·  {} moves the markers after it  ·  Delete removes",
                keys(Role::AlternateTool),
                keys(Role::Constrain)
            ),
            AudioMode::Slice => String::new(),
            _ => "Slice mode edits the warp markers of the clip".to_owned(),
        }
    }

    /// The interior markers (every marker but the two ends) of the framed list.
    fn interior(&self) -> std::ops::Range<usize> {
        1..self.framed.len().saturating_sub(1)
    }

    fn merge_id(&mut self) -> u64 {
        let id = self.next_merge;
        self.next_merge += 1;
        id
    }

    // ------------------------------------------------------------------ sync

    /// Reads the clip from the document.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let Some(id) = self.clip else { return };
        let found = doc
            .song
            .clip(id)
            .and_then(|(track, _, clip)| match &clip.content {
                ClipContent::Audio(a) => Some((track, clip, a)),
                _ => None,
            });
        let Some((track, clip, a)) = found else {
            if self.present {
                self.present = false;
                self.info = None;
                self.curve.clear();
                self.framed.clear();
                self.selected = None;
                self.drag = Drag::None;
                self.rename = None;
                self.repaint = true;
            }
            return;
        };
        let info = ClipInfo {
            track: track.device.clone(),
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content: a.clone(),
        };
        let meta = doc.assets.get(&a.asset).cloned();
        let name = crate::seq_paint::track_name(doc, &track.device);
        let accent = doc
            .device(&track.device)
            .map_or(self.st.base.highlight, |d| self.st.base.accent_for(d));
        let bars = Bars::of(&doc.song);
        if !self.present
            || self.info.as_ref() != Some(&info)
            || self.meta != meta
            || self.track_name != name
            || self.accent != accent
            || self.rate != doc.sample_rate
            || self.bars != bars
        {
            self.present = true;
            self.info = Some(info);
            self.meta = meta;
            self.track_name = name;
            self.accent = accent;
            self.rate = doc.sample_rate;
            self.bars = bars;
            self.rebuild(&doc.song);
            self.repaint = true;
        }
    }

    fn rebuild(&mut self, song: &Song) {
        let Some(info) = &self.info else { return };
        let a = &info.content;
        let length = info.length;
        self.framed.clear();
        self.curve.clear();
        if a.reverse {
            let span = source_at_tick(song, self.rate, info.start, a, info.start + length)
                - a.source_offset as f64;
            self.curve = vec![(0.0, span.max(0.0)), (length as f64, 0.0)];
        } else if a.warp.enabled {
            self.framed = slice_edit::framed(&a.warp, self.rate, length);
            self.curve = self
                .framed
                .iter()
                .map(|m| (m.tick as f64, m.source as f64))
                .collect();
        } else {
            let span = source_at_tick(song, self.rate, info.start, a, info.start + length)
                - a.source_offset as f64;
            self.curve = vec![(0.0, 0.0), (length as f64, span.max(0.0))];
        }
        if let Some(s) = self.selected
            && !(1..self.framed.len().saturating_sub(1)).contains(&s)
        {
            self.selected = None;
        }
        self.clamp_view();
    }

    // ------------------------------------------------------------------ hit testing

    fn hit(&self, pos: Point) -> Hit {
        if !self.present {
            return if self.layout().close.contains(pos) {
                Hit::Close
            } else {
                Hit::None
            };
        }
        let l = self.layout();
        if l.close.contains(pos) {
            return Hit::Close;
        }
        if l.title.contains(pos) {
            return Hit::Title;
        }
        for (i, m) in AudioMode::ALL.iter().enumerate() {
            if l.modes[i].contains(pos) {
                return Hit::Mode(*m);
            }
        }
        if l.speaker.contains(pos) {
            return Hit::Speaker;
        }
        if self.mode == AudioMode::Slice {
            for (r, h) in [
                (l.stretch, Hit::Stretch),
                (l.slide, Hit::Slide),
                (l.revert, Hit::Revert),
                (l.split, Hit::Split),
                (l.to_slicer, Hit::ToSlicer),
                (l.sens_down, Hit::SensDown),
                (l.sens_up, Hit::SensUp),
                (l.add_all, Hit::AddAll),
            ] {
                if r.contains(pos) {
                    return h;
                }
            }
        }
        if (l.wave.contains(pos) || l.ruler.contains(pos)) && self.slicing() {
            let nearest = self
                .interior()
                .map(|i| (i, (self.x_of(self.framed[i].tick as f64) - pos.x).abs()))
                .filter(|(_, d)| *d <= EDGE_PX)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = nearest {
                return Hit::Marker(i);
            }
        }
        if l.wave.contains(pos) {
            return Hit::Wave;
        }
        Hit::None
    }

    /// What the button under `pos` does, for its tooltip (`None` during a gesture and away from
    /// a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) || !self.present {
            return None;
        }
        let l = self.layout();
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit(pos) {
            Hit::Close => help(l.close, "Close the audio clip editor"),
            Hit::Title => help(l.title, "Double-click to rename the audio file"),
            Hit::Mode(m) => {
                let i = AudioMode::ALL.iter().position(|x| *x == m)?;
                match m {
                    AudioMode::Overview => Some(
                        Help::new(l.modes[i], "Look at the clip and its markers")
                            .with_command("editor.audio_mode_overview"),
                    ),
                    AudioMode::Slice => Some(
                        Help::new(l.modes[i], "Edit the warp markers of the clip")
                            .with_command("editor.audio_mode_slice"),
                    ),
                    _ => help(l.modes[i], "Not available yet"),
                }
            }
            Hit::Stretch => help(l.stretch, "Dragging a marker stretches the audio around it"),
            Hit::Slide => help(
                l.slide,
                "Dragging a marker slides it along the audio without changing the sound",
            ),
            Hit::Revert => Some(
                Help::new(
                    l.revert,
                    "Remove every marker: the clip plays at its natural speed",
                )
                .with_command("editor.audio_revert"),
            ),
            Hit::Split => Some(
                Help::new(l.split, "Cut the clip at every marker")
                    .with_command("editor.audio_split_markers"),
            ),
            Hit::ToSlicer => Some(
                Help::new(
                    l.to_slicer,
                    "Make a loop slicer that slices the clip at its markers",
                )
                .with_command("editor.audio_to_slicer"),
            ),
            Hit::SensDown => help(l.sens_down, "Find fewer transients"),
            Hit::SensUp => help(l.sens_up, "Find more transients"),
            Hit::AddAll => Some(
                Help::new(l.add_all, "Put a marker on every detected transient")
                    .with_command("editor.audio_add_transients"),
            ),
            Hit::Speaker => Some(
                Help::new(
                    l.speaker,
                    "Press on the clip to hear it as it is in its file, release to stop",
                )
                .with_command("tool.speaker"),
            ),
            Hit::Marker(_) | Hit::Wave | Hit::None => None,
        }
    }

    // ------------------------------------------------------------------ events

    /// Handles one event; returns the actions to apply.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
        let mut out = Vec::new();
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return out;
        }
        if self.clip.is_none() {
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => self.pointer_down(*pos, *button, *modifiers, *clicks, doc, &mut out),
            UiEvent::PointerUp { .. } => self.pointer_up(&mut out),
            UiEvent::PointerLeave => {
                self.hover = Hit::None;
                self.pointer = None;
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => self.wheel(delta.x, delta.y, *pos, *modifiers),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, doc, &mut out),
            _ => {}
        }
        out
    }

    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        _modifiers: Modifiers,
        clicks: u8,
        doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        let hit = self.hit(pos);
        // A click anywhere but on the name being typed commits it.
        if self.rename.is_some() && hit != Hit::Title {
            self.finish_rename(true, out);
        }
        if !matches!(self.drag, Drag::None) || button != PointerButton::Primary {
            return;
        }
        self.status.clear();
        match hit {
            Hit::Close => out.push(UiAction::CloseAudioEditor),
            Hit::Title => {
                if clicks >= 2 && self.rename.is_none() {
                    self.begin_rename();
                }
            }
            Hit::Mode(m) => self.choose_mode(m, doc, out),
            Hit::Stretch => self.slide = false,
            Hit::Slide => self.slide = true,
            Hit::Revert => self.revert(out),
            Hit::Split => self.split(doc, out),
            Hit::ToSlicer => self.send_to_slicer(out),
            Hit::SensDown => self.step_sensitivity(false, out),
            Hit::SensUp => self.step_sensitivity(true, out),
            Hit::AddAll => self.add_all_transients(doc, out),
            Hit::Speaker => self.speaker = !self.speaker,
            Hit::Marker(_) | Hit::Wave if self.speaker => self.start_audition(pos, doc, out),
            Hit::Marker(i) => {
                self.selected = Some(i);
                let Some(info) = &self.info else { return };
                let grab = self.x_of(self.framed[i].tick as f64) - pos.x;
                let orig = info.content.warp.clone();
                let last = orig.markers.clone();
                let merge = self.merge_id();
                self.drag = Drag::Marker(Box::new(MarkerDrag {
                    index: i,
                    grab,
                    start: pos,
                    orig,
                    last,
                    merge,
                    moved: false,
                    emitted: false,
                }));
            }
            Hit::Wave => {
                if clicks >= 2 && self.slicing() {
                    self.add_marker_at(pos.x, doc, out);
                } else {
                    self.selected = None;
                }
            }
            Hit::None => {}
        }
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.pointer = Some(pos);
        if matches!(self.drag, Drag::None) {
            let h = self.hit(pos);
            if h != self.hover {
                self.hover = h;
                self.repaint = true;
            }
            return;
        }
        self.repaint = true;
        let Some(id) = self.clip else { return };
        let kind = self.move_kind(modifiers);
        let (rate, length) = (self.rate, self.length());
        let tick = match &self.drag {
            Drag::Marker(d) => self.tick_of(pos.x + d.grab).round() as i64,
            Drag::None | Drag::Audition => return,
        };
        let Drag::Marker(d) = &mut self.drag else {
            return;
        };
        d.moved |= pos.distance(d.start) > DRAG_THRESHOLD;
        if !d.moved {
            return;
        }
        let result = match kind {
            MoveKind::Stretch => slice_edit::move_stretch(&d.orig, rate, length, d.index, tick),
            MoveKind::Slide => slice_edit::move_decoupled(&d.orig, rate, length, d.index, tick),
            MoveKind::Group => {
                slice_edit::move_group(&d.orig, rate, length, d.index, tick, Side::After)
            }
        };
        match result {
            Ok(list) if list != d.last => {
                d.last.clone_from(&list);
                d.emitted = true;
                out.push(UiAction::Command(Command::Song(
                    SongCommand::SetWarpMarkers {
                        id,
                        markers: list,
                        merge: Some(d.merge),
                    },
                )));
            }
            Ok(_) => {}
            Err(e) => self.status = e.to_string(),
        }
    }

    /// The way a marker drag moves with `modifiers` held: the constrain role carries the markers
    /// after it along, the alternate-tool role (or the Slide toggle) slides it along the audio.
    fn move_kind(&self, modifiers: Modifiers) -> MoveKind {
        let held = |role| {
            self.roles
                .is_active(role, modifiers, GestureClass::DragItem)
                || self.roles.is_active(role, modifiers, GestureClass::Hold)
        };
        if held(Role::Constrain) {
            MoveKind::Group
        } else if self.slide != held(Role::AlternateTool) {
            MoveKind::Slide
        } else {
            MoveKind::Stretch
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if matches!(self.drag, Drag::Audition) {
            self.drag = Drag::None;
            out.push(UiAction::StopAudition);
            return;
        }
        if let Drag::Marker(d) = std::mem::replace(&mut self.drag, Drag::None)
            && d.emitted
        {
            out.push(UiAction::EndMerge);
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        if matches!(self.drag, Drag::Audition) {
            self.drag = Drag::None;
            out.push(UiAction::StopAudition);
            return;
        }
        let Drag::Marker(d) = std::mem::replace(&mut self.drag, Drag::None) else {
            return;
        };
        if d.emitted
            && let Some(id) = self.clip
        {
            out.push(UiAction::Command(Command::Song(
                SongCommand::SetWarpMarkers {
                    id,
                    markers: d.orig.markers.clone(),
                    merge: Some(d.merge),
                },
            )));
            out.push(UiAction::EndMerge);
        }
    }

    fn wheel(&mut self, dx: f64, dy: f64, pos: Point, modifiers: Modifiers) {
        self.repaint = true;
        let l = self.layout();
        if !(l.wave.contains(pos) || l.ruler.contains(pos)) || self.length() < 1 {
            return;
        }
        if modifiers.command() {
            let anchor = self.tick_of(pos.x);
            self.zoom = (self.zoom * ZOOM_STEP.powf(dy)).clamp(1.0, self.max_zoom());
            let ppt = self.fit() * self.zoom;
            self.scroll = anchor - (pos.x - l.wave.x0) / ppt;
        } else {
            let d = if dx.abs() > 1e-9 { dx } else { dy };
            self.scroll -= d * SCROLL_STEP / (self.fit() * self.zoom);
        }
        self.clamp_view();
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, doc: &RackDoc, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if self.rename.is_some() {
            self.rename_key(key, modifiers, out);
            return;
        }
        // Command chords (undo, redo, clipboard) are registry commands the shell resolves;
        // what no command took is not a key of the view.
        if modifiers.command() {
            return;
        }
        match key {
            Key::Escape => {
                if !matches!(self.drag, Drag::None) {
                    self.cancel_drag(out);
                } else if self.selected.is_some() {
                    self.selected = None;
                } else {
                    out.push(UiAction::CloseAudioEditor);
                }
            }
            Key::Up => out.extend(self.neighbour(doc, false)),
            Key::Down => out.extend(self.neighbour(doc, true)),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ rename

    fn begin_rename(&mut self) {
        if let Some(m) = &self.meta {
            self.rename = Some(m.original_name.clone());
            self.drag = Drag::None;
        }
    }

    fn rename_key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        match key {
            Key::Enter => self.finish_rename(true, out),
            Key::Escape => self.finish_rename(false, out),
            Key::Backspace => {
                if let Some(t) = &mut self.rename {
                    t.pop();
                }
            }
            Key::Char(c) if !modifiers.command() && !c.is_control() => {
                if let Some(t) = &mut self.rename
                    && t.chars().count() < NAME_MAX_CHARS
                {
                    t.push(*c);
                }
            }
            Key::Space => {
                if let Some(t) = &mut self.rename
                    && t.chars().count() < NAME_MAX_CHARS
                {
                    t.push(' ');
                }
            }
            _ => {}
        }
    }

    /// Ends the rename: committing emits `RenameAsset` when the name is not empty and changed.
    fn finish_rename(&mut self, commit: bool, out: &mut Vec<UiAction>) {
        let Some(text) = self.rename.take() else {
            return;
        };
        self.repaint = true;
        let name = text.trim();
        if let (true, Some(info), Some(meta)) = (commit, &self.info, &self.meta)
            && !name.is_empty()
            && name != meta.original_name
        {
            out.push(UiAction::Command(Command::RenameAsset {
                asset: info.content.asset.clone(),
                name: name.to_string(),
            }));
        }
    }

    // ------------------------------------------------------------------ editing

    /// Chooses a mode. Slice on a clip without warp switches warp on at the song tempo (one undo
    /// step); a reversed clip says why it cannot be sliced.
    fn choose_mode(&mut self, mode: AudioMode, doc: &RackDoc, out: &mut Vec<UiAction>) {
        if !mode.is_enabled() || mode == self.mode {
            return;
        }
        self.mode = mode;
        self.drag = Drag::None;
        self.selected = None;
        if mode != AudioMode::Slice {
            return;
        }
        let Some(info) = &self.info else { return };
        if info.content.reverse {
            self.status = "A reversed clip cannot be warped, so it cannot be sliced".into();
            return;
        }
        if !info.content.warp.enabled
            && let Some(cmd) = self.enable_warp_command(doc)
        {
            out.push(UiAction::Command(cmd));
        }
        out.extend(self.detect_action());
    }

    /// The command that switches warp on at the song tempo (`None` when it is on).
    fn enable_warp_command(&self, doc: &RackDoc) -> Option<Command> {
        let (id, info) = (self.clip?, self.info.as_ref()?);
        if info.content.warp.enabled || info.content.reverse {
            return None;
        }
        Some(self.set_content(
            id,
            info,
            warped_at_song_tempo(&doc.song, info.start, &info.content),
        ))
    }

    fn set_content(&self, id: ClipId, info: &ClipInfo, content: AudioClipContent) -> Command {
        Command::Song(SongCommand::SetAudioClip {
            id,
            start: info.start,
            length: info.length,
            muted: info.muted,
            content,
            merge: None,
        })
    }

    /// The command that gives the clip `markers`: one `SetWarpMarkers`, or one `SetAudioClip`
    /// that also switches warp on when it is off.
    fn markers_command(
        &self,
        doc: &RackDoc,
        markers: Vec<WarpMarker>,
        merge: Option<u64>,
    ) -> Option<Command> {
        let (id, info) = (self.clip?, self.info.as_ref()?);
        if info.content.warp.enabled {
            return Some(Command::Song(SongCommand::SetWarpMarkers {
                id,
                markers,
                merge,
            }));
        }
        let mut content = warped_at_song_tempo(&doc.song, info.start, &info.content);
        content.warp.markers = markers;
        let Command::Song(SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted,
            content,
            ..
        }) = self.set_content(id, info, content)
        else {
            return None;
        };
        Some(Command::Song(SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted,
            content,
            merge,
        }))
    }

    /// The warp settings the marker operations start from: the clip's, or what switching warp on
    /// would give.
    fn working_warp(&self, doc: &RackDoc) -> Option<atomik_model::WarpSettings> {
        let info = self.info.as_ref()?;
        Some(warped_at_song_tempo(&doc.song, info.start, &info.content).warp)
    }

    /// The request for the transients of the open clip, while Slice mode edits it.
    fn detect_action(&self) -> Option<UiAction> {
        let clip = self.clip.filter(|_| self.slicing())?;
        Some(UiAction::DetectTransients {
            clip,
            sensitivity: self.sensitivity,
        })
    }

    /// Moves the sensitivity one step and asks for the transients again.
    fn step_sensitivity(&mut self, up: bool, out: &mut Vec<UiAction>) {
        let next = if up {
            self.sensitivity.saturating_add(SENSITIVITY_STEP).min(100)
        } else {
            self.sensitivity.saturating_sub(SENSITIVITY_STEP)
        };
        if next != self.sensitivity {
            self.sensitivity = next;
            out.extend(self.detect_action());
        }
    }

    /// The detected transient nearest to logical `x` within [`SNAP_PX`], as source frames from
    /// the clip's offset.
    fn transient_near(&self, x: f64) -> Option<u64> {
        let offset = self.info.as_ref()?.content.source_offset;
        self.transients
            .iter()
            .filter_map(|f| {
                let rel = f.checked_sub(offset)?;
                let tick = self.tick_of_source(rel as f64)?;
                Some((rel, (self.x_of(tick) - x).abs()))
            })
            .filter(|(_, d)| *d <= SNAP_PX)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(rel, _)| rel)
    }

    /// Adds a marker at logical `x`: on the detected transient within reach, else where the
    /// clip plays at `x`.
    fn add_marker_at(&mut self, x: f64, doc: &RackDoc, out: &mut Vec<UiAction>) {
        if !self.slicing() {
            return;
        }
        let Some(warp) = self.working_warp(doc) else {
            return;
        };
        let added = match self.transient_near(x) {
            Some(source) => slice_edit::add_at_source(&warp, self.rate, self.length(), source),
            None => {
                let tick = self.tick_of(x).round() as i64;
                slice_edit::add_at_tick(&warp, self.rate, self.length(), tick)
            }
        };
        match added {
            Ok((list, at)) => {
                if let Some(cmd) = self.markers_command(doc, list, None) {
                    self.selected = Some(at);
                    out.push(UiAction::Command(cmd));
                }
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Puts a marker on every detected transient: one command, one undo step.
    fn add_all_transients(&mut self, doc: &RackDoc, out: &mut Vec<UiAction>) {
        if !self.slicing() {
            return;
        }
        let Some(warp) = self.working_warp(doc) else {
            return;
        };
        let offset = self.info.as_ref().map_or(0, |i| i.content.source_offset);
        let sources: Vec<u64> = self
            .transients
            .iter()
            .filter_map(|f| f.checked_sub(offset))
            .collect();
        if sources.is_empty() {
            self.status = "No transients found: raise the sensitivity".into();
            return;
        }
        match slice_edit::add_at_sources(&warp, self.rate, self.length(), &sources) {
            Ok(list) => {
                if let Some(cmd) = self.markers_command(doc, list, None) {
                    self.selected = None;
                    out.push(UiAction::Command(cmd));
                }
            }
            Err(_) => self.status = "Every transient already has a marker".into(),
        }
    }

    /// Starts the speaker at logical `pos`: the clip's audio from there to its end, as it is in
    /// the file, until the button is released.
    fn start_audition(&mut self, pos: Point, doc: &RackDoc, out: &mut Vec<UiAction>) {
        let (Some(info), Some(meta)) = (&self.info, &self.meta) else {
            return;
        };
        if self.missing.contains(&info.content.asset) || !self.present {
            return;
        }
        let at = info.start + self.tick_of(pos.x).round() as i64;
        let span = audition_span(
            &doc.song,
            self.rate,
            info.start,
            &info.content,
            info.length,
            at,
            meta.frames,
        );
        match span {
            Some(span) => {
                out.push(UiAction::AuditionSpan {
                    asset: info.content.asset.clone(),
                    start_frame: span.start_frame,
                    end_frame: span.end_frame,
                    reverse: span.reverse,
                    gain_db: info.content.gain_db,
                });
                self.drag = Drag::Audition;
            }
            None => self.status = "The clip plays nothing from there".into(),
        }
    }

    fn revert(&mut self, out: &mut Vec<UiAction>) {
        let (Some(id), Some(info)) = (self.clip, &self.info) else {
            return;
        };
        if !self.editable() || info.content.warp.markers.is_empty() {
            return;
        }
        self.selected = None;
        out.push(UiAction::Command(Command::Song(
            SongCommand::SetWarpMarkers {
                id,
                markers: slice_edit::revert(),
                merge: None,
            },
        )));
    }

    /// Cuts the clip at every marker: one `Batch` of `SplitClip` (the right piece of each cut is
    /// the next one to cut), so one undo step.
    fn split(&mut self, doc: &RackDoc, out: &mut Vec<UiAction>) {
        let (Some(id), Some(info)) = (self.clip, &self.info) else {
            return;
        };
        if !self.editable() {
            return;
        }
        let ticks = slice_edit::split_ticks(&info.content.warp, info.length);
        if ticks.is_empty() {
            self.status = "The clip has no marker to cut at".into();
            return;
        }
        let next = doc.song.next_clip_id;
        let mut current = id;
        let mut commands = Vec::with_capacity(ticks.len());
        for (k, tick) in ticks.into_iter().enumerate() {
            commands.push(Command::Song(SongCommand::SplitClip {
                id: current,
                at: info.start + tick,
            }));
            current = ClipId(next + k as u64);
        }
        self.selected = None;
        out.push(UiAction::Command(Command::Batch(commands)));
    }

    fn send_to_slicer(&mut self, out: &mut Vec<UiAction>) {
        let Some(id) = self.clip else { return };
        if !self.editable() {
            return;
        }
        if self.framed.len() <= 2 {
            self.status = "The clip has no marker to slice at".into();
            return;
        }
        out.push(UiAction::ClipToSlicer(id));
    }

    /// Moves the selected marker by `ticks` (a stretch move).
    fn nudge(&mut self, ticks: i64, doc: &RackDoc) -> CommandOutcome {
        let Some(s) = self.selected.filter(|_| self.slicing()) else {
            return CommandOutcome::Declined;
        };
        let Some(warp) = self.info.as_ref().map(|i| i.content.warp.clone()) else {
            return CommandOutcome::Declined;
        };
        match slice_edit::nudge(&warp, self.rate, self.length(), s, ticks) {
            Ok(list) => match self.markers_command(doc, list, None) {
                Some(cmd) => CommandOutcome::Handled(vec![UiAction::Command(cmd)]),
                None => CommandOutcome::Declined,
            },
            Err(e) => CommandOutcome::NotApplicable(e.to_string()),
        }
    }

    /// Removes the selected marker.
    fn delete_selected(&mut self, doc: &RackDoc) -> CommandOutcome {
        let Some(s) = self.selected.filter(|_| self.slicing()) else {
            return CommandOutcome::Declined;
        };
        let Some(warp) = self.info.as_ref().map(|i| i.content.warp.clone()) else {
            return CommandOutcome::Declined;
        };
        match slice_edit::delete(&warp, self.rate, self.length(), s) {
            Ok(list) => {
                self.selected = None;
                self.repaint = true;
                match self.markers_command(doc, list, None) {
                    Some(cmd) => CommandOutcome::Handled(vec![UiAction::Command(cmd)]),
                    None => CommandOutcome::Declined,
                }
            }
            Err(e) => CommandOutcome::NotApplicable(e.to_string()),
        }
    }

    /// The previous or next audio clip of the song (tracks in order, clips by start) as an
    /// `OpenAudioEditor`.
    fn neighbour(&self, doc: &RackDoc, forward: bool) -> Option<UiAction> {
        let id = self.clip?;
        let mut all: Vec<(usize, i64, ClipId)> = Vec::new();
        for (t, track) in doc.song.tracks.iter().enumerate() {
            for lane in &track.lanes {
                for c in &lane.clips {
                    if matches!(c.content, ClipContent::Audio(_)) {
                        all.push((t, c.start, c.id));
                    }
                }
            }
        }
        all.sort_unstable();
        let at = all.iter().position(|(_, _, c)| *c == id)?;
        let to = if forward {
            at.checked_add(1).filter(|i| *i < all.len())?
        } else {
            at.checked_sub(1)?
        };
        Some(UiAction::OpenAudioEditor(all[to].2))
    }

    /// The step of a nudge, in ticks: a sixteenth, a beat or a tick.
    fn nudge_step(&self, size: NudgeSize) -> i64 {
        let beat = self
            .bars
            .beat_ticks_at(self.start() + self.selected_tick().unwrap_or(0))
            .max(4);
        match size {
            NudgeSize::Step => (beat / 4).max(1),
            NudgeSize::Beat => beat,
        }
    }

    fn selected_tick(&self) -> Option<i64> {
        self.selected
            .and_then(|s| self.framed.get(s))
            .map(|m| m.tick)
    }

    /// Runs a registry command the shell offers to this view; every command it does not own is
    /// declined so the next binding runs.
    pub fn command(
        &mut self,
        id: &str,
        _arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync(doc, registry);
        if self.clip.is_none() || !matches!(self.drag, Drag::None) || self.rename.is_some() {
            return CommandOutcome::Declined;
        }
        let mut out = Vec::new();
        self.repaint = true;
        match id {
            "editor.audio_mode_overview" => self.choose_mode(AudioMode::Overview, doc, &mut out),
            "editor.audio_mode_slice" => self.choose_mode(AudioMode::Slice, doc, &mut out),
            "editor.audio_add_marker" => {
                let Some(pos) = self.pointer.filter(|p| self.layout().wave.contains(*p)) else {
                    return CommandOutcome::Declined;
                };
                let before = out.len();
                self.add_marker_at(pos.x, doc, &mut out);
                if out.len() == before && !self.status.is_empty() {
                    return CommandOutcome::NotApplicable(self.status.clone());
                }
            }
            "editor.audio_add_transients" => self.add_all_transients(doc, &mut out),
            "tool.speaker" => self.speaker = !self.speaker,
            "editor.audio_revert" => self.revert(&mut out),
            "editor.audio_split_markers" => self.split(doc, &mut out),
            "editor.audio_to_slicer" => self.send_to_slicer(&mut out),
            "edit.delete" => return self.delete_selected(doc),
            "editor.nudge_left" => return self.nudge(-self.nudge_step(NudgeSize::Step), doc),
            "editor.nudge_right" => return self.nudge(self.nudge_step(NudgeSize::Step), doc),
            "editor.nudge_left_beat" => return self.nudge(-self.nudge_step(NudgeSize::Beat), doc),
            "editor.nudge_right_beat" => return self.nudge(self.nudge_step(NudgeSize::Beat), doc),
            "editor.nudge_left_tick" => return self.nudge(-1, doc),
            "editor.nudge_right_tick" => return self.nudge(1, doc),
            "editor.select_prev_note" => return self.select_step(false),
            "editor.select_next_note" => return self.select_step(true),
            "sequencer.select_prev_track" => {
                return self
                    .neighbour(doc, false)
                    .map_or(CommandOutcome::Declined, |a| {
                        CommandOutcome::Handled(vec![a])
                    });
            }
            "sequencer.select_next_track" => {
                return self
                    .neighbour(doc, true)
                    .map_or(CommandOutcome::Declined, |a| {
                        CommandOutcome::Handled(vec![a])
                    });
            }
            _ => return CommandOutcome::Declined,
        }
        CommandOutcome::Handled(out)
    }

    /// Selects the previous or next interior marker (the first or last one when none is).
    fn select_step(&mut self, forward: bool) -> CommandOutcome {
        let range = self.interior();
        if !self.slicing() || range.is_empty() {
            return CommandOutcome::Declined;
        }
        let next = match (self.selected, forward) {
            (None, true) => range.start,
            (None, false) => range.end - 1,
            (Some(s), true) => (s + 1).min(range.end - 1),
            (Some(s), false) => s.saturating_sub(1).max(range.start),
        };
        self.selected = Some(next);
        self.reveal(next);
        CommandOutcome::Handled(Vec::new())
    }

    /// Scrolls so the marker at `index` is inside the waveform.
    fn reveal(&mut self, index: usize) {
        let Some(m) = self.framed.get(index) else {
            return;
        };
        let w = self.layout().wave;
        let x = self.x_of(m.tick as f64);
        if x < w.x0 + 8.0 || x > w.x1 - 8.0 {
            let ppt = self.fit() * self.zoom;
            self.scroll = m.tick as f64 - w.width() / ppt / 2.0;
            self.clamp_view();
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the editor into its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        paint::paint(self, canvas);
        self.repaint = false;
    }
}

#[derive(Debug, Clone, Copy)]
enum NudgeSize {
    Step,
    Beat,
}
