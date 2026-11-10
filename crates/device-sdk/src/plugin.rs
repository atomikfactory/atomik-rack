//! Plain data for third-party plugin devices and the one place that
//! turns a scanned plugin layout into a [`DeviceDescriptor`].
//!
//! The SDK keeps one static [`DeviceDescriptor`] per device type. A plugin is a device type
//! that is only known at run time, so [`descriptor_for`] builds the descriptor once per
//! distinct (plugin, layout) and **interns** it: the descriptor, its param table, its socket
//! table and every string in them are leaked on purpose so the result is a `&'static`. The leak
//! is bounded: at most [`MAX_INTERNED_DESCRIPTORS`] distinct layouts per process, each with at
//! most [`MAX_PLUGIN_PARAMS`] params, and the same layout always gives the same pointer (a test
//! pins it), so rescanning, reopening a project or restarting the engine does not grow it.
//!
//! This module has no plugin-format code and no `unsafe`; the formats live in the backends.

use crate::descriptor::validate_parts;
use crate::{
    DescriptorError, Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

/// Upper bound of the plugin params a descriptor carries. A longer table is cut by the
/// scanner (and recorded there); [`descriptor_for`] refuses one.
pub const MAX_PLUGIN_PARAMS: usize = 2_048;

/// Upper bound of distinct interned plugin descriptors per process.
pub const MAX_INTERNED_DESCRIPTORS: usize = 4_096;

/// CV inputs of every plugin device (`cv_in_1` to `cv_in_8`).
pub const PLUGIN_CV_INPUTS: usize = 8;

/// Most extra stereo output pairs of a plugin device (`out_aux_1_l/r` to `out_aux_7_l/r`), so
/// a device has at most eight stereo output pairs with the main one.
pub const MAX_AUX_OUTPUTS: usize = 7;

/// Keys of the CV programmer's destination params, one per CV input: `cv.dest_1` to
/// `cv.dest_8`. The plain value is 0 for none, else the descriptor index of the plugin param
/// the CV drives (1 is the first plugin param; index 0 is `bypass`). The range is the same for
/// every plugin, so a value saved with the full param table is still in range when the device is
/// rebuilt from a compact saved layout; a value past the plugin's own params is no destination.
pub const CV_DEST_KEYS: [&str; PLUGIN_CV_INPUTS] = [
    "cv.dest_1",
    "cv.dest_2",
    "cv.dest_3",
    "cv.dest_4",
    "cv.dest_5",
    "cv.dest_6",
    "cv.dest_7",
    "cv.dest_8",
];

/// Keys of the CV programmer's amount params, one per CV input: `cv.amount_1` to
/// `cv.amount_8`, bipolar -1 to +1 of the destination's whole plain range per unit of CV.
pub const CV_AMOUNT_KEYS: [&str; PLUGIN_CV_INPUTS] = [
    "cv.amount_1",
    "cv.amount_2",
    "cv.amount_3",
    "cv.amount_4",
    "cv.amount_5",
    "cv.amount_6",
    "cv.amount_7",
    "cv.amount_8",
];

/// The key of the param that makes a plugin device send per-note expression as MPE (a lower zone
/// on channels 2 to 16) to a plugin that takes plain MIDI. 0 is off, 1 is on. It is the last
/// param of every plugin device, after the CV programmer's.
pub const MPE_SEND_KEY: &str = "mpe.send";

const CV_DEST_NAMES: [&str; PLUGIN_CV_INPUTS] = [
    "CV 1 dest",
    "CV 2 dest",
    "CV 3 dest",
    "CV 4 dest",
    "CV 5 dest",
    "CV 6 dest",
    "CV 7 dest",
    "CV 8 dest",
];

const CV_AMOUNT_NAMES: [&str; PLUGIN_CV_INPUTS] = [
    "CV 1", "CV 2", "CV 3", "CV 4", "CV 5", "CV 6", "CV 7", "CV 8",
];

/// Keys of the CV input sockets.
pub const CV_IN_KEYS: [&str; PLUGIN_CV_INPUTS] = [
    "cv_in_1", "cv_in_2", "cv_in_3", "cv_in_4", "cv_in_5", "cv_in_6", "cv_in_7", "cv_in_8",
];

const CV_IN_NAMES: [&str; PLUGIN_CV_INPUTS] = [
    "CV in 1", "CV in 2", "CV in 3", "CV in 4", "CV in 5", "CV in 6", "CV in 7", "CV in 8",
];

/// Keys of the aux output sockets, left and right of each pair.
pub const AUX_OUT_KEYS: [[&str; 2]; MAX_AUX_OUTPUTS] = [
    ["out_aux_1_l", "out_aux_1_r"],
    ["out_aux_2_l", "out_aux_2_r"],
    ["out_aux_3_l", "out_aux_3_r"],
    ["out_aux_4_l", "out_aux_4_r"],
    ["out_aux_5_l", "out_aux_5_r"],
    ["out_aux_6_l", "out_aux_6_r"],
    ["out_aux_7_l", "out_aux_7_r"],
];

const AUX_OUT_NAMES: [[&str; 2]; MAX_AUX_OUTPUTS] = [
    ["Out 2 L", "Out 2 R"],
    ["Out 3 L", "Out 3 R"],
    ["Out 4 L", "Out 4 R"],
    ["Out 5 L", "Out 5 R"],
    ["Out 6 L", "Out 6 R"],
    ["Out 7 L", "Out 7 R"],
    ["Out 8 L", "Out 8 R"],
];

/// Prefix of the type id of a CLAP plugin device.
pub const CLAP_PREFIX: &str = "clap.";
/// Prefix of the type id of a VST3 plugin device.
pub const VST3_PREFIX: &str = "vst3.";

/// The plugin format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PluginFormat {
    /// CLAP.
    Clap,
    /// VST3.
    Vst3,
}

