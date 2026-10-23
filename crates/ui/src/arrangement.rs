//! The arrangement view: a track list, a bar/beat ruler with a loop bar, and
//! lanes of clips on a snapped timeline, with automation editing inside automation clips.
//!
//! Hosting follows [`crate::RackView`]: `sync`, `handle` → [`UiAction`]s, `paint`. The view
//! lives in a sub-rectangle of the window ([`ArrangementView::set_bounds`], logical pixels);
//! events and bounds share the window's logical coordinates. The app passes the transport
//! position in with [`ArrangementView::set_playhead`]; there is no time source.
//!
//! Drags emit `SetClipGeometry` / `SetClipContent` (`SetAudioClip` for audio clips) with a
//! merge id per gesture (ids start at 2⁴⁰ so they never collide with the rack view's knob
//! drags) and finish with [`UiAction::EndMerge`].
//!
//! Audio: audio lanes draw their clips with a waveform from the peaks the host
//! hands in ([`ArrangementView::set_peaks`]; stereo is drawn as two stacked halves, channel 0
//! on top, mono at full height), fade-in / fade-out / gain handles
//! ([`ArrangementView::audio_handle_center`]) and trim edges. Audio track headers carry the
//! arm button, an input selector (cable commands) and a monitor button. File drops
//! ([`UiEvent::FileDrop`]) become [`UiAction::ImportAudio`].

use crate::action::{ContextTarget, ImportTarget, TransportAction, UiAction};
use crate::audio::{self, InputSel, TempoMap};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::{CommandOutcome, Roles};
use crate::marker_edit::RangePart;
use crate::paint::fit_label;
use crate::seq_edit::{self, Selection, SpanPart};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::tempo_lane::{self as lane, TEMPO_LANE_HEIGHT};
use crate::theme::{Theme, mix};
use crate::timeline::{Bars, PPQ, Snap, SnapGrid, TimeAxis, snap_floor, snap_round};
use crate::viewport::{Tool, ViewportState};
use crate::wave_paint::{MAX_COLUMNS, WaveChannel, WaveDetail, paint_channel};
use atomik_audio_file::PeakPyramid;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, AudioClipContent, AutomationPoint, Clip, ClipContent, ClipId, Command, Curve,
    DeviceKey, FadeCurve, LaneKind, MeterEvent, RackDoc, Song, SongCommand, TempoEvent,
    WorkflowCommand,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use vello::kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Stroke};
use vello::peniko::Color;

mod heights;
mod markers;
mod tempo;

pub use heights::{
    COMPACT_BELOW, MAX_ROW_HEIGHT, MAX_TRACK_ZOOM, MIN_ROW_HEIGHT, MIN_TRACK_ZOOM, TRACK_ZOOM_STEP,
    clamp_track_zoom, scaled_height,
};
pub use markers::{MARKER_LIST_WIDTH, MARKER_STRIP_HEIGHT};
pub use tempo::TempoMenu;

/// Width of the track list.
pub const TRACK_LIST_WIDTH: f64 = 208.0;
/// Height of the ruler (bar labels above the loop band).
pub const RULER_HEIGHT: f64 = 34.0;
const LABEL_BAND: f64 = 18.0;
/// Height of a track's note lane.
pub const TRACK_HEIGHT: f64 = 60.0;
/// Height of an automation lane.
pub const LANE_HEIGHT: f64 = 52.0;
/// Height of an "add track" row.
pub const ADD_ROW_HEIGHT: f64 = 28.0;
const CLIP_HEADER: f64 = 12.0;
/// Side of the mute button at the right end of a clip header.
const MUTE_BUTTON: f64 = 10.0;
/// Space the fade-out handle of an audio clip takes at the right end of the header.
const AUDIO_HANDLE_INSET: f64 = 12.0;
const EDGE_PX: f64 = 6.0;
const DRAG_THRESHOLD: f64 = 3.0;
const POINT_RADIUS: f64 = 6.0;
const SCROLL_STEP: f64 = 48.0;
/// Height of the horizontal scrollbar along the bottom of the timeline.
const HSCROLL_HEIGHT: f64 = 12.0;
const ZOOM_STEP: f64 = 1.15;
/// Zoom factor of the zoom commands (H and G).
const ZOOM_KEY: f64 = 1.25;
const MENU_ROW: f64 = 20.0;
const MENU_W: f64 = 200.0;
const MENU_ROWS: usize = 12;
const MERGE_BASE: u64 = 1 << 40;
/// Default zoom: logical pixels per 4/4 bar.
pub const DEFAULT_BAR_PX: f64 = 96.0;
/// Minimum clip length with snapping off (a 64th note).
const MIN_TICKS: i64 = PPQ / 16;
/// Header controls of a note track.
const NOTE_BUTTONS: [TrackButton; 5] = [
    TrackButton::Expand,
    TrackButton::Mute,
    TrackButton::Solo,
    TrackButton::Arm,
    TrackButton::AddLane,
];
/// Header controls of an audio track.
const AUDIO_BUTTONS: [TrackButton; 6] = [
    TrackButton::Expand,
    TrackButton::Mute,
    TrackButton::Solo,
    TrackButton::Arm,
    TrackButton::Input,
    TrackButton::Monitor,
];
/// Monitor mode `auto` of the audio track device (`off` = 0, `auto` = 1, `on` = 2).
const MONITOR_AUTO: u8 = 1;
/// Frame rate assumed for an audio clip whose asset is missing from the pool.
const DEFAULT_RATE: u32 = 48_000;
/// Edge length of a fade handle.
const HANDLE: f64 = 8.0;
/// How far a fade handle stays inside the clip edges.
const HANDLE_INSET: f64 = 5.0;
/// Narrowest audio clip that shows a gain handle.
const GAIN_MIN_WIDTH: f64 = 64.0;
/// Gain handle size.
const GAIN_W: f64 = 28.0;
const GAIN_H: f64 = 6.0;

/// A handle on an audio clip (see [`ArrangementView::audio_handle_center`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioHandle {
    /// Fade-in length (top left; drag horizontally).
    FadeIn,
    /// Fade-out length (top right; drag horizontally).
    FadeOut,
    /// Clip gain (the line across the clip; drag vertically, Shift = fine).
    Gain,
}

/// A control in a track's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackButton {
    /// Mute toggle.
    Mute,
    /// Solo toggle.
    Solo,
    /// Record-arm toggle.
    Arm,
    /// Show or hide the automation lanes.
    Expand,
    /// Open the "add automation lane" param picker (note tracks).
    AddLane,
    /// Open the input selector (audio tracks).
    Input,
    /// Cycle the monitor mode off / auto / on (audio tracks).
    Monitor,
}

#[derive(Debug, Clone, PartialEq)]
enum RowKind {
    Lane { track: usize, lane: usize },
    AddTrack(DeviceKey),
    AddAudio,
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    kind: RowKind,
    y: f64,
    h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipPart {
    Body,
    Header,
    Edge,
    AutoBody,
    Point(usize),
    EdgeStart,
    FadeIn,
    FadeOut,
    Gain,
    /// The mute button in the header strip.
    Mute,
}

#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Snap,
    Tool(Tool),
    Corner,
    /// The bottom edge of the header of track `i` (drag to resize the track).
    TrackEdge(usize),
    RulerLabel,
    LoopBand,
    /// A point of the tempo lane.
    TempoPoint(usize),
    /// The tempo lane away from its points.
    TempoLane,
    /// The meter tag of change `i` in the ruler.
    MeterTag(usize),
    /// The marker strip away from its markers and ranges (`true`: the row of markers).
    MarkerStrip(bool),
    /// A marker of the strip.
    Marker(u32),
    /// A range of the strip, and the part of it under the pointer.
    Range(u32, RangePart),
    /// The marker list.
    Panel(markers::ListHit),
    Button(usize, TrackButton),
    Header(usize),
    AddTrack(DeviceKey),
    AddAudio,
    HScroll,
    Menu(usize),
    Clip {
        track: usize,
        lane: usize,
        id: ClipId,
        part: ClipPart,
    },
    Lane {
        track: usize,
        lane: usize,
    },
    Empty,
}

#[derive(Debug, Clone)]
struct Menu {
    device: DeviceKey,
    kind: MenuKind,
    items: Vec<(String, String)>,
    origin: Point,
    scroll: usize,
    hover: Option<usize>,
    /// The row of the current choice (input menu), drawn lit.
    current: Option<usize>,
}

/// What a menu row does.
#[derive(Debug, Clone)]
enum MenuKind {
    /// Add an automation lane for the row's param.
    Param,
    /// Choose the audio input; row `i` selects `options[i]`.
    Input(Vec<InputSel>),
}

impl Menu {
    fn visible(&self) -> usize {
        self.items.len().min(MENU_ROWS)
    }
    fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + MENU_W,
            self.origin.y + self.visible() as f64 * MENU_ROW + 8.0,
        )
    }
    fn row_rect(&self, i: usize) -> Option<Rect> {
        (i >= self.scroll && i < self.scroll + self.visible()).then(|| {
            let y = self.origin.y + 4.0 + (i - self.scroll) as f64 * MENU_ROW;
            Rect::new(
                self.origin.x + 4.0,
                y,
                self.origin.x + MENU_W - 4.0,
                y + MENU_ROW,
            )
        })
    }
    fn row_at(&self, p: Point) -> Option<usize> {
        (self.scroll..self.scroll + self.visible())
            .find(|i| self.row_rect(*i).is_some_and(|r| r.contains(p)))
    }
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// The bottom edge of a track header is dragged (`HeightUi::drag` has the state).
    Height,
    Move {
        start: Point,
        grabbed_start: i64,
        originals: Vec<Clip>,
        merge: u64,
        moved: bool,
        emitted: bool,
        last_dt: i64,
        reselect: Option<ClipId>,
    },
    Resize {
        start: Point,
        clip: Clip,
        merge: u64,
        moved: bool,
        emitted: bool,
        loop_mode: bool,
        last: (i64, Option<i64>),
    },
    Draw {
        start: Point,
        device: DeviceKey,
        lane: usize,
        automation: bool,
        t0: i64,
        t1: i64,
        moved: bool,
        modifiers: Modifiers,
        /// The pencil is the effective tool: a click without a drag draws a one-bar clip.
        pencil: bool,
    },
    Point {
        start: Point,
        id: ClipId,
        merge: u64,
        shift: i64,
        top: f64,
        height: f64,
        param: ParamDescriptor,
        original: Vec<AutomationPoint>,
        points: Vec<AutomationPoint>,
        index: usize,
        moved: bool,
        emitted: bool,
        last: Vec<AutomationPoint>,
    },
    Loop {
        start: Point,
        t0: i64,
        t1: i64,
        moved: bool,
        modifiers: Modifiers,
    },
    /// Moving a point of the tempo lane.
    TempoPoint {
        start: Point,
        index: usize,
        original: Vec<TempoEvent>,
        /// The BPM scale at the press (it stays put while the point moves).
        range: (f64, f64),
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<TempoEvent>,
    },
    /// Moving a meter change to another bar.
    Meter {
        start: Point,
        index: usize,
        original: Vec<MeterEvent>,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<MeterEvent>,
    },
    /// Moving or resizing a marker or range ([`markers::MarkDrag`] holds the gesture).
    Mark,
    Pan {
        last: Point,
    },
    /// Dragging the horizontal scrollbar thumb; `grab` is the x offset inside the thumb.
    HScroll {
        grab: f64,
    },
    /// A fade, gain or left-trim gesture on an audio clip.
    Audio {
        start: Point,
        clip: Clip,
        content: AudioClipContent,
        grab: AudioGrab,
        rate: u32,
        asset_frames: u64,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: (i64, i64, AudioClipContent),
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AudioGrab {
    FadeIn,
    FadeOut,
    /// The gain scale: `(top y, height)`.
    Gain(f64, f64),
    TrimStart,
}

/// The arrangement view. See the module documentation for hosting.
#[derive(Debug)]
pub struct ArrangementView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    axis: TimeAxis,
    scroll_y: f64,
    snap: Snap,
    vp: ViewportState,
    song: Song,
    synced: bool,
    device_sig: Vec<(DeviceKey, String, String, Option<[u8; 3]>)>,
    rows: Vec<Row>,
    content_height: f64,
    lane_params: Vec<Vec<Option<ParamDescriptor>>>,
    collapsed: BTreeSet<DeviceKey>,
    selection: Selection<ClipId>,
    /// The selected track (a click on its header or the Up and Down keys): view state.
    selected_track: Option<DeviceKey>,
    /// The selected automation points of one clip (positions in its point list).
    point_sel: Option<(ClipId, BTreeSet<usize>)>,
    menu: Option<Menu>,
    drag: Drag,
    hover: Option<Hit>,
    next_merge: u64,
    playhead: i64,
    recording: bool,
    focused: bool,
    repaint: bool,
    peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// The samples of the assets held in RAM, for the waveform below the finest peak level.
    samples: BTreeMap<String, Arc<[Vec<f32>]>>,
    assets: BTreeMap<String, AssetMeta>,
    tempo: TempoMap,
    /// The meter map: bar lines, beat lines, bar numbers and the snap grid.
    bars: Bars,
    /// Whether the tempo lane is shown between the ruler and the lanes.
    tempo_lane: bool,
    /// The selected tempo point and meter change.
    tempo_sel: Option<usize>,
    meter_sel: Option<usize>,
    /// Where the last context menu of the tempo lane or ruler opened: the tick, the bar and the
    /// tempo point or meter change it was on.
    ctx_tick: i64,
    ctx_bar: u32,
    ctx_tempo: Option<usize>,
    ctx_meter: Option<usize>,
    /// The marker strip, the marker list and their state.
    mk: markers::MarkerUi,
    /// Vertical zoom and the track height drag.
    th: heights::HeightUi,
    /// The view's whole rectangle; `bounds` is what is left of it when the marker list is
    /// docked at the right edge.
    outer: Rect,
    input_channels: u16,
    audio_headers: Vec<AudioHeader>,
    drop_hover: Option<DropHover>,
}

/// The header state of an audio track, derived from the document (cables and params).
#[derive(Debug, Clone, PartialEq)]
struct AudioHeader {
    device: DeviceKey,
    input: InputSel,
    /// Monitor mode: 0 off, 1 auto, 2 on.
    monitor: u8,
}

/// A file drag hovering over the arrangement: where it would land and how many files.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DropHover {
    target: ImportTarget,
    tick: i64,
    count: usize,
}

