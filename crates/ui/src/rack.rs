//! The rack view: the root of the UI. See the crate docs for the hosting contract.

use crate::action::{ContextTarget, PatchOp, SliceMode, UiAction};
use crate::autolayout::{self, PanelSpec};
use crate::breadcrumb::{self, CrumbHit};
use crate::browser::{BROWSER_WIDTH, Browser, CreateKind};
use crate::cables::{self, SocketEnd};
use crate::canvas::{Canvas, CanvasExt};
use crate::container_panel::{self, ContainerButton, ContainerInfo};
use crate::display_list::DisplayList;
use crate::drum_gen_panel::DrumButton;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::fx::FxKind;
use crate::interface::{boundary_in_descriptor, boundary_out_descriptor, interface_descriptor};
use crate::keymap::{CommandOutcome, Roles};
use crate::library_drop;
use crate::paint::{self, CableLook, PanelInfo, Ring};
use crate::plugin_data::{PluginDeviceStatus, PluginEntryInfo};
use crate::plugin_panel::{PluginButton, PluginInfo};
use crate::seq_panel::{self, PlayerInfo, SeqButton};
use crate::sidechain::{self, ScEntry, ScSource};
use crate::theme::Theme;
use crate::tree::ViewTree;
use crate::widget::{self, ValueDrag};
use atomik_audio_file::PeakPyramid;
use atomik_device_sdk::{
    ControlKind, DeviceDescriptor, Direction, PanelLayout, RACK_UNIT, RACK_WIDTH, Registry,
};
use atomik_model::song::AUDIO_TRACK_TYPE;
use atomik_model::{
    BOUNDARY_KEY, Cable, Command, ConsoleCommand, ContainerCommand, CreateOptions, CreateSource,
    CreateTarget, DeviceKey, INTERFACE_KEY, PatternDoc, RackDevice, RackDoc, RackSlot, SocketRef,
    WorkflowCommand,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use vello::kurbo::{Affine, Circle, ParamCurve, Point, Rect, Size, Vec2};
use vello::peniko::Color;

mod big_meter_view;
mod columns;
mod console_links;
mod container_input;
mod container_move;
mod cpu_readout;
mod device_drag;
mod drum_input;
mod fx_input;
mod help;
mod io_input;
mod jacks;
mod navigator;
pub use jacks::{JackChoice, JackGroup, JackMenu, JackOp, JackPeer};
pub use navigator::NAVIGATOR_WIDTH;
mod master_input;
mod name_input;
mod player_input;
mod plugin_input;
mod selection;

pub use crate::paint::Side as Flip;
pub use columns::COLUMN_GAP;
pub use console_links::OutputDisplay;
pub use cpu_readout::load_under;

/// Time constant of the fall of a CV activity indicator, seconds.
pub const CV_DECAY_SECONDS: f32 = 0.18;

/// The brightness step of a displayed CV level (0..=1).
fn step_of(level: f32) -> u8 {
    (level * f32::from(paint::CV_ACTIVITY_STEPS)).round() as u8
}

/// Smallest zoom.
pub const MIN_ZOOM: f64 = 0.5;
/// Largest zoom.
pub const MAX_ZOOM: f64 = 2.0;
/// Margin around the rack, logical units.
pub const RACK_MARGIN: f64 = 24.0;
/// Logical pixels scrolled per wheel notch.
pub const SCROLL_STEP: f64 = 48.0;
/// Zoom factor per wheel notch (with Ctrl / Cmd).
pub const ZOOM_STEP: f64 = 1.1;
/// Pointer travel before a plate press becomes a move.
const MOVE_THRESHOLD: f64 = 3.0;

/// Supplies hand-made panel layouts by type id (e.g. `atomik_devices::panel_layout`).
pub type LayoutProvider = fn(&str) -> Option<&'static PanelLayout>;

/// When cables are drawn (on the back view; the front never shows cables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableVisibility {
    /// Always.
    Show,
    /// Only cables attached to the device under the pointer.
    HoverOnly,
    /// Only cables attached to the selected devices.
    SelectedOnly,
    /// Every cable except the ones the creation rules make: the console's insert, send and
    /// return cables and every cable of a console device. A dot on each jack of a hidden cable
    /// shows that it is there.
    HideAutoRouted,
    /// Never (except the one being dragged).
    Hide,
}

impl CableVisibility {
    /// The mode after this one in the order K cycles through.
    pub fn next(self) -> Self {
        match self {
            CableVisibility::Show => CableVisibility::HoverOnly,
            CableVisibility::HoverOnly => CableVisibility::SelectedOnly,
            CableVisibility::SelectedOnly => CableVisibility::HideAutoRouted,
            CableVisibility::HideAutoRouted => CableVisibility::Hide,
            CableVisibility::Hide => CableVisibility::Show,
        }
    }
}

/// The highlight state of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketHighlight {
    /// Nothing special.
    Idle,
    /// The fixed end of the cable being dragged.
    Source,
    /// A valid drop target for the cable being dragged.
    Compatible,
    /// An invalid drop target (dimmed).
    Incompatible,
    /// The valid target under the pointer.
    Target,
    /// An end of the hovered cable.
    Hovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Rack,
    Device(usize),
    Plate(usize),
    /// The name on a device's plate: a double-click types a new name.
    Name(usize),
    Fold(usize),
    Control(usize, usize),
    Socket(usize, usize),
    /// The Edit button of a sampler or drum panel.
    Edit(usize),
    /// The "TO CH" button of a drum panel.
    ToChannel(usize),
    /// A pad of a drum panel (device index, pad 1..=16).
    Pad(usize, u8),
    /// The background of the pad grid of a drum panel.
    PadGrid(usize),
    /// The Slice button of a slicer panel.
    SliceBtn(usize),
    /// The "To track" button of a slicer panel.
    ToTrack(usize),
    /// The sensitivity knob of a slicer panel.
    Sens(usize),
    /// The overview strip of a slicer panel.
    Strip(usize),
    /// The plot of an EQ panel (handles are picked by position).
    EqPlot(usize),
    /// The "SC from" button of a compressor panel.
    ScButton(usize),
    /// The TUNER button of an audio track panel.
    TunerButton(usize),
    /// A button of the Character preset matrix (device index, preset 0..16).
    Preset(usize, u8),
    /// The whole Character preset matrix: the focus target of the arrow keys.
    PresetGrid(usize),
    /// A button of a step sequencer or arpeggiator panel.
    Player(usize, PlayerBtn),
    /// A button of a container panel.
    Container(usize, ContainerButton),
    /// The name of a macro on a container panel (device index, macro 0-based).
    MacroName(usize, u8),
    /// A button of a plugin panel.
    Plugin(usize, PluginButton),
    /// The BYPASS switch of a built-in effect's name strip.
    Bypass(usize),
    /// A part of the patch strip on the foot of a name plate.
    Patch(usize, PatchOp),
    /// A socket cell of the Audio I/O panel (device index, socket): lamp, meter, clip lamp.
    IoCell(usize, usize),
    /// The clip lamp of an Audio I/O socket (click resets it).
    IoClip(usize, usize),
    /// The name plate of the Audio I/O panel (click resets every clip lamp).
    IoHeader(usize),
    /// The EDIT button of a master return (device index, slot 0..8).
    ReturnEdit(usize, usize),
}

/// A button of a player panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerBtn {
    /// A button or the preview of a step sequencer panel.
    Seq(SeqButton),
    /// An arpeggiator mode button (`mode` param value).
    Mode(u8),
    /// An arpeggiator rate button (`rate` param value).
    Rate(u8),
    /// The octave stepper's minus button.
    OctaveDec,
    /// The octave stepper's plus button.
    OctaveInc,
    /// The FROM CLIP button of a Key & Chord panel.
    FromClip,
    /// A button, a lane mute or the cells of a drum generator panel.
    Drum(DrumButton),
}

impl Hit {
    fn device(self) -> Option<usize> {
        match self {
            Hit::Rack => None,
            Hit::Device(i)
            | Hit::Plate(i)
            | Hit::Name(i)
            | Hit::Fold(i)
            | Hit::Edit(i)
            | Hit::ToChannel(i)
            | Hit::Pad(i, _)
            | Hit::PadGrid(i)
            | Hit::SliceBtn(i)
            | Hit::ToTrack(i)
            | Hit::Sens(i)
            | Hit::Strip(i)
            | Hit::EqPlot(i)
            | Hit::ScButton(i)
            | Hit::TunerButton(i)
            | Hit::Preset(i, _)
            | Hit::PresetGrid(i)
            | Hit::Player(i, _)
            | Hit::Container(i, _)
            | Hit::MacroName(i, _)
            | Hit::Plugin(i, _)
            | Hit::Bypass(i)
            | Hit::Patch(i, _)
            | Hit::Control(i, _)
            | Hit::IoCell(i, _)
            | Hit::IoClip(i, _)
            | Hit::IoHeader(i)
            | Hit::ReturnEdit(i, _)
            | Hit::Socket(i, _) => Some(i),
        }
    }
}

#[derive(Debug)]
struct DeviceView {
    key: DeviceKey,
    name: String,
    caption: String,
    accent: Color,
    desc: Option<DeviceDescriptor>,
    /// Snapshot of the document device (`None` for the interface).
    device: Option<RackDevice>,
    spec: Arc<PanelSpec>,
    /// The device type takes samples (sample strip, double-click opens the editor, drops).
    samples: bool,
    /// Drum panel: the selected pad (1..=16); 1 for other devices.
    pad: u8,
    /// Drum panel: the short name of each pad (empty for a pad without zones).
    pad_names: Vec<String>,
    /// Drum panel: for each pad, whether its individual outputs are cabled.
    pad_cabled: [bool; 16],
    /// Slicer panel: the sensitivity of its Slice button, percent.
    sens: u8,
    /// Step sequencer panel: the edited pattern, 0-based (view state).
    edit: usize,
    /// Drum generator panel: the visible page of steps, 0-based (view state).
    page: usize,
    /// Drum generator panel: what the cells edit, 0 velocity, 1 flam, 2 ratchet (view state).
    fx_mode: u8,
    /// The effect kind with a visual (`atomik.reverb`, ...).
    fx: Option<FxKind>,
    /// Effect panels: display meter values (see [`fx::update_display`]); player panels: the
    /// published meter values, rounded.
    meters: Vec<f32>,
    /// Mod Source panel: the scope history fed from the published meter values.
    scope: crate::modsrc_panel::Scope,
    /// Granular panel: what the sample strip says about the loaded sample.
    sample: Option<crate::granular_panel::SampleInfo>,
    /// Spectrum and phase meter panels: what the app fed from the device's scope tap.
    analyser: crate::analyser::AnalyserData,
    /// Master panel: the effect on each return and the key-in state.
    master: crate::master_panel::MasterData,
    /// Compressor panel: the name of the device cabled to the sidechain input.
    sc: Option<String>,
    /// Character panel: whether the Transform and Dry/Wet CV inputs have a cable.
    cv: [bool; 2],
    /// Character panel: how often a feed changed the display (the screen animation phase).
    anim: u32,
    /// Params a macro of an enclosing container drives (key, macro 0-based): drawn locked.
    driven: Vec<(String, u8)>,
    /// Params with an automation lane (keys): their controls are marked.
    automated: Vec<String>,
    /// Container panel: the console already has a channel fed by this container.
    has_channel: bool,
    /// Plugin panel: what the app knows about the live instance.
    plugin_status: PluginDeviceStatus,
    /// Plugin panel: identity of the descriptor the panel was built from (0 for none), so a
    /// re-registered descriptor rebuilds the panel.
    desc_id: usize,
    /// The key sockets of this view are addressed by, when it is not the view key (the two
    /// boundary panels of a container's inside both use `boundary`).
    socket_device: Option<DeviceKey>,
    /// Left edge in rack coordinates (the left of its column).
    x: f64,
    y: f64,
    /// The column it sits in (always 0 inside a container).
    column: u16,
    height: f64,
    folded: bool,
    cache_id: u64,
    revision: u64,
    lists: [Option<DisplayList>; 2],
}

/// What decides the plugin panel of a device besides its type: the knobs it shows.
fn plugin_signature(d: &RackDevice) -> Option<Vec<u32>> {
    d.plugin.as_deref().map(crate::plugin_panel::featured_pins)
}

/// Identity of the descriptor a plugin device is drawn from (0 for other devices), so a
/// descriptor the app registered again after a rescan or a layout sync rebuilds the panel.
fn descriptor_id(registry: &Registry, d: &RackDevice) -> usize {
    if d.plugin.is_none() {
        return 0;
    }
    registry
        .descriptor(&d.type_id)
        .map_or(0, |x| std::ptr::from_ref(x) as usize)
}

impl DeviceView {
    /// The panel has the BYPASS switch of the name strip (see
    /// [`autolayout::has_strip_bypass`]).
    fn has_strip_bypass(&self) -> bool {
        self.device.is_some() && self.desc.as_ref().is_some_and(autolayout::has_strip_bypass)
    }
    /// The effect's `bypass` param is on.
    fn is_bypassed(&self) -> bool {
        self.device
            .as_ref()
            .and_then(|d| d.params.get(autolayout::BYPASS_KEY))
            .is_some_and(|v| *v >= 0.5)
    }
    fn rect(&self) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.x + f64::from(RACK_WIDTH),
            self.y + self.height,
        )
    }
    /// Where the panel's origin is in rack coordinates.
    fn offset(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
    fn is_interface(&self) -> bool {
        self.device.is_none()
    }
    /// The key of this view's sockets in cables.
    fn socket_key(&self) -> &DeviceKey {
        self.socket_device.as_ref().unwrap_or(&self.key)
    }
}

#[derive(Debug, Clone)]
struct SockGeo {
    end: SocketEnd,
    pos: Point,
    visible: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    Value {
        dev: usize,
        slot: usize,
        drag: ValueDrag,
        merge: u64,
        last_y: f64,
        start: f32,
        value: f32,
    },
    Cable {
        anchor: SocketEnd,
        pos: Point,
        detached: Option<Cable>,
    },
    Move {
        dev: usize,
        /// The press and the pointer, rack coordinates.
        start: Point,
        at: Point,
        /// A plain press on a member of a multi-selection: a release in place collapses the
        /// selection to this device.
        collapse: bool,
    },
    /// A device-browser row is dragged: `active` once the pointer travelled past the threshold
    /// (a click without travel only highlights the row). Positions are view coordinates.
    Browser {
        row: crate::browser::BrowserRow,
        start: Point,
        active: bool,
    },
    Pan {
        last: Point,
    },
    /// The sensitivity knob of a slicer panel (view state, no command): start y and value.
    Sens {
        dev: usize,
        start_y: f64,
        start: u8,
    },
    /// The navigator strip is dragged: the viewport follows the pointer, keeping `grab` (rack
    /// units from the viewport's top-left) under it.
    Navigator {
        grab: Vec2,
    },
    /// A drum pad is held: its audition note is on until release.
    Pad {
        device: DeviceKey,
        key: u8,
    },
    /// An EQ handle is dragged (see [`EqDrag`]).
    Eq(EqDrag),
    /// A cell of a drum generator grid is pressed (see [`drum_input::DrumDrag`]).
    Drum(drum_input::DrumDrag),
}

/// The axis an EQ handle drag is locked to (the model merges consecutive `SetParam`s of one
/// param only, so a drag moves one param to stay one undo step).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EqAxis {
    Freq,
    Gain,
}

/// State of an EQ handle drag.
#[derive(Debug, Clone)]
struct EqDrag {
    dev: usize,
    band: usize,
    merge: u64,
    /// Handle position minus pointer position at the press, rack coordinates.
    grab: Vec2,
    /// The press, rack coordinates.
    origin: Point,
    axis: Option<EqAxis>,
    start_freq: f32,
    start_gain: f32,
    freq: f32,
    gain: f32,
}