impl PluginFormat {
    /// The lowercase name used in type ids and files: `clap` or `vst3`.
    pub fn name(self) -> &'static str {
        match self {
            PluginFormat::Clap => "clap",
            PluginFormat::Vst3 => "vst3",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            PluginFormat::Clap => CLAP_PREFIX,
            PluginFormat::Vst3 => VST3_PREFIX,
        }
    }
}

/// Which plugin a device is: the format and the plugin's own id (the CLAP id string, or the 32
/// lowercase hex digits of the VST3 class id).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginRef {
    /// The format.
    pub format: PluginFormat,
    /// The plugin id.
    pub id: String,
}

impl PluginRef {
    /// The registry type id: `clap.<id>` or `vst3.<id>`.
    pub fn type_id(&self) -> String {
        format!("{}{}", self.format.prefix(), self.id)
    }

    /// Parses a type id back (`None` for a built-in type or an empty id).
    pub fn from_type_id(type_id: &str) -> Option<Self> {
        [PluginFormat::Clap, PluginFormat::Vst3]
            .into_iter()
            .find_map(|format| {
                type_id
                    .strip_prefix(format.prefix())
                    .filter(|id| !id.is_empty())
                    .map(|id| Self {
                        format,
                        id: id.to_string(),
                    })
            })
    }
}

/// Whether `type_id` is in the reserved plugin namespace (`clap.` or `vst3.`).
pub fn is_plugin_type_id(type_id: &str) -> bool {
    type_id.starts_with(CLAP_PREFIX) || type_id.starts_with(VST3_PREFIX)
}

/// Channel layout of a main audio bus. Stereo is always negotiated; a mono bus is mapped by the
/// host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channels {
    /// One channel.
    Mono,
    /// Two channels.
    Stereo,
}

/// What the plugin says it is (CLAP features, VST3 subcategories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginKindHint {
    /// A synth or sampler.
    Instrument,
    /// An audio effect.
    Effect,
    /// A note effect (arpeggiator, transposer).
    NoteEffect,
}