impl ArrangementView {
    /// An empty view; call [`Self::set_bounds`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let song = atomik_model::empty_song(120.0);
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, 300.0),
            scale: 1.0,
            axis: TimeAxis::new(TRACK_LIST_WIDTH, DEFAULT_BAR_PX),
            scroll_y: 0.0,
            snap: Snap::default(),
            vp: ViewportState::new(),
            song,
            synced: false,
            device_sig: Vec::new(),
            rows: Vec::new(),
            content_height: 0.0,
            lane_params: Vec::new(),
            collapsed: BTreeSet::new(),
            selection: Selection::new(),
            selected_track: None,
            point_sel: None,
            menu: None,
            drag: Drag::None,
            hover: None,
            next_merge: MERGE_BASE,
            playhead: 0,
            recording: false,
            focused: false,
            repaint: true,
            peaks: BTreeMap::new(),
            samples: BTreeMap::new(),
            assets: BTreeMap::new(),
            tempo: TempoMap::default(),
            bars: Bars::default(),
            tempo_lane: false,
            tempo_sel: None,
            meter_sel: None,
            ctx_tick: 0,
            ctx_bar: 1,
            ctx_tempo: None,
            ctx_meter: None,
            mk: markers::MarkerUi::default(),
            th: heights::HeightUi::default(),
            outer: Rect::new(0.0, 0.0, 800.0, 300.0),
            input_channels: 0,
            audio_headers: Vec::new(),
            drop_hover: None,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        self.outer = bounds;
        self.apply_bounds();
    }

    /// Sets the timeline's rectangle from the whole one: the marker list, when shown, takes its
    /// width off the right edge.
    fn apply_bounds(&mut self) {
        let o = self.outer;
        let bounds = if self.mk.list {
            let x1 = (o.x1 - markers::MARKER_LIST_WIDTH).max(o.x0 + TRACK_LIST_WIDTH + 64.0);
            Rect::new(o.x0, o.y0, x1.min(o.x1), o.y1)
        } else {
            o
        };
        if bounds != self.bounds {
            self.bounds = bounds;
            self.axis.x0 = bounds.x0 + TRACK_LIST_WIDTH;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The view's rectangle (logical pixels), the marker list included.
    #[allow(
        clippy::misnamed_getters,
        reason = "`bounds` is the timeline part of the whole rectangle"
    )]
    pub fn bounds(&self) -> Rect {
        self.outer
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Sets the transport position shown by the playhead (song ticks).
    pub fn set_playhead(&mut self, tick: i64) {
        if tick != self.playhead {
            self.playhead = tick;
            self.repaint = true;
            if self.vp.following() {
                self.axis.page_to(tick, self.bounds.x1);
            }
        }
    }

    /// The playhead position.
    pub fn playhead(&self) -> i64 {
        self.playhead
    }

    /// Shows the recording state (red playhead).
    pub fn set_recording(&mut self, recording: bool) {
        if recording != self.recording {
            self.recording = recording;
            self.repaint = true;
        }
    }

    /// Marks the view as having keyboard focus (drawn as a thin top line). The app decides
    /// focus (e.g. the last view clicked) and sends key events only to the focused view.
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
        if !focused {
            self.marker_blur();
        }
    }

    /// Whether the view has focus.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// True while a pointer gesture is in progress: keep routing pointer events here until
    /// the button is released, even outside the bounds.
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Forces a repaint.
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    /// The snap resolution.
    pub fn snap(&self) -> Snap {
        self.snap
    }

    /// Sets the snap resolution.
    pub fn set_snap(&mut self, snap: Snap) {
        self.snap = snap;
        self.repaint = true;
    }

    /// The active tool.
    pub fn tool(&self) -> Tool {
        self.vp.tool()
    }

    /// The tool the toolbar shows: the active one, or its alternate while the alternate-tool
    /// role is held.
    pub fn shown_tool(&self) -> Tool {
        self.vp.shown_tool()
    }

    /// Selects a tool (the `tool.*` commands and the toolbar buttons do this).
    pub fn set_tool(&mut self, tool: Tool) {
        if self.vp.set_tool(tool) {
            self.repaint = true;
        }
    }

    /// Sets the modifier role table (the shell pushes it when the keymap changes).
    pub fn set_roles(&mut self, roles: Roles) {
        if self.vp.set_roles(roles) {
            self.repaint = true;
        }
    }

    /// Whether follow is on.
    pub fn follow(&self) -> bool {
        self.vp.follow()
    }

    /// Switches follow on or off (the workflow preference sets the start state).
    pub fn set_follow(&mut self, on: bool) {
        self.vp.set_follow(on);
        self.repaint = true;
    }

    /// The transport runs or is stopped: follow only pages while it runs.
    pub fn set_playing(&mut self, playing: bool) {
        self.vp.set_playing(playing);
    }

    /// Centre of a toolbar button (the corner above the track list).
    pub fn tool_button_center(&self, tool: Tool) -> Point {
        self.tool_rect(tool).center()
    }

    /// Zoom in logical pixels per tick.
    pub fn px_per_tick(&self) -> f64 {
        self.axis.px_per_tick
    }

    /// Sets the zoom (clamped), keeping the left edge.
    pub fn set_px_per_tick(&mut self, ppt: f64) {
        let factor = ppt / self.axis.px_per_tick;
        self.axis.zoom_at(factor, self.axis.x0);
        self.repaint = true;
    }

    /// The tick at the left edge of the timeline.
    pub fn scroll_ticks(&self) -> f64 {
        self.axis.scroll
    }

    /// Scrolls the timeline so `tick` is at its left edge.
    pub fn set_scroll_ticks(&mut self, tick: f64) {
        self.axis.scroll = tick.max(0.0);
        self.repaint = true;
    }

    /// Vertical scroll of the lanes, logical pixels.
    pub fn scroll_y(&self) -> f64 {
        self.scroll_y
    }

    /// Sets the vertical scroll (clamped).
    pub fn set_scroll_y(&mut self, y: f64) {
        self.scroll_y = y;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Logical x of a song tick.
    pub fn tick_to_x(&self, tick: i64) -> f64 {
        self.axis.tick_to_x(tick)
    }

    /// The song tick at a logical x.
    pub fn x_to_tick(&self, x: f64) -> i64 {
        self.axis.x_to_tick(x)
    }

    /// Selected clips, by id.
    pub fn selected_clips(&self) -> Vec<ClipId> {
        self.selection.items().iter().copied().collect()
    }

    /// Replaces the clip selection.
    pub fn set_selected_clips(&mut self, ids: &[ClipId]) {
        self.selection.set(ids.iter().copied());
        self.repaint = true;
    }

    /// The selected track (its device), if one is: a click on a track header or the Up and Down
    /// keys select it, a click on a clip drops it. Paste lands on it and Duplicate and Delete
    /// act on it when no clip is selected.
    pub fn selected_track(&self) -> Option<&DeviceKey> {
        self.selected_track.as_ref()
    }

    /// Selects a track (or none); the clip and point selections are cleared.
    pub fn set_selected_track(&mut self, device: Option<DeviceKey>) {
        if device.is_some() {
            self.selection.clear();
            self.point_sel = None;
        }
        self.selected_track = device;
        self.repaint = true;
    }

    /// The selected automation points: the clip and the positions in its point list.
    pub fn selected_points(&self) -> Option<(ClipId, Vec<usize>)> {
        self.point_sel
            .as_ref()
            .map(|(id, s)| (*id, s.iter().copied().collect()))
    }

    /// Selects points of an automation clip (the clip selection becomes that clip).
    pub fn set_selected_points(&mut self, clip: ClipId, indices: &[usize]) {
        if indices.is_empty() {
            self.point_sel = None;
        } else {
            self.point_sel = Some((clip, indices.iter().copied().collect()));
            self.selection.select_only(clip);
            self.selected_track = None;
        }
        self.repaint = true;
    }

    /// Centre of a track's header, away from its buttons.
    pub fn track_header_center(&self, device: &DeviceKey) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let row = self.row_of(track, 0)?;
        let r = self.row_rect(row);
        Some(Point::new(self.bounds.x0 + 100.0, r.y0 + 14.0))
    }

    /// Shows or hides a track's automation lanes.
    pub fn set_expanded(&mut self, device: &DeviceKey, expanded: bool) {
        if expanded {
            self.collapsed.remove(device);
        } else {
            self.collapsed.insert(device.clone());
        }
        self.synced = false;
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry (public)

    /// The timeline part of a lane row (logical pixels), if shown.
    pub fn lane_rect(&self, device: &DeviceKey, lane: usize) -> Option<Rect> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let row = self.row_of(track, lane)?;
        let r = self.row_rect(row);
        Some(Rect::new(self.axis.x0, r.y0, self.bounds.x1, r.y1))
    }

    /// A clip's rectangle (logical pixels), if its lane is shown.
    pub fn clip_rect(&self, id: ClipId) -> Option<Rect> {
        let (track, lane, clip) = self.find_clip(id)?;
        let row = self.row_of(track, lane)?;
        Some(self.clip_rect_in(self.row_rect(row), clip))
    }

    fn is_audio(&self, id: ClipId) -> bool {
        self.find_clip(id)
            .is_some_and(|(_, _, c)| matches!(c.content, ClipContent::Audio(_)))
    }

    /// Centre of a clip's header mute button, `None` when the clip is hidden or too narrow
    /// to show one.
    pub fn mute_button_center(&self, id: ClipId) -> Option<Point> {
        let r = self.clip_rect(id)?;
        Some(mute_button_rect(r, self.timeline_rect(), self.is_audio(id))?.center())
    }

    /// Centre of a track header control.
    pub fn track_button_center(&self, device: &DeviceKey, button: TrackButton) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        if !self.header_buttons(track).contains(&button) {
            return None;
        }
        let rr = self.row_rect(self.row_of(track, 0)?);
        button_shown(rr, button).then(|| self.button_rect(rr, button).center())
    }

    /// Centre of the "+ track" row of a device that can have a track but has none.
    pub fn add_track_center(&self, device: &DeviceKey) -> Option<Point> {
        let row = self
            .rows
            .iter()
            .find(|r| r.kind == RowKind::AddTrack(device.clone()))?;
        let r = self.row_rect(row);
        Some(Point::new(
            self.bounds.x0 + TRACK_LIST_WIDTH / 2.0,
            r.center().y,
        ))
    }

    /// Centre of a handle of an audio clip (logical pixels), if the clip is an audio clip in a
    /// shown lane and the handle exists (a clip narrower than 64 px has no gain handle).
    pub fn audio_handle_center(&self, id: ClipId, handle: AudioHandle) -> Option<Point> {
        let (track, lane, clip) = self.find_clip(id)?;
        let ClipContent::Audio(a) = &clip.content else {
            return None;
        };
        let r = self.clip_rect_in(self.row_rect(self.row_of(track, lane)?), clip);
        let g = self.audio_geo(r, clip, a, self.timeline_rect());
        match handle {
            AudioHandle::FadeIn => Some(g.fade_in.center()),
            AudioHandle::FadeOut => Some(g.fade_out.center()),
            AudioHandle::Gain => g.gain.map(|h| h.center()),
        }
    }

    /// Centre of the "+ audio track" row at the end of the track list.
    pub fn add_audio_track_center(&self) -> Option<Point> {
        let row = self.rows.iter().find(|r| r.kind == RowKind::AddAudio)?;
        let r = self.row_rect(row);
        Some(Point::new(
            self.bounds.x0 + TRACK_LIST_WIDTH / 2.0,
            r.center().y,
        ))
    }

    /// Where a file drop at the pointer would go and the snapped tick: see
    /// [`UiAction::ImportAudio`]. `None` outside the track area (ruler, corner, outside the
    /// view). A pointer in the track list counts as the left edge of the timeline.
    pub fn drop_target_at(&self, pos: Point) -> Option<(ImportTarget, i64)> {
        if !self.bounds.contains(pos) || pos.y < self.lanes_top() {
            return None;
        }
        let x = pos.x.max(self.axis.x0);
        let tick = snap_round(self.axis.x_to_tick(x), self.step(Modifiers::NONE)).max(0);
        let target = match self.row_at(pos.y).map(|r| &r.kind) {
            Some(RowKind::Lane { track, .. }) if self.song.tracks[*track].is_audio() => {
                ImportTarget::ExistingTrack(self.song.tracks[*track].device.clone())
            }
            _ => ImportTarget::NewTracks,
        };
        Some((target, tick))
    }

    /// The drop indicator state while files hover over the view: the target and snapped tick.
    pub fn drop_hover(&self) -> Option<(ImportTarget, i64)> {
        self.drop_hover.as_ref().map(|h| (h.target.clone(), h.tick))
    }

    /// Sets the waveform peaks of an asset (its content hash), replacing earlier ones. The
    /// view draws audio clips of assets without peaks as an empty body with the file name.
    pub fn set_peaks(&mut self, asset: &str, peaks: Arc<PeakPyramid>) {
        self.peaks.insert(asset.to_string(), peaks);
        self.repaint = true;
    }

    /// Sets the samples of an asset held in RAM (planar, one `Vec` per channel, shared with the
    /// engine), which the waveform reads below the finest peak level. Assets without samples
    /// (streamed from disk) keep the finest level at every zoom.
    pub fn set_samples(&mut self, asset: &str, samples: Arc<[Vec<f32>]>) {
        self.samples.insert(asset.to_string(), samples);
        self.repaint = true;
    }

    /// Whether the samples of an asset are held (see [`Self::set_samples`]).
    pub fn has_samples(&self, asset: &str) -> bool {
        self.samples.contains_key(asset)
    }

    /// Forgets the peaks of an asset.
    pub fn clear_peaks(&mut self, asset: &str) {
        let had_samples = self.samples.remove(asset).is_some();
        if self.peaks.remove(asset).is_some() || had_samples {
            self.repaint = true;
        }
    }

    /// The y of the loop band in the ruler.
    pub fn loop_band_y(&self) -> f64 {
        self.bounds.y0 + (LABEL_BAND + RULER_HEIGHT) / 2.0
    }

    /// The y of the bar-label band of the ruler (click to locate).
    pub fn ruler_label_y(&self) -> f64 {
        self.bounds.y0 + LABEL_BAND / 2.0
    }

    /// Centre of the snap selector.
    pub fn snap_button_center(&self) -> Point {
        self.snap_rect().center()
    }

    /// Labels of the rows of the open menu (the "add automation lane" param picker or an
    /// audio input selector), empty when no menu is open.
    pub fn menu_labels(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map(|m| m.items.iter().map(|(_, l)| l.clone()).collect())
            .unwrap_or_default()
    }

    /// The row of the current choice of the open menu (the input selector marks the cabled
    /// input), if any.
    pub fn menu_current(&self) -> Option<usize> {
        self.menu.as_ref()?.current
    }

    /// Param keys offered by the open "add automation lane" menu (empty when closed).
    pub fn param_menu_items(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map(|m| m.items.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default()
    }

    /// Centre of menu row `i`, when visible.
    pub fn param_menu_row_center(&self, i: usize) -> Option<Point> {
        self.menu.as_ref()?.row_rect(i).map(|r| r.center())
    }

    /// The y range `(top = max, bottom = min)` of values inside an automation clip.
    pub fn automation_value_range(&self, id: ClipId) -> Option<(f64, f64)> {
        let r = self.clip_rect(id)?;
        matches!(self.find_clip(id)?.2.content, ClipContent::Automation(_)).then(|| value_range(r))
    }

    /// Position of automation point `index` of a clip (its first occurrence).
    pub fn automation_point_pos(&self, id: ClipId, index: usize) -> Option<Point> {
        let (track, lane, clip) = self.find_clip(id)?;
        self.point_positions(track, lane, clip)
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, p)| p)
    }

    // ------------------------------------------------------------------ sync

    /// Updates the view from the document. Emits nothing; cheap when nothing changed.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let sig: Vec<_> = doc
            .devices
            .iter()
            .map(|d| (d.key.clone(), d.type_id.clone(), d.name.clone(), d.color))
            .collect();
        let headers: Vec<AudioHeader> = doc
            .song
            .tracks
            .iter()
            .filter(|t| t.is_audio())
            .map(|t| AudioHeader {
                device: t.device.clone(),
                input: audio::input_selection(&doc.cables, &t.device),
                monitor: doc
                    .param_value(&t.device, "monitor", registry)
                    .map_or(MONITOR_AUTO, |v| v.round().clamp(0.0, 2.0) as u8),
            })
            .collect();
        if self.synced
            && self.song == doc.song
            && self.device_sig == sig
            && self.assets == doc.assets
            && self.input_channels == doc.input_channels
            && self.audio_headers == headers
        {
            return;
        }
        self.audio_headers = headers;
        self.song = doc.song.clone();
        self.tempo = TempoMap::new(&self.song.tempo);
        self.bars = Bars::of(&self.song);
        self.assets.clone_from(&doc.assets);
        self.input_channels = doc.input_channels;
        self.device_sig = sig;
        self.synced = true;
        self.rebuild(doc, registry);
        self.repaint = true;
    }

    fn rebuild(&mut self, doc: &RackDoc, registry: &Registry) {
        let mut rows = Vec::new();
        let mut y = 0.0;
        self.lane_params = self
            .song
            .tracks
            .iter()
            .map(|t| {
                t.lanes
                    .iter()
                    .map(|l| match &l.kind {
                        LaneKind::Notes | LaneKind::Audio => None,
                        LaneKind::Automation(p) => {
                            Some(sp::param_desc(doc, registry, &t.device, p))
                        }
                    })
                    .collect()
            })
            .collect();
        let lane_h = self.lane_row_height();
        for (ti, t) in self.song.tracks.iter().enumerate() {
            let h = self.track_row_height(&t.device);
            rows.push(Row {
                kind: RowKind::Lane { track: ti, lane: 0 },
                y,
                h,
            });
            y += h;
            if !self.collapsed.contains(&t.device) {
                for li in 1..t.lanes.len() {
                    rows.push(Row {
                        kind: RowKind::Lane {
                            track: ti,
                            lane: li,
                        },
                        y,
                        h: lane_h,
                    });
                    y += lane_h;
                }
            }
        }
        for d in &doc.devices {
            if sp::has_notes_input(registry, &d.type_id)
                && !self.song.tracks.iter().any(|t| t.device == d.key)
            {
                rows.push(Row {
                    kind: RowKind::AddTrack(d.key.clone()),
                    y,
                    h: ADD_ROW_HEIGHT,
                });
                y += ADD_ROW_HEIGHT;
            }
        }
        rows.push(Row {
            kind: RowKind::AddAudio,
            y,
            h: ADD_ROW_HEIGHT,
        });
        y += ADD_ROW_HEIGHT;
        self.rows = rows;
        self.content_height = y;
        let song = &self.song;
        self.selection.retain(|id| find_clip(song, *id).is_some());
        if self
            .selected_track
            .as_ref()
            .is_some_and(|d| !song.tracks.iter().any(|t| &t.device == d))
        {
            self.selected_track = None;
        }
        if let Some((id, set)) = &mut self.point_sel {
            let len = match find_clip(song, *id).map(|(_, _, c)| &c.content) {
                Some(ClipContent::Automation(p)) => p.len(),
                _ => 0,
            };
            set.retain(|i| *i < len);
        }
        if self.point_sel.as_ref().is_some_and(|(_, s)| s.is_empty()) {
            self.point_sel = None;
        }
        if let Some(m) = &self.menu
            && !song.tracks.iter().any(|t| t.device == m.device)
        {
            self.menu = None;
        }
        let (tempos, meters) = (song.tempo.len(), song.meter.len());
        self.tempo_sel = self.tempo_sel.filter(|i| *i < tempos);
        self.meter_sel = self.meter_sel.filter(|i| *i < meters);
        self.marker_sync();
        self.clamp_scroll();
    }

    // ------------------------------------------------------------------ geometry (private)

    /// The bottom of the ruler (the tempo lane, when shown, sits below it).
    fn ruler_bottom(&self) -> f64 {
        self.bounds.y0 + RULER_HEIGHT
    }

    /// The bottom of the marker strip (the ruler's bottom when it is hidden).
    fn markers_bottom(&self) -> f64 {
        self.ruler_bottom()
            + if self.mk.strip {
                MARKER_STRIP_HEIGHT
            } else {
                0.0
            }
    }

    fn lanes_top(&self) -> f64 {
        self.markers_bottom()
            + if self.tempo_lane {
                TEMPO_LANE_HEIGHT
            } else {
                0.0
            }
    }

    /// The tempo lane over the timeline (empty height when hidden).
    fn tempo_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.markers_bottom(),
            self.bounds.x1.max(self.axis.x0),
            self.lanes_top(),
        )
    }

    /// The snap grid of a gesture: the resolution over the meter map, or none while the
    /// disable-snap role is held.
    fn grid(&self, modifiers: Modifiers) -> SnapGrid {
        if self.vp.snap_disabled(modifiers) {
            SnapGrid::off()
        } else {
            SnapGrid::new(self.snap, self.bars.clone()).zoomed(self.axis.px_per_tick)
        }
    }

    fn timeline_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.lanes_top(),
            self.bounds.x1.max(self.axis.x0),
            self.bounds.y1.max(self.lanes_top()),
        )
    }

    fn ruler_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.bounds.y0,
            self.bounds.x1.max(self.axis.x0),
            self.ruler_bottom(),
        )
    }

    /// The horizontal scrollbar track along the bottom of the timeline.
    fn hscroll_rect(&self) -> Rect {
        let tl = self.timeline_rect();
        Rect::new(tl.x0, (tl.y1 - HSCROLL_HEIGHT).max(tl.y0), tl.x1, tl.y1)
    }

    /// Total scrollable span in ticks: the content end plus room, never less than the view.
    fn hscroll_total(&self) -> f64 {
        let end = self
            .song
            .tracks
            .iter()
            .flat_map(|t| t.lanes.iter())
            .flat_map(|l| l.clips.iter())
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0) as f64;
        let view = self.hscroll_rect().width() / self.axis.px_per_tick;
        (end + 4.0 * self.bars.bar_ticks_at(end as i64) as f64)
            .max(self.axis.scroll + view)
            .max(1.0)
    }

    /// The scrollbar thumb rectangle.
    fn hscroll_thumb(&self) -> Rect {
        let tr = self.hscroll_rect();
        let total = self.hscroll_total();
        let view = tr.width() / self.axis.px_per_tick;
        let len = (tr.width() * (view / total).min(1.0))
            .max(24.0)
            .min(tr.width());
        let x = tr.x0 + (tr.width() - len) * (self.axis.scroll / (total - view).max(1.0)).min(1.0);
        Rect::new(x, tr.y0 + 2.0, x + len, tr.y1 - 2.0)
    }

    /// Scrolls so the thumb's left edge is at `x`.
    fn hscroll_to(&mut self, x: f64) {
        let tr = self.hscroll_rect();
        let total = self.hscroll_total();
        let view = tr.width() / self.axis.px_per_tick;
        let len = self.hscroll_thumb().width();
        let span = (tr.width() - len).max(1.0);
        let f = ((x - tr.x0) / span).clamp(0.0, 1.0);
        self.axis.scroll = f * (total - view).max(0.0);
    }

    fn snap_rect(&self) -> Rect {
        let (x, y) = (self.bounds.x0, self.bounds.y0);
        Rect::new(x + 46.0, y + 8.0, x + 100.0, y + 26.0)
    }

    /// A toolbar button in the corner, right of the snap selector.
    fn tool_rect(&self, tool: Tool) -> Rect {
        let i = Tool::ALL.iter().position(|t| *t == tool).unwrap_or(0) as f64;
        let (x, y) = (self.bounds.x0 + 106.0 + i * 25.0, self.bounds.y0 + 8.0);
        Rect::new(x, y, x + 23.0, y + 18.0)
    }

    fn clamp_scroll(&mut self) {
        let visible = (self.bounds.y1 - self.lanes_top()).max(0.0);
        let max = (self.content_height - visible).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
    }

    fn row_rect(&self, row: &Row) -> Rect {
        let y = self.lanes_top() + row.y - self.scroll_y;
        Rect::new(self.bounds.x0, y, self.bounds.x1, y + row.h)
    }

    fn row_of(&self, track: usize, lane: usize) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.kind == RowKind::Lane { track, lane })
    }

    fn row_at(&self, y: f64) -> Option<&Row> {
        self.rows.iter().find(|r| {
            let rr = self.row_rect(r);
            y >= rr.y0 && y < rr.y1
        })
    }

    fn clip_rect_in(&self, row: Rect, clip: &Clip) -> Rect {
        Rect::new(
            self.axis.tick_to_x(clip.start),
            row.y0 + 3.0,
            self.axis.tick_to_x(clip.start + clip.length),
            row.y1 - 3.0,
        )
    }

    /// A header control's rectangle in the header row `row`. A row shorter than
    /// [`COMPACT_BELOW`] has one line: the chevron, the name and the three buttons, centred
    /// vertically; taller rows keep the two lines, the buttons on the second.
    fn button_rect(&self, row: Rect, button: TrackButton) -> Rect {
        let (x, y) = (self.bounds.x0, row.y0);
        if row.height() < COMPACT_BELOW {
            let cy = y + row.height() / 2.0;
            let (top, bottom) = (cy - 8.0, cy + 8.0);
            return match button {
                TrackButton::Expand => Rect::new(x + 10.0, cy - 7.0, x + 24.0, cy + 7.0),
                TrackButton::Mute => Rect::new(x + 112.0, top, x + 132.0, bottom),
                TrackButton::Solo => Rect::new(x + 136.0, top, x + 156.0, bottom),
                TrackButton::Arm => Rect::new(x + 160.0, top, x + 180.0, bottom),
                TrackButton::AddLane | TrackButton::Input | TrackButton::Monitor => Rect::ZERO,
            };
        }
        match button {
            TrackButton::Expand => Rect::new(x + 10.0, y + 9.0, x + 24.0, y + 23.0),
            TrackButton::Mute => Rect::new(x + 30.0, y + 33.0, x + 50.0, y + 49.0),
            TrackButton::Solo => Rect::new(x + 54.0, y + 33.0, x + 74.0, y + 49.0),
            TrackButton::Arm => Rect::new(x + 78.0, y + 33.0, x + 98.0, y + 49.0),
            TrackButton::AddLane => Rect::new(x + 108.0, y + 33.0, x + 162.0, y + 49.0),
            TrackButton::Input => Rect::new(x + 104.0, y + 33.0, x + 164.0, y + 49.0),
            TrackButton::Monitor => Rect::new(x + 168.0, y + 33.0, x + 200.0, y + 49.0),
        }
    }

    fn find_clip(&self, id: ClipId) -> Option<(usize, usize, &Clip)> {
        find_clip(&self.song, id)
    }

    fn param_of(&self, track: usize, lane: usize) -> ParamDescriptor {
        self.lane_params
            .get(track)
            .and_then(|l| l.get(lane).copied().flatten())
            .unwrap_or(sp::FALLBACK_PARAM)
    }

    fn point_positions(&self, track: usize, lane: usize, clip: &Clip) -> Vec<(usize, Point)> {
        let ClipContent::Automation(points) = &clip.content else {
            return Vec::new();
        };
        let Some(row) = self.row_of(track, lane) else {
            return Vec::new();
        };
        let r = self.clip_rect_in(self.row_rect(row), clip);
        let (top, bottom) = value_range(r);
        let p = self.param_of(track, lane);
        let mut out = Vec::new();
        for w in seq_edit::content_windows(clip) {
            let len = w.song_end - w.song_start;
            for (i, ap) in points.iter().enumerate() {
                if ap.tick >= w.content_start && ap.tick < w.content_start + len {
                    let x = self
                        .axis
                        .tick_to_x(w.song_start + ap.tick - w.content_start);
                    let n = f64::from(seq_edit::value_to_norm(&p, ap.value));
                    out.push((i, Point::new(x, bottom - n * (bottom - top))));
                }
            }
        }
        out
    }

    fn hit(&self, pos: Point) -> Hit {
        if let Some(m) = &self.menu {
            if let Some(i) = m.row_at(pos) {
                return Hit::Menu(i);
            }
            if m.rect().contains(pos) {
                return Hit::Empty;
            }
        }
        if self.mk.list && self.marker_list_rect().contains(pos) {
            return Hit::Panel(self.list_hit(pos));
        }
        if !self.bounds.contains(pos) {
            return Hit::Empty;
        }
        let tl = self.timeline_rect();
        if self.hscroll_rect().contains(pos) {
            return Hit::HScroll;
        }
        if pos.y < self.ruler_bottom() {
            if pos.x < tl.x0 {
                if let Some(t) = Tool::ALL
                    .into_iter()
                    .find(|t| self.tool_rect(*t).contains(pos))
                {
                    return Hit::Tool(t);
                }
                return if self.snap_rect().contains(pos) {
                    Hit::Snap
                } else {
                    Hit::Corner
                };
            }
            if pos.y < self.bounds.y0 + LABEL_BAND {
                if let Some(i) = (0..self.song.meter.len())
                    .find(|i| self.meter_tag_rect(*i).is_some_and(|r| r.contains(pos)))
                {
                    return Hit::MeterTag(i);
                }
                return Hit::RulerLabel;
            }
            return Hit::LoopBand;
        }
        if pos.y < self.markers_bottom() {
            // The marker strip (only when it is shown).
            if pos.x < tl.x0 {
                return Hit::Empty;
            }
            return self.strip_hit(pos);
        }
        if pos.y < self.lanes_top() {
            // The tempo lane (only when it is shown).
            if pos.x < tl.x0 {
                return Hit::Empty;
            }
            let lane_rect = self.tempo_rect();
            let range = lane::bpm_range(&self.song.tempo);
            let nearest = self
                .song
                .tempo
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    (
                        i,
                        lane::point_pos(e, lane_rect, &self.axis, range).distance(pos),
                    )
                })
                .filter(|(_, d)| *d <= lane::POINT_HIT)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            return nearest.map_or(Hit::TempoLane, |(i, _)| Hit::TempoPoint(i));
        }
        let Some(row) = self.row_at(pos.y) else {
            return Hit::Empty;
        };
        let rr = self.row_rect(row);
        match &row.kind {
            RowKind::AddTrack(d) => {
                if pos.x < tl.x0 {
                    Hit::AddTrack(d.clone())
                } else {
                    Hit::Empty
                }
            }
            RowKind::AddAudio => {
                if pos.x < tl.x0 {
                    Hit::AddAudio
                } else {
                    Hit::Empty
                }
            }
            RowKind::Lane { track, lane } => {
                let (track, lane) = (*track, *lane);
                if pos.x < tl.x0 {
                    if lane == 0 && pos.y >= rr.y1 - heights::EDGE_GRAB {
                        return Hit::TrackEdge(track);
                    }
                    if lane == 0 {
                        for &b in self.header_buttons(track) {
                            if button_shown(rr, b) && self.button_rect(rr, b).contains(pos) {
                                return Hit::Button(track, b);
                            }
                        }
                    }
                    return Hit::Header(track);
                }
                let Some(l) = self.song.tracks.get(track).and_then(|t| t.lanes.get(lane)) else {
                    return Hit::Empty;
                };
                for clip in l.clips.iter().rev() {
                    let r = self.clip_rect_in(rr, clip);
                    if !(r.contains(pos)) {
                        continue;
                    }
                    let id = clip.id;
                    let automation = matches!(clip.content, ClipContent::Automation(_));
                    if automation {
                        let pts = self.point_positions(track, lane, clip);
                        let ps: Vec<Point> = pts.iter().map(|(_, p)| *p).collect();
                        if let Some(k) = seq_edit::nearest_point(&ps, pos, POINT_RADIUS) {
                            return Hit::Clip {
                                track,
                                lane,
                                id,
                                part: ClipPart::Point(pts[k].0),
                            };
                        }
                    }
                    if mute_button_rect(r, tl, matches!(clip.content, ClipContent::Audio(_)))
                        .is_some_and(|b| b.contains(pos))
                    {
                        return Hit::Clip {
                            track,
                            lane,
                            id,
                            part: ClipPart::Mute,
                        };
                    }
                    if let Some(part) = self.audio_handle_at(pos, r, clip, tl) {
                        return Hit::Clip {
                            track,
                            lane,
                            id,
                            part,
                        };
                    }
                    let audio = matches!(clip.content, ClipContent::Audio(_));
                    let part = match seq_edit::hit_span(pos.x, r.x0, r.x1, EDGE_PX) {
                        Some(SpanPart::End) => ClipPart::Edge,
                        _ if audio && pos.x < r.x0 + EDGE_PX.min(r.width() / 3.0) => {
                            ClipPart::EdgeStart
                        }
                        _ if pos.y < r.y0 + CLIP_HEADER => ClipPart::Header,
                        _ if automation => ClipPart::AutoBody,
                        _ => ClipPart::Body,
                    };
                    return Hit::Clip {
                        track,
                        lane,
                        id,
                        part,
                    };
                }
                Hit::Lane { track, lane }
            }
        }
    }

    fn merge_id(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first.
    /// `Resize` is ignored (use [`Self::set_bounds`]).
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::Resize(_) => {}
            UiEvent::ModifiersChanged { modifiers } => {
                if self.vp.set_held(*modifiers) {
                    self.repaint = true;
                }
            }
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => self.pointer_down(*pos, *button, *modifiers, *clicks, doc, registry, &mut out),
            UiEvent::PointerUp { pos, modifiers, .. } => {
                self.pointer_up(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint = true;
                }
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
            } => self.key(key, *modifiers, &mut out),
            UiEvent::Key { .. } => {}
            UiEvent::FileHover { pos, paths } => self.file_hover(*pos, paths),
            UiEvent::FileHoverCancel => self.set_drop_hover(None),
            UiEvent::FileDrop { pos, paths } => {
                self.set_drop_hover(None);
                if !paths.is_empty()
                    && let Some((target, tick)) = self.drop_target_at(*pos)
                {
                    out.push(UiAction::ImportAudio {
                        paths: paths.clone(),
                        target,
                        tick,
                    });
                }
            }
        }
        out
    }

    fn file_hover(&mut self, pos: Point, paths: &[std::path::PathBuf]) {
        let hover = if paths.is_empty() {
            None
        } else {
            self.drop_target_at(pos).map(|(target, tick)| DropHover {
                target,
                tick,
                count: paths.len(),
            })
        };
        self.set_drop_hover(hover);
    }

    fn set_drop_hover(&mut self, hover: Option<DropHover>) {
        if self.drop_hover != hover {
            self.drop_hover = hover;
            self.repaint = true;
        }
    }

    fn step(&self, modifiers: Modifiers) -> SnapGrid {
        self.grid(modifiers)
    }

    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        if !matches!(self.drag, Drag::None) {
            return;
        }
        if button == PointerButton::Middle {
            self.drag = Drag::Pan { last: pos };
            return;
        }
        if button == PointerButton::Primary && self.menu.is_none() && self.hit(pos) == Hit::HScroll
        {
            let thumb = self.hscroll_thumb();
            let grab = if thumb.contains(pos) {
                pos.x - thumb.x0
            } else {
                thumb.width() / 2.0
            };
            self.drag = Drag::HScroll { grab };
            self.hscroll_to(pos.x - grab);
            return;
        }
        let hit = self.hit(pos);
        if self.rename_press(pos, out) {
            return;
        }
        if let Some(m) = self.menu.take() {
            if let Hit::Menu(i) = hit
                && button == PointerButton::Primary
            {
                match &m.kind {
                    MenuKind::Param => out.push(song_cmd(SongCommand::AddAutomationLane {
                        device: m.device.clone(),
                        param: m.items[i].0.clone(),
                    })),
                    MenuKind::Input(options) => {
                        let current = self
                            .audio_header(&m.device)
                            .map_or(InputSel::Off, |h| h.input);
                        // One undo step for the whole change.
                        let cables = audio::input_change(&m.device, current, options[i]);
                        if !cables.is_empty() {
                            out.push(UiAction::Command(Command::Batch(cables)));
                        }
                    }
                }
            }
            return;
        }
        let primary = button == PointerButton::Primary;
        let tool = self.vp.effective_tool(modifiers);
        if !matches!(hit, Hit::TempoPoint(_) | Hit::MeterTag(_)) {
            self.tempo_sel = None;
            self.meter_sel = None;
        }
        if !matches!(hit, Hit::Marker(_) | Hit::Range(..) | Hit::Panel(_)) {
            self.mk.sel = None;
        }
        match hit {
            Hit::Marker(id) => self.marker_down(id, pos, button, clicks, tool, out),
            Hit::Range(id, part) => self.range_down(id, part, pos, button, clicks, tool, out),
            Hit::MarkerStrip(row) => {
                self.strip_down(row, pos, button, modifiers, clicks, tool, out);
            }
            Hit::Panel(h) => self.list_down(h, pos, button, clicks, out),
            Hit::TempoPoint(i) => self.tempo_point_down(i, button, pos, tool, out),
            Hit::TempoLane => self.tempo_lane_down(pos, button, modifiers, clicks, tool, out),
            Hit::MeterTag(i) => self.meter_tag_down(i, button, pos, tool, out),
            Hit::RulerLabel if button == PointerButton::Secondary => {
                self.ctx_bar =
                    u32::try_from(self.bars.bar_of(self.axis.x_to_tick(pos.x)).max(1)).unwrap_or(1);
                self.ctx_meter = None;
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::RulerMeter,
                    at: pos,
                });
            }
            Hit::TrackEdge(track) if primary => self.height_down(track, pos, clicks, out),
            Hit::Snap => self.snap = self.snap.cycle(primary),
            Hit::Tool(t) if primary => self.set_tool(t),
            Hit::RulerLabel if primary && tool == Tool::Razor => {
                // The razor in the ruler splits every track at the snapped position.
                let at = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
                out.extend(self.split_all_at(at, None));
            }
            Hit::RulerLabel if primary => {
                let t = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0);
                out.push(UiAction::Transport(TransportAction::Locate(t)));
            }
            Hit::LoopBand if primary => {
                let t = self.axis.x_to_tick(pos.x);
                self.drag = Drag::Loop {
                    start: pos,
                    t0: t,
                    t1: t,
                    moved: false,
                    modifiers,
                };
            }
            Hit::Button(track, b) if primary => self.track_button(track, b, doc, registry, out),
            Hit::AddTrack(device) if primary => {
                out.push(song_cmd(SongCommand::AddTrack { device }));
            }
            Hit::AddAudio if primary => {
                out.push(song_cmd(SongCommand::AddAudioTrack { name: None }));
            }
            Hit::Clip {
                track,
                lane,
                id,
                part,
            } => self.clip_down(track, lane, id, part, pos, button, modifiers, clicks, out),
            Hit::Header(track) => {
                // A click on a track header selects the track (the clip selection gives way).
                let device = self.song.tracks[track].device.clone();
                self.set_selected_track(Some(device));
                if button == PointerButton::Secondary {
                    out.push(UiAction::ContextMenu {
                        target: ContextTarget::TrackHeader,
                        at: pos,
                    });
                }
            }
            Hit::Lane { .. } if button == PointerButton::Secondary => {
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Lane,
                    at: pos,
                });
            }
            Hit::Lane { .. } if primary && matches!(tool, Tool::Razor | Tool::Eraser) => {}
            Hit::Lane { track, lane } if primary => {
                self.selected_track = None;
                self.point_sel = None;
                let t = self.axis.x_to_tick(pos.x);
                let device = self.song.tracks[track].device.clone();
                if clicks >= 2 {
                    // A double-click on an empty lane creates a one-bar clip.
                    let start = snap_floor(t, self.step(modifiers)).max(0);
                    let length = self.bars.bar_ticks_at(start);
                    out.push(new_clip_cmd(device, lane, lane > 0, start, length));
                    return;
                }
                self.drag = Drag::Draw {
                    start: pos,
                    device,
                    lane,
                    automation: lane > 0,
                    t0: t,
                    t1: t,
                    moved: false,
                    modifiers,
                    pencil: tool == Tool::Pencil,
                };
            }
            _ => {}
        }
    }

    /// The controls in the header of a track: audio tracks have an input selector and a
    /// monitor button where note tracks have the "add automation lane" button.
    fn header_buttons(&self, track: usize) -> &'static [TrackButton] {
        if self.song.tracks[track].is_audio() {
            &AUDIO_BUTTONS
        } else {
            &NOTE_BUTTONS
        }
    }

    /// Opens a menu, moved up when it would run below the view.
    fn open_menu(&mut self, mut menu: Menu) {
        let h = menu.rect().height();
        if menu.origin.y + h > self.bounds.y1 {
            menu.origin.y = (self.bounds.y1 - h).max(self.bounds.y0);
        }
        self.menu = Some(menu);
    }

    /// The cached header state of an audio track.
    fn audio_header(&self, device: &DeviceKey) -> Option<&AudioHeader> {
        self.audio_headers.iter().find(|h| &h.device == device)
    }

    fn track_button(
        &mut self,
        track: usize,
        b: TrackButton,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        let t = &self.song.tracks[track];
        let flags = |muted, solo, armed| {
            song_cmd(SongCommand::SetTrackFlags {
                device: t.device.clone(),
                muted,
                solo,
                armed,
            })
        };
        match b {
            TrackButton::Mute => out.push(flags(!t.muted, t.solo, t.armed)),
            TrackButton::Solo => out.push(flags(t.muted, !t.solo, t.armed)),
            TrackButton::Arm => out.push(flags(t.muted, t.solo, !t.armed)),
            TrackButton::Expand => {
                let d = t.device.clone();
                let expanded = self.collapsed.contains(&d);
                self.set_expanded(&d, expanded);
                self.sync(doc, registry);
            }
            TrackButton::AddLane => {
                let used: BTreeSet<&str> = t
                    .lanes
                    .iter()
                    .filter_map(|l| match &l.kind {
                        LaneKind::Automation(p) => Some(p.as_str()),
                        LaneKind::Notes | LaneKind::Audio => None,
                    })
                    .collect();
                let items: Vec<(String, String)> = doc
                    .device(&t.device)
                    .and_then(|d| registry.get(&d.type_id))
                    .map(|f| {
                        f.descriptor
                            .params
                            .iter()
                            .filter(|p| !used.contains(p.key))
                            .map(|p| (p.key.to_string(), p.name.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                if items.is_empty() {
                    return;
                }
                let Some(row) = self.row_of(track, 0) else {
                    return;
                };
                let br = self.button_rect(self.row_rect(row), TrackButton::AddLane);
                let menu = Menu {
                    device: t.device.clone(),
                    kind: MenuKind::Param,
                    items,
                    origin: Point::new(br.x0, br.y1 + 2.0),
                    scroll: 0,
                    hover: None,
                    current: None,
                };
                self.open_menu(menu);
            }
            TrackButton::Input => {
                let device = t.device.clone();
                let current = self
                    .audio_header(&device)
                    .map_or(InputSel::Off, |h| h.input);
                let options = audio::input_options(self.input_channels);
                let Some(row) = self.row_of(track, 0) else {
                    return;
                };
                let br = self.button_rect(self.row_rect(row), TrackButton::Input);
                let menu = Menu {
                    device,
                    kind: MenuKind::Input(options.clone()),
                    items: options
                        .iter()
                        .map(|o| (audio::input_label(*o), audio::input_label(*o)))
                        .collect(),
                    origin: Point::new(br.x0, br.y1 + 2.0),
                    scroll: 0,
                    hover: None,
                    current: options.iter().position(|o| *o == current),
                };
                self.open_menu(menu);
            }
            TrackButton::Monitor => {
                let device = t.device.clone();
                let mode = self
                    .audio_header(&device)
                    .map_or(MONITOR_AUTO, |h| h.monitor);
                out.push(UiAction::Command(Command::SetParam {
                    key: device,
                    param: "monitor".to_string(),
                    value: f32::from((mode + 1) % 3),
                    merge: None,
                }));
            }
        }
    }

    #[allow(clippy::too_many_arguments, reason = "hit fields are passed through")]
    fn clip_down(
        &mut self,
        track: usize,
        lane: usize,
        id: ClipId,
        part: ClipPart,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some((_, _, clip)) = self.find_clip(id) else {
            return;
        };
        let clip = clip.clone();
        let primary = button == PointerButton::Primary;
        if part == ClipPart::Mute {
            // The button acts on its own clip only: no selection change, no drag, any tool.
            if primary {
                out.push(mute_cmd(vec![id], !clip.muted));
            }
            return;
        }
        if primary {
            match self.vp.effective_tool(modifiers) {
                Tool::Razor => {
                    let at = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers));
                    out.extend(self.split_all_at(at, Some(id)));
                    return;
                }
                Tool::Eraser => {
                    match (&part, &clip.content) {
                        (ClipPart::Point(i), ClipContent::Automation(points)) => {
                            out.push(content_cmd(id, seq_edit::remove_point(points, *i), None));
                        }
                        _ => {
                            self.selection.clear();
                            out.push(song_cmd(SongCommand::RemoveClip { id }));
                        }
                    }
                    return;
                }
                // The pencil only draws: clips are not selected or moved; automation points
                // still draw and move.
                Tool::Pencil if !matches!(part, ClipPart::Point(_) | ClipPart::AutoBody) => {
                    return;
                }
                _ => {}
            }
        }
        if primary && !matches!(part, ClipPart::Point(_)) {
            self.point_sel = None;
            self.selected_track = None;
        }
        match part {
            ClipPart::Body | ClipPart::Header | ClipPart::AutoBody
                if button == PointerButton::Secondary =>
            {
                // A right click works on what it points at: a clip outside the selection
                // becomes the selection, one inside keeps it.
                if !self.selection.contains(&id) {
                    self.selection.select_only(id);
                }
                self.point_sel = None;
                self.selected_track = None;
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Clip,
                    at: pos,
                });
            }
            ClipPart::Point(i) => {
                let ClipContent::Automation(points) = &clip.content else {
                    return;
                };
                if !primary {
                    if button == PointerButton::Secondary {
                        out.push(content_cmd(id, seq_edit::remove_point(points, i), None));
                    }
                    return;
                }
                if modifiers.shift {
                    out.push(content_cmd(id, seq_edit::toggle_curve(points, i), None));
                    return;
                }
                self.selected_track = None;
                if modifiers.command() {
                    // Ctrl-click adds a point to the selection, or takes it out again.
                    let mut set = match &self.point_sel {
                        Some((c, s)) if *c == id => s.clone(),
                        _ => BTreeSet::new(),
                    };
                    if !set.remove(&i) {
                        set.insert(i);
                    }
                    self.selection.select_only(id);
                    self.point_sel = (!set.is_empty()).then_some((id, set));
                    return;
                }
                self.selection.select_only(id);
                let keep = matches!(&self.point_sel, Some((c, s)) if *c == id && s.contains(&i));
                if !keep {
                    self.point_sel = Some((id, BTreeSet::from([i])));
                }
                let merge = self.merge_id();
                self.start_point_drag(track, lane, &clip, points.clone(), i, pos, merge, false);
            }
            ClipPart::AutoBody if primary => {
                let ClipContent::Automation(points) = &clip.content else {
                    return;
                };
                self.selection.select_only(id);
                let step = self.step(modifiers);
                let raw = self.axis.x_to_tick(pos.x);
                let w = seq_edit::content_windows(&clip)
                    .into_iter()
                    .find(|w| (w.song_start..w.song_end).contains(&raw));
                let Some(w) = w else { return };
                let shift = w.song_start - w.content_start;
                let row = self.row_of(track, lane).map(|r| self.row_rect(r));
                let Some(row) = row else { return };
                let (top, bottom) = value_range(self.clip_rect_in(row, &clip));
                let param = self.param_of(track, lane);
                let tick = (snap_round(raw, step) - shift).max(0);
                let value = seq_edit::value_from_y(&param, pos.y, top, bottom - top);
                let (list, index) = seq_edit::insert_point(
                    points,
                    AutomationPoint {
                        tick,
                        value,
                        curve: Curve::Linear,
                    },
                );
                let merge = self.merge_id();
                out.push(content_cmd(id, list.clone(), Some(merge)));
                self.start_point_drag(track, lane, &clip, list, index, pos, merge, true);
                if let Drag::Point { original, .. } = &mut self.drag {
                    original.clone_from(points);
                }
            }
            ClipPart::FadeIn | ClipPart::FadeOut | ClipPart::Gain | ClipPart::EdgeStart
                if primary =>
            {
                let ClipContent::Audio(content) = &clip.content else {
                    return;
                };
                if part == ClipPart::EdgeStart {
                    self.selection.click(id, modifiers.command(), false);
                    if !self.selection.contains(&id) {
                        return;
                    }
                } else {
                    self.selection.select_only(id);
                }
                let Some(row) = self.row_of(track, lane).map(|r| self.row_rect(r)) else {
                    return;
                };
                let (top, bottom) = value_range(self.clip_rect_in(row, &clip));
                let grab = match part {
                    ClipPart::FadeIn => AudioGrab::FadeIn,
                    ClipPart::FadeOut => AudioGrab::FadeOut,
                    ClipPart::Gain => AudioGrab::Gain(top, bottom - top),
                    _ => AudioGrab::TrimStart,
                };
                let merge = self.merge_id();
                let last = (clip.start, clip.length, content.clone());
                self.drag = Drag::Audio {
                    start: pos,
                    rate: self.asset_rate(&content.asset),
                    asset_frames: self
                        .assets
                        .get(&content.asset)
                        .map_or(u64::MAX >> 2, |m| m.frames),
                    content: content.clone(),
                    clip,
                    grab,
                    merge,
                    moved: false,
                    emitted: false,
                    last,
                };
            }
            ClipPart::Edge if primary => {
                self.selection.click(id, modifiers.command(), false);
                if !self.selection.contains(&id) {
                    return;
                }
                let merge = self.merge_id();
                let loop_mode = modifiers.shift && !matches!(clip.content, ClipContent::Audio(_));
                self.drag = Drag::Resize {
                    start: pos,
                    last: (clip.length, clip.loop_length),
                    clip,
                    merge,
                    moved: false,
                    emitted: false,
                    loop_mode,
                };
            }
            ClipPart::Body | ClipPart::Header if primary => {
                if clicks >= 2 && matches!(clip.content, ClipContent::Notes(_)) {
                    self.selection.select_only(id);
                    out.push(UiAction::OpenPianoRoll(id));
                    return;
                }
                let was_selected = self.selection.contains(&id);
                self.selection
                    .click(id, modifiers.command(), modifiers.shift);
                if !self.selection.contains(&id) {
                    return;
                }
                let reselect = (was_selected
                    && !modifiers.command()
                    && !modifiers.shift
                    && self.selection.len() > 1)
                    .then_some(id);
                let originals: Vec<Clip> = self
                    .song_clips()
                    .filter(|c| self.selection.contains(&c.id))
                    .cloned()
                    .collect();
                let merge = self.merge_id();
                self.drag = Drag::Move {
                    start: pos,
                    grabbed_start: clip.start,
                    originals,
                    merge,
                    moved: false,
                    emitted: false,
                    last_dt: 0,
                    reselect,
                };
            }
            _ => {}
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "drag state is built in one place"
    )]
    fn start_point_drag(
        &mut self,
        track: usize,
        lane: usize,
        clip: &Clip,
        points: Vec<AutomationPoint>,
        index: usize,
        pos: Point,
        merge: u64,
        emitted: bool,
    ) {
        let raw = self.axis.x_to_tick(pos.x);
        let windows = seq_edit::content_windows(clip);
        let w = windows
            .iter()
            .find(|w| (w.song_start..w.song_end).contains(&raw))
            .or(windows.first())
            .copied();
        let Some(w) = w else { return };
        let Some(row) = self.row_of(track, lane).map(|r| self.row_rect(r)) else {
            return;
        };
        let (top, bottom) = value_range(self.clip_rect_in(row, clip));
        self.drag = Drag::Point {
            start: pos,
            id: clip.id,
            merge,
            shift: w.song_start - w.content_start,
            top,
            height: bottom - top,
            param: self.param_of(track, lane),
            original: points.clone(),
            last: points.clone(),
            points,
            index,
            moved: false,
            emitted,
        };
    }

    fn song_clips(&self) -> impl Iterator<Item = &Clip> {
        self.song
            .tracks
            .iter()
            .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter()))
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if matches!(self.drag, Drag::None) {
            let h = self.hit(pos);
            if let Some(m) = &mut self.menu {
                let row = m.row_at(pos);
                if row != m.hover {
                    m.hover = row;
                    self.repaint = true;
                }
            }
            if self.hover.as_ref() != Some(&h) {
                self.hover = Some(h);
                self.repaint = true;
            }
            return;
        }
        if let Drag::Pan { last } = &mut self.drag {
            let d = *last - pos;
            *last = pos;
            self.axis.scroll_px(d.x);
            self.scroll_y += d.y;
            self.clamp_scroll();
            self.vp.manual_scroll();
            self.repaint = true;
            return;
        }
        if let Drag::HScroll { grab } = self.drag {
            self.hscroll_to(pos.x - grab);
            self.vp.manual_scroll();
            self.repaint = true;
            return;
        }
        if matches!(self.drag, Drag::TempoPoint { .. } | Drag::Meter { .. }) {
            self.tempo_drag_move(pos, modifiers, out);
            return;
        }
        if matches!(self.drag, Drag::Height) {
            self.height_move(pos, out);
            return;
        }
        if matches!(self.drag, Drag::Mark) {
            let actions = self.marker_drag_move(pos, modifiers);
            out.extend(actions);
            return;
        }
        let step = self.step(modifiers);
        let min = seq_edit::min_length(&step, MIN_TICKS);
        let axis = self.axis;
        let (tempo, assets) = (&self.tempo, &self.assets);
        match &mut self.drag {
            Drag::None
            | Drag::Pan { .. }
            | Drag::HScroll { .. }
            | Drag::TempoPoint { .. }
            | Drag::Height
            | Drag::Meter { .. }
            | Drag::Mark => {}
            Drag::Move {
                start,
                grabbed_start,
                originals,
                merge,
                moved,
                emitted,
                last_dt,
                ..
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let new = seq_edit::drag_start(*grabbed_start, delta, step);
                let min_start = originals.iter().map(|c| c.start).min().unwrap_or(0);
                let dt = (new - *grabbed_start).max(-min_start);
                if dt != *last_dt {
                    for c in originals.iter() {
                        out.push(geometry_cmd(
                            c,
                            c.start + dt,
                            c.length,
                            c.loop_length,
                            *merge,
                        ));
                    }
                    *last_dt = dt;
                    *emitted = true;
                }
                self.repaint = true;
            }
            Drag::Resize {
                start,
                clip,
                merge,
                moved,
                emitted,
                loop_mode,
                last,
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let new = if *loop_mode {
                    let orig = clip.loop_length.unwrap_or(clip.length);
                    let l = seq_edit::drag_length(clip.start, clip.start + orig, delta, step, min);
                    (clip.length, Some(l))
                } else {
                    let l = seq_edit::drag_length(
                        clip.start,
                        clip.start + clip.length,
                        delta,
                        step,
                        min,
                    );
                    (l, clip.loop_length)
                };
                if new != *last {
                    out.push(if let ClipContent::Audio(a) = &clip.content {
                        let rate = assets.get(&a.asset).map_or(DEFAULT_RATE, |m| m.sample_rate);
                        let frames = clip_frames(tempo, clip.start, new.0, rate);
                        let (fade_in, fade_out) = audio::clamp_fades(a.fade_in, a.fade_out, frames);
                        let content = AudioClipContent {
                            fade_in,
                            fade_out,
                            ..a.clone()
                        };
                        audio_clip_cmd(clip, clip.start, new.0, content, *merge)
                    } else {
                        geometry_cmd(clip, clip.start, new.0, new.1, *merge)
                    });
                    *last = new;
                    *emitted = true;
                }
                self.repaint = true;
            }
            Drag::Audio {
                start,
                clip,
                content,
                grab,
                rate,
                asset_frames,
                merge,
                moved,
                emitted,
                last,
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let frames = clip_frames(tempo, clip.start, clip.length, *rate);
                let at = |x: f64| {
                    tempo.frame_at_tick(axis.scroll + (x - axis.x0) / axis.px_per_tick, *rate)
                };
                let delta = (at(pos.x) - at(start.x)).round() as i64;
                let mut new = (clip.start, clip.length, content.clone());
                match grab {
                    AudioGrab::FadeIn => {
                        new.2.fade_in =
                            audio::drag_fade_in(content.fade_in, delta, frames, content.fade_out);
                    }
                    AudioGrab::FadeOut => {
                        new.2.fade_out =
                            audio::drag_fade_out(content.fade_out, delta, frames, content.fade_in);
                    }
                    AudioGrab::Gain(_, height) => {
                        new.2.gain_db = audio::gain_drag(
                            content.gain_db,
                            pos.y - start.y,
                            *height,
                            modifiers.shift,
                        );
                    }
                    AudioGrab::TrimStart => {
                        let wanted =
                            snap_round(clip.start + axis.dx_to_ticks(pos.x - start.x), step);
                        let orig = (clip.start, clip.length, content.source_offset);
                        let t = audio::trim_left(tempo, *rate, orig, *asset_frames, wanted, min);
                        let frames = clip_frames(tempo, t.start, t.length, *rate);
                        let (fade_in, fade_out) =
                            audio::clamp_fades(content.fade_in, content.fade_out, frames);
                        new = (
                            t.start,
                            t.length,
                            AudioClipContent {
                                source_offset: t.source_offset,
                                fade_in,
                                fade_out,
                                ..content.clone()
                            },
                        );
                    }
                }
                if new != *last {
                    out.push(audio_clip_cmd(clip, new.0, new.1, new.2.clone(), *merge));
                    *last = new;
                    *emitted = true;
                }
                self.repaint = true;
            }
            Drag::Draw {
                start,
                t1,
                moved,
                modifiers: m,
                ..
            }
            | Drag::Loop {
                start,
                t1,
                moved,
                modifiers: m,
                ..
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                *t1 = axis.x_to_tick(pos.x);
                *m = modifiers;
                self.repaint = true;
            }
            Drag::Point {
                start,
                id,
                merge,
                shift,
                top,
                height,
                param,
                points,
                index,
                moved,
                emitted,
                last,
                ..
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let tick = snap_round(axis.x_to_tick(pos.x), step) - *shift;
                let value = seq_edit::value_from_y(param, pos.y, *top, *height);
                let new = seq_edit::move_point(points, *index, tick, value);
                if new != *last {
                    out.push(content_cmd(*id, new.clone(), Some(*merge)));
                    *last = new;
                    *emitted = true;
                }
                self.repaint = true;
            }
        }
    }

    fn pointer_up(&mut self, pos: Point, _modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.repaint = true;
        match drag {
            Drag::None | Drag::Pan { .. } | Drag::HScroll { .. } => {}
            Drag::Mark => self.marker_drag_up(out),
            Drag::Height => self.height_up(out),
            Drag::Move {
                emitted,
                moved,
                reselect,
                ..
            } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
                if !moved && let Some(id) = reselect {
                    self.selection.select_only(id);
                }
            }
            Drag::Resize { emitted, .. }
            | Drag::Point { emitted, .. }
            | Drag::TempoPoint { emitted, .. }
            | Drag::Meter { emitted, .. }
            | Drag::Audio { emitted, .. } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Draw {
                device,
                lane,
                automation,
                t0,
                t1,
                moved,
                modifiers,
                pencil,
                ..
            } => {
                if !moved {
                    self.selection.clear();
                    self.point_sel = None;
                    if pencil {
                        // The pencil draws with a plain click: a one-bar clip.
                        let start = snap_floor(t0, self.step(modifiers)).max(0);
                        let length = self.bars.bar_ticks_at(start);
                        out.push(new_clip_cmd(device, lane, automation, start, length));
                    }
                    return;
                }
                let (start, length) = seq_edit::draw_span(t0, t1, self.step(modifiers), MIN_TICKS);
                out.push(new_clip_cmd(device, lane, automation, start, length));
            }
            Drag::Loop {
                t0,
                t1,
                moved,
                modifiers,
                ..
            } => {
                if moved {
                    let (s, len) = seq_edit::draw_span(t0, t1, self.step(modifiers), MIN_TICKS);
                    out.push(song_cmd(SongCommand::SetLoop {
                        range: (s, s + len),
                        enabled: true,
                    }));
                } else {
                    out.push(song_cmd(SongCommand::SetLoop {
                        range: self.song.loop_range,
                        enabled: !self.song.loop_enabled,
                    }));
                }
            }
        }
        let _ = pos;
    }

    fn wheel(&mut self, dx: f64, dy: f64, pos: Point, modifiers: Modifiers) {
        self.repaint = true;
        if self.list_wheel(dy, pos) {
            return;
        }
        if let Some(m) = &mut self.menu
            && m.rect().contains(pos)
        {
            let max = m.items.len().saturating_sub(MENU_ROWS);
            m.scroll = if dy > 0.0 {
                m.scroll.saturating_sub(1)
            } else {
                (m.scroll + 1).min(max)
            };
            return;
        }
        if modifiers.alt && !modifiers.command() {
            // Alt + wheel: the vertical zoom, the row under the pointer stays under it.
            self.zoom_tracks_at(TRACK_ZOOM_STEP.powf(dy), pos.y);
            return;
        }
        if modifiers.command() {
            let anchor = pos.x.max(self.axis.x0);
            self.axis.zoom_at(ZOOM_STEP.powf(dy), anchor);
            self.vp.set_zoomed_to_selection(false);
            return;
        }
        self.vp.manual_scroll();
        if modifiers.shift && dx.abs() < 1e-9 {
            self.axis.scroll_px(-dy * SCROLL_STEP);
            return;
        }
        self.axis.scroll_px(-dx * SCROLL_STEP);
        self.scroll_y -= dy * SCROLL_STEP;
        self.clamp_scroll();
    }

    /// The keys that are not commands: Escape (closes a menu, cancels a drag, clears the
    /// selection) and Home. Everything else a key can do is a registry command
    /// ([`Self::command`]), so a remap or a context really applies.
    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if self.mk.rename.is_some() {
            self.rename_key(key, modifiers, out);
            return;
        }
        if modifiers.command() {
            return;
        }
        match key {
            Key::Escape => {
                if self.menu.take().is_some() {
                    return;
                }
                if !matches!(self.drag, Drag::None) {
                    self.cancel_drag(out);
                } else {
                    self.selection.clear();
                    self.selected_track = None;
                    self.point_sel = None;
                    self.tempo_sel = None;
                    self.meter_sel = None;
                    self.mk.sel = None;
                }
            }
            Key::Home => {
                self.axis.scroll = 0.0;
                self.vp.manual_scroll();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ commands

    /// Runs a registry command the shell offers to this view: the edit commands
    /// on the clip selection, snap, zoom, follow, the tool palette and the transport loop
    /// commands that need the selection. A command that is not this view's, or has nothing to
    /// work on, is [`CommandOutcome::Declined`] so the next binding runs.
    pub fn command(
        &mut self,
        id: &str,
        arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync(doc, registry);
        let outcome = self.run_command(id, arg);
        if outcome.is_taken() {
            self.repaint = true;
        }
        outcome
    }

    fn run_command(&mut self, id: &str, arg: Option<&str>) -> CommandOutcome {
        if let Some(tool) = Tool::from_command(id) {
            self.set_tool(tool);
            return CommandOutcome::Handled(Vec::new());
        }
        if let Some(outcome) = self.tempo_command(id, arg) {
            return outcome;
        }
        if let Some(outcome) = self.marker_command(id, arg) {
            return outcome;
        }
        match id {
            "edit.select_all" => {
                let all: Vec<ClipId> = self.song_clips().map(|c| c.id).collect();
                self.selection.set(all);
                self.selected_track = None;
                self.point_sel = None;
                self.tempo_sel = None;
                self.meter_sel = None;
                CommandOutcome::Handled(Vec::new())
            }
            "edit.duplicate" => {
                if let Some((clip, indices)) = self.selected_points() {
                    return CommandOutcome::Handled(vec![workflow_cmd(
                        WorkflowCommand::DuplicatePoints { clip, indices },
                    )]);
                }
                let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                if !ids.is_empty() {
                    return CommandOutcome::Handled(vec![workflow_cmd(
                        WorkflowCommand::DuplicateClips { ids },
                    )]);
                }
                match self.selected_track.clone() {
                    Some(device) => CommandOutcome::Handled(vec![workflow_cmd(
                        WorkflowCommand::DuplicateTrack { device },
                    )]),
                    None => CommandOutcome::Declined,
                }
            }
            "edit.delete" => {
                if let Some((id, indices)) = self.selected_points() {
                    let points = match find_clip(&self.song, id).map(|(_, _, c)| &c.content) {
                        Some(ClipContent::Automation(p)) => p.clone(),
                        _ => return CommandOutcome::Declined,
                    };
                    let kept: Vec<AutomationPoint> = points
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| !indices.contains(i))
                        .map(|(_, p)| p)
                        .collect();
                    self.point_sel = None;
                    return CommandOutcome::Handled(vec![content_cmd(id, kept, None)]);
                }
                let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                match ids.as_slice() {
                    [] => match self.selected_track.take() {
                        Some(device) => {
                            CommandOutcome::Handled(vec![song_cmd(SongCommand::RemoveTrack {
                                device,
                            })])
                        }
                        None => CommandOutcome::Declined,
                    },
                    [id] => {
                        CommandOutcome::Handled(vec![song_cmd(SongCommand::RemoveClip { id: *id })])
                    }
                    _ => {
                        CommandOutcome::Handled(vec![workflow_cmd(WorkflowCommand::RemoveClips {
                            ids: ids.clone(),
                        })])
                    }
                }
            }
            "sequencer.join" => {
                let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                if ids.len() < 2 {
                    return CommandOutcome::NotApplicable(
                        "Select two or more clips on one track to join them".into(),
                    );
                }
                CommandOutcome::Handled(vec![workflow_cmd(WorkflowCommand::JoinClips { ids })])
            }
            "sequencer.select_next_track" => self.select_track(1),
            "sequencer.select_prev_track" => self.select_track(-1),
            "sequencer.split_at_playhead" => {
                let at = self.playhead;
                let only: Option<Vec<ClipId>> = (!self.selection.is_empty())
                    .then(|| self.selected_in_order().iter().map(|c| c.id).collect());
                CommandOutcome::Handled(self.split_all_at_in(at, only.as_deref()))
            }
            "sequencer.toggle_mute" => {
                let clips = self.selected_in_order();
                if clips.is_empty() {
                    return CommandOutcome::Handled(Vec::new());
                }
                // Any unmuted target mutes them all; only an all-muted selection unmutes.
                let muted = clips.iter().any(|c| !c.muted);
                let ids = clips.iter().map(|c| c.id).collect();
                CommandOutcome::Handled(vec![mute_cmd(ids, muted)])
            }
            "sequencer.open_clip" => {
                let first = self
                    .selected_in_order()
                    .into_iter()
                    .find(|c| matches!(c.content, ClipContent::Notes(_)))
                    .map(|c| c.id);
                match first {
                    Some(id) => CommandOutcome::Handled(vec![UiAction::OpenPianoRoll(id)]),
                    None => CommandOutcome::Declined,
                }
            }
            "sequencer.select_next_clip" => self.select_neighbour(true),
            "sequencer.select_prev_clip" => self.select_neighbour(false),
            "transport.loop_to_selection" | "transport.loop_and_play" => {
                let Some((s, e)) = self.selection_span() else {
                    return CommandOutcome::Declined;
                };
                let mut out = vec![song_cmd(SongCommand::SetLoop {
                    range: (s, e),
                    enabled: true,
                })];
                if id == "transport.loop_and_play" {
                    out.push(UiAction::Transport(TransportAction::Locate(s)));
                    out.push(UiAction::Transport(TransportAction::Play));
                }
                CommandOutcome::Handled(out)
            }
            "sequencer.snap_toggle" => {
                self.snap = self.vp.toggle_snap(self.snap);
                CommandOutcome::Handled(Vec::new())
            }
            "sequencer.set_snap" => match arg.and_then(snap_from_arg) {
                Some(snap) => {
                    self.snap = snap;
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.follow" => {
                self.vp.toggle_follow();
                CommandOutcome::Handled(Vec::new())
            }
            "view.zoom_in" | "view.zoom_out" => {
                let factor = if id == "view.zoom_in" {
                    ZOOM_KEY
                } else {
                    1.0 / ZOOM_KEY
                };
                let anchor = (self.axis.x0 + self.bounds.x1) / 2.0;
                self.axis.zoom_at(factor, anchor);
                self.vp.set_zoomed_to_selection(false);
                CommandOutcome::Handled(Vec::new())
            }
            "sequencer.zoom_tracks_in" | "sequencer.zoom_tracks_out" => {
                let factor = if id == "sequencer.zoom_tracks_in" {
                    TRACK_ZOOM_STEP
                } else {
                    1.0 / TRACK_ZOOM_STEP
                };
                let anchor = (self.lanes_top() + self.bounds.y1) / 2.0;
                self.zoom_tracks_at(factor, anchor);
                CommandOutcome::Handled(Vec::new())
            }
            "sequencer.zoom_tracks_reset" => {
                let mut out = Vec::new();
                self.reset_track_heights(&mut out);
                CommandOutcome::Handled(out)
            }
            "sequencer.zoom_selection" => {
                self.zoom_selection();
                CommandOutcome::Handled(Vec::new())
            }
            _ => CommandOutcome::Declined,
        }
    }

    /// `[start, end)` of the selected clips.
    fn selection_span(&self) -> Option<(i64, i64)> {
        let sel = self.selected_in_order();
        let s = sel.iter().map(|c| c.start).min()?;
        let e = sel.iter().map(|c| c.start + c.length).max()?;
        Some((s, e))
    }

    /// Z: zoom to the selected clips, or (again, or with no selection) to the whole song.
    fn zoom_selection(&mut self) {
        let x1 = self.bounds.x1;
        match self.selection_span() {
            Some((s, e)) if !self.vp.zoomed_to_selection() => {
                self.axis.fit_span(s, e, x1);
                self.vp.set_zoomed_to_selection(true);
            }
            _ => {
                let end = self
                    .song_clips()
                    .map(|c| c.start + c.length)
                    .max()
                    .unwrap_or(0)
                    .max(self.bars.bar_ticks_at(0));
                self.axis.fit_span(0, end, x1);
                self.vp.set_zoomed_to_selection(false);
            }
        }
    }

    /// Up and Down: the track after or before the selected one (or the track of the first
    /// selected clip). With nothing selected Down starts at the first track and Up at the last.
    /// The clip selection gives way to the track selection.
    fn select_track(&mut self, delta: i64) -> CommandOutcome {
        let n = self.song.tracks.len();
        if n == 0 {
            return CommandOutcome::Declined;
        }
        let of_clip = self
            .selected_in_order()
            .first()
            .and_then(|c| self.find_clip(c.id).map(|(t, _, _)| t));
        let from = self
            .selected_track
            .as_ref()
            .and_then(|d| self.song.tracks.iter().position(|t| &t.device == d))
            .or(of_clip);
        let to = match from {
            Some(i) => (i as i64 + delta).clamp(0, n as i64 - 1) as usize,
            None if delta > 0 => 0,
            None => n - 1,
        };
        let device = self.song.tracks[to].device.clone();
        self.set_selected_track(Some(device));
        CommandOutcome::Handled(Vec::new())
    }

    /// Left and Right: the clip before or after the selection in time order.
    fn select_neighbour(&mut self, forward: bool) -> CommandOutcome {
        let mut order: Vec<(i64, usize, usize, ClipId)> = Vec::new();
        for (ti, t) in self.song.tracks.iter().enumerate() {
            for (li, l) in t.lanes.iter().enumerate() {
                order.extend(l.clips.iter().map(|c| (c.start, ti, li, c.id)));
            }
        }
        order.sort();
        if order.is_empty() {
            return CommandOutcome::Declined;
        }
        let selected: Vec<usize> = order
            .iter()
            .enumerate()
            .filter(|(_, o)| self.selection.contains(&o.3))
            .map(|(i, _)| i)
            .collect();
        let target = match (selected.first(), selected.last(), forward) {
            (Some(_), Some(&last), true) => (last + 1).min(order.len() - 1),
            (Some(&first), Some(_), false) => first.saturating_sub(1),
            (_, _, true) => 0,
            (_, _, false) => order.len() - 1,
        };
        self.selection.select_only(order[target].3);
        CommandOutcome::Handled(Vec::new())
    }

    /// `SplitClip` for every clip (of `only`, when given) that `at` falls strictly inside, in
    /// song order.
    fn split_all_at_in(&self, at: i64, only: Option<&[ClipId]>) -> Vec<UiAction> {
        self.song_clips()
            .filter(|c| only.is_none_or(|ids| ids.contains(&c.id)))
            .filter(|c| at > c.start && at < c.start + c.length)
            .map(|c| song_cmd(SongCommand::SplitClip { id: c.id, at }))
            .collect()
    }

    /// The razor: split `only` (one clip) or, with `None`, every clip at `at`.
    fn split_all_at(&self, at: i64, only: Option<ClipId>) -> Vec<UiAction> {
        match only {
            Some(id) => self.split_all_at_in(at, Some(&[id])),
            None => self.split_all_at_in(at, None),
        }
    }

    fn selected_in_order(&self) -> Vec<&Clip> {
        self.song_clips()
            .filter(|c| self.selection.contains(&c.id))
            .collect()
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        if matches!(self.drag, Drag::TempoPoint { .. } | Drag::Meter { .. }) {
            return self.cancel_tempo_drag(out);
        }
        if matches!(self.drag, Drag::Height) {
            return self.height_cancel(out);
        }
        if matches!(self.drag, Drag::Mark) {
            self.drag = Drag::None;
            return self.marker_cancel_drag(out);
        }
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Move {
                originals,
                merge,
                emitted: true,
                ..
            } => {
                for c in &originals {
                    out.push(geometry_cmd(c, c.start, c.length, c.loop_length, merge));
                }
                out.push(UiAction::EndMerge);
            }
            Drag::Resize {
                clip,
                merge,
                emitted: true,
                ..
            } => {
                out.push(geometry_cmd(
                    &clip,
                    clip.start,
                    clip.length,
                    clip.loop_length,
                    merge,
                ));
                out.push(UiAction::EndMerge);
            }
            Drag::Audio {
                clip,
                content,
                merge,
                emitted: true,
                ..
            } => {
                out.push(audio_clip_cmd(
                    &clip,
                    clip.start,
                    clip.length,
                    content,
                    merge,
                ));
                out.push(UiAction::EndMerge);
            }
            Drag::Point {
                id,
                merge,
                original,
                emitted: true,
                ..
            } => {
                out.push(content_cmd(id, original, Some(merge)));
                out.push(UiAction::EndMerge);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the view into its bounds. The canvas receives **physical pixels** (everything
    /// is scaled by the scale factor), like [`crate::RackView::paint`].
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        let b = self.bounds;
        canvas.push_transform(Affine::scale(self.scale));
        canvas.push_clip(&self.outer.into());
        sp::rect(canvas, self.outer, self.st.background);
        let tl = self.timeline_rect();
        self.paint_lanes(canvas, doc, tl);
        self.paint_marker_strip(canvas);
        self.paint_tempo_lane(canvas);
        self.paint_ruler(canvas);
        self.paint_track_list(canvas, doc);
        self.paint_corner(canvas);
        self.paint_hscroll(canvas);
        let sep = self.st.separator;
        sp::line(canvas, tl.x0 - 0.5, b.y0, tl.x0 - 0.5, b.y1, sep);
        sp::line(canvas, b.x0, tl.y0 - 0.5, b.x1, tl.y0 - 0.5, sep);
        self.paint_marker_list(canvas);
        if let Some(m) = &self.menu {
            paint_menu(canvas, &self.st, m);
        }
        if self.focused {
            sp::line(
                canvas,
                b.x0,
                b.y0 + 0.5,
                b.x1,
                b.y0 + 0.5,
                self.st.base.focus,
            );
        }
        canvas.pop_clip();
        canvas.pop_transform();
        self.repaint = false;
    }

    fn visible_rows(&self) -> impl Iterator<Item = (&Row, Rect)> {
        let top = self.lanes_top();
        let bottom = self.bounds.y1;
        self.rows.iter().filter_map(move |r| {
            let rr = self.row_rect(r);
            (rr.y1 > top && rr.y0 < bottom).then_some((r, rr))
        })
    }

    fn paint_lanes(&self, c: &mut dyn Canvas, doc: &RackDoc, tl: Rect) {
        let st = &self.st;
        c.push_clip(&tl.into());
        for (row, rr) in self.visible_rows() {
            let lr = Rect::new(tl.x0, rr.y0, tl.x1, rr.y1);
            match &row.kind {
                RowKind::Lane { track, lane } => {
                    let bg = if *lane > 0 {
                        st.lane_auto
                    } else if track % 2 == 0 {
                        st.lane
                    } else {
                        st.lane_alt
                    };
                    sp::rect(c, lr, bg);
                    sp::grid(c, st, &self.axis, lr, &self.bars);
                }
                RowKind::AddTrack(_) | RowKind::AddAudio => sp::rect(c, lr, st.background),
            }
            sp::line(c, lr.x0, lr.y1 - 0.5, lr.x1, lr.y1 - 0.5, st.separator);
        }
        let below = Rect::new(
            tl.x0,
            self.lanes_top() + self.content_height - self.scroll_y,
            tl.x1,
            tl.y1,
        );
        if below.height() > 0.0 {
            sp::grid(c, st, &self.axis, below, &self.bars);
        }
        if self.song.loop_enabled {
            let (a, z) = self.song.loop_range;
            let r = Rect::new(self.axis.tick_to_x(a), tl.y0, self.axis.tick_to_x(z), tl.y1);
            sp::rect(c, r, st.loop_shade);
        }
        for (row, rr) in self.visible_rows() {
            let RowKind::Lane { track, lane } = row.kind else {
                continue;
            };
            let t = &self.song.tracks[track];
            let accent = sp::device_accent(&st.base, doc, &t.device);
            let name = sp::track_name(doc, &t.device);
            let param = self.param_of(track, lane);
            for clip in &t.lanes[lane].clips {
                let r = self.clip_rect_in(rr, clip);
                if r.x1 < tl.x0 - 2.0 || r.x0 > tl.x1 + 2.0 {
                    continue;
                }
                let selected = self.selection.contains(&clip.id);
                self.paint_clip(c, r, clip, accent, &name, &param, selected, t.muted, tl);
            }
        }
        if let Drag::Draw {
            device,
            lane,
            t0,
            t1,
            moved: true,
            modifiers,
            ..
        } = &self.drag
            && let Some(lr) = self.lane_rect(device, *lane)
        {
            let (s, len) = seq_edit::draw_span(*t0, *t1, self.step(*modifiers), MIN_TICKS);
            let r = Rect::new(
                self.axis.tick_to_x(s),
                lr.y0 + 3.0,
                self.axis.tick_to_x(s + len),
                lr.y1 - 3.0,
            );
            let accent = sp::device_accent(&st.base, doc, device);
            c.fill_with(RoundedRect::from_rect(r, 3.0), accent.with_alpha(0.25));
            c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.0, accent);
        }
        self.paint_drop_hint(c, doc, tl);
        let col = if self.recording {
            st.record
        } else {
            st.playhead
        };
        let x = self.axis.tick_to_x(self.playhead);
        if x >= tl.x0 - 1.0 && x <= tl.x1 + 1.0 {
            let x = x.round() + 0.5;
            c.stroke_with(
                vello::kurbo::Line::new(pt(x, tl.y0), pt(x, tl.y1)),
                1.5,
                col,
            );
        }
        c.pop_clip();
    }

    /// The drop indicator: the audio lane under the pointer highlighted (or a placeholder for
    /// the new tracks below the last row) and an insertion line at the snapped tick.
    fn paint_drop_hint(&self, c: &mut dyn Canvas, doc: &RackDoc, tl: Rect) {
        let Some(hover) = &self.drop_hover else {
            return;
        };
        let st = &self.st;
        let (area, accent) = match &hover.target {
            ImportTarget::ExistingTrack(device) => {
                let Some(r) = self.lane_rect(device, 0) else {
                    return;
                };
                (r, sp::device_accent(&st.base, doc, device))
            }
            ImportTarget::NewTracks => {
                let y = self.lanes_top() + self.content_height - self.scroll_y;
                let h = self.default_row_height() * hover.count.max(1) as f64;
                (Rect::new(tl.x0, y, tl.x1, y + h), st.selection)
            }
        };
        let shape = RoundedRect::from_rect(area.inset(-1.0), 3.0);
        c.fill_with(shape, accent.with_alpha(0.14));
        c.stroke(
            &shape.into(),
            &Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]),
            &accent.into(),
        );
        let x = self.axis.tick_to_x(hover.tick).round() + 0.5;
        if x >= tl.x0 && x <= tl.x1 {
            c.stroke_with(
                vello::kurbo::Line::new(pt(x, area.y0), pt(x, area.y1)),
                2.0,
                st.selection,
            );
        }
    }

    /// Frame rate of an asset's audio, for tick / frame maths (the session rate for imported
    /// assets, which are converted on import); 48 kHz for an asset missing from the pool.
    fn asset_rate(&self, asset: &str) -> u32 {
        self.assets
            .get(asset)
            .map_or(DEFAULT_RATE, |m| m.sample_rate)
    }

    /// Logical x of a fractional tick.
    fn x_of_tick(&self, tick: f64) -> f64 {
        self.axis.x0 + (tick - self.axis.scroll) * self.axis.px_per_tick
    }

    /// The fractional tick at a logical x.
    fn tick_of_x(&self, x: f64) -> f64 {
        self.axis.scroll + (x - self.axis.x0) / self.axis.px_per_tick
    }

    /// Frames a clip lasts (its length in real time at its asset's rate).
    fn clip_frames(&self, clip: &Clip, rate: u32) -> u64 {
        self.tempo
            .frame_at(clip.start + clip.length, rate)
            .saturating_sub(self.tempo.frame_at(clip.start, rate))
    }

    /// Pixel geometry of an audio clip's handles for the clip rectangle `r` (clip lane `tl`).
    fn audio_geo(&self, r: Rect, clip: &Clip, a: &AudioClipContent, tl: Rect) -> AudioGeo {
        let rate = self.asset_rate(&a.asset);
        let start_frame = self.tempo.frame_at_tick(clip.start as f64, rate);
        let frames = self.clip_frames(clip, rate);
        let frame_x = |f: f64| self.x_of_tick(self.tempo.tick_at_frame(start_frame + f, rate));
        let fade_in_x = frame_x(a.fade_in as f64);
        let fade_out_x = frame_x(frames.saturating_sub(a.fade_out) as f64);
        let (top, bottom) = value_range(r);
        let gain_y = audio::gain_to_y(a.gain_db, top, bottom);
        let cy = r.y0 + CLIP_HEADER / 2.0;
        let hx = |x: f64| x.max(r.x0 + HANDLE_INSET).min(r.x1 - HANDLE_INSET);
        let mid = (r.x0.max(tl.x0) + r.x1.min(tl.x1)) / 2.0;
        AudioGeo {
            rate,
            start_frame,
            fade_in_x,
            fade_out_x,
            gain_y,
            fade_in: Rect::from_center_size(pt(hx(fade_in_x), cy), (HANDLE, HANDLE)),
            fade_out: Rect::from_center_size(pt(hx(fade_out_x), cy), (HANDLE, HANDLE)),
            gain: (r.width() >= GAIN_MIN_WIDTH)
                .then(|| Rect::from_center_size(pt(mid, gain_y), (GAIN_W, GAIN_H))),
        }
    }

    /// Which audio clip handle is under `pos`, if any (fade-in, fade-out, gain).
    fn audio_handle_at(&self, pos: Point, r: Rect, clip: &Clip, tl: Rect) -> Option<ClipPart> {
        let ClipContent::Audio(a) = &clip.content else {
            return None;
        };
        let g = self.audio_geo(r, clip, a, tl);
        let slop = |h: Rect| h.inflate(2.0, 2.0).contains(pos);
        if slop(g.fade_in) {
            Some(ClipPart::FadeIn)
        } else if slop(g.fade_out) {
            Some(ClipPart::FadeOut)
        } else if g.gain.is_some_and(|h| h.inflate(0.0, 2.0).contains(pos)) {
            Some(ClipPart::Gain)
        } else {
            None
        }
    }

    /// Paints the waveform, fades, gain line and handles of an audio clip.
    #[allow(clippy::too_many_arguments, reason = "painting inputs")]
    fn paint_audio(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        vis: Rect,
        body: Rect,
        clip: &Clip,
        a: &AudioClipContent,
        accent: Color,
        muted: bool,
        tl: Rect,
    ) {
        let st = &self.st;
        let g = self.audio_geo(r, clip, a, tl);
        let meta = self.assets.get(&a.asset);
        let (top, bottom) = value_range(r);
        let x_start = vis.x0.max(tl.x0).floor();
        let x_end = vis.x1.min(tl.x1).ceil();
        // Source frame under a logical x: the offset plus the real time since the clip start.
        let source = |x: f64| {
            audio::source_frame(
                &self.tempo,
                g.rate,
                clip.start,
                a.source_offset,
                self.tick_of_x(x),
            )
        };
        if let Some(meta) = meta {
            let end_x = self.x_of_tick(self.tempo.tick_at_frame(
                g.start_frame + meta.frames.saturating_sub(a.source_offset) as f64,
                g.rate,
            ));
            if end_x < vis.x1 {
                let empty = Rect::new(end_x.max(vis.x0), body.y0, vis.x1, body.y1);
                c.fill_with(empty, st.outside);
            }
            if let Some(peaks) = self.peaks.get(&a.asset) {
                let mid = (x_start + x_end) / 2.0;
                let fpp = (source(mid + 1.0) - source(mid)).abs().max(1e-4);
                let samples = self.samples.get(&a.asset).map(|s| &s[..]);
                let detail = WaveDetail::pick(peaks, samples, fpp);
                let gain = 10f64.powf(f64::from(a.gain_db) / 20.0);
                let wave = st.audio_wave(accent, muted);
                let bands = audio::channel_bands(body.y0 + 2.0, body.y1 - 2.0, detail.channels());
                let columns = ((x_end - x_start).max(0.0) as usize).min(MAX_COLUMNS);
                // A source frame back to its x: the inverse of `source`.
                let frame_x = |f: f64| {
                    self.x_of_tick(
                        self.tempo
                            .tick_at_frame(g.start_frame + (f - a.source_offset as f64), g.rate),
                    )
                };
                for (ch, (cy, half)) in bands.into_iter().enumerate() {
                    let channel = WaveChannel {
                        detail,
                        ch,
                        cy,
                        half,
                        gain,
                        color: wave,
                        x_start,
                        columns,
                        line_x: (vis.x0, vis.x1),
                        limit: meta.frames,
                        frames_per_pixel: fpp,
                    };
                    paint_channel(c, &channel, &source, &frame_x);
                }
            } else {
                let text = fit_label(&meta.original_name, vis.width() - 12.0, 9.0);
                sp::label(
                    c,
                    &text,
                    vis.x0.max(tl.x0) + 6.0,
                    body.center().y + 3.0,
                    9.0,
                    st.base.text_muted,
                    Align::Start,
                );
            }
        } else {
            sp::label(
                c,
                "missing asset",
                vis.x0.max(tl.x0) + 6.0,
                body.center().y + 3.0,
                9.0,
                st.record,
                Align::Start,
            );
        }
        let curve = a.fade_curve;
        if a.fade_in > 0 && g.fade_in_x > r.x0 {
            let (path, line) = fade_paths(r.x0, g.fade_in_x, top, bottom, curve, true);
            c.fill_with(path, st.fade_shade);
            c.stroke(
                &line.into(),
                &Stroke::new(1.0),
                &st.base.text.with_alpha(0.7).into(),
            );
        }
        if a.fade_out > 0 && g.fade_out_x < r.x1 {
            let (path, line) = fade_paths(g.fade_out_x, r.x1, top, bottom, curve, false);
            c.fill_with(path, st.fade_shade);
            c.stroke(
                &line.into(),
                &Stroke::new(1.0),
                &st.base.text.with_alpha(0.7).into(),
            );
        }
        let hover = match &self.hover {
            Some(Hit::Clip { id, part, .. }) if *id == clip.id => Some(*part),
            _ => None,
        };
        let lit = |part: ClipPart| {
            if hover == Some(part) {
                st.selection
            } else {
                st.handle.with_alpha(0.85)
            }
        };
        let gy = g.gain_y.round() + 0.5;
        sp::line(c, vis.x0, gy, vis.x1, gy, st.gain_line.with_alpha(0.55));
        if let Some(h) = g.gain {
            let shape = RoundedRect::from_rect(h, 2.0);
            let col = if hover == Some(ClipPart::Gain) {
                st.handle
            } else {
                st.gain_line
            };
            c.fill_with(shape, col.with_alpha(0.9));
            c.stroke_with(shape, 1.0, st.base.plate);
        }
        for (rect, part) in [
            (g.fade_in, ClipPart::FadeIn),
            (g.fade_out, ClipPart::FadeOut),
        ] {
            let mut tri = BezPath::new();
            let (l, rr, t, b) = (rect.x0, rect.x1, rect.y0, rect.y1);
            if part == ClipPart::FadeIn {
                tri.move_to(pt(l, b));
                tri.line_to(pt(rr, t));
                tri.line_to(pt(rr, b));
            } else {
                tri.move_to(pt(l, t));
                tri.line_to(pt(rr, b));
                tri.line_to(pt(l, b));
            }
            tri.close_path();
            c.fill_with(tri, lit(part));
        }
    }

    #[allow(clippy::too_many_arguments, reason = "painting inputs")]
    fn paint_clip(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        clip: &Clip,
        accent: Color,
        name: &str,
        param: &ParamDescriptor,
        selected: bool,
        track_muted: bool,
        tl: Rect,
    ) {
        let st = &self.st;
        let muted = clip.muted || track_muted;
        // Keep huge clips inside a sane coordinate range.
        let vis = Rect::new(r.x0.max(tl.x0 - 4.0), r.y0, r.x1.min(tl.x1 + 4.0), r.y1);
        let shape = RoundedRect::from_rect(vis, 3.0);
        let fill = if matches!(clip.content, ClipContent::Audio(_)) {
            st.audio_body(accent, muted)
        } else {
            st.clip_body(accent, muted)
        };
        c.fill_with(shape, fill);
        c.push_clip(&shape.into());
        let head = Rect::new(vis.x0, vis.y0, vis.x1, vis.y0 + CLIP_HEADER);
        c.fill_with(head, st.clip_head(accent, muted).with_alpha(0.9));
        let body = Rect::new(vis.x0, vis.y0 + CLIP_HEADER, vis.x1, vis.y1);
        let windows = seq_edit::content_windows(clip);
        let light = mix(st.clip_head(accent, muted), st.base.text, 0.35);
        match &clip.content {
            ClipContent::Notes(notes) if !notes.is_empty() => {
                let lo = notes.iter().map(|n| n.key).min().unwrap_or(60);
                let hi = notes.iter().map(|n| n.key).max().unwrap_or(60);
                let span = f64::from(hi - lo).max(11.0) + 1.0;
                let lo = f64::from(lo) - ((span - 1.0) - f64::from(hi - lo)) / 2.0;
                let (y0, y1) = (body.y0 + 3.0, body.y1 - 3.0);
                let kh = ((y1 - y0) / span).max(1.5);
                for w in &windows {
                    let (wx0, wx1) = (
                        self.axis.tick_to_x(w.song_start),
                        self.axis.tick_to_x(w.song_end),
                    );
                    if wx1 < tl.x0 || wx0 > tl.x1 || wx1 - wx0 < 2.0 {
                        continue;
                    }
                    let len = w.song_end - w.song_start;
                    for n in notes {
                        if n.tick < w.content_start || n.tick >= w.content_start + len {
                            continue;
                        }
                        let s = w.song_start + n.tick - w.content_start;
                        let e = (s + n.length).min(w.song_end);
                        let x0 = self.axis.tick_to_x(s);
                        let x1 = self.axis.tick_to_x(e).max(x0 + 1.5);
                        if x1 < tl.x0 || x0 > tl.x1 {
                            continue;
                        }
                        let y = y1 - (f64::from(n.key) - lo + 1.0) * (y1 - y0) / span;
                        let h = kh.clamp(2.5, 4.0);
                        c.fill_with(Rect::new(x0, y - h / 2.0, x1 - 0.5, y + h / 2.0), light);
                    }
                }
            }
            ClipContent::Automation(points) => {
                let (top, bottom) = value_range(r);
                let ny =
                    |v: f32| bottom - f64::from(seq_edit::value_to_norm(param, v)) * (bottom - top);
                for w in &windows {
                    let (wx0, wx1) = (
                        self.axis.tick_to_x(w.song_start),
                        self.axis.tick_to_x(w.song_end),
                    );
                    if wx1 < tl.x0 || wx0 > tl.x1 || wx1 - wx0 < 1.0 {
                        continue;
                    }
                    let len = w.song_end - w.song_start;
                    let path = automation_path(
                        points,
                        w.content_start,
                        len,
                        |t| self.axis.tick_to_x(w.song_start + t - w.content_start),
                        &ny,
                    );
                    let Some(path) = path else { continue };
                    let mut fill = path.clone();
                    fill.line_to(pt(wx1, bottom));
                    fill.line_to(pt(wx0, bottom));
                    fill.close_path();
                    c.fill_with(fill, accent.with_alpha(0.16));
                    c.stroke(&path.into(), &Stroke::new(1.5), &light.into());
                }
                for (i, p) in self.point_positions_for(clip, r, param) {
                    if p.x < tl.x0 - 6.0 || p.x > tl.x1 + 6.0 {
                        continue;
                    }
                    let step = points.get(i).is_some_and(|ap| ap.curve == Curve::Step);
                    if step {
                        let q = Rect::from_center_size(p, (6.0, 6.0));
                        c.fill_with(q, light);
                        c.stroke_with(q, 1.0, st.base.plate);
                    } else {
                        c.fill_with(Circle::new(p, 3.2), light);
                        c.stroke_with(Circle::new(p, 3.2), 1.0, st.base.plate);
                    }
                }
            }
            ClipContent::Audio(a) => self.paint_audio(c, r, vis, body, clip, a, accent, muted, tl),
            ClipContent::Notes(_) => {}
        }
        for w in windows.iter().skip(1) {
            let x = self.axis.tick_to_x(w.song_start).round() + 0.5;
            if x > tl.x0 && x < tl.x1 {
                sp::line(c, x, vis.y0, x, vis.y1, st.base.plate.with_alpha(0.7));
                let mut notch = BezPath::new();
                notch.move_to(pt(x, vis.y0));
                notch.line_to(pt(x + 5.0, vis.y0));
                notch.line_to(pt(x, vis.y0 + 5.0));
                notch.close_path();
                c.fill_with(notch, st.base.plate);
            }
        }
        let button = mute_button_rect(r, tl, matches!(clip.content, ClipContent::Audio(_)));
        if vis.width() > 34.0 {
            // Audio clips keep the label clear of the fade-in handle.
            let x = vis.x0.max(tl.x0)
                + if matches!(clip.content, ClipContent::Audio(_)) {
                    14.0
                } else {
                    4.0
                };
            let label = match &clip.content {
                ClipContent::Automation(_) => param.name,
                ClipContent::Notes(_) | ClipContent::Audio(_) => name,
            };
            let right = button.map_or(vis.x1, |b| b.x0);
            let text = fit_label(label, right - x - 4.0, 8.0);
            c.text(&TextRun::new(text, pt(x, vis.y0 + 9.0), 8.0, st.base.plate).weight(700));
        }
        if let Some(b) = button {
            let hover = matches!(
                &self.hover,
                Some(Hit::Clip { id, part: ClipPart::Mute, .. }) if *id == clip.id
            );
            self.paint_mute_button(c, b, clip.muted, hover);
        }
        c.pop_clip();
        let (w, col) = if selected {
            (1.5, st.selection)
        } else {
            (1.0, st.base.bevel_dark)
        };
        c.stroke_with(RoundedRect::from_rect(vis, 3.0), w, col);
    }

    /// The "M" toggle of a clip header: lit when the clip itself is muted.
    fn paint_mute_button(&self, c: &mut dyn Canvas, b: Rect, on: bool, hover: bool) {
        let st = &self.st;
        let shape = RoundedRect::from_rect(b, 2.0);
        if on {
            c.fill_with(shape, st.base.highlight);
        } else {
            let a = if hover { 0.45 } else { 0.22 };
            c.fill_with(shape, st.base.plate.with_alpha(a));
        }
        let ink = if on { st.base.plate } else { st.base.text };
        c.text(
            &TextRun::new(
                "M",
                pt(b.x0 + 2.2, b.y1 - 2.2),
                8.0,
                ink.with_alpha(if on { 1.0 } else { 0.8 }),
            )
            .weight(700),
        );
    }

    fn point_positions_for(
        &self,
        clip: &Clip,
        r: Rect,
        param: &ParamDescriptor,
    ) -> Vec<(usize, Point)> {
        let ClipContent::Automation(points) = &clip.content else {
            return Vec::new();
        };
        let (top, bottom) = value_range(r);
        let mut out = Vec::new();
        for w in seq_edit::content_windows(clip) {
            let len = w.song_end - w.song_start;
            for (i, ap) in points.iter().enumerate() {
                if ap.tick >= w.content_start && ap.tick < w.content_start + len {
                    let x = self
                        .axis
                        .tick_to_x(w.song_start + ap.tick - w.content_start);
                    let n = f64::from(seq_edit::value_to_norm(param, ap.value));
                    out.push((i, Point::new(x, bottom - n * (bottom - top))));
                }
            }
        }
        out
    }

    fn paint_hscroll(&self, c: &mut dyn Canvas) {
        let tr = self.hscroll_rect();
        sp::rect(c, tr, mix(self.st.ruler, self.st.background, 0.5));
        let active =
            matches!(self.drag, Drag::HScroll { .. }) || self.hover.as_ref() == Some(&Hit::HScroll);
        let t = if active { 0.9 } else { 0.55 };
        sp::rect(
            c,
            self.hscroll_thumb(),
            mix(self.st.separator, self.st.base.focus, t),
        );
    }

    fn paint_ruler(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let rr = self.ruler_rect();
        c.push_clip(&rr.into());
        sp::rect(c, rr, st.ruler);
        let labels = Rect::new(rr.x0, rr.y0, rr.x1, rr.y0 + LABEL_BAND);
        sp::ruler_labels(c, st, &self.axis, labels, &self.bars);
        self.paint_meter_tags(c);
        let band = Rect::new(rr.x0, rr.y0 + LABEL_BAND, rr.x1, rr.y1);
        sp::rect(c, band, mix(st.ruler, st.background, 0.5));
        let (a, z, on) = match &self.drag {
            Drag::Loop {
                t0,
                t1,
                moved: true,
                modifiers,
                ..
            } => {
                let (s, len) = seq_edit::draw_span(*t0, *t1, self.step(*modifiers), MIN_TICKS);
                (s, s + len, true)
            }
            _ => (
                self.song.loop_range.0,
                self.song.loop_range.1,
                self.song.loop_enabled,
            ),
        };
        let lr = Rect::new(
            self.axis.tick_to_x(a),
            band.y0 + 3.0,
            self.axis.tick_to_x(z),
            band.y1 - 3.0,
        );
        let col = if on { st.loop_on } else { st.loop_off };
        c.fill_with(
            RoundedRect::from_rect(lr, 2.0),
            col.with_alpha(if on { 0.75 } else { 0.4 }),
        );
        for x in [lr.x0 + 1.5, lr.x1 - 1.5] {
            sp::line(c, x, lr.y0, x, lr.y1, col);
        }
        sp::line(c, rr.x0, band.y0 + 0.5, rr.x1, band.y0 + 0.5, st.separator);
        let x = self.axis.tick_to_x(self.playhead);
        let pc = if self.recording {
            st.record
        } else {
            st.playhead
        };
        let mut flag = BezPath::new();
        flag.move_to(pt(x - 5.0, rr.y1 - 7.0));
        flag.line_to(pt(x + 5.0, rr.y1 - 7.0));
        flag.line_to(pt(x, rr.y1));
        flag.close_path();
        c.fill_with(flag, pc);
        c.pop_clip();
    }

    fn paint_track_list(&self, c: &mut dyn Canvas, doc: &RackDoc) {
        let st = &self.st;
        let t = &st.base;
        let list = Rect::new(
            self.bounds.x0,
            self.lanes_top(),
            self.axis.x0,
            self.bounds.y1.max(self.lanes_top()),
        );
        c.push_clip(&list.into());
        sp::rect(c, list, st.header);
        for (row, rr) in self.visible_rows() {
            let hr = Rect::new(list.x0, rr.y0, list.x1, rr.y1);
            match &row.kind {
                RowKind::AddAudio => {
                    let r = hr.inset(-4.0);
                    c.stroke(
                        &RoundedRect::from_rect(r, 3.0).into(),
                        &Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]),
                        &t.text_muted.with_alpha(0.5).into(),
                    );
                    let col = if self.hover == Some(Hit::AddAudio) {
                        t.text
                    } else {
                        t.text_muted
                    };
                    sp::label(
                        c,
                        "+  Audio track",
                        r.x0 + 10.0,
                        r.center().y + 3.5,
                        9.5,
                        col,
                        Align::Start,
                    );
                }
                RowKind::AddTrack(d) => {
                    let r = hr.inset(-4.0);
                    c.stroke(
                        &RoundedRect::from_rect(r, 3.0).into(),
                        &Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]),
                        &t.text_muted.with_alpha(0.5).into(),
                    );
                    let name = sp::track_name(doc, d);
                    let hovered = self.hover == Some(Hit::AddTrack(d.clone()));
                    let col = if hovered { t.text } else { t.text_muted };
                    let text = fit_label(&format!("+  Track for {name}"), r.width() - 16.0, 9.5);
                    sp::label(
                        c,
                        &text,
                        r.x0 + 10.0,
                        r.center().y + 3.5,
                        9.5,
                        col,
                        Align::Start,
                    );
                }
                RowKind::Lane { track, lane } => {
                    let tr = &self.song.tracks[*track];
                    let accent = sp::device_accent(t, doc, &tr.device);
                    if *lane == 0 {
                        self.paint_track_header(c, doc, hr, *track, accent);
                    } else {
                        sp::rect(c, hr, st.header_lane);
                        sp::rect(
                            c,
                            Rect::new(hr.x0 + 16.0, hr.y0 + 6.0, hr.x0 + 18.0, hr.y1 - 6.0),
                            accent.with_alpha(0.7),
                        );
                        let p = self.param_of(*track, *lane);
                        let name = fit_label(p.name, TRACK_LIST_WIDTH - 40.0, 10.0);
                        sp::label(
                            c,
                            &name,
                            hr.x0 + 26.0,
                            hr.y0 + 21.0,
                            10.0,
                            t.text,
                            Align::Start,
                        );
                        let range = format!(
                            "{} – {}",
                            crate::widget::format_value(&p, p.min),
                            crate::widget::format_value(&p, p.max)
                        );
                        sp::label(
                            c,
                            &fit_label(&range, TRACK_LIST_WIDTH - 40.0, 8.0),
                            hr.x0 + 26.0,
                            hr.y0 + 35.0,
                            8.0,
                            t.text_muted,
                            Align::Start,
                        );
                    }
                }
            }
            sp::line(c, hr.x0, hr.y1 - 0.5, hr.x1, hr.y1 - 0.5, st.separator);
            if let RowKind::Lane { track, lane: 0 } = row.kind {
                let active =
                    matches!(self.drag, Drag::Height) && self.th.drag_track() == Some(track);
                if active || self.hover == Some(Hit::TrackEdge(track)) {
                    sp::line(c, hr.x0, hr.y1 - 1.5, hr.x1, hr.y1 - 1.5, st.selection);
                }
            }
        }
        c.pop_clip();
    }

    fn paint_track_header(
        &self,
        c: &mut dyn Canvas,
        doc: &RackDoc,
        hr: Rect,
        track: usize,
        accent: Color,
    ) {
        let st = &self.st;
        let t = &st.base;
        let tr = &self.song.tracks[track];
        sp::rect(c, hr, st.header_track);
        sp::line(c, hr.x0, hr.y0 + 0.5, hr.x1, hr.y0 + 0.5, t.bevel_light);
        sp::rect(c, Rect::new(hr.x0, hr.y0, hr.x0 + 4.0, hr.y1), accent);
        // Expand chevron (dim when the track has no automation lanes).
        let er = self.button_rect(hr, TrackButton::Expand);
        let expanded = !self.collapsed.contains(&tr.device);
        let has_lanes = tr.lanes.len() > 1;
        let col = if has_lanes {
            t.text
        } else {
            t.text_muted.with_alpha(0.5)
        };
        let ctr = er.center();
        let mut chev = BezPath::new();
        if expanded {
            chev.move_to(pt(ctr.x - 4.0, ctr.y - 2.0));
            chev.line_to(pt(ctr.x + 4.0, ctr.y - 2.0));
            chev.line_to(pt(ctr.x, ctr.y + 3.0));
        } else {
            chev.move_to(pt(ctr.x - 2.0, ctr.y - 4.0));
            chev.line_to(pt(ctr.x + 3.0, ctr.y));
            chev.line_to(pt(ctr.x - 2.0, ctr.y + 4.0));
        }
        chev.close_path();
        c.fill_with(chev, col);
        let compact = hr.height() < COMPACT_BELOW;
        let name = fit_label(
            &sp::track_name(doc, &tr.device),
            if compact {
                76.0
            } else {
                TRACK_LIST_WIDTH - 44.0
            },
            11.5,
        );
        let name_y = if compact {
            hr.y0 + hr.height() / 2.0 + 4.0
        } else {
            hr.y0 + 21.0
        };
        c.text(&TextRun::new(name, pt(hr.x0 + 30.0, name_y), 11.5, t.text).weight(600));
        let hovered = |b| self.hover == Some(Hit::Button(track, b));
        for (b, label, on, lit) in [
            (TrackButton::Mute, "M", tr.muted, st.mute),
            (TrackButton::Solo, "S", tr.solo, st.solo),
            (TrackButton::Arm, "R", tr.armed, st.record),
        ] {
            sp::button(
                c,
                st,
                self.button_rect(hr, b),
                label,
                on.then_some(lit),
                hovered(b),
            );
        }
        if compact {
            return;
        }
        if tr.is_audio() {
            let header = self.audio_header(&tr.device);
            let input = header.map_or(InputSel::Off, |h| h.input);
            let ir = self.button_rect(hr, TrackButton::Input);
            sp::lcd(c, st, ir, hovered(TrackButton::Input).then_some(t.focus));
            let label = fit_label(&audio::input_label(input), ir.width() - 8.0, 9.0);
            sp::label(
                c,
                &label,
                ir.center().x,
                ir.center().y + 3.5,
                9.0,
                t.highlight,
                Align::Center,
            );
            let mode = header.map_or(MONITOR_AUTO, |h| h.monitor);
            let (text, lit) = match mode {
                0 => ("OFF", None),
                MONITOR_AUTO => ("AUTO", None),
                _ => ("ON", Some(st.record)),
            };
            let mr = self.button_rect(hr, TrackButton::Monitor);
            sp::button(c, st, mr, "", lit, hovered(TrackButton::Monitor));
            sp::label(
                c,
                text,
                mr.center().x,
                mr.center().y + 3.0,
                7.5,
                if lit.is_some() { t.text } else { t.text_muted },
                Align::Center,
            );
        } else {
            let ar = self.button_rect(hr, TrackButton::AddLane);
            sp::button(c, st, ar, "", None, hovered(TrackButton::AddLane));
            sp::label(
                c,
                "+ AUTO",
                ar.center().x,
                ar.center().y + 3.0,
                8.0,
                t.text_muted,
                Align::Center,
            );
        }
    }

    fn paint_corner(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let b = self.bounds;
        let r = Rect::new(b.x0, b.y0, self.axis.x0, self.ruler_bottom());
        sp::rect(c, r, st.header);
        c.text(
            &TextRun::new("SNAP", pt(r.x0 + 12.0, r.y0 + 21.0), 8.0, t.text_muted)
                .weight(700)
                .spacing(1.0),
        );
        let sr = self.snap_rect();
        sp::lcd(
            c,
            st,
            sr,
            (self.hover == Some(Hit::Snap)).then_some(t.focus),
        );
        sp::label(
            c,
            self.snap.label(),
            sr.center().x,
            sr.center().y + 3.5,
            9.5,
            t.highlight,
            Align::Center,
        );
        let shown = self.vp.shown_tool();
        for tool in Tool::ALL {
            let tr = self.tool_rect(tool);
            let active = tool == shown;
            sp::lcd(
                c,
                st,
                tr,
                (self.hover == Some(Hit::Tool(tool)) || active).then_some(if active {
                    t.highlight
                } else {
                    t.focus
                }),
            );
            sp::label(
                c,
                tool.label(),
                tr.center().x,
                tr.center().y + 3.0,
                7.5,
                if active { t.highlight } else { t.text_muted },
                Align::Center,
            );
        }
    }
}