/// An open "SC from" list.
#[derive(Debug, Clone)]
struct ScMenu {
    dev: usize,
    entries: Vec<ScEntry>,
    current: Option<ScSource>,
    /// Menu rectangle in rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

/// Height of one row of the "SC from" list.
const SC_ROW_H: f64 = 18.0;
/// Most rows shown in the "SC from" list (Off plus sources).
const SC_MAX_ROWS: usize = 14;

/// The rack view. See the crate documentation for how an app hosts it.
#[derive(Debug)]
pub struct RackView {
    theme: Theme,
    layout_provider: Option<LayoutProvider>,
    devices: Vec<DeviceView>,
    specs: BTreeMap<String, Arc<PanelSpec>>,
    cache_ids: BTreeMap<DeviceKey, u64>,
    next_cache_id: u64,
    tree: ViewTree<Hit>,
    sockets: BTreeMap<SocketRef, SockGeo>,
    cables: Vec<Cable>,
    input_channels: u16,
    output_channels: u16,
    synced: bool,
    flip: Flip,
    cable_visibility: CableVisibility,
    size: Size,
    scale: f64,
    zoom: f64,
    scroll: Vec2,
    pointer: Point,
    hover: Option<Hit>,
    hover_cable: Option<Cable>,
    /// Displayed CV activity per cable, 0..=1 (decaying; see [`RackView::set_cv_activity`]).
    cv_activity: BTreeMap<Cable, f32>,
    drag: Drag,
    next_merge: u64,
    /// The primary selected device (local key): the last clicked.
    selected: Option<DeviceKey>,
    /// Every selected device (local keys), the primary included.
    selection: BTreeSet<DeviceKey>,
    /// Where a Shift-click range starts.
    anchor: Option<DeviceKey>,
    /// The role table of the click gestures (toggle and range selection).
    roles: Roles,
    browser: Browser,
    toast: Option<String>,
    /// The mapped controls and the learn state (see [`crate::MidiMarks`]).
    midi: crate::MidiMarks,
    /// The scale of the clip open in the piano roll, for the FROM CLIP button of a Key & Chord
    /// panel (`None`: no clip, or a clip without a scale; the button is disabled).
    clip_scale: Option<atomik_model::theory::Scale>,
    repaint: bool,
    content_height: f64,
    /// How many columns the view lays out (1 inside a container).
    columns: u16,
    /// The column a right click on the empty rack was in (the target of Remove Column).
    column_target: u16,
    /// The slot under the last right click on the empty rack, for a paste from its menu.
    context_slot: Option<RackSlot>,
    /// The clock of the last auto-scroll tick, while one is running.
    tick_clock: Option<f64>,
    /// The rack navigator is switched on (see [`NAVIGATOR_WIDTH`]).
    navigator: bool,
    /// The pointer is over the navigator strip.
    nav_hover: bool,
    /// Bumped by every rebuild of the device views: the navigator's block cache key.
    nav_layout: u64,
    /// The revision stamp of the navigator's cached block list.
    nav_revision: u64,
    nav_cache: Option<navigator::NavCache>,
    show_console: bool,
    /// Draw the channel-to-master console links as cables (default off).
    show_links: bool,
    /// The console links of the document in view (hidden unless `show_links`).
    links: BTreeSet<Cable>,
    /// The output display of each channel and of the master.
    outputs: BTreeMap<DeviceKey, OutputDisplay>,
    /// The CPU load shown on each plate (percent; see `set_cpu_loads`); empty = readout off.
    cpu: BTreeMap<DeviceKey, f32>,
    /// The Master Section while it is the first device: it cannot be moved or deleted.
    locked: Option<DeviceKey>,
    /// What the output displays were last computed from (the console and the device names).
    console_seen: Option<(Option<DeviceKey>, Vec<DeviceKey>)>,
    /// The sampler panel (device index) files are being dragged over.
    drop_hover: Option<usize>,
    /// The drum pad (device index, pad) files are being dragged over.
    drop_pad: Option<(usize, u8)>,
    /// A library item (a patch, a container or a device) is being dragged over the rack: the
    /// drop line follows the pointer.
    lib_drop: bool,
    /// Selected pad of each drum machine (view state, not in the document).
    pads: BTreeMap<DeviceKey, u8>,
    /// Sensitivity of each loop slicer panel (view state, not in the document).
    sens: BTreeMap<DeviceKey, u8>,
    /// The waveform peaks of the pool assets a granular panel draws (fed by the app, see
    /// [`RackView::set_sample_peaks`]).
    sample_peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// The open "SC from" list of a compressor panel.
    sc_menu: Option<ScMenu>,
    /// The open pick list of a long choice (the master's control-room source).
    choice_menu: Option<master_input::ChoiceMenu>,
    /// The Audio I/O panel's state: host channels, lamps, levels, clip flags.
    io: crate::io_panel::IoView,
    /// The Big Meter overlay (View > Big Meter).
    big: crate::big_meter::BigMeter,
    /// Edited pattern of each step sequencer (view state, not in the document).
    edits: BTreeMap<DeviceKey, usize>,
    /// The pattern clipboard of the sequencer panels (COPY, PASTE).
    pattern_clip: Option<PatternDoc>,
    /// The drum generator panels' pattern clipboard.
    drum_clip: Option<atomik_model::DrumPatternDoc>,
    /// The visible page of each drum generator panel (view state).
    pages: BTreeMap<DeviceKey, usize>,
    /// What the cells of each drum generator panel edit (survives rebuilds).
    fx_modes: BTreeMap<DeviceKey, u8>,
    /// The pattern name (or macro name) being edited on a panel.
    rename: Option<Rename>,
    /// The hover tooltips of the BYPASS and EDIT buttons show (the "Show tooltips" setting).
    button_tips: bool,
    /// Hovering a control shows its name and value, not only while it is automated or driven.
    value_tips: bool,
    /// How far a knob or fader has to be dragged to sweep its range.
    knob_range: widget::KnobRange,
    /// The device the patch browser is aimed at: its patch name is drawn in the browse colour.
    patch_browse: Option<DeviceKey>,
    /// The container whose inside is shown: the local keys from the top level down (empty =
    /// the top level). View state, not in the document.
    scope: Vec<DeviceKey>,
    /// The display names of the containers of `scope` (for the breadcrumb).
    scope_names: Vec<String>,
    /// A scope change decided by a gesture, applied after the actions of that event were lifted.
    pending_scope: Option<Vec<DeviceKey>>,
    /// Actions already carrying absolute (path) keys, appended after the lifted ones.
    direct: Vec<UiAction>,
    /// Macro params of the shown devices: view key to (param, macro, driven by this scope's
    /// container).
    driven: BTreeMap<DeviceKey, Vec<(String, u8, bool)>>,
    /// The params of the shown devices that have an automation lane (top level only): view key
    /// to param keys, recomputed from the document at every sync.
    automated: BTreeMap<DeviceKey, Vec<String>>,
    /// The values the controls show instead of the stored ones (the automation at the playhead,
    /// see [`RackView::set_shown_values`]); top level only.
    shown: atomik_model::ShownValues,
    /// The breadcrumb element under the pointer.
    crumb_hover: Option<CrumbHit>,
    /// Where the container of the scope sits among the devices of the rack (the place a device
    /// dragged out of it lands after); set on every sync inside a container.
    scope_rack_at: usize,
    /// The project, kept for the press that may start a drag out of an open container.
    host_doc: Option<RackDoc>,
    /// The containers the device being dragged may be dropped into (the model accepts the move).
    move_in_ok: Vec<DeviceKey>,
    /// The device being dragged may be dropped on the breadcrumb (the model accepts the move out).
    move_out_ok: bool,
    /// The open "map to macro" menu.
    macro_menu: Option<container_input::MacroMenu>,
    /// What the app knows about each plugin instance, by path key.
    plugin_status: BTreeMap<DeviceKey, PluginDeviceStatus>,
    /// The installed plugins (the browser group and the RELINK menu).
    plugin_catalog: Vec<PluginEntryInfo>,
    /// The open RELINK menu of a plugin panel.
    plugin_menu: Option<plugin_input::PluginMenu>,
    /// The stored names of the macros of the container in view (menu labels).
    scope_macros: [String; 8],
    /// Levels the scope was left because its container disappeared, not yet reported.
    popped: usize,
    /// First revision of a new panel view (bumped when the scope changes, so the scene cache
    /// never mixes panels of two scopes).
    epoch: u64,
    /// The Workflow setting "Auto-route new devices" (default on): creating routes the device,
    /// deleting bridges its audio path. Shift at the create action inverts it for one device.
    auto_route: bool,
}

/// What a [`Rename`] types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameKind {
    /// A pattern name of a player panel.
    Pattern,
    /// A macro name of a container panel.
    Macro,
    /// The name of the device itself, on its plate.
    Device,
}

/// A device name, a pattern name or a macro name being edited in place.
#[derive(Debug, Clone)]
struct Rename {
    dev: usize,
    index: u8,
    text: String,
    kind: RenameKind,
}

fn param_cmd(key: &DeviceKey, param: &str, value: f32, merge: Option<u64>) -> UiAction {
    UiAction::Command(Command::SetParam {
        key: key.clone(),
        param: param.to_string(),
        value,
        merge,
    })
}

impl RackView {
    /// Tells the view the scale of the clip open in the piano roll (`None` when no clip is open
    /// or it has no scale): it enables the FROM CLIP button of the Key & Chord panels.
    pub fn set_clip_scale(&mut self, scale: Option<atomik_model::theory::Scale>) {
        if self.clip_scale != scale {
            self.clip_scale = scale;
            for d in &mut self.devices {
                if d.spec.key_chord.is_some() {
                    d.revision += 1;
                    d.lists = [None, None];
                }
            }
            self.repaint = true;
        }
    }

    /// Gives the granular sampler panels the waveform peaks of pool asset `asset` (its content
    /// hash), replacing earlier ones; the panels that draw it are repainted. The app shares the
    /// pyramids it feeds the arrangement and the sample editor with.
    pub fn set_sample_peaks(&mut self, asset: &str, peaks: Arc<PeakPyramid>) {
        self.sample_peaks.insert(asset.to_string(), peaks);
        self.refresh_granular_panels(asset);
    }

    /// Forgets the peaks of `asset` (its audio was replaced or removed).
    pub fn clear_sample_peaks(&mut self, asset: &str) {
        if self.sample_peaks.remove(asset).is_some() {
            self.refresh_granular_panels(asset);
        }
    }

    /// Re-records the granular panels that have a zone on `asset`.
    fn refresh_granular_panels(&mut self, asset: &str) {
        for d in &mut self.devices {
            let uses = d.spec.granular.is_some()
                && d.device
                    .as_ref()
                    .is_some_and(|x| x.zones.iter().any(|z| z.asset == asset));
            if uses {
                d.revision += 1;
                d.lists = [None, None];
                self.repaint = true;
            }
        }
    }

    /// Tells the view which controls have a MIDI mapping (a dot on each; an outline on each
    /// while MIDI Learn is on, and in the warning colour on the one waiting for a hardware
    /// control).
    pub fn set_midi_marks(&mut self, marks: crate::MidiMarks) {
        if self.midi != marks {
            self.midi = marks;
            self.repaint = true;
        }
    }