/// One parameter of a plugin as the scanner reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginParamInfo {
    /// The plugin's own, stable 32-bit param id.
    pub id: u32,
    /// Display name.
    pub name: String,
    /// The module (group) path, empty for none.
    pub module: String,
    /// Minimum plain value (CLAP; VST3 params are normalised and ignore it).
    pub min: f64,
    /// Maximum plain value (CLAP; ignored for VST3).
    pub max: f64,
    /// Default: the plain value for CLAP, the normalised value for VST3.
    pub default: f64,
    /// Discrete param.
    pub stepped: bool,
    /// VST3 `stepCount` of a stepped param (the plain range is then `0..=steps`).
    pub steps: u32,
}

/// What the host needs to know about a plugin to build a device for it, without loading it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginLayout {
    /// Plugin name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Version string.
    pub version: String,
    /// The plugin's own category.
    pub kind_hint: PluginKindHint,
    /// Main audio input bus, if any.
    pub main_in: Option<Channels>,
    /// Main audio output bus, if any.
    pub main_out: Option<Channels>,
    /// A sidechain (aux input) bus exists.
    pub sidechain: bool,
    /// The plugin accepts notes.
    pub notes_in: bool,
    /// The plugin emits notes.
    pub notes_out: bool,
    /// Extra stereo output pairs after the main output (0 to [`MAX_AUX_OUTPUTS`]): each is a
    /// pair of `out_aux_<k>_l/r` sockets on the device. A mono aux bus is mapped by the host.
    pub aux_out: u8,
    /// The visible, writable params in the plugin's own order.
    pub params: Vec<PluginParamInfo>,
}

/// A plugin's opaque saved state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginStateBlob(pub Vec<u8>);

/// How a device is being run: live, or in an offline bounce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Real time (the live app, or a bounce that must match what was heard).
    Realtime,
    /// Offline render.
    Offline,
}

// ----- the layout hash ---------------------------------------------------------------------

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }

    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
}

fn channels_code(c: Option<Channels>) -> u64 {
    match c {
        None => 0,
        Some(Channels::Mono) => 1,
        Some(Channels::Stereo) => 2,
    }
}

impl PluginLayout {
    /// True when `other` has the same port shape: the same main buses, sidechain, note ports and
    /// aux outputs (what gives the device its sockets). Names, vendor, version and the param
    /// table are not compared: a plugin may rename or reshape its params per patch.
    pub fn same_ports(&self, other: &Self) -> bool {
        self.main_in == other.main_in
            && self.main_out == other.main_out
            && self.sidechain == other.sidechain
            && self.notes_in == other.notes_in
            && self.notes_out == other.notes_out
            && self.aux_out == other.aux_out
    }

    /// A deterministic 64-bit FNV-1a hash over every field, in order (no `std` hasher).
    pub fn layout_hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.str(&self.name);
        h.str(&self.vendor);
        h.str(&self.version);
        h.u64(match self.kind_hint {
            PluginKindHint::Instrument => 1,
            PluginKindHint::Effect => 2,
            PluginKindHint::NoteEffect => 3,
        });
        h.u64(channels_code(self.main_in));
        h.u64(channels_code(self.main_out));
        h.u64(u64::from(self.sidechain));
        h.u64(u64::from(self.notes_in));
        h.u64(u64::from(self.notes_out));
        if self.aux_out > 0 {
            h.u64(u64::from(self.aux_out));
        }
        h.u64(self.params.len() as u64);
        for p in &self.params {
            h.u64(u64::from(p.id));
            h.str(&p.name);
            h.str(&p.module);
            h.f64(p.min);
            h.f64(p.max);
            h.f64(p.default);
            h.u64(u64::from(p.stepped));
            h.u64(u64::from(p.steps));
        }
        h.0
    }
}

// ----- descriptor building -----------------------------------------------------------------

fn leak_str(s: &str) -> &'static str {
    #[cfg(test)]
    LEAKS.with(|n| n.set(n.get() + 1));
    Box::leak(s.to_string().into_boxed_str())
}