/// The snap a `sequencer.set_snap` argument names.
pub(crate) fn snap_from_arg(arg: &str) -> Option<Snap> {
    Some(match arg {
        "off" => Snap::Off,
        "bar" => Snap::Bar,
        "beat" => Snap::Beat,
        "eighth" => Snap::Eighth,
        "sixteenth" => Snap::Sixteenth,
        "thirtysecond" => Snap::ThirtySecond,
        "sixtyfourth" => Snap::SixtyFourth,
        "quarter_triplet" => Snap::QuarterTriplet,
        "eighth_triplet" => Snap::EighthTriplet,
        "sixteenth_triplet" => Snap::SixteenthTriplet,
        "thirtysecond_triplet" => Snap::ThirtySecondTriplet,
        "grid" => Snap::Grid,
        _ => return None,
    })
}

/// `AddClip` of an empty clip (notes, or automation in an automation lane).
fn new_clip_cmd(
    device: DeviceKey,
    lane: usize,
    automation: bool,
    start: i64,
    length: i64,
) -> UiAction {
    let content = if automation {
        ClipContent::Automation(Vec::new())
    } else {
        ClipContent::Notes(Vec::new())
    };
    song_cmd(SongCommand::AddClip {
        device,
        lane,
        clip: Clip {
            id: ClipId(0),
            start,
            length,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            grid: None,
            content,
        },
    })
}