    /// A view with no devices; call [`Self::sync`] with a document.
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            layout_provider: None,
            devices: Vec::new(),
            specs: BTreeMap::new(),
            sample_peaks: BTreeMap::new(),
            cache_ids: BTreeMap::new(),
            next_cache_id: 1,
            tree: ViewTree::new(Rect::ZERO, Hit::Rack),
            sockets: BTreeMap::new(),
            cables: Vec::new(),
            input_channels: 0,
            output_channels: 0,
            synced: false,
            flip: Flip::Front,
            cable_visibility: CableVisibility::Show,
            size: Size::new(800.0, 600.0),
            scale: 1.0,
            zoom: 1.0,
            scroll: Vec2::ZERO,
            pointer: Point::ZERO,
            hover: None,
            hover_cable: None,
            cv_activity: BTreeMap::new(),
            drag: Drag::None,
            next_merge: 1,
            selected: None,
            selection: BTreeSet::new(),
            anchor: None,
            roles: Roles::defaults(crate::keymap::Os::host()),
            browser: Browser {
                show_console: true,
                ..Browser::default()
            },
            toast: None,
            midi: crate::MidiMarks::default(),
            clip_scale: None,
            repaint: true,
            content_height: 0.0,
            columns: 1,
            column_target: 0,
            context_slot: None,
            tick_clock: None,
            navigator: false,
            nav_hover: false,
            nav_layout: 0,
            nav_revision: 0,
            nav_cache: None,
            show_console: true,
            show_links: false,
            links: BTreeSet::new(),
            outputs: BTreeMap::new(),
            cpu: BTreeMap::new(),
            locked: None,
            console_seen: None,
            drop_hover: None,
            drop_pad: None,
            lib_drop: false,
            pads: BTreeMap::new(),
            sens: BTreeMap::new(),
            sc_menu: None,
            choice_menu: None,
            io: crate::io_panel::IoView::default(),
            big: crate::big_meter::BigMeter::default(),
            edits: BTreeMap::new(),
            pattern_clip: None,
            drum_clip: None,
            pages: BTreeMap::new(),
            fx_modes: BTreeMap::new(),
            rename: None,
            button_tips: true,
            value_tips: false,
            knob_range: widget::KnobRange::default(),
            patch_browse: None,
            scope: Vec::new(),
            scope_names: Vec::new(),
            pending_scope: None,
            direct: Vec::new(),
            driven: BTreeMap::new(),
            automated: BTreeMap::new(),
            shown: Default::default(),
            crumb_hover: None,
            scope_rack_at: 0,
            host_doc: None,
            move_in_ok: Vec::new(),
            move_out_ok: false,
            macro_menu: None,
            plugin_status: BTreeMap::new(),
            plugin_catalog: Vec::new(),
            plugin_menu: None,
            scope_macros: Default::default(),
            popped: 0,
            epoch: 0,
            auto_route: true,
        }
    }

    /// Uses hand-made panel layouts where `provider` returns one (auto-layout otherwise).
    pub fn set_layout_provider(&mut self, provider: LayoutProvider) {
        self.layout_provider = Some(provider);
        self.specs.clear();
        self.synced = false;
    }

    /// The theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Replaces the theme (re-records every panel).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.synced = false;
        for d in &mut self.devices {
            d.revision += 1;
            d.lists = [None, None];
        }
        self.nav_cache = None;
        self.repaint = true;
    }

    /// Shows or hides the console's channel-strip and master devices. Since
    /// They are shown by default (the app persists the choice). Hidden devices keep
    /// their cables, but cables to them are not drawn. The device browser lists the console
    /// device types only while they are shown.
    pub fn set_show_console_devices(&mut self, show: bool) {
        if show != self.show_console {
            self.show_console = show;
            self.browser.show_console = show;
            self.synced = false;
            self.repaint = true;
        }
    }

    /// Whether console devices are shown in the rack.
    pub fn show_console_devices(&self) -> bool {
        self.show_console
    }

    /// Aims the patch browse colour at device `key` (or at none): its patch name is drawn in
    /// orange while the file browser applies patches to it.
    pub fn set_patch_browse(&mut self, key: Option<DeviceKey>) {
        if self.patch_browse == key {
            return;
        }
        let old = std::mem::replace(&mut self.patch_browse, key);
        for d in &mut self.devices {
            if Some(&d.key) == old.as_ref() || Some(&d.key) == self.patch_browse.as_ref() {
                d.revision += 1;
                d.lists = [None, None];
            }
        }
        self.repaint = true;
    }

    /// Shows or hides the hover tooltips of the buttons the rack explains by itself (BYPASS,
    /// a return's EDIT). The names of jacks and presets and the value readout of a drag stay.
    pub fn set_button_tips(&mut self, on: bool) {
        if on != self.button_tips {
            self.button_tips = on;
            self.repaint = true;
        }
    }

    /// Shows the name and value of every control under the pointer (default: only a control that
    /// is automated or driven by a macro says so).
    pub fn set_value_tips(&mut self, on: bool) {
        if on != self.value_tips {
            self.value_tips = on;
            self.repaint = true;
        }
    }

    /// Sets how far a knob or fader has to be dragged to sweep its range.
    pub fn set_knob_range(&mut self, range: widget::KnobRange) {
        self.knob_range = range;
    }

    /// Draws the console links (channel `out` and `send{k}` to the master) as cables, or hides
    /// them (the default): a channel then reads "Output: Master" instead.
    pub fn set_show_console_links(&mut self, show: bool) {
        if show != self.show_links {
            self.show_links = show;
            self.hover_cable = None;
            self.repaint = true;
        }
    }

    /// Whether the console links are drawn as cables.
    pub fn show_console_links(&self) -> bool {
        self.show_links
    }

    /// Whether a device is being dragged out of the device browser (Escape cancels it).
    pub fn is_browser_dragging(&self) -> bool {
        matches!(self.drag, Drag::Browser { .. })
    }

    /// The tooltip of the locked Master Section.
    pub const MASTER_LOCKED_TEXT: &'static str =
        "The Master Section is locked: it cannot be moved or deleted";

    /// Whether `key` is the Master Section while it is the first device of the rack: it cannot
    /// be moved, deleted or have a device placed above it.
    pub fn is_locked(&self, key: &DeviceKey) -> bool {
        self.locked.as_ref() == Some(&self.local_key(key))
    }

    /// The output display of a channel ("Output: Master", ...) or of the master (where `main`
    /// goes); `None` for any other device.
    pub fn output_display(&self, key: &DeviceKey) -> Option<&OutputDisplay> {
        self.outputs.get(&self.local_key(key))
    }

    /// The document devices shown in the rack, in rack order.
    fn shown<'a>(&self, doc: &'a RackDoc, registry: &Registry) -> Vec<&'a RackDevice> {
        doc.devices
            .iter()
            .filter(|d| self.show_console || !crate::console::is_console_device(doc, registry, d))
            .collect()
    }

    // ------------------------------------------------------------------ sync

    /// Updates the views from the document. Emits nothing. Cheap when nothing changed;
    /// only devices whose data changed are re-recorded. Inside a container (see
    /// [`Self::set_scope`]) the views show that container's inside; a scope whose container no
    /// longer exists is left for the nearest one that does.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let view = self.prepare(doc, registry);
        self.sync_in(&view, registry);
    }

    /// [`Self::sync`] on the document of the current scope (`doc` is the scope's view
    /// document, see [`Self::prepare`]).
    fn sync_in(&mut self, doc: &RackDoc, registry: &Registry) {
        let shown = self.shown(doc, registry);
        let trailing = usize::from(!self.scope.is_empty());
        let same_structure = self.synced
            && self.input_channels == doc.input_channels
            && self.output_channels == doc.output_channels
            && self.columns == if trailing == 1 { 1 } else { doc.columns.max(1) }
            && self.devices.len() == shown.len() + 1 + trailing
            && self.devices[1..=shown.len()]
                .iter()
                .zip(&shown)
                .all(|(v, d)| {
                    v.key == d.key
                        && (trailing == 1 || v.column == d.column.min(self.columns - 1))
                        && v.device
                            .as_ref()
                            .is_some_and(|s| s.type_id == d.type_id && s.folded == d.folded)
                        && v.device
                            .as_ref()
                            .is_some_and(|s| plugin_signature(s) == plugin_signature(d))
                        && v.desc_id == descriptor_id(registry, d)
                        // An audio track's panel opens and closes with its tuner.
                        && (v.fx != Some(FxKind::AudioTrack)
                            || v.spec.fx.is_some() == v.analyser.tuner_on)
                });
        if !same_structure {
            self.rebuild(doc, registry);
            self.repaint = true;
            return;
        }
        let mut renewed = false;
        for (i, d) in shown.iter().enumerate() {
            let d: &RackDevice = d;
            let driven = self.driven_for(&d.key);
            let automated = self.automated_for(&d.key);
            let has_channel = doc.console.channel_of(&d.key).is_some();
            let v = &mut self.devices[i + 1];
            if v.device.as_ref() != Some(d)
                || v.driven != driven
                || v.automated != automated
                || v.has_channel != has_channel
            {
                renewed |= v.device.as_ref().is_some_and(|old| old.name != d.name);
                v.driven = driven;
                v.automated = automated;
                v.has_channel = has_channel;
                v.name.clone_from(&d.name);
                v.accent = self.theme.accent_for(d);
                v.device = Some(d.clone());
                if v.spec.seq.is_some() || v.spec.drum_gen.is_some() {
                    let last = pattern_count(d) - 1;
                    if v.edit > last {
                        v.edit = last;
                        self.edits.insert(v.key.clone(), last);
                    }
                }
                v.revision += 1;
                if let Some(n) = self.tree.find(|h| *h == Hit::Device(i + 1)) {
                    self.tree.mark_dirty(n);
                }
                self.repaint = true;
            }
            // The strip's text also follows the pool (an asset registered or renamed).
            let v = &mut self.devices[i + 1];
            if v.spec.granular.is_some() {
                let sample = crate::granular_panel::sample_info(d, doc);
                if v.sample != sample {
                    v.sample = sample;
                    v.revision += 1;
                    self.repaint = true;
                }
            }
        }
        let cables_changed = self.cables != doc.cables;
        if cables_changed {
            self.cables.clone_from(&doc.cables);
            for v in &mut self.devices {
                if v.spec.drum.is_some() {
                    let now = pad_cabled_of(&v.key, &doc.cables);
                    if now != v.pad_cabled {
                        v.pad_cabled = now;
                        v.revision += 1;
                    }
                }
                if v.fx == Some(FxKind::Character) {
                    let now = cv_cabled_of(&v.key, &doc.cables);
                    if now != v.cv {
                        v.cv = now;
                        v.revision += 1;
                    }
                }
            }
            if let Some(h) = &self.hover_cable
                && !self.cables.contains(h)
            {
                self.hover_cable = None;
            }
            self.repaint = true;
        }
        self.refresh_sidechain(doc);
        self.refresh_master(doc);
        self.refresh_io();
        self.refresh_console(doc, registry, cables_changed || renewed);
    }

    /// Re-reads the master panel data (return effect names, key-in state) from `doc`.
    fn refresh_master(&mut self, doc: &RackDoc) {
        let data = crate::master_panel::master_data(doc);
        for v in &mut self.devices {
            if v.device
                .as_ref()
                .is_none_or(|d| d.type_id != crate::master_panel::MASTER_TYPE)
            {
                continue;
            }
            if v.master != data {
                v.master.clone_from(&data);
                v.revision += 1;
                self.repaint = true;
            }
        }
    }

    /// Re-reads the console links, the output displays and the master lock from `doc` (when
    /// `force`, its cables or a device name changed, or the console's members changed).
    fn refresh_console(&mut self, doc: &RackDoc, registry: &Registry, force: bool) {
        let seen = (
            doc.console.master.clone(),
            doc.console
                .channels
                .iter()
                .map(|c| c.channel.clone())
                .collect::<Vec<_>>(),
        );
        if !force && self.console_seen.as_ref() == Some(&seen) {
            return;
        }
        self.console_seen = Some(seen);
        let links = console_links::links_of(doc);
        let outputs = console_links::displays_of(doc, registry);
        let locked = doc
            .console
            .master
            .clone()
            .filter(|m| doc.devices.first().is_some_and(|d| &d.key == m));
        if links != self.links || outputs != self.outputs || locked != self.locked {
            self.repaint = true;
        }
        self.links = links;
        self.outputs = outputs;
        self.locked = locked;
    }

    /// Re-reads which device keys each compressor panel (cables or names changed).
    fn refresh_sidechain(&mut self, doc: &RackDoc) {
        for v in &mut self.devices {
            if v.fx != Some(FxKind::Compressor) {
                continue;
            }
            let now = sidechain::current_name(doc, &v.key);
            if now != v.sc {
                v.sc = now;
                v.revision += 1;
                self.repaint = true;
            }
        }
    }

    /// The panel of an audio track: with the tuner display when it is shown, shorter when not
    /// (both cached, the second under its own key).
    fn audio_track_spec(&mut self, desc: &DeviceDescriptor, tuner_on: bool) -> Arc<PanelSpec> {
        if tuner_on {
            return self.spec_for(desc.type_id, Some(desc), false);
        }
        let key = format!("{}#tuner-off", desc.type_id);
        Arc::clone(
            self.specs
                .entry(key)
                .or_insert_with(|| Arc::new(crate::fx_layout::audio_track_layout(desc, false))),
        )
    }

    fn spec_for(
        &mut self,
        type_id: &str,
        desc: Option<&DeviceDescriptor>,
        samples: bool,
    ) -> Arc<PanelSpec> {
        if let Some(s) = self.specs.get(type_id) {
            return Arc::clone(s);
        }
        let spec = match (desc, self.layout_provider.and_then(|f| f(type_id))) {
            (Some(d), _) if type_id == autolayout::DRUM_TYPE => autolayout::drum_layout(d),
            (Some(d), _) if type_id == autolayout::SLICER_TYPE => autolayout::slicer_layout(d),
            (Some(d), _) if type_id == seq_panel::SEQ_TYPE => seq_panel::seq_layout(d),
            (Some(d), _) if type_id == seq_panel::CV_SEQ_TYPE => seq_panel::cv_seq_layout(d),
            (Some(d), _) if type_id == crate::drum_gen_panel::DRUM_GEN_TYPE => {
                crate::drum_gen_panel::drum_gen_layout(d)
            }
            (Some(d), _) if type_id == container_panel::CONTAINER_TYPE => {
                container_panel::container_layout(d)
            }
            (Some(d), _) if type_id == crate::modsrc_panel::MODSRC_TYPE => {
                crate::modsrc_panel::modsrc_layout(d)
            }
            (Some(d), _) if type_id == crate::wavetable_panel::WAVETABLE_TYPE => {
                crate::wavetable_panel::wavetable_layout(d)
            }
            (Some(d), _) if type_id == crate::fm_panel::FM_TYPE => crate::fm_panel::fm_layout(d),
            (Some(d), _) if type_id == crate::granular_panel::GRANULAR_TYPE => {
                crate::granular_panel::granular_layout(d)
            }
            (Some(d), _) if type_id == crate::arp_panel::ARP_TYPE => {
                crate::arp_panel::arp_layout(d)
            }
            (Some(d), _) if type_id == crate::key_chord_panel::KEY_CHORD_TYPE => {
                crate::key_chord_panel::key_chord_layout(d)
            }
            (Some(d), _) if crate::fx::FxKind::of(type_id).is_some() => {
                crate::fx_layout::fx_layout(
                    crate::fx::FxKind::of(type_id).unwrap_or(crate::fx::FxKind::Reverb),
                    d,
                )
            }
            (Some(d), Some(layout)) => autolayout::from_panel_layout(layout, d),
            (Some(d), None) if samples => autolayout::auto_layout_with_samples(d),
            (Some(d), None) => autolayout::auto_layout(d),
            (None, _) => PanelSpec {
                height_units: 1,
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
                front: Vec::new(),
                front_sections: Vec::new(),
                back: Vec::new(),
                back_sections: Vec::new(),
            },
        };
        let spec = Arc::new(spec);
        // The interface spec depends on the channel count, so it is not cached by type.
        if desc.is_some_and(|d| !d.sockets.is_empty() || !d.params.is_empty())
            && type_id != crate::interface::INTERFACE_TYPE_ID
        {
            self.specs.insert(type_id.to_string(), Arc::clone(&spec));
        }
        spec
    }

    fn cache_id(&mut self, key: &DeviceKey) -> u64 {
        if let Some(id) = self.cache_ids.get(key) {
            return *id;
        }
        let id = self.next_cache_id;
        self.next_cache_id += 1;
        self.cache_ids.insert(key.clone(), id);
        id
    }

    fn rebuild(&mut self, doc: &RackDoc, registry: &Registry) {
        let mut old: BTreeMap<DeviceKey, DeviceView> =
            self.devices.drain(..).map(|d| (d.key.clone(), d)).collect();
        let unit = f64::from(RACK_UNIT);

        let scoped = !self.scope.is_empty();
        let (iface_desc, iface_key, iface_name, iface_caption) = if scoped {
            (
                boundary_in_descriptor(),
                DeviceKey(BOUNDARY_KEY.to_string()),
                "Container",
                "INPUT",
            )
        } else {
            (
                interface_descriptor(doc.input_channels, doc.output_channels),
                DeviceKey(INTERFACE_KEY.to_string()),
                "Audio I/O",
                "AUDIO I/O",
            )
        };
        let spec = self.spec_for(iface_desc.type_id, Some(&iface_desc), false);
        let cache_id = self.cache_id(&self.abs_key(&iface_key));
        let prev = old.remove(&iface_key);
        let same = prev
            .as_ref()
            .is_some_and(|p| p.desc.map(|d| d.sockets.len()) == Some(iface_desc.sockets.len()));
        let (revision, lists) = match prev {
            Some(p) if same => (p.revision, p.lists),
            Some(p) => (p.revision + 1, [None, None]),
            None => (self.epoch, [None, None]),
        };
        let height = spec.height();
        self.devices.push(DeviceView {
            key: iface_key,
            name: iface_name.into(),
            caption: iface_caption.into(),
            accent: self.theme.accent_for_type(iface_desc.type_id),
            desc: Some(iface_desc),
            device: None,
            spec,
            samples: false,
            pad: 1,
            pad_names: Vec::new(),
            pad_cabled: [false; 16],
            sens: autolayout::DEFAULT_SLICE_SENSITIVITY,
            edit: 0,
            page: 0,
            fx_mode: 0,
            fx: None,
            meters: Vec::new(),
            scope: crate::modsrc_panel::Scope::new(),
            sample: None,
            analyser: Default::default(),
            master: Default::default(),
            sc: None,
            cv: [false; 2],
            anim: 0,
            driven: Vec::new(),
            automated: Vec::new(),
            has_channel: false,
            plugin_status: PluginDeviceStatus::default(),
            desc_id: 0,
            socket_device: None,
            x: 0.0,
            y: 0.0,
            column: 0,
            height,
            folded: false,
            cache_id,
            revision,
            lists,
        });
        // Each column stacks from its own top; the Audio I/O strip heads column 0.
        let columns = if scoped { 1 } else { doc.columns.max(1) };
        let mut ys = vec![0.0; usize::from(columns)];
        ys[0] = height;

        for d in self.shown(doc, registry) {
            let column = if scoped { 0 } else { d.column.min(columns - 1) };
            let (x, y) = (columns::column_left(column), ys[usize::from(column)]);
            let desc = registry.get(&d.type_id).map(|f| *f.descriptor);
            let desc_id = descriptor_id(registry, d);
            let samples = registry.takes_samples(&d.type_id);
            let tuner_on = old.get(&d.key).is_some_and(|p| p.analyser.tuner_on);
            let spec = match (&d.plugin, &desc) {
                (Some(p), Some(dd)) => self.plugin_spec(&d.type_id, dd, p),
                (None, Some(dd)) if d.type_id == AUDIO_TRACK_TYPE => {
                    self.audio_track_spec(dd, tuner_on)
                }
                _ => self.spec_for(&d.type_id, desc.as_ref(), samples),
            };
            let plugin_status = self
                .plugin_status
                .get(&self.abs_key(&d.key))
                .cloned()
                .unwrap_or_default();
            let height = if d.folded { unit / 2.0 } else { spec.height() };
            let cache_id = self.cache_id(&self.abs_key(&d.key));
            let pad = self.pads.get(&d.key).copied().unwrap_or(1).clamp(1, 16);
            let pad_names = if spec.drum.is_some() {
                drum_pad_names(d, doc)
            } else if spec.drum_gen.is_some() {
                linked_pad_names(d, doc)
            } else {
                Vec::new()
            };
            let pad_cabled = if spec.drum.is_some() {
                pad_cabled_of(&d.key, &doc.cables)
            } else {
                [false; 16]
            };
            let sens = self
                .sens
                .get(&d.key)
                .copied()
                .unwrap_or(autolayout::DEFAULT_SLICE_SENSITIVITY);
            let edit = if spec.seq.is_some() || spec.drum_gen.is_some() {
                self.edits
                    .get(&d.key)
                    .copied()
                    .unwrap_or(0)
                    .min(pattern_count(d) - 1)
            } else {
                0
            };
            let page = if spec.drum_gen.is_some() {
                let wanted = self.pages.get(&d.key).copied().unwrap_or(0);
                let length = d
                    .drums
                    .as_ref()
                    .and_then(|x| x.patterns.get(edit))
                    .map_or(16, |p| p.length);
                crate::drum_gen_panel::clamp_page(wanted, length)
            } else {
                0
            };
            let fx_mode = self.fx_modes.get(&d.key).copied().unwrap_or(0);
            let fx = FxKind::of(&d.type_id);
            let sc = if fx == Some(FxKind::Compressor) {
                sidechain::current_name(doc, &d.key)
            } else {
                None
            };
            let driven = self.driven_for(&d.key);
            let automated = self.automated_for(&d.key);
            let has_channel = doc.console.channel_of(&d.key).is_some();
            let mut prev = old.remove(&d.key);
            let meters = prev.as_ref().map(|p| p.meters.clone()).unwrap_or_default();
            let anim = prev.as_ref().map_or(0, |p| p.anim);
            let scope = prev
                .as_mut()
                .map(|p| std::mem::take(&mut p.scope))
                .unwrap_or_default();
            let analyser = prev
                .as_mut()
                .map(|p| std::mem::take(&mut p.analyser))
                .unwrap_or_default();
            let cv = if fx == Some(FxKind::Character) {
                cv_cabled_of(&d.key, &doc.cables)
            } else {
                [false; 2]
            };
            let master = if d.type_id == crate::master_panel::MASTER_TYPE {
                crate::master_panel::master_data(doc)
            } else {
                Default::default()
            };
            let sample = if spec.granular.is_some() {
                crate::granular_panel::sample_info(d, doc)
            } else {
                None
            };
            let (revision, lists) = match prev {
                Some(p)
                    if p.device.as_ref() == Some(d)
                        && p.driven == driven
                        && p.automated == automated
                        && p.has_channel == has_channel
                        && p.pad == pad
                        && p.pad_names == pad_names
                        && p.pad_cabled == pad_cabled
                        && p.sens == sens
                        && p.edit == edit
                        && p.page == page
                        && p.plugin_status == plugin_status
                        && p.master == master
                        && p.cv == cv
                        && p.sample == sample
                        && p.sc == sc =>
                {
                    (p.revision, p.lists)
                }
                Some(p) => (p.revision + 1, [None, None]),
                None => (self.epoch, [None, None]),
            };
            self.devices.push(DeviceView {
                key: d.key.clone(),
                name: d.name.clone(),
                caption: desc.map_or_else(|| d.type_id.clone(), |x| x.name.to_uppercase()),
                accent: self.theme.accent_for(d),
                desc,
                device: Some(d.clone()),
                spec,
                samples,
                pad,
                pad_names,
                pad_cabled,
                sens,
                edit,
                page,
                fx_mode,
                fx,
                meters,
                scope,
                sample,
                analyser,
                master,
                sc,
                cv,
                anim,
                driven,
                automated,
                has_channel,
                plugin_status,
                desc_id,
                socket_device: None,
                x,
                y,
                column,
                height,
                folded: d.folded,
                cache_id,
                revision,
                lists,
            });
            ys[usize::from(column)] += height;
        }
        let y = ys[0];
        if scoped {
            let desc = boundary_out_descriptor();
            let key = DeviceKey(format!("{BOUNDARY_KEY}~out"));
            let spec = self.spec_for(desc.type_id, Some(&desc), false);
            let cache_id = self.cache_id(&self.abs_key(&key));
            let prev = old.remove(&key);
            let (revision, lists) = match prev {
                Some(p) => (p.revision, p.lists),
                None => (self.epoch, [None, None]),
            };
            let height = spec.height();
            self.devices.push(DeviceView {
                key,
                name: "Container".into(),
                caption: "OUTPUT".into(),
                accent: self.theme.accent_for_type(desc.type_id),
                desc: Some(desc),
                device: None,
                spec,
                samples: false,
                pad: 1,
                pad_names: Vec::new(),
                pad_cabled: [false; 16],
                sens: autolayout::DEFAULT_SLICE_SENSITIVITY,
                edit: 0,
                page: 0,
                fx_mode: 0,
                fx: None,
                meters: Vec::new(),
                scope: crate::modsrc_panel::Scope::new(),
                sample: None,
                analyser: Default::default(),
                master: Default::default(),
                sc: None,
                cv: [false; 2],
                anim: 0,
                driven: Vec::new(),
                automated: Vec::new(),
                has_channel: false,
                plugin_status: PluginDeviceStatus::default(),
                desc_id: 0,
                socket_device: Some(DeviceKey(BOUNDARY_KEY.to_string())),
                x: 0.0,
                y,
                column: 0,
                height,
                folded: false,
                cache_id,
                revision,
                lists,
            });
            ys[0] += height;
        }
        self.nav_layout += 1;
        self.columns = columns;
        self.content_height = ys.iter().copied().fold(0.0, f64::max);
        self.cables.clone_from(&doc.cables);
        self.refresh_console(doc, registry, true);
        self.input_channels = doc.input_channels;
        self.output_channels = doc.output_channels;
        self.refresh_io();
        self.synced = true;
        self.prune_selection();
        // Indices may have shifted: drop transient state tied to them.
        if !matches!(self.drag, Drag::Pad { .. }) {
            self.drag = Drag::None;
        }
        self.hover = None;
        self.drop_hover = None;
        self.drop_pad = None;
        self.lib_drop = false;
        self.rename = None;
        self.rebuild_tree();
        self.clamp_scroll();
    }

    fn rebuild_tree(&mut self) {
        let focus_key = self.focused_control();
        let mut tree = ViewTree::new(Rect::new(-1e7, -1e7, 1e7, 1e7), Hit::Rack);
        let mut sockets = BTreeMap::new();
        let root = tree.root();
        for (i, d) in self.devices.iter().enumerate() {
            let off = d.offset();
            let dn = tree.add(root, d.rect(), Hit::Device(i), false);
            if !d.is_interface() {
                let pn = tree.add(
                    dn,
                    paint::plate_rect(d.height, d.folded) + off,
                    Hit::Plate(i),
                    false,
                );
                if self.locked.as_ref() != Some(&d.key) {
                    let name = paint::name_rect(d.height, d.folded, d.has_strip_bypass());
                    tree.add(pn, name + off, Hit::Name(i), false);
                }
                let fc = paint::fold_button_center(d.height, d.folded) + off;
                tree.add(
                    pn,
                    Circle::new(fc, 8.0).bounding_box_rect(),
                    Hit::Fold(i),
                    false,
                );
                if d.has_strip_bypass() {
                    tree.add(
                        pn,
                        paint::bypass_button_rect(d.height, d.folded) + off,
                        Hit::Bypass(i),
                        false,
                    );
                }
                if self.flip == Flip::Front && paint::has_patch_strip(d.device.as_ref(), d.folded) {
                    let strip = paint::patch_strip(d.height);
                    for (rect, op) in [
                        (strip.prev, PatchOp::Previous),
                        (strip.name, PatchOp::Browse),
                        (strip.next, PatchOp::Next),
                        (strip.save, PatchOp::Save),
                    ] {
                        tree.add(pn, rect + off, Hit::Patch(i, op), false);
                    }
                }
                if d.fx == Some(FxKind::AudioTrack) && !d.folded && self.flip == Flip::Front {
                    tree.add(
                        pn,
                        paint::tuner_button_rect() + off,
                        Hit::TunerButton(i),
                        false,
                    );
                }
            }
            let Some(desc) = d.desc else { continue };
            if !d.folded {
                match self.flip {
                    Flip::Front => {
                        if let Some(strip) = d.spec.samples {
                            tree.add(
                                dn,
                                paint::sample_edit_button(strip) + off,
                                Hit::Edit(i),
                                false,
                            );
                        }
                        if let Some(dr) = d.spec.drum {
                            tree.add(dn, dr.pads + off, Hit::PadGrid(i), false);
                            for pad in 1..=16u8 {
                                let r = autolayout::drum_pad_rect(dr.pads, pad);
                                tree.add(dn, r + off, Hit::Pad(i, pad), false);
                            }
                            tree.add(dn, dr.edit + off, Hit::Edit(i), false);
                            tree.add(dn, dr.to_channel + off, Hit::ToChannel(i), false);
                        }
                        if let Some(sl) = d.spec.slicer {
                            tree.add(dn, sl.strip + off, Hit::Strip(i), false);
                            tree.add(dn, sl.sens + off, Hit::Sens(i), false);
                            tree.add(dn, sl.slice + off, Hit::SliceBtn(i), false);
                            tree.add(dn, sl.to_track + off, Hit::ToTrack(i), false);
                            tree.add(dn, sl.edit + off, Hit::Edit(i), false);
                        }
                        if let Some(cp) = d.spec.container {
                            for b in [
                                ContainerButton::Open,
                                ContainerButton::Edit,
                                ContainerButton::ToChannel,
                            ] {
                                tree.add(dn, cp.button(b) + off, Hit::Container(i, b), false);
                            }
                            for (n, r) in cp.names.iter().enumerate() {
                                tree.add(dn, *r + off, Hit::MacroName(i, n as u8), false);
                            }
                        }
                        for (r, hit) in Self::plugin_hits(d, i) {
                            tree.add(dn, r + off, hit, false);
                        }
                        if let Some(sp) = d.spec.seq {
                            tree.add(
                                dn,
                                sp.preview + off,
                                Hit::Player(i, PlayerBtn::Seq(SeqButton::Preview)),
                                false,
                            );
                            for (b, r) in sp.buttons() {
                                tree.add(dn, r + off, Hit::Player(i, PlayerBtn::Seq(b)), false);
                            }
                        }
                        if let Some(dp) = d.spec.drum_gen {
                            let grid = Hit::Player(i, PlayerBtn::Drum(DrumButton::Grid));
                            tree.add(dn, dp.grid + off, grid, false);
                            for (b, r) in dp.buttons() {
                                tree.add(dn, r + off, Hit::Player(i, PlayerBtn::Drum(b)), false);
                            }
                        }
                        if let Some(ap) = d.spec.arp {
                            for (m, r) in ap.modes.iter().enumerate() {
                                let hit = Hit::Player(i, PlayerBtn::Mode(m as u8));
                                tree.add(dn, *r + off, hit, false);
                            }
                            for (m, r) in ap.rates.iter().enumerate() {
                                let hit = Hit::Player(i, PlayerBtn::Rate(m as u8));
                                tree.add(dn, *r + off, hit, false);
                            }
                            let dec = Hit::Player(i, PlayerBtn::OctaveDec);
                            tree.add(dn, ap.octave_dec + off, dec, false);
                            let inc = Hit::Player(i, PlayerBtn::OctaveInc);
                            tree.add(dn, ap.octave_inc + off, inc, false);
                        }
                        if let Some(kc) = d.spec.key_chord {
                            let hit = Hit::Player(i, PlayerBtn::FromClip);
                            tree.add(dn, kc.from_clip + off, hit, false);
                        }
                        if let Some(fx) = d.spec.fx {
                            if fx.kind == FxKind::Eq {
                                tree.add(dn, fx.plot.inset(8.0) + off, Hit::EqPlot(i), false);
                            }
                            if let Some(b) = fx.button {
                                tree.add(dn, b + off, Hit::ScButton(i), false);
                            }
                            if let Some(g) = fx.character {
                                let grid = tree.add(dn, g.matrix + off, Hit::PresetGrid(i), true);
                                for n in 0..crate::fx::CHARACTER_PRESETS {
                                    let r = crate::character_panel::cell_rect(g.matrix, n);
                                    tree.add(grid, r + off, Hit::Preset(i, n as u8), false);
                                }
                            }
                        }
                        if desc.type_id == crate::master_panel::MASTER_TYPE && d.device.is_some() {
                            for (slot, r) in crate::master_panel::edit_rects(&d.spec, &desc) {
                                tree.add(dn, r + off, Hit::ReturnEdit(i, slot), false);
                            }
                        }
                        if desc.type_id == crate::interface::INTERFACE_TYPE_ID {
                            tree.add(
                                dn,
                                paint::plate_rect(d.height, false) + off,
                                Hit::IoHeader(i),
                                false,
                            );
                            for cell in crate::io_panel::cells(&desc) {
                                let cn = tree.add(
                                    dn,
                                    cell.rect + off,
                                    Hit::IoCell(i, cell.socket),
                                    false,
                                );
                                tree.add(cn, cell.clip + off, Hit::IoClip(i, cell.socket), false);
                            }
                        }
                        for (slot, pc) in d.spec.front.iter().enumerate() {
                            if !d.spec.on_back(pc) {
                                tree.add(dn, pc.rect + off, Hit::Control(i, slot), true);
                            }
                        }
                    }
                    Flip::Back => {
                        for (slot, pc) in d.spec.front.iter().enumerate() {
                            if d.spec.on_back(pc) {
                                tree.add(dn, pc.rect + off, Hit::Control(i, slot), true);
                            }
                        }
                        for (slot, ps) in d.spec.back.iter().enumerate() {
                            tree.add(dn, ps.rect + off, Hit::Socket(i, slot), false);
                        }
                    }
                }
            }
            for ps in &d.spec.back {
                let s = &desc.sockets[ps.socket];
                let pos = if d.folded {
                    Point::new(d.x + ps.center().x, d.y + d.height / 2.0)
                } else {
                    ps.center() + off
                };
                let at = SocketRef {
                    device: d.socket_key().clone(),
                    socket: s.key.to_string(),
                };
                sockets.insert(
                    at.clone(),
                    SockGeo {
                        end: SocketEnd {
                            at,
                            kind: s.kind,
                            direction: s.direction,
                        },
                        pos,
                        visible: !d.folded,
                    },
                );
            }
        }
        self.tree = tree;
        self.sockets = sockets;
        if let Some((key, param)) = focus_key {
            let target = self.devices.iter().enumerate().find_map(|(i, d)| {
                (d.key == key)
                    .then(|| {
                        let desc = d.desc?;
                        d.spec
                            .front
                            .iter()
                            .position(|pc| {
                                desc.params[d.spec.param_index(&desc, pc, d.pad)].key == param
                            })
                            .map(|slot| (i, slot))
                    })
                    .flatten()
            });
            let node = target.and_then(|(i, s)| self.tree.find(|h| *h == Hit::Control(i, s)));
            let node = node.or_else(|| {
                // The Character matrix is not a placed control: its focus is the param preset.
                (param == "preset")
                    .then(|| self.devices.iter().position(|d| d.key == key))
                    .flatten()
                    .and_then(|i| self.tree.find(|h| *h == Hit::PresetGrid(i)))
            });
            self.tree.set_focus(node);
        }
    }

    // ------------------------------------------------------------------ geometry

    fn rack_area(&self) -> Rect {
        let x0 = self.browser_edge();
        let x1 = self
            .navigator_rect()
            .map_or(self.size.width, |r| r.x0)
            .max(x0);
        Rect::new(x0, self.bar_height(), x1, self.size.height)
    }

    /// The right edge of the device browser (0 while it is closed).
    fn browser_edge(&self) -> f64 {
        if self.browser.visible {
            BROWSER_WIDTH
        } else {
            0.0
        }
    }

    /// Height of the breadcrumb bar: only inside a container.
    fn bar_height(&self) -> f64 {
        if self.scope.is_empty() {
            0.0
        } else {
            breadcrumb::BAR_HEIGHT
        }
    }

    /// Whether the columns alone are wider than the rack area: the rack then scrolls sideways
    /// from the left; otherwise it is centred.
    fn is_wide(&self) -> bool {
        self.content_width() * self.zoom + 2.0 * RACK_MARGIN > self.rack_area().width()
    }

    fn origin(&self) -> Point {
        let area = self.rack_area();
        let rack_w = self.content_width() * self.zoom;
        let avail = area.width() - 2.0 * RACK_MARGIN;
        let x = if self.is_wide() {
            area.x0 + RACK_MARGIN - self.scroll.x
        } else {
            let centred = area.x0 + RACK_MARGIN + (avail - rack_w) / 2.0;
            if self.zone_visible() {
                // The new-column zone of a drag stays in view: a rack that fits moves left only
                // as far as it takes, even if that cuts the margin on its left.
                centred.min(area.x1 - RACK_MARGIN - self.extent_width() * self.zoom)
            } else {
                centred
            }
        };
        Point::new(x, area.y0 + RACK_MARGIN - self.scroll.y)
    }

    fn max_scroll(&self) -> Vec2 {
        let area = self.rack_area();
        // A rack that fits is centred and does not scroll sideways.
        Vec2::new(
            if self.is_wide() {
                (self.extent_width() * self.zoom + 2.0 * RACK_MARGIN - area.width()).max(0.0)
            } else {
                0.0
            },
            (self.content_height * self.zoom + 2.0 * RACK_MARGIN - area.height()).max(0.0),
        )
    }

    fn clamp_scroll(&mut self) {
        let m = self.max_scroll();
        self.scroll = Vec2::new(self.scroll.x.clamp(0.0, m.x), self.scroll.y.clamp(0.0, m.y));
    }

    /// Converts a logical view position to rack coordinates.
    pub fn view_to_rack(&self, p: Point) -> Point {
        ((p - self.origin()) / self.zoom).to_point()
    }

    /// Converts rack coordinates to a logical view position.
    pub fn rack_to_view(&self, p: Point) -> Point {
        self.origin() + p.to_vec2() * self.zoom
    }

    /// Height of the tallest column (rack coordinates).
    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    /// Width of all the columns (rack coordinates): one rack width for a single column.
    pub fn content_width(&self) -> f64 {
        columns::columns_width(self.columns)
    }

    /// A device's rectangle in rack coordinates (`"interface"` is the top strip).
    pub fn device_rect(&self, key: &DeviceKey) -> Option<Rect> {
        self.device_index(key).map(|i| self.devices[i].rect())
    }

    /// The index of the view of a device. Inside a container `key` may be the local key or the
    /// path key.
    fn device_index(&self, key: &DeviceKey) -> Option<usize> {
        let key = self.local_key(key);
        self.devices.iter().position(|d| d.key == key)
    }

    /// Centre of a control (logical view coordinates), when it is on screen-side (front,
    /// unfolded).
    pub fn control_center(&self, key: &DeviceKey, param: &str) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        let desc = d.desc?;
        let pc = d
            .spec
            .front
            .iter()
            .find(|pc| desc.params[d.spec.param_index(&desc, pc, d.pad)].key == param)?;
        // A control of the back face (the Character CV trims) answers on the back only.
        if (self.flip == Flip::Back) != d.spec.on_back(pc) {
            return None;
        }
        let c = match pc.kind {
            ControlKind::Knob => Point::new(pc.rect.center().x, pc.rect.y0 + paint::KNOB_DY),
            _ => pc.rect.center(),
        };
        Some(self.rack_to_view(c + d.offset()))
    }

    /// Centre of the FROM CLIP button of a Key & Chord panel (logical view coordinates), when it
    /// is on screen-side (front, unfolded).
    pub fn from_clip_button_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        let b = d.spec.key_chord?.from_clip;
        Some(self.rack_to_view(b.center() + d.offset()))
    }

    /// Centre of the Edit button of a sampler panel (logical view coordinates), when it is on
    /// screen-side (front, unfolded).
    pub fn edit_button_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        let b = match (d.spec.samples, d.spec.drum) {
            _ if d.spec.slicer.is_some() => d.spec.slicer.map(|s| s.edit)?,
            (Some(strip), _) => paint::sample_edit_button(strip),
            (None, Some(drum)) => drum.edit,
            (None, None) => return None,
        };
        Some(self.rack_to_view(b.center() + d.offset()))
    }

    /// The rectangle `pick` selects of a slicer panel and the device's y (front, unfolded).
    fn slicer_rect(
        &self,
        key: &DeviceKey,
        pick: impl Fn(&autolayout::SlicerPanel) -> Rect,
    ) -> Option<(Rect, Vec2)> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        Some((pick(&d.spec.slicer?), d.offset()))
    }

    fn slicer_point(
        &self,
        key: &DeviceKey,
        pick: impl Fn(&autolayout::SlicerPanel) -> Rect,
    ) -> Option<Point> {
        let (rect, off) = self.slicer_rect(key, pick)?;
        Some(self.rack_to_view(rect.center() + off))
    }

    /// Centre of the Slice button of a slicer panel (logical view coordinates), when it is on
    /// screen-side (front, unfolded).
    pub fn slice_button_center(&self, key: &DeviceKey) -> Option<Point> {
        self.slicer_point(key, |s| s.slice)
    }

    /// Centre of the "To track" button of a slicer panel (logical view coordinates).
    pub fn to_track_button_center(&self, key: &DeviceKey) -> Option<Point> {
        self.slicer_point(key, |s| s.to_track)
    }

    /// Centre of the overview strip of a slicer panel (logical view coordinates): a
    /// double-click on it opens the slice editor.
    pub fn slice_strip_center(&self, key: &DeviceKey) -> Option<Point> {
        self.slicer_point(key, |s| s.strip)
    }

    /// Centre of the sensitivity knob of a slicer panel (logical view coordinates).
    pub fn sensitivity_knob_center(&self, key: &DeviceKey) -> Option<Point> {
        let (rect, off) = self.slicer_rect(key, |s| s.sens)?;
        let c = autolayout::slicer_sens_knob(rect);
        Some(self.rack_to_view(c + off))
    }

    /// The slicing sensitivity (percent) of a loop slicer panel, `None` for other devices. The
    /// panel's Slice button emits it; it is view state, not part of the document.
    pub fn slice_sensitivity(&self, key: &DeviceKey) -> Option<u8> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.slicer.is_some().then_some(d.sens)
    }

    /// Sets the sensitivity of a loop slicer panel (percent, clamped; no action emitted).
    pub fn set_slice_sensitivity(&mut self, key: &DeviceKey, percent: u8) {
        if let Some(i) = self.device_index(key)
            && self.devices[i].spec.slicer.is_some()
        {
            self.store_sens(i, percent.min(100));
        }
    }

    fn store_sens(&mut self, i: usize, percent: u8) {
        let d = &mut self.devices[i];
        if d.sens != percent {
            d.sens = percent;
            d.revision += 1;
            d.lists = [None, None];
            self.sens.insert(d.key.clone(), percent);
            self.repaint = true;
        }
    }

    /// Centre of the "TO CH" button of a drum panel (logical view coordinates), when it is on
    /// screen-side (front, unfolded).
    pub fn to_channel_button_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        if d.folded {
            return None;
        }
        let b = d.spec.drum?.to_channel;
        Some(self.rack_to_view(b.center() + d.offset()))
    }

    /// The selected pad (1..=16) of a drum machine panel; `None` for other devices.
    pub fn selected_pad(&self, key: &DeviceKey) -> Option<u8> {
        let d = &self.devices[self.device_index(key)?];
        d.spec.drum.is_some().then_some(d.pad)
    }

    /// Selects a pad of a drum machine panel (no action emitted; also done by a click on it).
    pub fn set_selected_pad(&mut self, key: &DeviceKey, pad: u8) {
        if let Some(i) = self.device_index(key)
            && self.devices[i].spec.drum.is_some()
        {
            self.select_pad(i, pad.clamp(1, 16));
        }
    }

    /// Centre of a pad of a drum panel (logical view coordinates), front side, unfolded.
    pub fn pad_center(&self, key: &DeviceKey, pad: u8) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if self.flip != Flip::Front || d.folded {
            return None;
        }
        let r = autolayout::drum_pad_rect(d.spec.drum?.pads, pad);
        Some(self.rack_to_view(r.center() + d.offset()))
    }

    /// A point of the pad grid between two pads (logical view coordinates): the background that
    /// a double-click opens the step editor from.
    pub fn pad_grid_background(&self, key: &DeviceKey) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        if self.flip != Flip::Front || d.folded {
            return None;
        }
        let g = d.spec.drum?.pads;
        let p = Point::new(
            g.x0 + autolayout::DRUM_PAD_CELL.0,
            g.y1 - autolayout::DRUM_PAD_CELL.1 / 2.0,
        );
        Some(self.rack_to_view(p + d.offset()))
    }

    /// The drum pad files are being dragged over (highlighted as a drop target).
    pub fn drop_pad(&self) -> Option<(&DeviceKey, u8)> {
        self.drop_pad.map(|(i, p)| (&self.devices[i].key, p))
    }

    /// Centre of a jack (logical view coordinates), on the back view only.
    pub fn socket_center(&self, socket: &SocketRef) -> Option<Point> {
        let local = SocketRef {
            device: self.local_key(&socket.device),
            socket: socket.socket.clone(),
        };
        let g = self.sockets.get(&local)?;
        (self.flip == Flip::Back && g.visible).then(|| self.rack_to_view(g.pos))
    }

    /// Centre of a device's name plate (logical view coordinates).
    pub fn plate_center(&self, key: &DeviceKey) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        (!d.is_interface())
            .then(|| self.rack_to_view(paint::plate_rect(d.height, d.folded).center() + d.offset()))
    }

    /// Centre of a device's fold button (logical view coordinates).
    pub fn fold_button_center(&self, key: &DeviceKey) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        (!d.is_interface())
            .then(|| self.rack_to_view(paint::fold_button_center(d.height, d.folded) + d.offset()))
    }

    /// The centre of a part of the patch strip of device `key` (logical view coordinates), when
    /// its plate has one (unfolded devices of the document, console devices excepted).
    pub fn patch_strip_center(&self, key: &DeviceKey, op: PatchOp) -> Option<Point> {
        let d = &self.devices[self.device_index(key)?];
        paint::has_patch_strip(d.device.as_ref(), d.folded).then(|| {
            let strip = paint::patch_strip(d.height);
            let rect = match op {
                PatchOp::Previous => strip.prev,
                PatchOp::Browse | PatchOp::Siblings => strip.name,
                PatchOp::Next => strip.next,
                PatchOp::Save => strip.save,
            };
            self.rack_to_view(rect.center() + d.offset())
        })
    }

    /// The BYPASS switch of an effect's name strip (logical view coordinates, front or back
    /// side): `None` for a device without one (instruments, utilities, plugins, the
    /// interface).
    pub fn bypass_button_rect(&self, key: &DeviceKey) -> Option<Rect> {
        let d = &self.devices[self.device_index(key)?];
        d.has_strip_bypass().then(|| {
            let r = paint::bypass_button_rect(d.height, d.folded) + d.offset();
            let (a, b) = (
                self.rack_to_view(Point::new(r.x0, r.y0)),
                self.rack_to_view(Point::new(r.x1, r.y1)),
            );
            Rect::new(a.x, a.y, b.x, b.y)
        })
    }

    /// Midpoint of a cable's curve (logical view coordinates).
    pub fn cable_midpoint(&self, cable: &Cable) -> Option<Point> {
        let c = self.cable_curve(cable)?;
        Some(self.rack_to_view(c.eval(0.5)))
    }

    /// Feeds the linear peak of the signal on each CV cable since the last call, once per frame
    /// (`dt` seconds since the previous call). The view keeps a displayed level per cable that
    /// rises at once and falls with a time constant of [`CV_DECAY_SECONDS`], so a pulse stays
    /// visible and a steady modulation does not flicker. Cables missing from `peaks` are
    /// forgotten. It repaints only when a cable's brightness step changed while the back is
    /// showing.
    pub fn set_cv_activity(&mut self, peaks: &[(Cable, f32)], dt: f32) {
        let fall = if dt.is_finite() && dt > 0.0 {
            (-dt / CV_DECAY_SECONDS).exp()
        } else {
            1.0
        };
        let steps = f32::from(paint::CV_ACTIVITY_STEPS);
        let mut next = BTreeMap::new();
        let mut changed = false;
        for (cable, peak) in peaks {
            let peak = if peak.is_finite() {
                peak.abs().min(1.0)
            } else {
                0.0
            };
            // Square root: a small modulation (0.05) must still be visible.
            let held = self.cv_activity.get(cable).copied().unwrap_or(0.0);
            let level = peak.sqrt().max(held * fall);
            let level = if level < 0.5 / steps { 0.0 } else { level };
            changed |= step_of(held) != step_of(level);
            next.insert(cable.clone(), level);
        }
        // A cable that left the feed while lit goes dark.
        changed |= self
            .cv_activity
            .iter()
            .any(|(c, l)| step_of(*l) != 0 && !next.contains_key(c));
        self.cv_activity = next;
        if changed && self.flip == Flip::Back {
            self.repaint = true;
        }
    }

    /// The brightness step (0 = off, up to [`paint::CV_ACTIVITY_STEPS`]) of a cable's CV
    /// activity indicator.
    pub fn cv_activity_step(&self, cable: &Cable) -> u8 {
        self.cv_activity.get(cable).map_or(0, |l| step_of(*l))
    }

    fn cable_curve(&self, cable: &Cable) -> Option<vello::kurbo::CubicBez> {
        let a = self.sockets.get(&cable.from)?;
        let b = self.sockets.get(&cable.to)?;
        Some(cables::cable_curve(a.pos, b.pos))
    }

    // ------------------------------------------------------------------ state accessors

    /// Shows the front or the back of the rack.
    pub fn set_flip(&mut self, flip: Flip) {
        if self.flip != flip {
            self.flip = flip;
            self.drag = Drag::None;
            self.hover = None;
            self.hover_cable = None;
            self.rebuild_tree();
            self.repaint = true;
        }
    }

    /// The visible side.
    pub fn flip(&self) -> Flip {
        self.flip
    }

    /// Sets cable visibility.
    pub fn set_cable_visibility(&mut self, visibility: CableVisibility) {
        self.cable_visibility = visibility;
        self.repaint = true;
    }

    /// Cable visibility.
    pub fn cable_visibility(&self) -> CableVisibility {
        self.cable_visibility
    }

    /// Zoom factor (0.5–2).
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Sets the zoom (clamped to 0.5–2), keeping the top-left rack point fixed.
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Zooms keeping the rack point under `anchor` (logical view position) fixed.
    pub fn zoom_at(&mut self, zoom: f64, anchor: Point) {
        let r = self.view_to_rack(anchor);
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll.y = self.rack_area().y0 + RACK_MARGIN + r.y * self.zoom - anchor.y;
        self.scroll.x = self.rack_area().x0 + RACK_MARGIN + r.x * self.zoom - anchor.x;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Vertical scroll offset in logical pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll.y
    }

    /// Sets the vertical scroll (clamped).
    pub fn set_scroll(&mut self, y: f64) {
        self.scroll.y = y;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Scrolls by a delta in logical pixels (clamped).
    pub fn scroll_by(&mut self, delta: Vec2) {
        self.scroll += delta;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The display scale factor.
    pub fn scale_factor(&self) -> f64 {
        self.scale
    }

    /// Size of the render target in physical pixels.
    pub fn physical_size(&self) -> (u32, u32) {
        (
            (self.size.width * self.scale).ceil().max(1.0) as u32,
            (self.size.height * self.scale).ceil().max(1.0) as u32,
        )
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Forces a repaint (e.g. after the window was exposed).
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    /// The selected device.
    pub fn selected(&self) -> Option<&DeviceKey> {
        self.selected.as_ref()
    }

    /// Selects a device (no action emitted).
    pub fn set_selected(&mut self, key: Option<DeviceKey>) {
        self.selected = key.map(|k| self.local_key(&k));
        self.selection = self.selected.iter().cloned().collect();
        self.anchor.clone_from(&self.selected);
        self.repaint = true;
    }

    /// The control with keyboard focus: (device, param key).
    pub fn focused_control(&self) -> Option<(DeviceKey, String)> {
        match self.tree.focused().map(|n| *self.tree.data(n)) {
            Some(Hit::Control(i, slot)) => {
                let d = self.devices.get(i)?;
                let desc = d.desc?;
                let pc = d.spec.front.get(slot)?;
                Some((
                    d.key.clone(),
                    desc.params[d.spec.param_index(&desc, pc, d.pad)]
                        .key
                        .to_string(),
                ))
            }
            Some(Hit::PresetGrid(i)) => {
                Some((self.devices.get(i)?.key.clone(), "preset".to_string()))
            }
            _ => None,
        }
    }

    /// Whether the view owns the keyboard as a text field: the browser filter has focus, or a
    /// name is being edited. The shell then routes plain keys (letters, digits, Space, Enter)
    /// to the view instead of to commands.
    pub fn is_text_owner(&self) -> bool {
        self.rename.is_some() || (self.browser.visible && self.browser.focused)
    }

    /// Shows or hides the device browser (showing it focuses its search field). Either way it
    /// is the normal browser afterwards, not the create mode.
    pub fn set_browser_visible(&mut self, visible: bool) {
        self.browser.visible = visible;
        self.browser.focused = visible;
        self.browser.create = None;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Opens the browser in create mode: the list is filtered to
    /// `kind`, the search field is focused and empty, Enter creates the highlighted device
    /// (and closes the browser unless [`Self::set_create_keeps_browser_open`]), Esc cancels.
    pub fn open_browser_create(&mut self, kind: CreateKind, registry: &Registry) {
        self.browser.visible = true;
        self.browser.focused = true;
        self.browser.create = Some(kind);
        self.browser.query.clear();
        self.browser.scroll = 0.0;
        self.browser.highlighted = Browser::first_selectable(&self.browser.list(registry));
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The kind the browser was opened to create (`None`: the normal browser).
    pub fn browser_create(&self) -> Option<CreateKind> {
        self.browser.create
    }

    /// Whether a create-mode browser stays open after creating a device (the Workflow
    /// setting; default off).
    pub fn set_create_keeps_browser_open(&mut self, keep: bool) {
        self.browser.keep_open = keep;
    }

    /// The Workflow setting "Auto-route new devices" (default on): the browser creates devices
    /// routed (Shift inverts it for one device) and Delete bridges the audio path of a removed
    /// effect.
    pub fn set_auto_route(&mut self, on: bool) {
        self.auto_route = on;
    }

    /// The Workflow setting "Auto-route new devices".
    pub fn auto_route(&self) -> bool {
        self.auto_route
    }

    /// The send entry of the effect create browser: shown with the Master Section selected (the
    /// created effect goes to the first free send/return slot), greyed when all eight are used.
    /// `None` outside the effect create mode and for any other selection.
    pub fn browser_send_entry(&self, doc: &RackDoc) -> Option<crate::browser::SendEntry> {
        use crate::browser::SendEntry;
        if !self.browser.visible
            || self.browser.create != Some(CreateKind::Effect)
            || !self.scope.is_empty()
        {
            return None;
        }
        let master = doc.console.master.as_ref()?;
        (self.selected.as_ref() == Some(master)).then(|| {
            atomik_model::first_free_send_slot(doc).map_or(SendEntry::Full, SendEntry::Slot)
        })
    }

    /// Whether the browser is shown.
    pub fn browser_visible(&self) -> bool {
        self.browser.visible
    }

    /// The type ids listed by the browser (after search filtering).
    pub fn browser_rows(&self, registry: &Registry) -> Vec<&'static str> {
        self.browser
            .rows(registry)
            .iter()
            .map(|d| d.type_id)
            .collect()
    }

    /// Every row of the browser list: the device types, then the Plugins group (see
    /// [`crate::browser::Browser::list`]).
    pub fn browser_list(&self, registry: &Registry) -> Vec<crate::browser::BrowserRow> {
        self.browser.list(registry)
    }

    /// Centre of browser row `i` (logical view coordinates).
    pub fn browser_row_center(&self, i: usize) -> Option<Point> {
        self.browser
            .visible
            .then(|| self.browser.row_rect(i).center())
    }

    /// Shows an error (e.g. a rejected command) until the next click.
    pub fn notify_rejected(&mut self, message: impl Into<String>) {
        self.toast = Some(message.into());
        self.repaint = true;
    }

    /// Sets the values the controls show instead of the stored ones: the automation at the
    /// playhead, by `(device, param key)` (see [`atomik_model::RackDoc::automation_values_at`]).
    /// A control shows its param's entry; with none it shows the document's value. The next sync
    /// or paint re-records only the panels whose displayed values changed. Returns whether the set
    /// changed. Top level only: inside a container the controls show the document's values.
    pub fn set_shown_values(&mut self, shown: &atomik_model::ShownValues) -> bool {
        if &self.shown == shown {
            return false;
        }
        self.shown.clone_from(shown);
        self.repaint = true;
        true
    }

    /// The value the control of `param` on top-level device `device` shows now, `None` for a
    /// device or param the view does not know. What the panel was last synced with: the shown
    /// value of an automated param, else the document's.
    pub fn shown_value(&self, device: &DeviceKey, param: &str) -> Option<f32> {
        let v = self.devices.iter().find(|d| &d.key == device)?;
        let desc = v.desc.as_ref()?;
        let p = desc.param_by_key(param)?;
        Some(
            v.device
                .as_ref()
                .map_or(p.default, |d| widget::current_value(d, p)),
        )
    }

    /// Whether the control of `param` on top-level device `device` is marked as automated (the
    /// param has an automation lane).
    pub fn is_automated(&self, device: &DeviceKey, param: &str) -> bool {
        self.devices
            .iter()
            .find(|d| &d.key == device)
            .is_some_and(|d| d.automated.iter().any(|k| k == param))
    }

    /// (device, panel revision) in rack order; a revision changes only when that device's
    /// panel must be re-recorded.
    pub fn panel_revisions(&self) -> Vec<(DeviceKey, u64)> {
        self.devices
            .iter()
            .map(|d| (d.key.clone(), d.revision))
            .collect()
    }

    /// The cable under the pointer.
    pub fn hovered_cable(&self) -> Option<&Cable> {
        self.hover_cable.as_ref()
    }

    fn hover_device(&self) -> Option<usize> {
        self.hover.and_then(Hit::device)
    }

    /// Cables currently drawn.
    pub fn visible_cables(&self, doc: &RackDoc) -> Vec<Cable> {
        if self.flip != Flip::Back {
            return Vec::new();
        }
        self.cables_in_scope(doc)
            .iter()
            .filter(|c| self.sockets.contains_key(&c.from) && self.sockets.contains_key(&c.to))
            .filter(|c| self.show_links || !self.links.contains(c))
            .filter(|c| !self.is_detached(c))
            .filter(|c| self.mode_shows(c) || self.hover_cable.as_ref() == Some(*c))
            .cloned()
            .collect()
    }

    /// The cables of the open scope (the document's at the top level).
    fn cables_in_scope<'a>(&'a self, doc: &'a RackDoc) -> &'a [Cable] {
        if self.scope.is_empty() {
            &doc.cables
        } else {
            &self.cables
        }
    }

    /// Whether the cable display mode draws `cable` (a hovered cable is drawn in every mode).
    fn mode_shows(&self, cable: &Cable) -> bool {
        let on = |k: &DeviceKey| cable.from.device == *k || cable.to.device == *k;
        match self.cable_visibility {
            CableVisibility::Show => true,
            CableVisibility::Hide => false,
            CableVisibility::HoverOnly => self
                .hover_device()
                .is_some_and(|i| on(self.devices[i].socket_key())),
            CableVisibility::SelectedOnly => self
                .devices
                .iter()
                .filter(|d| {
                    self.selection.contains(&d.key) || self.selected.as_ref() == Some(&d.key)
                })
                .any(|d| on(d.socket_key())),
            CableVisibility::HideAutoRouted => {
                let console = |k: &DeviceKey| {
                    self.devices.iter().any(|d| {
                        d.socket_key() == k
                            && d.desc
                                .as_ref()
                                .is_some_and(|x| atomik_model::console::is_console_type(x.type_id))
                    })
                };
                let kind = self.sockets.get(&cable.from).map(|g| g.end.kind);
                let role = kind.map(|k| cables::role_of(cable, k));
                !matches!(
                    role,
                    Some(cables::CableRole::Insert | cables::CableRole::Send)
                ) && !console(&cable.from.device)
                    && !console(&cable.to.device)
            }
        }
    }

    /// The highlight of a socket (drag compatibility or cable hover).
    pub fn socket_highlight(&self, socket: &SocketRef) -> SocketHighlight {
        if let Drag::Cable { anchor, .. } = &self.drag {
            if &anchor.at == socket {
                return SocketHighlight::Source;
            }
            let Some(g) = self.sockets.get(socket) else {
                return SocketHighlight::Idle;
            };
            if !cables::compatible(anchor, &g.end) {
                return SocketHighlight::Incompatible;
            }
            if self.drop_target().is_some_and(|t| &t.end.at == socket) {
                return SocketHighlight::Target;
            }
            return SocketHighlight::Compatible;
        }
        if let Some(c) = &self.hover_cable
            && (&c.from == socket || &c.to == socket)
        {
            return SocketHighlight::Hovered;
        }
        SocketHighlight::Idle
    }

    /// The tooltip text (value while dragging a control, socket name when hovering a jack).
    pub fn tooltip(&self) -> Option<String> {
        match &self.drag {
            Drag::Value {
                dev, slot, value, ..
            } => {
                let d = &self.devices[*dev];
                let desc = d.desc?;
                let p = &desc.params[d.spec.param_index(&desc, &d.spec.front[*slot], d.pad)];
                let text = (d.fx == Some(FxKind::Character))
                    .then(|| crate::fx::character_value_text(p.key, *value))
                    .flatten()
                    .unwrap_or_else(|| widget::format_value(p, *value));
                Some(format!("{}  {}", autolayout::control_label(p), text))
            }
            Drag::None => match self.hover {
                Some(Hit::Socket(i, slot)) => self.jack_tooltip(i, slot),
                Some(h @ (Hit::IoCell(..) | Hit::IoClip(..) | Hit::IoHeader(_))) => {
                    self.io_tooltip(h)
                }
                Some(h @ Hit::ReturnEdit(..)) if self.button_tips => self.return_edit_tooltip(h),
                Some(Hit::Preset(i, n)) => self.preset_name(i, n),
                // A control a macro drives says what it shows and who moves it.
                Some(Hit::Control(i, slot)) => {
                    let (p, _, value) = self.param_of(i, slot)?;
                    let Some(m) = self.driven_by(i, p.key) else {
                        if self.devices[i].automated.iter().any(|k| k == p.key) {
                            return Some(format!(
                                "{}  {}  (automated)",
                                autolayout::control_label(p),
                                widget::format_value(p, value)
                            ));
                        }
                        return self.value_tips.then(|| {
                            format!(
                                "{}  {}",
                                autolayout::control_label(p),
                                widget::format_value(p, value)
                            )
                        });
                    };
                    Some(format!(
                        "{}  {}  (driven by macro {})",
                        autolayout::control_label(p),
                        widget::format_value(p, value),
                        u16::from(m) + 1
                    ))
                }
                Some(Hit::Patch(i, op)) if self.button_tips => {
                    let name = &self.devices[i].device.as_ref()?.patch_name;
                    Some(match op {
                        PatchOp::Previous => "Previous patch of this device type".to_string(),
                        PatchOp::Next => "Next patch of this device type".to_string(),
                        PatchOp::Browse | PatchOp::Siblings if name.is_empty() => {
                            "Browse patches (right-click: the patches of this folder)".to_string()
                        }
                        PatchOp::Browse | PatchOp::Siblings => {
                            format!(
                                "{name}: browse patches (right-click: the patches of this folder)"
                            )
                        }
                        PatchOp::Save => "Save this device as a patch".to_string(),
                    })
                }
                Some(Hit::Bypass(i)) if self.button_tips => {
                    let player = self.devices[i]
                        .desc
                        .as_ref()
                        .is_some_and(|d| crate::browser::is_player(d.type_id));
                    let container = self.devices[i]
                        .desc
                        .as_ref()
                        .is_some_and(|d| d.type_id == container_panel::CONTAINER_TYPE);
                    if container {
                        return Some(
                            if self.devices[i].is_bypassed() {
                                "Bypassed: the inside is left out and the audio passes straight \
                                 through. Click to switch the container back in"
                            } else {
                                "Click to bypass the container: its inside is left out and the \
                                 audio passes straight through"
                            }
                            .to_string(),
                        );
                    }
                    Some(
                        match (player, self.devices[i].is_bypassed()) {
                            (false, true) => {
                                "Bypassed: the dry signal passes. Click to switch the effect back in"
                            }
                            (false, false) => "Click to bypass the effect: the dry signal passes",
                            (true, true) => {
                                "Bypassed: the notes pass through unchanged. Click to switch the player back in"
                            }
                            (true, false) => {
                                "Click to bypass the player: the notes pass through unchanged"
                            }
                        }
                        .to_string(),
                    )
                }
                Some(Hit::Plate(i) | Hit::Name(i)) => {
                    let d = &self.devices[i];
                    if self.locked.as_ref() == Some(&d.key) {
                        Some(Self::MASTER_LOCKED_TEXT.to_string())
                    } else {
                        self.outputs.get(&d.key).and_then(|o| o.tooltip.clone())
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn hit(&self, view_pos: Point) -> Option<Hit> {
        if !self.rack_area().contains(view_pos) {
            return None;
        }
        let rp = self.view_to_rack(view_pos);
        self.tree.hit_test(rp).map(|n| *self.tree.data(n))
    }

    fn socket_geo(&self, i: usize, slot: usize) -> Option<&SockGeo> {
        let d = &self.devices[i];
        let s = &d.desc?.sockets[d.spec.back.get(slot)?.socket];
        self.sockets.get(&SocketRef {
            device: d.socket_key().clone(),
            socket: s.key.to_string(),
        })
    }

    /// The compatible socket under the pointer while dragging a cable.
    fn drop_target(&self) -> Option<&SockGeo> {
        let Drag::Cable { anchor, .. } = &self.drag else {
            return None;
        };
        match self.hit(self.pointer) {
            Some(Hit::Socket(i, slot)) => self
                .socket_geo(i, slot)
                .filter(|g| cables::compatible(anchor, &g.end)),
            _ => None,
        }
    }

    fn param_of(
        &self,
        i: usize,
        slot: usize,
    ) -> Option<(&'static atomik_core::ParamDescriptor, ControlKind, f32)> {
        let d = self.devices.get(i)?;
        let desc = d.desc?;
        let pc = d.spec.front.get(slot)?;
        // Descriptors of registered devices are 'static; the interface has no params.
        let p: &'static atomik_core::ParamDescriptor =
            &desc.params[d.spec.param_index(&desc, pc, d.pad)];
        let value = d
            .device
            .as_ref()
            .map_or(p.default, |dev| widget::current_value(dev, p));
        Some((p, pc.kind, value))
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first. Inside a
    /// container the commands and actions carry **path keys** (`layer/synth`), and a gesture
    /// that changes the scope has already changed it when this returns.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        // A press inside a container may start a drag out of it, which the model plans on the
        // whole project (the scope document has only the inside).
        self.host_doc = (matches!(event, UiEvent::PointerDown { .. }) && self.scope.len() == 1)
            .then(|| doc.clone());
        let view = self.prepare(doc, registry);
        let out = self.handle_in(event, &view, registry);
        self.finish(out, registry)
    }

    fn handle_in(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync_in(doc, registry);
        let mut out = Vec::new();
        match event {
            UiEvent::ModifiersChanged { .. } => {}
            UiEvent::Resize(size) => {
                self.size = *size;
                self.clamp_scroll();
                self.repaint = true;
            }
            UiEvent::ScaleChanged(s) => {
                self.scale = s.max(0.1);
                self.repaint = true;
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
                self.pointer_up(*pos, *modifiers, doc, registry, &mut out)
            }
            UiEvent::PointerLeave => {
                self.nav_hover = false;
                self.hover = None;
                self.hover_cable = None;
                self.browser.hover = None;
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => self.wheel(*delta, *pos, *modifiers, registry, &mut out),
            UiEvent::Key {
                key,
                modifiers,
                pressed,
            } => {
                if *pressed {
                    self.key(key, *modifiers, doc, registry, &mut out);
                }
            }
            UiEvent::FileHover { pos, paths } => {
                let library = paths.iter().any(|p| library_drop::is_library(p));
                let line =
                    library_drop::rack_item(paths).is_some() && self.rack_area().contains(*pos);
                if line {
                    self.pointer = *pos;
                }
                self.set_lib_drop(line);
                // A library item is never a sample: the sampler panel does not light up.
                let target = if paths.is_empty() || library {
                    None
                } else {
                    self.sampler_at(*pos)
                };
                let pad = target.and_then(|_| self.pad_at(*pos));
                self.set_drop_hover(target);
                self.set_drop_pad(pad);
            }
            UiEvent::FileHoverCancel => {
                self.set_drop_hover(None);
                self.set_drop_pad(None);
                self.set_lib_drop(false);
            }
            UiEvent::FileDrop { pos, paths } => {
                self.set_lib_drop(false);
                if paths.iter().any(|p| library_drop::is_library(p)) {
                    self.set_drop_hover(None);
                    self.set_drop_pad(None);
                    // A patch dropped on the patch strip of a device is for that device.
                    if let Some(path) = library_drop::patch_item(paths)
                        && let Some(Hit::Patch(i, _)) = self.hit(*pos)
                    {
                        out.push(UiAction::ApplyLibraryPatch {
                            path: path.clone(),
                            device: self.devices[i].key.clone(),
                        });
                        return out;
                    }
                    if let Some(path) = library_drop::rack_item(paths)
                        && self.rack_area().contains(*pos)
                    {
                        let none = vec![false; self.devices.len()];
                        let dp = self.drop_point(self.view_to_rack(*pos), &none);
                        let index = self.slot_index(doc, &dp, &[]);
                        // Inside a container there is one column: the index counts its devices.
                        let column = if self.scope.is_empty() { dp.column } else { 0 };
                        out.push(UiAction::InsertLibrary {
                            path: path.clone(),
                            container: self.scope_path(),
                            slot: RackSlot { column, index },
                        });
                    }
                    return out;
                }
                let target = self.sampler_at(*pos);
                let pad = target.and_then(|_| self.pad_at(*pos));
                self.set_drop_hover(None);
                self.set_drop_pad(None);
                if let Some(i) = target
                    && !paths.is_empty()
                {
                    out.push(UiAction::ImportSamples {
                        paths: paths.clone(),
                        device: self.devices[i].key.clone(),
                        at_key: pad.map(|(_, p)| atomik_model::pad_key(p)),
                    });
                }
            }
        }
        out
    }

    /// The sampler panel (a front-side, unfolded device that takes samples) at a view
    /// position, as a device index.
    fn sampler_at(&self, pos: Point) -> Option<usize> {
        if self.in_browser(pos) || self.flip != Flip::Front {
            return None;
        }
        let i = self.hit(pos)?.device()?;
        let d = &self.devices[i];
        (d.samples && !d.folded).then_some(i)
    }

    /// The drum pad (device index, pad) at a view position on the front side.
    fn pad_at(&self, pos: Point) -> Option<(usize, u8)> {
        match self.hit(pos)? {
            Hit::Pad(i, pad) => Some((i, pad)),
            _ => None,
        }
    }

    fn set_lib_drop(&mut self, on: bool) {
        if self.lib_drop != on {
            self.lib_drop = on;
            self.repaint = true;
        }
    }

    fn set_drop_pad(&mut self, target: Option<(usize, u8)>) {
        if self.drop_pad != target {
            self.drop_pad = target;
            self.repaint = true;
        }
    }

    /// Selects a pad of a drum panel (view state; the panel shows that pad's knobs).
    fn select_pad(&mut self, i: usize, pad: u8) {
        let d = &mut self.devices[i];
        if d.pad != pad {
            d.pad = pad;
            d.revision += 1;
            d.lists = [None, None];
            self.pads.insert(d.key.clone(), pad);
            self.repaint = true;
        }
    }

    fn set_drop_hover(&mut self, target: Option<usize>) {
        if self.drop_hover != target {
            self.drop_hover = target;
            self.repaint = true;
        }
    }

    /// The sampler panel files are being dragged over (it is highlighted as a drop target).
    pub fn drop_hover(&self) -> Option<&DeviceKey> {
        self.drop_hover.map(|i| &self.devices[i].key)
    }

    fn in_browser(&self, pos: Point) -> bool {
        self.browser.visible && pos.x < BROWSER_WIDTH
    }

    fn select(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select_only(i, out);
    }

    fn update_hover(&mut self, pos: Point) {
        let hit = if self.in_browser(pos) {
            None
        } else {
            self.hit(pos)
        };
        let old_hover = (self.hover, self.hover_cable.clone());
        self.hover = hit;
        self.hover_cable = None;
        if self.flip == Flip::Back
            && !matches!(hit, Some(Hit::Socket(..) | Hit::Control(..) | Hit::Fold(_)))
            && self.rack_area().contains(pos)
        {
            let rp = self.view_to_rack(pos);
            let tol = cables::HIT_TOLERANCE / self.zoom.min(1.0);
            let candidates: Vec<Cable> = self.cables.clone();
            for c in candidates.iter().rev() {
                if self.mode_shows(c)
                    && (self.show_links || !self.links.contains(c))
                    && let Some(curve) = self.cable_curve(c)
                    && cables::hits_curve(&curve, rp, tol)
                {
                    self.hover_cable = Some(c.clone());
                    break;
                }
            }
        }
        let rows = self.browser.hover;
        self.browser.hover = None;
        if old_hover != (self.hover, self.hover_cable.clone()) || rows != self.browser.hover {
            self.repaint = true;
        }
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.pointer = pos;
        let rp = self.view_to_rack(pos);
        if self.sc_menu.is_some() {
            self.sc_hover(pos);
        }
        if self.choice_menu.is_some() {
            self.choice_hover(pos);
        }
        if self.plugin_menu.is_some() {
            self.plugin_menu_hover(pos);
        }
        if self.macro_menu.is_some() {
            self.macro_hover(pos);
        }
        self.crumb_hover_at(pos);
        match self.drag.clone() {
            Drag::Eq(e) => self.eq_move(e, rp, out),
            Drag::Drum(d) => self.drum_move(d, pos, out),
            Drag::Value {
                dev,
                slot,
                mut drag,
                merge,
                last_y,
                start,
                value,
            } => {
                let Some((p, _, _)) = self.param_of(dev, slot) else {
                    return;
                };
                let v = drag.update(p, last_y - pos.y, self.fine_adjust(modifiers));
                if v.to_bits() != value.to_bits() {
                    out.push(param_cmd(&self.devices[dev].key, p.key, v, Some(merge)));
                }
                self.drag = Drag::Value {
                    dev,
                    slot,
                    drag,
                    merge,
                    last_y: pos.y,
                    start,
                    value: v,
                };
                self.repaint = true;
            }
            Drag::Cable {
                anchor, detached, ..
            } => {
                self.drag = Drag::Cable {
                    anchor,
                    pos: rp,
                    detached,
                };
                self.repaint = true;
            }
            Drag::Move {
                dev,
                start,
                collapse,
                ..
            } => {
                self.drag = Drag::Move {
                    dev,
                    start,
                    at: rp,
                    collapse,
                };
                self.repaint = true;
            }
            Drag::Browser { row, start, active } => {
                let active = active || (pos - start).hypot() > MOVE_THRESHOLD;
                self.drag = Drag::Browser { row, start, active };
                self.repaint = true;
            }
            Drag::Pan { last } => {
                self.drag = Drag::Pan { last: pos };
                self.scroll_by(last - pos);
            }
            Drag::Navigator { .. } => self.navigator_follow(pos),
            Drag::Pad { .. } => {}
            Drag::Sens {
                dev,
                start_y,
                start,
            } => {
                let v = f64::from(start) + (start_y - pos.y) * 0.8;
                self.store_sens(dev, v.round().clamp(0.0, 100.0) as u8);
            }
            Drag::None => {
                self.navigator_hover(pos);
                if self.in_navigator(pos) {
                    if self.hover.take().is_some() || self.hover_cable.take().is_some() {
                        self.repaint = true;
                    }
                } else if self.in_browser(pos) {
                    let row = self.browser.row_at(pos, 64);
                    if row != self.browser.hover || self.hover.is_some() {
                        self.browser.hover = row;
                        self.hover = None;
                        self.hover_cable = None;
                        self.repaint = true;
                    }
                } else {
                    self.update_hover(pos);
                }
            }
        }
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
        self.toast = None;
        self.context_slot = None;
        self.pointer = pos;
        self.repaint = true;
        if self.big_meter_press(pos, button) {
            return;
        }
        if self.macro_press(pos, out) || self.plugin_menu_press(pos) {
            return;
        }
        if self.sc_press(pos, doc, registry, out) || self.choice_press(pos, out) {
            return;
        }
        if self.crumb_press(pos, button) {
            return;
        }
        if self.navigator_press(pos, button) {
            return;
        }
        if self.in_browser(pos) {
            self.browser.focused = true;
            let rows = self.browser.list(registry);
            if let Some(i) = self.browser.row_at(pos, rows.len())
                && rows[i].selectable()
            {
                self.browser.highlighted = i;
                if clicks >= 2 && button == PointerButton::Primary {
                    out.extend(self.add_row_action(&rows[i], doc, modifiers));
                } else if button == PointerButton::Primary {
                    // A press that travels past the threshold drags the device out.
                    self.drag = Drag::Browser {
                        row: rows[i].clone(),
                        start: pos,
                        active: false,
                    };
                }
            }
            return;
        }
        self.browser.focused = false;
        if self.device_rename_press(pos, out) {
            return;
        }
        self.cancel_rename();
        if button == PointerButton::Middle {
            self.drag = Drag::Pan { last: pos };
            return;
        }
        let Some(hit) = self.hit(pos) else { return };
        let rp = self.view_to_rack(pos);
        match (hit, button) {
            (Hit::Fold(i), PointerButton::Primary) if modifiers.alt => {
                // Alt folds or unfolds the whole column the device is in, to the state the
                // click would give this device.
                let (column, folded) = (self.devices[i].column, !self.devices[i].folded);
                let keys: Vec<DeviceKey> = self
                    .inner_range()
                    .map(|j| &self.devices[j])
                    .filter(|d| d.column == column && self.locked.as_ref() != Some(&d.key))
                    .map(|d| d.key.clone())
                    .collect();
                out.push(UiAction::Command(Command::Workflow(
                    WorkflowCommand::SetFoldedMany { keys, folded },
                )));
            }
            (Hit::Fold(i), PointerButton::Primary) => {
                let d = &self.devices[i];
                out.push(UiAction::Command(Command::SetFolded {
                    key: d.key.clone(),
                    folded: !d.folded,
                }));
            }
            (Hit::Bypass(i), PointerButton::Primary) => self.bypass_down(i, out),
            (Hit::Patch(i, op), PointerButton::Primary) => {
                self.select(i, out);
                out.push(UiAction::DevicePatch {
                    device: self.devices[i].key.clone(),
                    op,
                });
            }
            (Hit::Patch(i, PatchOp::Browse), PointerButton::Secondary) => {
                self.select(i, out);
                out.push(UiAction::DevicePatch {
                    device: self.devices[i].key.clone(),
                    op: PatchOp::Siblings,
                });
            }
            (Hit::Name(i), PointerButton::Primary) if clicks >= 2 => {
                self.begin_device_rename(i, out)
            }
            (Hit::Plate(i) | Hit::Name(i), PointerButton::Primary) => {
                let collapse = self.select_click(i, modifiers, out);
                if self.locked.as_ref() == Some(&self.devices[i].key) {
                    // The Master Section is selected by the press but never dragged.
                    if collapse {
                        self.collapse_to(i, out);
                    }
                    return;
                }
                self.drag = Drag::Move {
                    dev: i,
                    start: rp,
                    at: rp,
                    collapse,
                };
                self.plan_container_drops(i, doc, registry);
            }
            (Hit::Edit(i), PointerButton::Primary) => {
                self.select(i, out);
                let device = self.devices[i].key.clone();
                out.push(if self.devices[i].spec.drum.is_some() {
                    UiAction::OpenStepEditor { device, clip: None }
                } else if self.devices[i].spec.slicer.is_some() {
                    UiAction::OpenSliceEditor(device)
                } else {
                    UiAction::OpenSampleEditor(device)
                });
            }
            (Hit::SliceBtn(i), PointerButton::Primary) => {
                self.select(i, out);
                let d = &self.devices[i];
                // Without a loop there is nothing to slice.
                if d.device.as_ref().is_some_and(|dev| !dev.zones.is_empty()) {
                    out.push(UiAction::SliceLoop {
                        device: d.key.clone(),
                        mode: SliceMode::Transients {
                            sensitivity: d.sens,
                        },
                    });
                }
            }
            (Hit::ToTrack(i), PointerButton::Primary) => {
                self.select(i, out);
                let d = &self.devices[i];
                if d.device.as_ref().is_some_and(|dev| !dev.zones.is_empty()) {
                    out.push(UiAction::SlicesToTrack {
                        device: d.key.clone(),
                    });
                }
            }
            (Hit::Sens(i), PointerButton::Primary) => {
                self.select(i, out);
                if clicks >= 2 {
                    self.store_sens(i, autolayout::DEFAULT_SLICE_SENSITIVITY);
                } else {
                    self.drag = Drag::Sens {
                        dev: i,
                        start_y: pos.y,
                        start: self.devices[i].sens,
                    };
                }
            }
            (Hit::Strip(i), PointerButton::Primary) if clicks >= 2 => {
                self.select(i, out);
                out.push(UiAction::OpenSliceEditor(self.devices[i].key.clone()));
            }
            (Hit::ToChannel(i), PointerButton::Primary) => {
                self.select(i, out);
                let d = &self.devices[i];
                if !self.scope.is_empty() {
                    self.toast = Some(
                        "Devices inside a container reach the console through the container".into(),
                    );
                } else if !d.pad_cabled[usize::from(d.pad - 1)] {
                    out.push(UiAction::Command(Command::Console(
                        ConsoleCommand::AddChannelFor {
                            source: d.key.clone(),
                            out_l: format!("pad{}_out_l", d.pad),
                            out_r: format!("pad{}_out_r", d.pad),
                            name: None,
                        },
                    )));
                }
            }
            (Hit::Pad(i, pad), PointerButton::Primary) => {
                self.select(i, out);
                self.select_pad(i, pad);
                let device = self.devices[i].key.clone();
                let key = atomik_model::pad_key(pad);
                out.push(UiAction::AuditionVelocity {
                    device: device.clone(),
                    key,
                    velocity: PAD_AUDITION_VELOCITY,
                    on: true,
                });
                self.drag = Drag::Pad { device, key };
            }
            (Hit::PadGrid(i), PointerButton::Primary) if clicks >= 2 => {
                self.select(i, out);
                out.push(UiAction::OpenStepEditor {
                    device: self.devices[i].key.clone(),
                    clip: None,
                });
            }
            (Hit::Container(i, b), PointerButton::Primary) => self.container_down(i, b, out),
            (Hit::MacroName(i, n), PointerButton::Primary) => {
                self.macro_name_down(i, n, clicks, out);
            }
            (Hit::Plugin(i, b), PointerButton::Primary) => self.plugin_down(i, b, rp, out),
            (Hit::Device(i), PointerButton::Primary)
                if clicks >= 2
                    && self.flip == Flip::Front
                    && self.devices[i].spec.container.is_some() =>
            {
                self.container_down(i, ContainerButton::Open, out);
            }
            (Hit::Device(i), PointerButton::Primary)
                if clicks >= 2
                    && self.flip == Flip::Front
                    && self.devices[i].samples
                    && self.devices[i].spec.drum.is_none()
                    && self.devices[i].spec.slicer.is_none() =>
            {
                self.select(i, out);
                out.push(UiAction::OpenSampleEditor(self.devices[i].key.clone()));
            }
            (Hit::Player(i, b), PointerButton::Primary) => {
                self.player_down(i, b, modifiers, clicks, out);
            }
            (Hit::EqPlot(i), PointerButton::Primary) => self.eq_down(i, rp, clicks, out),
            (Hit::Preset(i, n), PointerButton::Primary) => self.preset_down(i, n, out),
            (Hit::ScButton(i), PointerButton::Primary) => self.sc_open(i, doc, registry),
            (Hit::TunerButton(i), PointerButton::Primary) => self.tuner_toggle(i, out),
            (h @ (Hit::IoClip(..) | Hit::IoHeader(_)), PointerButton::Primary) => self.io_down(h),
            (Hit::ReturnEdit(i, slot), PointerButton::Primary) => {
                self.return_edit_down(i, slot, out)
            }
            (Hit::Plate(i) | Hit::Name(i) | Hit::Device(i), PointerButton::Secondary) => {
                self.select_for_menu(i, out);
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Device,
                    at: pos,
                });
            }
            (Hit::Device(i), PointerButton::Primary) => {
                if self.select_click(i, modifiers, out) {
                    self.collapse_to(i, out);
                }
            }
            (
                Hit::Plate(i)
                | Hit::Edit(i)
                | Hit::ToChannel(i)
                | Hit::Pad(i, _)
                | Hit::PadGrid(i)
                | Hit::SliceBtn(i)
                | Hit::ToTrack(i)
                | Hit::Sens(i)
                | Hit::Strip(i)
                | Hit::EqPlot(i)
                | Hit::Preset(i, _)
                | Hit::PresetGrid(i)
                | Hit::Player(i, _)
                | Hit::Container(i, _)
                | Hit::MacroName(i, _)
                | Hit::Plugin(i, _)
                | Hit::ScButton(i)
                | Hit::TunerButton(i),
                _,
            ) => self.select(i, out),
            (Hit::Control(i, slot), _) => {
                if let Some(n) = self.tree.find(|h| *h == Hit::Control(i, slot)) {
                    self.tree.set_focus(Some(n));
                }
                if self.plugin_unpin_wanted(i, button, modifiers) {
                    self.plugin_unpin(i, slot, out);
                } else if self.macro_menu_wanted(i, slot, button, modifiers) {
                    self.macro_menu_open(i, slot, rp);
                } else {
                    self.control_down(i, slot, button, clicks, modifiers, out);
                }
            }
            (Hit::Socket(i, slot), PointerButton::Primary) => self.cable_down(i, slot, rp, doc),
            (Hit::Socket(i, slot), PointerButton::Secondary) => {
                self.jack_menu_down(i, slot, pos, out)
            }
            (Hit::Rack, PointerButton::Primary) if clicks >= 2 => {
                self.set_browser_visible(true);
                out.push(UiAction::OpenBrowser);
            }
            (Hit::Rack, PointerButton::Primary) => self.clear_selection_by_click(),
            (Hit::Rack, PointerButton::Secondary) => {
                self.column_target = self.column_at(rp.x);
                self.context_slot = self.slot_at(rp, doc);
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Rack,
                    at: pos,
                });
            }
            _ => {}
        }
    }

    /// A press on the BYPASS switch of an effect's name strip: toggles its `bypass` param,
    /// one undo step like any other param edit.
    fn bypass_down(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        if self.driven_by(i, autolayout::BYPASS_KEY).is_some() {
            self.notify_driven(i, autolayout::BYPASS_KEY);
            return;
        }
        let on = self.devices[i].is_bypassed();
        let key = self.devices[i].key.clone();
        out.push(param_cmd(
            &key,
            autolayout::BYPASS_KEY,
            if on { 0.0 } else { 1.0 },
            None,
        ));
    }

    fn control_down(
        &mut self,
        i: usize,
        slot: usize,
        button: PointerButton,
        clicks: u8,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some((p, kind, value)) = self.param_of(i, slot) else {
            return;
        };
        out.push(UiAction::ParamTouched {
            device: self.devices[i].key.clone(),
            param: p.key.to_owned(),
        });
        // A primary click with the edit-automation role held shows the control's lane and does
        // nothing else (no drag, no change of value).
        if button == PointerButton::Primary && self.edit_automation_click(modifiers) {
            out.push(UiAction::EditAutomation {
                device: self.devices[i].key.clone(),
                param: p.key.to_owned(),
            });
            return;
        }
        // A param a macro drives is locked, but its right-click menu stays (Remove from Macro).
        if self.driven_by(i, p.key).is_some()
            && !(self.scope.is_empty() && button == PointerButton::Secondary)
        {
            self.notify_driven(i, p.key);
            return;
        }
        let key = self.devices[i].key.clone();
        match (kind, button) {
            // A right click on a knob, a fader or a switch opens its MIDI menu.
            (
                ControlKind::Knob | ControlKind::Fader | ControlKind::Toggle,
                PointerButton::Secondary,
            ) => {
                out.push(UiAction::ControlMenu {
                    device: key,
                    param: p.key.to_owned(),
                    at: self.pointer,
                });
            }
            (ControlKind::Knob | ControlKind::Fader, PointerButton::Primary) => {
                // A double-click or a click with the reset role held resets to the default.
                if clicks >= 2 || self.reset_click(modifiers) {
                    out.push(param_cmd(&key, p.key, p.clamp(p.default), None));
                } else {
                    let ppr = if kind == ControlKind::Fader {
                        widget::FADER_PIXELS_PER_RANGE
                    } else {
                        widget::KNOB_PIXELS_PER_RANGE
                    };
                    let merge = self.next_merge;
                    self.next_merge += 1;
                    self.drag = Drag::Value {
                        dev: i,
                        slot,
                        drag: ValueDrag::new(p, value, ppr * self.knob_range.travel()),
                        merge,
                        last_y: self.pointer.y,
                        start: value,
                        value,
                    };
                    if let Some(n) = self.tree.find(|h| *h == Hit::Control(i, slot)) {
                        self.tree.set_capture(n);
                    }
                }
            }
            (ControlKind::Toggle, PointerButton::Primary) => {
                out.push(param_cmd(&key, p.key, widget::toggled(p, value), None));
            }
            (ControlKind::Choice, PointerButton::Primary)
                if crate::master_panel::is_pick_list(p) =>
            {
                self.choice_open(i, slot);
            }
            (ControlKind::Choice, PointerButton::Primary | PointerButton::Secondary) => {
                let forward = button == PointerButton::Primary;
                out.push(param_cmd(
                    &key,
                    p.key,
                    widget::cycle_choice(p, value, forward),
                    None,
                ));
            }
            _ => {}
        }
    }

    fn cable_down(&mut self, i: usize, slot: usize, rp: Point, doc: &RackDoc) {
        let Some(g) = self.socket_geo(i, slot).cloned() else {
            return;
        };
        let mut anchor = g.end.clone();
        let mut detached = None;
        if g.end.direction == Direction::Input
            && let Some(c) = doc.cables.iter().find(|c| c.to == g.end.at)
            && let Some(from) = self.sockets.get(&c.from)
        {
            anchor = from.end.clone();
            detached = Some(c.clone());
        }
        self.hover_cable = None;
        self.drag = Drag::Cable {
            anchor,
            pos: rp,
            detached,
        };
    }

    fn pointer_up(
        &mut self,
        pos: Point,
        modifiers: Modifiers,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.pointer = pos;
        // A device drop is worked out in the layout of the drag (which may show the new-column
        // zone), so that drag ends after it.
        let drag = if matches!(self.drag, Drag::Move { .. } | Drag::Browser { .. }) {
            self.drag.clone()
        } else {
            std::mem::replace(&mut self.drag, Drag::None)
        };
        self.repaint = true;
        match drag {
            Drag::None | Drag::Pan { .. } | Drag::Navigator { .. } => {}
            Drag::Pad { device, key } => out.push(audition_off(device, key)),
            Drag::Sens { .. } => {}
            Drag::Eq(_) => out.push(UiAction::EndMerge),
            Drag::Drum(d) => self.drum_up(&d, out),
            Drag::Value { .. } => {
                self.tree.release_capture();
                out.push(UiAction::EndMerge);
            }
            Drag::Cable {
                anchor, detached, ..
            } => {
                self.drag = Drag::Cable {
                    anchor: anchor.clone(),
                    pos: Point::ZERO,
                    detached: detached.clone(),
                };
                let target = self.drop_target().map(|g| g.end.clone());
                self.drag = Drag::None;
                self.cable_up(&anchor, target, detached, modifiers, doc, out);
            }
            Drag::Move {
                dev,
                start,
                at,
                collapse,
            } => {
                // Released over the navigator the move is dropped, not applied.
                if (at - start).hypot() > MOVE_THRESHOLD && !self.in_navigator(pos) {
                    self.move_drop(dev, doc, modifiers, out);
                } else if collapse {
                    self.collapse_to(dev, out);
                }
            }
            Drag::Browser {
                row, active: true, ..
            } => self.browser_drop(&row, doc, registry, modifiers, out),
            Drag::Browser { .. } => {}
        }
        self.drag = Drag::None;
        // A drag may have shown the new-column zone: the range is the columns again.
        self.clamp_scroll();
        self.update_hover(pos);
    }

    fn step_control(&self, i: usize, slot: usize, notches: f64, fine: bool) -> Option<UiAction> {
        let (p, kind, value) = self.param_of(i, slot)?;
        if self.driven_by(i, p.key).is_some() {
            return None;
        }
        let new = match kind {
            ControlKind::Knob | ControlKind::Fader => widget::wheel_value(p, value, notches, fine),
            ControlKind::Choice => widget::cycle_choice(p, value, notches > 0.0),
            ControlKind::Toggle => widget::toggled(p, value),
            ControlKind::Display => return None,
        };
        (new.to_bits() != value.to_bits())
            .then(|| param_cmd(&self.devices[i].key, p.key, new, None))
    }

    fn wheel(
        &mut self,
        delta: Vec2,
        pos: Point,
        modifiers: Modifiers,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.sc_menu = None;
        self.choice_menu = None;
        self.macro_menu = None;
        self.plugin_menu = None;
        if self.navigator_wheel(delta, pos, modifiers) {
            return;
        }
        if self.in_browser(pos) {
            let rows = self.browser.list(registry).len() as f64;
            let max = (rows * 36.0 - (self.size.height - 80.0)).max(0.0);
            self.browser.scroll = (self.browser.scroll - delta.y * SCROLL_STEP).clamp(0.0, max);
            self.repaint = true;
            return;
        }
        if modifiers.command() {
            self.zoom_at(self.zoom * ZOOM_STEP.powf(delta.y), pos);
            return;
        }
        if matches!(self.drag, Drag::None)
            && let Some(Hit::EqPlot(i)) = self.hit(pos)
            && self.eq_wheel(
                i,
                self.view_to_rack(pos),
                delta.y.signum(),
                modifiers.shift,
                out,
            )
        {
            return;
        }
        if matches!(self.drag, Drag::None)
            && let Some(Hit::Preset(i, _) | Hit::PresetGrid(i)) = self.hit(pos)
        {
            out.extend(self.preset_step(i, delta.y.signum()));
            return;
        }
        if matches!(self.drag, Drag::None)
            && let Some(Hit::Control(i, slot)) = self.hit(pos)
        {
            if let Some(a) =
                self.step_control(i, slot, delta.y.signum(), self.fine_adjust(modifiers))
            {
                out.push(a);
            }
            return;
        }
        self.wheel_scroll(delta, modifiers);
        self.update_hover(pos);
    }

    /// Scrolls the rack by a wheel delta (Shift turns the vertical wheel sideways).
    fn wheel_scroll(&mut self, delta: Vec2, modifiers: Modifiers) {
        let d = if modifiers.shift && delta.x.abs() < 1e-9 {
            Vec2::new(-delta.y, 0.0)
        } else {
            -delta
        };
        self.scroll_by(d * SCROLL_STEP);
    }

    /// Where a device added from the browser goes: below the selection, else at the end.
    fn add_index(&self, doc: &RackDoc) -> usize {
        self.selected
            .as_ref()
            .and_then(|k| doc.devices.iter().position(|d| &d.key == k))
            .map_or(doc.devices.len(), |i| i + 1)
    }

    /// The action the browser's add (Enter on a device row) emits for the built-in device
    /// `desc`: below the selection, else at the end, through the creation chain for an
    /// instrument or an effect. Top level only: a container is added to with
    /// `ContainerCommand::AddInner`. For callers that hold a device by type, such as the
    /// library's device rows.
    pub fn add_device_action_for(
        &self,
        desc: &atomik_device_sdk::DeviceDescriptor,
        doc: &RackDoc,
        modifiers: Modifiers,
    ) -> UiAction {
        self.add_device_action(desc.type_id, desc.kind, self.add_index(doc), modifiers)
    }

    fn add_device_action(
        &self,
        type_id: &str,
        kind: atomik_device_sdk::DeviceKind,
        index: usize,
        modifiers: Modifiers,
    ) -> UiAction {
        // A note player is created on the stack of the selection (Shift adds it unrouted).
        if self.scope.is_empty() && crate::browser::is_player(type_id) {
            return self.create_player_action(type_id, modifiers);
        }
        // At the top level an instrument or an effect is created through the creation chain
        //; inside a container, and for every other device, it is a plain add.
        match self.creation_of(kind) {
            Some(create) if self.scope.is_empty() => self.create_action(
                CreateSource::Type(type_id.to_string()),
                create,
                index,
                modifiers,
            ),
            _ => UiAction::Command(Command::AddDevice {
                index,
                type_id: type_id.to_string(),
                key: None,
            }),
        }
    }

    /// Which creation command a device kind goes through (a utility only in the effect create
    /// browser).
    fn creation_of(&self, kind: atomik_device_sdk::DeviceKind) -> Option<CreateKind> {
        use atomik_device_sdk::DeviceKind;
        match kind {
            DeviceKind::Instrument => Some(CreateKind::Instrument),
            DeviceKind::Effect => Some(CreateKind::Effect),
            DeviceKind::Utility if self.browser.create == Some(CreateKind::Effect) => {
                Some(CreateKind::Effect)
            }
            _ => None,
        }
    }

    /// The creation command of the browser: route = the setting, inverted by Shift; Alt skips
    /// the track. An effect is routed from the selected device.
    fn create_action(
        &self,
        source: CreateSource,
        kind: CreateKind,
        index: usize,
        modifiers: Modifiers,
    ) -> UiAction {
        let options = CreateOptions {
            route: self.auto_route != modifiers.shift,
            skip_track: modifiers.alt,
            disarm: Vec::new(),
        };
        UiAction::Command(Command::Workflow(match kind {
            CreateKind::Instrument => WorkflowCommand::CreateInstrument {
                index,
                source,
                key: None,
                options,
            },
            CreateKind::Player => match &source {
                CreateSource::Type(type_id) => {
                    return self.create_player_action(type_id, modifiers);
                }
                CreateSource::Plugin { .. } => WorkflowCommand::CreateEffect {
                    source,
                    target: CreateTarget::Nothing,
                    key: None,
                    options,
                },
            },
            CreateKind::Effect => WorkflowCommand::CreateEffect {
                source,
                target: self
                    .selected
                    .clone()
                    .map_or(CreateTarget::Nothing, CreateTarget::Selected),
                key: None,
                options,
            },
        }))
    }

    /// The creation command of a note player: the stack of the selected device, route = the
    /// setting, inverted by Shift.
    fn create_player_action(&self, type_id: &str, modifiers: Modifiers) -> UiAction {
        UiAction::Command(Command::Workflow(WorkflowCommand::CreatePlayer {
            type_id: type_id.to_string(),
            target: self
                .selected
                .clone()
                .map_or(CreateTarget::Nothing, CreateTarget::Selected),
            options: CreateOptions {
                route: self.auto_route != modifiers.shift,
                skip_track: false,
                disarm: Vec::new(),
            },
        }))
    }

    /// The action that adds a browser row: a device type, or a plugin (`PluginCommand::Add`;
    /// inside a scope the command is lifted into the container on the way out).
    fn add_row_action(
        &self,
        row: &crate::browser::BrowserRow,
        doc: &RackDoc,
        modifiers: Modifiers,
    ) -> Option<UiAction> {
        self.add_row_action_at(row, self.add_index(doc), modifiers)
    }

    /// [`Self::add_row_action`] at rack index `index` (the flat index of the add commands).
    pub(super) fn add_row_action_at(
        &self,
        row: &crate::browser::BrowserRow,
        index: usize,
        modifiers: Modifiers,
    ) -> Option<UiAction> {
        match row {
            crate::browser::BrowserRow::Device(d) => {
                Some(self.add_device_action(d.type_id, d.kind, index, modifiers))
            }
            crate::browser::BrowserRow::Plugin(n) => {
                let e = self.plugin_catalog.get(*n)?;
                let create = match e.kind {
                    atomik_device_sdk::PluginKindHint::Instrument => Some(CreateKind::Instrument),
                    atomik_device_sdk::PluginKindHint::Effect => Some(CreateKind::Effect),
                    _ => None,
                };
                if self.scope.is_empty()
                    && let Some(kind) = create
                {
                    return Some(self.create_action(
                        CreateSource::Plugin {
                            plugin: e.plugin.clone(),
                            layout: e.layout.clone(),
                            path_hint: e.path.clone(),
                        },
                        kind,
                        index,
                        modifiers,
                    ));
                }
                Some(UiAction::Command(Command::Plugin(
                    atomik_model::PluginCommand::Add {
                        index,
                        container: None,
                        plugin: e.plugin.clone(),
                        layout: e.layout.clone(),
                        path_hint: e.path.clone(),
                        key: None,
                        auto_split: false,
                    },
                )))
            }
            _ => None,
        }
    }

    fn key(
        &mut self,
        key: &Key,
        modifiers: Modifiers,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        self.context_slot = None;
        // Chords with the primary modifier are commands (undo, duplicate, ...): the shell
        // resolves them through the keymap and offers them to `command`.
        if modifiers.command() {
            return;
        }
        if self.rename.is_some() {
            self.rename_key_press(key, out);
            return;
        }
        // Escape drops a device being dragged out of the browser, whatever has the focus.
        if matches!(self.drag, Drag::Browser { .. }) && matches!(key, Key::Escape) {
            self.drag = Drag::None;
            self.tick_clock = None;
            self.clamp_scroll();
            return;
        }
        if self.browser.visible && self.browser.focused {
            let rows = self.browser.list(registry);
            match key {
                Key::Char(c) => {
                    self.browser.query.push(*c);
                    self.browser.query_changed(registry);
                }
                Key::Space => {
                    self.browser.query.push(' ');
                    self.browser.query_changed(registry);
                }
                Key::Backspace => {
                    self.browser.query.pop();
                    self.browser.query_changed(registry);
                }
                // Create mode: Esc cancels and closes, nothing is removed.
                Key::Escape if self.browser.create.is_some() => self.set_browser_visible(false),
                Key::Escape if self.browser.query.is_empty() => self.browser.focused = false,
                Key::Escape => {
                    self.browser.query.clear();
                    self.browser.query_changed(registry);
                }
                Key::Down if !rows.is_empty() => {
                    self.browser.highlighted =
                        Browser::next_selectable(&rows, self.browser.highlighted);
                }
                Key::Up => {
                    self.browser.highlighted =
                        Browser::prev_selectable(&rows, self.browser.highlighted);
                }
                Key::Enter => {
                    if let Some(row) = rows.get(self.browser.highlighted)
                        && row.selectable()
                    {
                        out.extend(self.add_row_action(row, doc, modifiers));
                        if self.browser.create.is_some() && !self.browser.keep_open {
                            self.set_browser_visible(false);
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        let focused = match self.tree.focused().map(|n| *self.tree.data(n)) {
            Some(Hit::Control(i, s)) => Some((i, s)),
            _ => None,
        };
        let fine = self.fine_adjust(modifiers);
        let grid = match self.tree.focused().map(|n| *self.tree.data(n)) {
            Some(Hit::PresetGrid(i)) => Some(i),
            _ => None,
        };
        match key {
            Key::Escape if self.sc_menu.take().is_some() => {}
            Key::Escape if self.choice_menu.take().is_some() => {}
            Key::Escape if self.macro_menu.take().is_some() => {}
            Key::Escape if self.plugin_menu.take().is_some() => {}
            Key::Escape if self.escape_goes_up() => self.go_up(1),
            // With nothing to cancel or unfocus, Escape clears the selection.
            Key::Escape if matches!(self.drag, Drag::None) && self.tree.focused().is_none() => {
                self.clear_selection_by_click();
            }
            // A cable drag ends with nothing changed: a cable picked up from an input stays.
            Key::Escape if matches!(self.drag, Drag::Cable { .. }) => {
                self.drag = Drag::None;
                self.update_hover(self.pointer);
            }
            Key::Escape => self.cancel_drag(out),
            Key::Tab => {
                if modifiers.shift {
                    self.tree.focus_prev();
                } else {
                    self.tree.focus_next();
                }
            }
            // Delete is the `edit.delete` command; Backspace with nothing selected leaves the
            // container scope.
            Key::Backspace if self.selected.is_none() && !self.scope.is_empty() => {
                self.go_up(1);
            }
            Key::Up | Key::Right | Key::Down | Key::Left | Key::Home | Key::End
                if grid.is_some() =>
            {
                if let Some(i) = grid {
                    out.extend(self.preset_key(i, key));
                }
            }
            Key::Up | Key::Right | Key::Down | Key::Left | Key::PageUp | Key::PageDown => {
                if let Some((i, s)) = focused {
                    let n = match key {
                        Key::Up | Key::Right => 1.0,
                        Key::Down | Key::Left => -1.0,
                        Key::PageUp => 10.0,
                        _ => -10.0,
                    };
                    out.extend(self.step_control(i, s, n, fine));
                } else if matches!(key, Key::PageUp | Key::PageDown) {
                    let dir = if *key == Key::PageUp { -1.0 } else { 1.0 };
                    self.scroll_by(Vec2::new(0.0, dir * self.size.height * 0.8));
                }
            }
            Key::Space | Key::Enter => {
                if let Some((i, s)) = focused
                    && let Some((_, kind, _)) = self.param_of(i, s)
                    && matches!(kind, ControlKind::Toggle | ControlKind::Choice)
                {
                    out.extend(self.step_control(i, s, 1.0, false));
                }
            }
            Key::Home => self.set_scroll(0.0),
            Key::End => self.set_scroll(f64::MAX),
            _ => {}
        }
    }

    /// Runs a registry command the shell offers to this view: select all, delete, duplicate,
    /// disconnect and the neighbour keys work on the multi-selection; the device
    /// clipboard is the app's (it needs the document and the clipboard holder). Anything else,
    /// or nothing selected, is [`CommandOutcome::Declined`].
    pub fn command(
        &mut self,
        id: &str,
        _arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        // The selection holds keys local to the scope: work on the scope's view of the
        // document and lift the actions out, as `handle` does.
        let view = self.prepare(doc, registry);
        self.sync_in(&view, registry);
        match self.selection_command(id, &view, registry) {
            Some(outcome) => outcome,
            None => CommandOutcome::Declined,
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Value {
                dev,
                slot,
                merge,
                start,
                value,
                ..
            } => {
                self.tree.release_capture();
                if start.to_bits() != value.to_bits()
                    && let Some((p, _, _)) = self.param_of(dev, slot)
                {
                    out.push(param_cmd(&self.devices[dev].key, p.key, start, Some(merge)));
                }
                out.push(UiAction::EndMerge);
            }
            Drag::None => self.tree.set_focus(None),
            Drag::Pad { device, key } => out.push(audition_off(device, key)),
            Drag::Sens { dev, start, .. } => self.store_sens(dev, start),
            Drag::Eq(e) => self.eq_cancel(&e, out),
            Drag::Drum(d) => self.drum_cancel(&d, out),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the whole view. The canvas receives **physical pixels**: everything is scaled by
    /// the scale factor. Panels are cached per device; cables are an overlay.
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        let view = self.prepare(doc, registry);
        self.paint_in(canvas, &view, registry);
    }

    fn paint_in(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync_in(doc, registry);
        let (w, h) = (self.size.width, self.size.height);
        canvas.push_transform(Affine::scale(self.scale));
        paint::background(canvas, &self.theme, Rect::new(0.0, 0.0, w, h));
        let area = self.rack_area();
        if !self.scope.is_empty() {
            self.paint_breadcrumb(canvas);
        }
        canvas.push_clip(&area.into());
        let origin = self.origin();
        canvas.push_transform(Affine::translate(origin.to_vec2()) * Affine::scale(self.zoom));
        let top = self.view_to_rack(Point::new(area.x0, area.y0)).y;
        let bottom = self.view_to_rack(Point::new(area.x0, h)).y;
        let (rail_top, rail_bottom) = (top.min(0.0) - 40.0, bottom.max(self.content_height) + 40.0);
        for column in 0..self.columns {
            if column == 0 {
                paint::rails(canvas, &self.theme, rail_top, rail_bottom);
            } else {
                canvas.push_transform(Affine::translate((columns::column_left(column), 0.0)));
                paint::rails(canvas, &self.theme, rail_top, rail_bottom);
                canvas.pop_transform();
            }
        }
        let side = self.flip as usize;
        for i in 0..self.devices.len() {
            let d = &self.devices[i];
            if d.y + d.height < top || d.y > bottom {
                continue;
            }
            let stale = d.lists[side]
                .as_ref()
                .is_none_or(|l| l.revision() != d.revision);
            if stale {
                let mut list = DisplayList::cached(d.cache_id * 2 + side as u64, d.revision);
                let info = PanelInfo {
                    name: &d.name,
                    editing_name: self
                        .rename
                        .as_ref()
                        .filter(|r| r.dev == i && r.kind == RenameKind::Device)
                        .map(|r| r.text.as_str()),
                    caption: &d.caption,
                    accent: d.accent,
                    desc: d.desc.as_ref(),
                    device: d.device.as_ref(),
                    spec: &d.spec,
                    side: self.flip,
                    folded: d.folded,
                    height: d.height,
                    patch_browse: self.patch_browse.as_ref() == Some(&d.key),
                    seed: paint::seed_of(&d.key.0),
                    pad: d.pad,
                    pad_names: &d.pad_names,
                    pad_cabled: d.pad_cabled[usize::from(d.pad - 1)],
                    sensitivity: d.sens,
                    fx_meters: &d.meters,
                    analyser: &d.analyser,
                    sc_source: d.sc.as_deref(),
                    fx_cv: d.cv,
                    fx_phase: (f64::from(d.anim) * 0.03).rem_euclid(1.0) as f32,
                    player: PlayerInfo {
                        edit: d.edit,
                        meters: &d.meters,
                        page: d.page,
                        fx_mode: d.fx_mode,
                        pad_names: &d.pad_names,
                        scope: d.spec.modsrc.is_some().then_some(&d.scope),
                        rename: self
                            .rename
                            .as_ref()
                            .filter(|r| r.dev == i && r.kind == RenameKind::Pattern)
                            .map(|r| r.text.as_str()),
                    },
                    plugin: PluginInfo {
                        status: &d.plugin_status,
                        meters: &d.meters,
                        has_channel: d.has_channel,
                    },
                    container: ContainerInfo {
                        rename: self
                            .rename
                            .as_ref()
                            .filter(|r| r.dev == i && r.kind == RenameKind::Macro)
                            .map(|r| (usize::from(r.index), r.text.as_str())),
                        has_channel: d.has_channel,
                    },
                    driven: &d.driven,
                    automated: &d.automated,
                    io: self.io.info(),
                    master: d.master.info(),
                    from_clip: self.clip_scale.is_some(),
                    grain: crate::granular_panel::GrainInfo {
                        meters: &d.meters,
                        sample: d.sample.as_ref(),
                        peaks: d
                            .device
                            .as_ref()
                            .filter(|_| d.spec.granular.is_some())
                            .and_then(|dev| crate::granular_panel::shown_zone(dev, &d.meters))
                            .and_then(|z| self.sample_peaks.get(&z.asset))
                            .map(|p| &**p),
                    },
                };
                paint::record_panel(&mut list, &self.theme, &info);
                self.devices[i].lists[side] = Some(list);
            }
            let d = &self.devices[i];
            canvas.push_transform(Affine::translate(d.offset()));
            if let Some(list) = &d.lists[side] {
                canvas.draw_list(list);
            }
            canvas.pop_transform();
        }
        self.paint_overlays(canvas, doc, top, bottom);
        self.paint_midi_marks(canvas, top, bottom);
        canvas.pop_transform();
        canvas.pop_clip();
        self.paint_navigator(canvas);
        self.paint_big_meter(canvas);
        if let Some(t) = self.tooltip() {
            paint::tooltip(canvas, &self.theme, self.pointer, &t);
        }
        if let Some(t) = &self.toast {
            paint::toast(canvas, &self.theme, w, h, t);
        }
        if self.browser.visible {
            let send = self.browser_send_entry(doc);
            self.browser.paint(canvas, &self.theme, registry, h, send);
        }
        self.paint_browser_ghost(canvas);
        canvas.pop_transform();
        self.tree.clear_dirty();
        self.repaint = false;
    }

    /// The marks of the mapped controls of the front panels in view.
    fn paint_midi_marks(&self, c: &mut dyn Canvas, top: f64, bottom: f64) {
        if self.flip != Flip::Front || self.midi.is_empty() {
            return;
        }
        for d in &self.devices {
            if d.folded || d.y + d.height < top || d.y > bottom {
                continue;
            }
            let Some(desc) = d.desc else { continue };
            for pc in &d.spec.front {
                let Some(p) = desc.params.get(d.spec.param_index(&desc, pc, d.pad)) else {
                    continue;
                };
                self.midi
                    .paint_control(c, &self.theme, &d.key, p.key, pc.rect + d.offset());
            }
        }
    }

    fn paint_overlays(&self, c: &mut dyn Canvas, doc: &RackDoc, top: f64, bottom: f64) {
        let theme = &self.theme;
        // Every selected device is outlined; the primary one (the editors and the live keyboard
        // follow it) is drawn solid, the others lighter.
        for key in &self.selection {
            if let Some(i) = self.device_index(key) {
                let primary = self.selected.as_ref() == Some(key) || self.selection.len() == 1;
                let color = if primary {
                    theme.selection
                } else {
                    theme.selection.with_alpha(0.55)
                };
                paint::outline(c, self.devices[i].rect().inset(-1.0), 3.0, 2.0, color);
            }
        }
        // The output display of the channels and the master, and the lock of the master.
        for d in self.devices.iter().skip(1) {
            if d.y + d.height < top || d.y > bottom {
                continue;
            }
            let plate = paint::plate_rect(d.height, d.folded) + d.offset();
            if let Some(o) = self.outputs.get(&d.key) {
                paint::output_display(c, theme, plate, d.folded, &o.text, o.warn);
            }
            if let Some(pct) = self.cpu.get(&d.key) {
                paint::cpu_readout(c, theme, plate, d.folded, *pct);
            }
            if self.locked.as_ref() == Some(&d.key) {
                let at = if d.folded {
                    Point::new(plate.x0 + 112.0, plate.center().y - 1.0)
                } else {
                    Point::new(plate.x1 - 10.0, plate.y0 + 13.0)
                };
                paint::lock_icon(c, theme, at);
            }
        }
        if let Some(n) = self.tree.focused() {
            paint::outline(c, self.tree.rect(n).inset(1.0), 4.0, 1.5, theme.focus);
        }
        if let Some(i) = self.drop_hover {
            let r = self.devices[i].rect();
            paint::fill_rect(c, r, theme.highlight.with_alpha(0.12));
            paint::outline(c, r.inset(-1.0), 3.0, 2.5, theme.highlight);
        }
        if let Some((i, pad)) = self.drop_pad
            && let Some(dr) = self.devices[i].spec.drum
        {
            let r = autolayout::drum_pad_rect(dr.pads, pad) + self.devices[i].offset();
            paint::fill_rect(c, r, theme.highlight.with_alpha(0.3));
            paint::outline(c, r.inset(-1.0), 3.0, 2.5, theme.highlight);
        }
        if self.flip == Flip::Back {
            if let Drag::Cable { anchor, .. } = &self.drag {
                for g in self.sockets.values() {
                    if !g.visible || g.pos.y < top - 20.0 || g.pos.y > bottom + 20.0 {
                        continue;
                    }
                    let ring = match self.socket_highlight(&g.end.at) {
                        SocketHighlight::Source | SocketHighlight::Target => Ring::Strong,
                        SocketHighlight::Compatible => Ring::Highlight,
                        SocketHighlight::Incompatible => Ring::Dim,
                        _ => continue,
                    };
                    let _ = anchor;
                    paint::socket_ring(c, theme, g.pos, ring);
                }
            }
            // CV activity: a lit disc on both jacks of every active CV cable (also while the
            // cables themselves are hidden); the cable glows below.
            for cable in self.cv_activity.keys() {
                let step = self.cv_activity_step(cable);
                if step == 0 {
                    continue;
                }
                for end in [&cable.from, &cable.to] {
                    if let Some(g) = self.sockets.get(end)
                        && g.visible
                        && g.pos.y >= top - 20.0
                        && g.pos.y <= bottom + 20.0
                    {
                        paint::jack_activity(c, theme, g.pos, g.end.kind, step);
                    }
                }
            }
            for cable in self.visible_cables(doc) {
                let Some(curve) = self.cable_curve(&cable) else {
                    continue;
                };
                let bb = vello::kurbo::Shape::bounding_box(&curve);
                if bb.y1 < top - 20.0 || bb.y0 > bottom + 20.0 {
                    continue;
                }
                let kind = self.sockets.get(&cable.from).map(|g| g.end.kind);
                let color = kind.map_or(theme.cable_audio, |k| {
                    theme.cable_role_color(cables::role_of(&cable, k))
                });
                let look = if self.hover_cable.as_ref() == Some(&cable) {
                    CableLook::Hovered
                } else {
                    CableLook::Normal
                };
                paint::cable(c, theme, curve, color, look, true);
                let step = self.cv_activity_step(&cable);
                if step > 0 {
                    paint::cable_activity(c, curve, color, step);
                }
            }
            // A dot on both jacks of a cable the display mode hides, in its role colour.
            if matches!(
                self.cable_visibility,
                CableVisibility::HoverOnly
                    | CableVisibility::SelectedOnly
                    | CableVisibility::HideAutoRouted
            ) {
                for cable in self.cables_in_scope(doc) {
                    let (Some(a), Some(b)) =
                        (self.sockets.get(&cable.from), self.sockets.get(&cable.to))
                    else {
                        continue;
                    };
                    if (!self.show_links && self.links.contains(cable))
                        || self.is_detached(cable)
                        || self.mode_shows(cable)
                    {
                        continue;
                    }
                    let color = theme.cable_role_color(cables::role_of(cable, a.end.kind));
                    for g in [a, b] {
                        if g.visible && g.pos.y >= top - 20.0 && g.pos.y <= bottom + 20.0 {
                            c.fill_with(Circle::new(g.pos, 3.5), color.with_alpha(0.85));
                        }
                    }
                }
            }
            if let Some(hc) = &self.hover_cable {
                for end in [&hc.from, &hc.to] {
                    if let Some(g) = self.sockets.get(end) {
                        paint::socket_ring(c, theme, g.pos, Ring::Highlight);
                    }
                }
            }
            if let Drag::Cable { anchor, pos, .. } = &self.drag {
                let start = self.sockets.get(&anchor.at).map_or(*pos, |g| g.pos);
                let end = self.drop_target().map_or(*pos, |g| g.pos);
                let curve = cables::cable_curve(start, end);
                paint::cable(
                    c,
                    theme,
                    curve,
                    theme.cable_color(anchor.kind),
                    CableLook::Dragging,
                    true,
                );
            }
        }
        self.paint_sc_menu(c);
        self.paint_choice_menu(c);
        self.paint_macro_menu(c);
        self.paint_plugin_menu(c);
        self.paint_drag_feedback(c);
    }
}

trait CircleExt {
    fn bounding_box_rect(&self) -> Rect;
}

impl CircleExt for Circle {
    fn bounding_box_rect(&self) -> Rect {
        vello::kurbo::Shape::bounding_box(self)
    }
}

/// The number of patterns a step sequencer view shows (a device without stored `steps` has the
/// default single pattern).
fn pattern_count(d: &RackDevice) -> usize {
    let n = match (&d.steps, &d.drums) {
        (Some(s), _) => s.patterns.len(),
        (None, Some(x)) => x.patterns.len(),
        (None, None) => 1,
    };
    n.max(1)
}

/// Velocity (7-bit) of the audition when a drum pad is clicked: fixed, not read from the click
/// height.
pub const PAD_AUDITION_VELOCITY: u8 = 100;

fn audition_off(device: DeviceKey, key: u8) -> UiAction {
    UiAction::AuditionVelocity {
        device,
        key,
        velocity: PAD_AUDITION_VELOCITY,
        on: false,
    }
}

/// The short name of a file: its name without the extension.
fn file_stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

/// For each pad of drum machine `key`, whether `padN_out_l` or `padN_out_r` is cabled.
/// Whether the Transform and the Dry/Wet CV inputs of a Character have a cable.
fn cv_cabled_of(key: &DeviceKey, cables: &[Cable]) -> [bool; 2] {
    let cabled = |socket: &str| {
        cables
            .iter()
            .any(|c| &c.to.device == key && c.to.socket == socket)
    };
    [cabled("transform_cv_in"), cabled("mix_cv_in")]
}

fn pad_cabled_of(key: &DeviceKey, cables: &[Cable]) -> [bool; 16] {
    let mut out = [false; 16];
    for c in cables.iter().filter(|c| &c.from.device == key) {
        let pad = c
            .from
            .socket
            .strip_prefix("pad")
            .and_then(|r| r.split_once("_out_"))
            .and_then(|(n, _)| n.parse::<usize>().ok());
        if let Some(n) = pad.filter(|n| (1..=16).contains(n)) {
            out[n - 1] = true;
        }
    }
    out
}

/// The 16 pad labels of a drum machine panel: the file name (no extension) of the first zone
/// of the pad, `missing` if its asset is not in the pool, and empty for a pad without zones.
fn drum_pad_names(d: &RackDevice, doc: &RackDoc) -> Vec<String> {
    (1..=atomik_model::PAD_COUNT)
        .map(|pad| match atomik_model::pad_zones(&d.zones, pad).next() {
            None => String::new(),
            Some(zi) => doc
                .assets
                .get(&d.zones[zi].asset)
                .map_or_else(|| "missing".to_string(), |m| file_stem(&m.original_name)),
        })
        .collect()
}

/// The 16 pad names of the drum machine a drum generator's `notes_out` reaches, directly or
/// through other note players (the first such drum machine found, following the cables
/// breadth first); empty when it reaches none.
fn linked_pad_names(generator: &RackDevice, doc: &RackDoc) -> Vec<String> {
    let mut seen = vec![generator.key.clone()];
    let mut frontier = vec![generator.key.clone()];
    for _ in 0..8 {
        let mut next = Vec::new();
        for from in &frontier {
            for c in doc
                .cables
                .iter()
                .filter(|c| &c.from.device == from && c.from.socket == "notes_out")
            {
                let to = &c.to.device;
                if seen.contains(to) {
                    continue;
                }
                seen.push(to.clone());
                let Some(t) = doc.device(to) else {
                    continue;
                };
                if t.type_id == autolayout::DRUM_TYPE {
                    return drum_pad_names(t, doc);
                }
                if crate::browser::is_player(&t.type_id) {
                    next.push(to.clone());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Vec::new()
}