fn valid_plugin_id(r: &PluginRef) -> bool {
    match r.format {
        PluginFormat::Clap => {
            !r.id.is_empty()
                && r.id.len() <= 256
                && r.id.chars().all(|c| !c.is_whitespace() && !c.is_control())
        }
        PluginFormat::Vst3 => {
            r.id.len() == 32 && r.id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
    }
}

const BYPASS: ParamDescriptor = ParamDescriptor {
    id: ParamId(0),
    key: "bypass",
    name: "Bypass",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    taper: Taper::Linear,
    unit: Unit::Toggle,
    discrete: true,
};

/// `(min, max, default, discrete)` of one plugin param in its format's plain value.
fn plain_range(format: PluginFormat, p: &PluginParamInfo) -> Result<(f32, f32, f32, bool), ()> {
    match format {
        PluginFormat::Clap => {
            let (min, max) = (p.min as f32, p.max as f32);
            if !(min.is_finite() && max.is_finite() && min <= max) {
                return Err(());
            }
            let default = if p.default.is_nan() {
                min
            } else {
                (p.default as f32).clamp(min, max)
            };
            Ok((min, max, default, p.stepped))
        }
        PluginFormat::Vst3 => {
            let norm = if p.default.is_nan() {
                0.0
            } else {
                p.default.clamp(0.0, 1.0)
            };
            if p.stepped {
                if p.steps == 0 {
                    return Err(());
                }
                let steps = f64::from(p.steps);
                Ok((0.0, steps as f32, (norm * steps).round() as f32, true))
            } else {
                Ok((0.0, 1.0, norm as f32, false))
            }
        }
    }
}

// Counts, per thread, what `build_descriptor` leaks, so a test can pin that a refused layout
// leaks nothing.
#[cfg(test)]
thread_local! {
    pub(crate) static LEAKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn leaked<T: ?Sized>(value: Box<T>) -> &'static T {
    #[cfg(test)]
    LEAKS.with(|n| n.set(n.get() + 1));
    Box::leak(value)
}

fn build_descriptor(
    plugin: &PluginRef,
    layout: &PluginLayout,
) -> Result<&'static DeviceDescriptor, DescriptorError> {
    if !valid_plugin_id(plugin) {
        return Err(DescriptorError::InvalidPluginId(plugin.id.clone()));
    }
    if layout.params.len() > MAX_PLUGIN_PARAMS {
        return Err(DescriptorError::TooManyParams {
            count: layout.params.len(),
            max: MAX_PLUGIN_PARAMS,
        });
    }
    // Everything that can refuse the layout is decided before anything is leaked.
    let mut seen = BTreeSet::new();
    let mut ranges = Vec::with_capacity(layout.params.len());
    for p in &layout.params {
        if !seen.insert(p.id) {
            return Err(DescriptorError::DuplicatePluginParam(p.id));
        }
        ranges.push(
            plain_range(plugin.format, p)
                .map_err(|()| DescriptorError::InvalidPluginParam(p.id))?,
        );
    }

    let mut sockets: Vec<SocketDescriptor> = Vec::with_capacity(8);
    let mut push = |key: &'static str, name: &'static str, kind: SocketKind, direction| {
        sockets.push(SocketDescriptor {
            id: SocketId(sockets.len() as u16),
            key,
            name,
            kind,
            direction,
        });
    };
    if layout.main_in.is_some() {
        push("in_l", "In L", SocketKind::Audio, Direction::Input);
        push("in_r", "In R", SocketKind::Audio, Direction::Input);
    }
    if layout.sidechain {
        push(
            "sc_in_l",
            "Sidechain L",
            SocketKind::Audio,
            Direction::Input,
        );
        push(
            "sc_in_r",
            "Sidechain R",
            SocketKind::Audio,
            Direction::Input,
        );
    }
    if layout.notes_in {
        push("notes_in", "Notes in", SocketKind::Notes, Direction::Input);
    }
    if layout.main_out.is_some() {
        push("out_l", "Out L", SocketKind::Audio, Direction::Output);
        push("out_r", "Out R", SocketKind::Audio, Direction::Output);
    }
    if layout.notes_out {
        push(
            "notes_out",
            "Notes out",
            SocketKind::Notes,
            Direction::Output,
        );
    }
    if sockets.is_empty() {
        return Err(DescriptorError::NoSockets);
    }
    // Appended after the sockets above, so the ids of a project's older cables never move: the
    // CV inputs of the programmer (every plugin device has them), then the aux output pairs.
    let mut push = |key: &'static str, name: &'static str, kind: SocketKind, direction| {
        sockets.push(SocketDescriptor {
            id: SocketId(sockets.len() as u16),
            key,
            name,
            kind,
            direction,
        });
    };
    for k in 0..PLUGIN_CV_INPUTS {
        push(
            CV_IN_KEYS[k],
            CV_IN_NAMES[k],
            SocketKind::Cv,
            Direction::Input,
        );
    }
    for k in 0..usize::from(layout.aux_out).min(MAX_AUX_OUTPUTS) {
        for side in 0..2 {
            push(
                AUX_OUT_KEYS[k][side],
                AUX_OUT_NAMES[k][side],
                SocketKind::Audio,
                Direction::Output,
            );
        }
    }

    let mut params = Vec::with_capacity(layout.params.len() + 1);
    params.push(BYPASS);
    for (p, &(min, max, default, discrete)) in layout.params.iter().zip(&ranges) {
        let key = leak_str(&format!("p{}", p.id));
        let name = if p.name.is_empty() {
            key
        } else {
            leak_str(&p.name)
        };
        params.push(ParamDescriptor {
            id: ParamId(params.len() as u32),
            key,
            name,
            min,
            max,
            default,
            taper: Taper::Linear,
            unit: Unit::None,
            discrete,
        });
    }
    // The CV programmer: a destination and an amount per CV input, after the plugin params.
    for k in 0..PLUGIN_CV_INPUTS {
        params.push(ParamDescriptor {
            id: ParamId(params.len() as u32),
            key: CV_DEST_KEYS[k],
            name: CV_DEST_NAMES[k],
            min: 0.0,
            max: MAX_PLUGIN_PARAMS as f32,
            default: 0.0,
            taper: Taper::Linear,
            unit: Unit::None,
            discrete: true,
        });
        params.push(ParamDescriptor {
            id: ParamId(params.len() as u32),
            key: CV_AMOUNT_KEYS[k],
            name: CV_AMOUNT_NAMES[k],
            min: -1.0,
            max: 1.0,
            default: 0.0,
            taper: Taper::Linear,
            unit: Unit::None,
            discrete: false,
        });
    }
    // MPE out for a plugin that takes plain MIDI: the last param.
    params.push(ParamDescriptor {
        id: ParamId(params.len() as u32),
        key: MPE_SEND_KEY,
        name: "Send MPE (channels 2-16)",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    });
    // The tables are checked on the heap; only a valid descriptor has its tables leaked.
    validate_parts(&params, &sockets, &[])?;

    let has_audio = layout.main_in.is_some() || layout.main_out.is_some();
    let kind = if layout.notes_in
        && layout.main_out.is_some()
        && layout.kind_hint == PluginKindHint::Instrument
    {
        DeviceKind::Instrument
    } else if !has_audio {
        DeviceKind::Utility
    } else {
        DeviceKind::Effect
    };

    let descriptor = DeviceDescriptor {
        type_id: leak_str(&plugin.type_id()),
        name: leak_str(&layout.name),
        kind,
        version: 3,
        params: leaked(params.into_boxed_slice()),
        sockets: leaked(sockets.into_boxed_slice()),
        stages: &[],
    };
    Ok(leaked(Box::new(descriptor)))
}