fn paint_menu(c: &mut dyn Canvas, st: &SeqTheme, m: &Menu) {
    let t = &st.base;
    let r = m.rect();
    c.fill_with(
        RoundedRect::from_rect(r.inset(2.0) + vello::kurbo::Vec2::new(0.0, 3.0), 5.0),
        t.cable_shadow,
    );
    c.fill_with(RoundedRect::from_rect(r, 4.0), t.browser_bg);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 4.5),
        1.0,
        t.bevel_dark,
    );
    for i in m.scroll..m.scroll + m.visible() {
        let Some(rr) = m.row_rect(i) else { continue };
        if m.current == Some(i) {
            c.fill_with(
                RoundedRect::from_rect(rr, 3.0),
                st.selection.with_alpha(0.22),
            );
        }
        if m.hover == Some(i) {
            c.fill_with(RoundedRect::from_rect(rr, 3.0), t.browser_row);
        }
        let text = fit_label(&m.items[i].1, MENU_W - 24.0, 10.0);
        sp::label(
            c,
            &text,
            rr.x0 + 8.0,
            rr.y0 + 14.0,
            10.0,
            t.text,
            Align::Start,
        );
    }
    if m.items.len() > MENU_ROWS {
        let frac = MENU_ROWS as f64 / m.items.len() as f64;
        let h = (r.height() - 8.0) * frac;
        let max = (m.items.len() - MENU_ROWS) as f64;
        let y = r.y0 + 4.0 + (r.height() - 8.0 - h) * (m.scroll as f64 / max);
        c.fill_with(
            RoundedRect::new(r.x1 - 6.0, y, r.x1 - 3.0, y + h, 1.5),
            t.text_muted.with_alpha(0.6),
        );
    }
}

