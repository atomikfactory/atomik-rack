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
    let mut seen = BTreeSet::new();
    let mut params = Vec::with_capacity(layout.params.len() + 1);
    params.push(BYPASS);
    for p in &layout.params {
        if !seen.insert(p.id) {
            return Err(DescriptorError::DuplicatePluginParam(p.id));
        }
        let (min, max, default, discrete) = plain_range(plugin.format, p)
            .map_err(|()| DescriptorError::InvalidPluginParam(p.id))?;
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
        version: 1,
        params: Box::leak(params.into_boxed_slice()),
        sockets: Box::leak(sockets.into_boxed_slice()),
        stages: &[],
    };
    descriptor.validate()?;
    Ok(Box::leak(Box::new(descriptor)))
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
/// `sc_in_l`, `sc_in_r`, `notes_in`, `out_l`, `out_r`, `notes_out`. Params: `bypass` (id 0)
/// then `p<plugin id>` for every plugin param in the plugin's own order.
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

/// What `DeviceFactory::create` builds for a plugin type: a stand-in over a placeholder
/// descriptor. A `fn()` cannot carry the plugin's descriptor, so users that need the right
/// stand-in call [`crate::Registry::create`], which knows the type.
pub(crate) fn placeholder_stand_in() -> Box<dyn Device> {
    static PLACEHOLDER: DeviceDescriptor = DeviceDescriptor {
        type_id: "clap.atomik.placeholder",
        name: "Plugin",
        kind: DeviceKind::Utility,
        version: 1,
        params: &[],
        sockets: &[],
        stages: &[],
    };
    Box::new(PluginStandIn::new(&PLACEHOLDER))
}

#[cfg(test)]
mod tests;