type InternKey = (PluginFormat, String, u64);

#[derive(Default)]
struct InternState {
    entries: BTreeMap<InternKey, Vec<(PluginLayout, &'static DeviceDescriptor)>>,
    count: usize,
}

/// The table of interned descriptors. [`descriptor_for`] uses the process-wide one; tests build
/// their own with a small limit.
pub(crate) struct Interner {
    limit: usize,
    state: Mutex<InternState>,
}

impl Interner {
    pub(crate) const fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(InternState {
                entries: BTreeMap::new(),
                count: 0,
            }),
        }
    }

    /// Number of interned descriptors.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().count
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InternState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn descriptor_for(
        &self,
        plugin: &PluginRef,
        layout: &PluginLayout,
    ) -> Result<&'static DeviceDescriptor, DescriptorError> {
        let key = (plugin.format, plugin.id.clone(), layout.layout_hash());
        // One lock for lookup and insert, so two threads never intern one layout twice.
        let mut state = self.lock();
        // Known layouts are served without building (and so without re-validating) anything.
        if let Some(found) = state
            .entries
            .get(&key)
            .and_then(|v| v.iter().find(|(l, _)| l == layout))
        {
            return Ok(found.1);
        }
        if state.count >= self.limit {
            return Err(DescriptorError::InternLimit { max: self.limit });
        }
        let descriptor = build_descriptor(plugin, layout)?;
        state.count += 1;
        state
            .entries
            .entry(key)
            .or_default()
            .push((layout.clone(), descriptor));
        Ok(descriptor)
    }
}