/// The y range `(top, bottom)` for values inside an automation clip rectangle.
fn value_range(r: Rect) -> (f64, f64) {
    let (top, bottom) = (r.y0 + CLIP_HEADER + 4.0, r.y1 - 4.0);
    if bottom - top >= 4.0 {
        (top, bottom)
    } else {
        // A clip too short for the margins: a small range in the middle of the body.
        let mid = (r.y0 + CLIP_HEADER + r.y1) / 2.0;
        (mid - 2.0, mid + 2.0)
    }
}

/// Whether a header control has room in the header row `row` (a short row drops the second
/// line of controls: the automation, input and monitor buttons).
fn button_shown(row: Rect, button: TrackButton) -> bool {
    row.height() >= COMPACT_BELOW
        || matches!(
            button,
            TrackButton::Expand | TrackButton::Mute | TrackButton::Solo | TrackButton::Arm
        )
}

/// The polyline of an automation window: content ticks `[c0, c0 + len)`, mapped with `x`
/// and `y`. `None` without points.
fn automation_path(
    points: &[AutomationPoint],
    c0: i64,
    len: i64,
    x: impl Fn(i64) -> f64,
    y: &impl Fn(f32) -> f64,
) -> Option<BezPath> {
    let v0 = seq_edit::automation_value_at(points, c0)?;
    let mut path = BezPath::new();
    path.move_to(pt(x(c0), y(v0)));
    let end = c0 + len;
    let mut prev: Option<AutomationPoint> = points.iter().rev().find(|p| p.tick <= c0).copied();
    for p in points.iter().filter(|p| p.tick > c0 && p.tick < end) {
        if prev.is_some_and(|q| q.curve == Curve::Step) {
            path.line_to(pt(x(p.tick), y(prev.map_or(v0, |q| q.value))));
        }
        path.line_to(pt(x(p.tick), y(p.value)));
        prev = Some(*p);
    }
    let ve = if prev.is_some_and(|q| q.curve == Curve::Step) {
        prev.map_or(v0, |q| q.value)
    } else {
        seq_edit::automation_value_at(points, end)?
    };
    path.line_to(pt(x(end), y(ve)));
    Some(path)
}

fn find_clip(song: &Song, id: ClipId) -> Option<(usize, usize, &Clip)> {
    for (ti, t) in song.tracks.iter().enumerate() {
        for (li, l) in t.lanes.iter().enumerate() {
            if let Some(c) = l.clips.iter().find(|c| c.id == id) {
                return Some((ti, li, c));
            }
        }
    }
    None
}

fn workflow_cmd(c: WorkflowCommand) -> UiAction {
    UiAction::Command(Command::Workflow(c))
}

/// The mute button of a clip header at the right end of the visible strip, `None` when the
/// clip is narrower than three buttons (it would cover the label and the edge handle).
fn mute_button_rect(r: Rect, tl: Rect, audio: bool) -> Option<Rect> {
    // Audio clips keep the top-right corner for the fade-out handle.
    let right = r.x1.min(tl.x1) - if audio { AUDIO_HANDLE_INSET } else { 0.0 };
    if r.width() < 3.0 * MUTE_BUTTON || right - r.x0.max(tl.x0) < 3.0 * MUTE_BUTTON {
        return None;
    }
    Some(Rect::new(
        right - 2.0 - MUTE_BUTTON,
        r.y0 + 1.0,
        right - 2.0,
        r.y0 + 1.0 + MUTE_BUTTON,
    ))
}