static INTERNER: Interner = Interner::with_limit(MAX_INTERNED_DESCRIPTORS);

/// Builds (once per distinct plugin and layout) and interns the descriptor of a plugin device. Non-RT.
///
/// Sockets, in this order, each only when the plugin has the matching port: `in_l`, `in_r`,
/// `sc_in_l`, `sc_in_r`, `notes_in`, `out_l`, `out_r`, `notes_out`; then always the eight CV
/// inputs `cv_in_1` to `cv_in_8`, then the aux output pairs `out_aux_<k>_l/r` the layout has.
/// Params: `bypass` (id 0), `p<plugin id>` for every plugin param in the plugin's own order,
/// then the CV programmer's `cv.dest_<k>` and `cv.amount_<k>` for k = 1 to 8, then `mpe.send`.
///
/// # Errors
/// An invalid plugin id, more than [`MAX_PLUGIN_PARAMS`] params, a duplicate or non-finite
/// param, a layout with no socket, or more than [`MAX_INTERNED_DESCRIPTORS`] distinct layouts.
pub fn descriptor_for(
    plugin: &PluginRef,
    layout: &PluginLayout,
) -> Result<&'static DeviceDescriptor, DescriptorError> {
    INTERNER.descriptor_for(plugin, layout)
}

// ----- the stand-in device ------------------------------------------------------------------

/// The harmless stand-in of a plugin device: an instrument is silent, an
/// effect passes its dry main input to its main output, a note effect emits nothing. It is what
/// [`crate::Registry::create`] builds for a plugin type, and what the app inserts for a missing
/// plugin. Params are accepted and ignored (the document keeps them).
#[derive(Debug, Clone, Copy)]
pub struct PluginStandIn {
    descriptor: &'static DeviceDescriptor,
    /// `(input, output)` socket pairs of the dry path (effects only).
    dry: [Option<(SocketId, SocketId)>; 2],
}

impl PluginStandIn {
    /// A stand-in for the plugin type described by `descriptor`.
    pub fn new(descriptor: &'static DeviceDescriptor) -> Self {
        let pair = |i: &str, o: &str| {
            let (i, o) = (descriptor.socket_by_key(i)?, descriptor.socket_by_key(o)?);
            (descriptor.kind == DeviceKind::Effect).then_some((i.id, o.id))
        };
        Self {
            descriptor,
            dry: [pair("in_l", "out_l"), pair("in_r", "out_r")],
        }
    }
}

impl Device for PluginStandIn {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        self.descriptor
    }

    fn prepare(&mut self, _info: &PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, _id: ParamId, _plain: f32) {}

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for (input, output) in self.dry.iter().flatten() {
            let batch = *ctx.input(*input);
            *ctx.output(*output) = batch;
        }
    }
}

#[cfg(test)]
mod tests;