fn mute_cmd(ids: Vec<ClipId>, muted: bool) -> UiAction {
    workflow_cmd(WorkflowCommand::SetClipsMuted { ids, muted })
}

fn song_cmd(c: SongCommand) -> UiAction {
    UiAction::Command(Command::Song(c))
}

fn clip_frames(tempo: &TempoMap, start: i64, length: i64, rate: u32) -> u64 {
    tempo
        .frame_at(start + length, rate)
        .saturating_sub(tempo.frame_at(start, rate))
}

fn audio_clip_cmd(
    c: &Clip,
    start: i64,
    length: i64,
    content: AudioClipContent,
    merge: u64,
) -> UiAction {
    song_cmd(SongCommand::SetAudioClip {
        id: c.id,
        start,
        length,
        muted: c.muted,
        content,
        merge: Some(merge),
    })
}

fn geometry_cmd(
    c: &Clip,
    start: i64,
    length: i64,
    loop_length: Option<i64>,
    merge: u64,
) -> UiAction {
    if let ClipContent::Audio(content) = &c.content {
        return audio_clip_cmd(c, start, length, content.clone(), merge);
    }
    song_cmd(SongCommand::SetClipGeometry {
        id: c.id,
        start,
        length,
        offset: c.offset,
        loop_length,
        muted: c.muted,
        merge: Some(merge),
    })
}

fn content_cmd(id: ClipId, points: Vec<AutomationPoint>, merge: Option<u64>) -> UiAction {
    song_cmd(SongCommand::SetClipContent {
        id,
        content: ClipContent::Automation(points),
        merge,
    })
}

/// Pixel geometry of an audio clip's handles and fade ends.
#[derive(Debug, Clone, Copy)]
struct AudioGeo {
    /// Frame rate of the clip's asset.
    rate: u32,
    /// Song frame of the clip start (fractional ticks are not rounded).
    start_frame: f64,
    /// x where the fade-in ends.
    fade_in_x: f64,
    /// x where the fade-out starts.
    fade_out_x: f64,
    /// y of the gain line.
    gain_y: f64,
    /// Fade-in handle.
    fade_in: Rect,
    /// Fade-out handle.
    fade_out: Rect,
    /// Gain handle (clips narrower than [`GAIN_MIN_WIDTH`] have none).
    gain: Option<Rect>,
}

/// The shaded area above a fade envelope and the envelope line, between `x_a` and `x_b`
/// (`top` = full gain, `bottom` = silence). `rising` for a fade-in, else a fade-out.
fn fade_paths(
    x_a: f64,
    x_b: f64,
    top: f64,
    bottom: f64,
    curve: FadeCurve,
    rising: bool,
) -> (BezPath, BezPath) {
    const STEPS: usize = 12;
    let gain = |t: f64| {
        let t = if rising { t } else { 1.0 - t };
        match curve {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => (t * std::f64::consts::FRAC_PI_2).sin(),
        }
    };
    let point = |i: usize| {
        let t = i as f64 / STEPS as f64;
        pt(x_a + t * (x_b - x_a), bottom - gain(t) * (bottom - top))
    };
    let mut line = BezPath::new();
    line.move_to(point(0));
    for i in 1..=STEPS {
        line.line_to(point(i));
    }
    let mut shade = BezPath::new();
    shade.move_to(pt(x_a, top));
    shade.line_to(pt(x_b, top));
    for i in (0..=STEPS).rev() {
        shade.line_to(point(i));
    }
    shade.close_path();
    (shade, line)
}
