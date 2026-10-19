//! The controller half of a test plugin: `IEditController`, `IMidiMapping`, `IUnitInfo`,
//! `INoteExpressionController` and the editor view, over its own copy of the [`Dsp`] (it reads
//! the parameter table, converts values and keeps the normalised values).

use crate::api::{Dsp, Info, ParamSpec, PluginDef};
use crate::component::{
    MIDI_ALL_OFF, MIDI_BEND, MIDI_MOD, MIDI_PRESSURE, midi_param, read_message, send_message,
};
use crate::hooks::{Shared, command, find_by_number, find_by_ptr, probe};
use crate::util::{copy_wstr, norm_of, plain_of, read_all, write_all};
use std::ffi::{c_char, c_void};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use vst3::{Class, ComPtr, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One row of the controller's table.
#[derive(Clone)]
struct Entry {
    id: u32,
    title: String,
    unit: i32,
    steps: i32,
    default_norm: f64,
    flags: i32,
    spec: Option<ParamSpec>,
}

#[derive(Clone)]
struct Unit {
    id: i32,
    parent: i32,
    name: String,
}

struct Table {
    entries: Vec<Entry>,
    units: Vec<Unit>,
    values: Vec<f64>,
}

fn build_table(dsp: &dyn Dsp, midi: bool, old: Option<&Table>) -> Table {
    let mut units = vec![Unit {
        id: kRootUnitId,
        parent: kNoParentUnitId,
        name: "Root".to_string(),
    }];
    let mut entries = Vec::new();
    for spec in dsp.params() {
        // `A/B` makes a unit `A` under the root and a unit `B` under `A`.
        let mut parent = kRootUnitId;
        let mut path = String::new();
        for part in spec.module.split('/').filter(|p| !p.is_empty()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(part);
            let id = match units.iter().find(|u| u.parent == parent && u.name == part) {
                Some(u) => u.id,
                None => {
                    let id = units.len() as i32;
                    units.push(Unit {
                        id,
                        parent,
                        name: part.to_string(),
                    });
                    id
                }
            };
            parent = id;
        }
        let steps = if spec.stepped {
            (spec.max - spec.min).round() as i32
        } else {
            0
        };
        entries.push(Entry {
            id: spec.id,
            title: spec.name.clone(),
            unit: parent,
            steps,
            default_norm: norm_of(&spec, spec.default),
            flags: ParameterInfo_::ParameterFlags_::kCanAutomate,
            spec: Some(spec),
        });
    }
    if midi {
        for (id, title) in [
            (MIDI_BEND, "Pitch Bend"),
            (MIDI_MOD, "Mod Wheel"),
            (MIDI_PRESSURE, "Pressure"),
            (MIDI_ALL_OFF, "All Notes Off"),
        ] {
            entries.push(Entry {
                id,
                title: title.to_string(),
                unit: kRootUnitId,
                steps: 0,
                default_norm: if id == MIDI_BEND { 0.5 } else { 0.0 },
                flags: ParameterInfo_::ParameterFlags_::kIsHidden
                    | ParameterInfo_::ParameterFlags_::kCanAutomate,
                spec: None,
            });
        }
    }
    let values = entries
        .iter()
        .map(|e| {
            old.and_then(|t| {
                t.entries
                    .iter()
                    .position(|o| o.id == e.id)
                    .map(|i| t.values[i])
            })
            .unwrap_or(e.default_norm)
        })
        .collect();
    Table {
        entries,
        units,
        values,
    }
}

/// The controller core.
pub(crate) struct ControllerCore {
    pub info: &'static Info,
    shared: Mutex<Arc<Shared>>,
    dsp: Mutex<Box<dyn Dsp>>,
    table: Mutex<Table>,
    grown: Mutex<bool>,
    host: Mutex<Option<ComPtr<IHostApplication>>>,
    peer: Mutex<Option<ComPtr<IConnectionPoint>>>,
}

impl ControllerCore {
    pub fn new(def: &'static PluginDef, shared: Arc<Shared>) -> Self {
        let dsp = (def.create)();
        let table = build_table(&*dsp, def.info.midi, None);
        Self {
            info: def.info,
            shared: Mutex::new(shared),
            dsp: Mutex::new(dsp),
            table: Mutex::new(table),
            grown: Mutex::new(false),
            host: Mutex::new(None),
            peer: Mutex::new(None),
        }
    }

    fn shared(&self) -> Arc<Shared> {
        Arc::clone(&lock(&self.shared))
    }

    /// Applies a pending run-time growth of the table.
    fn sync(&self) -> MutexGuard<'_, Table> {
        let wanted = self.shared().grown();
        {
            let mut grown = lock(&self.grown);
            if wanted && !*grown {
                *grown = true;
                let mut dsp = lock(&self.dsp);
                dsp.grow();
                let mut table = lock(&self.table);
                *table = build_table(&**dsp, self.info.midi, Some(&table));
            }
        }
        lock(&self.table)
    }

    pub unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        // SAFETY: the host passes null or a valid FUnknown.
        if let Some(ctx) = unsafe { ComRef::from_raw(context) } {
            *lock(&self.host) = ctx.cast::<IHostApplication>();
        }
        kResultOk
    }

    pub fn terminate(&self) -> tresult {
        *lock(&self.host) = None;
        *lock(&self.peer) = None;
        kResultOk
    }

    // ----- state -------------------------------------------------------------------------------

    pub unsafe fn set_component_state(&self, stream: *mut IBStream) -> tresult {
        self.shared().add(probe::CTRL_COMPONENT_STATES, 1);
        // SAFETY: the host passes a valid stream.
        let Some(bytes) = (unsafe { read_all(stream) }) else {
            return kInvalidArgument;
        };
        let mut dsp = lock(&self.dsp);
        if !dsp.load(&bytes) {
            return kResultFalse;
        }
        let mut table = self.sync();
        for i in 0..table.entries.len() {
            if let Some(spec) = table.entries[i].spec.clone() {
                table.values[i] = norm_of(&spec, dsp.param(spec.id));
            }
        }
        kResultOk
    }

    pub unsafe fn get_state(&self, stream: *mut IBStream) -> tresult {
        self.shared().add(probe::CTRL_STATE_SAVES, 1);
        let table = self.sync();
        let mut bytes = b"CT01".to_vec();
        let visible: Vec<f64> = table
            .entries
            .iter()
            .zip(&table.values)
            .filter(|(e, _)| e.spec.is_some())
            .map(|(_, v)| *v)
            .collect();
        bytes.extend_from_slice(&(visible.len() as u32).to_le_bytes());
        for v in visible {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        drop(table);
        // SAFETY: the host passes a valid stream.
        if unsafe { write_all(stream, &bytes) } {
            kResultOk
        } else {
            kResultFalse
        }
    }

    pub unsafe fn set_state(&self, stream: *mut IBStream) -> tresult {
        self.shared().add(probe::CTRL_STATE_LOADS, 1);
        // SAFETY: the host passes a valid stream.
        let Some(bytes) = (unsafe { read_all(stream) }) else {
            return kInvalidArgument;
        };
        let Some(rest) = bytes.strip_prefix(b"CT01") else {
            return kResultFalse;
        };
        let Some((count, values)) = rest.split_first_chunk::<4>() else {
            return kResultFalse;
        };
        let count = u32::from_le_bytes(*count) as usize;
        if values.len() != count * 8 {
            return kResultFalse;
        }
        let mut table = self.sync();
        let (chunks, _) = values.as_chunks::<8>();
        let mut k = 0;
        for i in 0..table.entries.len() {
            if table.entries[i].spec.is_some() {
                if let Some(c) = chunks.get(k) {
                    table.values[i] = f64::from_le_bytes(*c).clamp(0.0, 1.0);
                }
                k += 1;
            }
        }
        kResultOk
    }

    // ----- parameters -------------------------------------------------------------------------

    pub fn param_count(&self) -> i32 {
        self.sync().entries.len() as i32
    }

    pub unsafe fn param_info(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        let table = self.sync();
        let Some(e) = usize::try_from(index)
            .ok()
            .and_then(|i| table.entries.get(i))
        else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid ParameterInfo.
        let info = unsafe { &mut *info };
        info.id = e.id;
        copy_wstr(&e.title, &mut info.title);
        copy_wstr(&e.title, &mut info.shortTitle);
        copy_wstr("", &mut info.units);
        info.stepCount = e.steps;
        info.defaultNormalizedValue = e.default_norm;
        info.unitId = e.unit;
        info.flags = e.flags;
        kResultOk
    }

    fn entry_of(table: &Table, id: u32) -> Option<&Entry> {
        table.entries.iter().find(|e| e.id == id)
    }

    pub unsafe fn string_by_value(&self, id: u32, normalized: f64, out: *mut String128) -> tresult {
        let table = self.sync();
        let Some(spec) = Self::entry_of(&table, id).and_then(|e| e.spec.as_ref()) else {
            return kInvalidArgument;
        };
        let text = format!("{:.3}", plain_of(spec, normalized));
        // SAFETY: the host passes a valid String128.
        copy_wstr(&text, unsafe { &mut *out });
        kResultOk
    }

    pub unsafe fn value_by_string(&self, id: u32, text: *mut TChar, out: *mut f64) -> tresult {
        let table = self.sync();
        let Some(spec) = Self::entry_of(&table, id).and_then(|e| e.spec.as_ref()) else {
            return kInvalidArgument;
        };
        let mut s = String::new();
        let mut p = text;
        // SAFETY: the host passes a NUL-terminated UTF-16 string.
        unsafe {
            while *p != 0 {
                s.push(char::from_u32(u32::from(*p)).unwrap_or('?'));
                p = p.add(1);
            }
        }
        match s.trim().parse::<f64>() {
            Ok(v) => {
                // SAFETY: a valid out pointer.
                unsafe { *out = norm_of(spec, v) };
                kResultOk
            }
            Err(_) => kInvalidArgument,
        }
    }

    pub fn normalized_to_plain(&self, id: u32, n: f64) -> f64 {
        let table = self.sync();
        Self::entry_of(&table, id)
            .and_then(|e| e.spec.as_ref())
            .map_or(n, |s| plain_of(s, n))
    }

    pub fn plain_to_normalized(&self, id: u32, plain: f64) -> f64 {
        let table = self.sync();
        Self::entry_of(&table, id)
            .and_then(|e| e.spec.as_ref())
            .map_or(plain, |s| norm_of(s, plain))
    }

    pub fn param_normalized(&self, id: u32) -> f64 {
        let table = self.sync();
        table
            .entries
            .iter()
            .position(|e| e.id == id)
            .map_or(0.0, |i| table.values[i])
    }

    pub fn set_param_normalized(&self, id: u32, value: f64) -> tresult {
        let shared = self.shared();
        shared.add(probe::CTRL_SET_PARAMS, 1);
        shared.set(probe::LAST_SET_ID, u64::from(id));
        shared.set(probe::LAST_SET_VALUE, value.to_bits());
        let mut table = self.sync();
        match table.entries.iter().position(|e| e.id == id) {
            Some(i) => {
                table.values[i] = value.clamp(0.0, 1.0);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    pub unsafe fn set_handler(&self, handler: *mut IComponentHandler) -> tresult {
        // SAFETY: the host passes null or a valid handler.
        let handler = unsafe { ComRef::from_raw(handler) }.map(|h| h.to_com_ptr());
        *lock(&self.shared().handler) = handler;
        kResultOk
    }

    // ----- MIDI, units, expressions -----------------------------------------------------------

    pub unsafe fn midi_assignment(
        &self,
        bus: i32,
        channel: i16,
        number: i16,
        id: *mut u32,
    ) -> tresult {
        if !self.info.midi || bus != 0 || channel != 0 {
            return kResultFalse;
        }
        match midi_param(number) {
            Some(p) => {
                // SAFETY: a valid out pointer.
                unsafe { *id = p };
                kResultTrue
            }
            None => kResultFalse,
        }
    }

    pub fn unit_count(&self) -> i32 {
        self.sync().units.len() as i32
    }

    pub unsafe fn unit_info(&self, index: i32, info: *mut UnitInfo) -> tresult {
        let table = self.sync();
        let Some(u) = usize::try_from(index).ok().and_then(|i| table.units.get(i)) else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid UnitInfo.
        let info = unsafe { &mut *info };
        info.id = u.id;
        info.parentUnitId = u.parent;
        copy_wstr(&u.name, &mut info.name);
        info.programListId = kNoProgramListId;
        kResultOk
    }

    pub fn expression_count(&self) -> i32 {
        if self.info.midi { 4 } else { 0 }
    }

    pub unsafe fn expression_info(&self, index: i32, info: *mut NoteExpressionTypeInfo) -> tresult {
        if !self.info.midi || !(0..4).contains(&index) {
            return kInvalidArgument;
        }
        let (id, title, default) = [
            (0u32, "Volume", 0.5),
            (1, "Pan", 0.5),
            (2, "Tuning", 0.5),
            (5, "Brightness", 0.0),
        ][index as usize];
        // SAFETY: the host passes a valid info struct.
        let info = unsafe { &mut *info };
        info.typeId = id;
        copy_wstr(title, &mut info.title);
        copy_wstr(title, &mut info.shortTitle);
        copy_wstr("", &mut info.units);
        info.unitId = -1;
        info.valueDesc.defaultValue = default;
        info.valueDesc.minimum = 0.0;
        info.valueDesc.maximum = 1.0;
        info.valueDesc.stepCount = 0;
        info.associatedParameterId = kNoParamId;
        info.flags = 0;
        kResultOk
    }

    // ----- view --------------------------------------------------------------------------------

    pub fn create_view(&self, name: *const c_char) -> *mut IPlugView {
        // SAFETY: the host passes a NUL-terminated name or null.
        let name = unsafe { crate::util::cstr_text(name) };
        if !self.info.gui || name != "editor" {
            return std::ptr::null_mut();
        }
        let shared = self.shared();
        shared.add(probe::VIEWS_CREATED, 1);
        let view = ComWrapper::new(View {
            shared: Arc::clone(&shared),
        });
        let Some(ptr) = view.to_com_ptr::<IPlugView>() else {
            return std::ptr::null_mut();
        };
        shared.view.store(ptr.as_ptr() as u64, Ordering::Relaxed);
        ptr.into_raw()
    }

    // ----- connection --------------------------------------------------------------------------

    pub unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        self.shared().add(probe::CONNECTS, 1);
        // SAFETY: the host passes a valid connection point.
        let Some(peer) = (unsafe { ComRef::from_raw(other) }) else {
            return kInvalidArgument;
        };
        *lock(&self.peer) = Some(peer.to_com_ptr());
        kResultOk
    }

    pub unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        *lock(&self.peer) = None;
        kResultOk
    }

    pub unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        // SAFETY: the host passes a valid message.
        let Some(message) = (unsafe { ComRef::from_raw(message) }) else {
            return kInvalidArgument;
        };
        // SAFETY: a valid message.
        let (name, id) = unsafe { read_message(message) };
        if name == "atomik.link"
            && let Some(number) = id
            && let Some(found) = find_by_number(number as u64)
        {
            // Adopt the component's probes; keep what this half already learned.
            let old = std::mem::replace(&mut *lock(&self.shared), Arc::clone(&found));
            found.add(probe::CONNECTS, old.get(probe::CONNECTS));
            if let Some(h) = lock(&old.handler).take() {
                *lock(&found.handler) = Some(h);
            }
            let peer = lock(&self.peer).clone();
            let host = lock(&self.host).clone();
            if let Some(peer) = peer {
                send_message(host.as_ref(), &peer, "atomik.ack", number);
            }
        }
        self.shared().add(probe::MESSAGES, 1);
        kResultOk
    }
}

// ----- the view ---------------------------------------------------------------------------------

/// A headless editor: it records what the host asked for and creates no window.
pub(crate) struct View {
    shared: Arc<Shared>,
}

impl Class for View {
    type Interfaces = (IPlugView, IPlugViewContentScaleSupport);
}

const VIEW_W: i32 = 320;
const VIEW_H: i32 = 200;

impl View {
    fn platform_ok(kind: *const c_char) -> bool {
        // SAFETY: the host passes a NUL-terminated string or null.
        let kind = unsafe { crate::util::cstr_text(kind) };
        match std::env::consts::OS {
            "windows" => kind == "HWND",
            "macos" => kind == "NSView",
            _ => kind == "X11EmbedWindowID",
        }
    }
}

impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, kind: FIDString) -> tresult {
        if Self::platform_ok(kind) {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn attached(&self, parent: *mut c_void, kind: FIDString) -> tresult {
        if !Self::platform_ok(kind) {
            return kResultFalse;
        }
        self.shared.or(probe::GUI_CALLS, 1);
        self.shared.set(probe::GUI_PARENT, parent as usize as u64);
        kResultOk
    }

    unsafe fn removed(&self) -> tresult {
        self.shared.or(probe::GUI_CALLS, 2);
        kResultOk
    }

    unsafe fn onWheel(&self, _distance: f32) -> tresult {
        kResultFalse
    }

    unsafe fn onKeyDown(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn onKeyUp(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        self.shared.or(probe::GUI_CALLS, 4);
        // SAFETY: the host passes a valid rect.
        let size = unsafe { &mut *size };
        size.left = 0;
        size.top = 0;
        size.right = VIEW_W;
        size.bottom = VIEW_H;
        kResultOk
    }

    unsafe fn onSize(&self, new_size: *mut ViewRect) -> tresult {
        self.shared.or(probe::GUI_CALLS, 8);
        // SAFETY: the host passes a valid rect.
        let r = unsafe { &*new_size };
        self.shared
            .set(probe::GUI_WIDTH, (r.right - r.left).max(0) as u64);
        self.shared
            .set(probe::GUI_HEIGHT, (r.bottom - r.top).max(0) as u64);
        kResultOk
    }

    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> tresult {
        self.shared.or(probe::GUI_CALLS, 16);
        // SAFETY: the host passes null or a valid frame.
        *lock(&self.shared.frame) = unsafe { ComRef::from_raw(frame) }.map(|f| f.to_com_ptr());
        kResultOk
    }

    unsafe fn canResize(&self) -> tresult {
        self.shared.or(probe::GUI_CALLS, 64);
        kResultTrue
    }

    unsafe fn checkSizeConstraint(&self, rect: *mut ViewRect) -> tresult {
        self.shared.or(probe::GUI_CALLS, 128);
        // SAFETY: the host passes a valid rect.
        let r = unsafe { &mut *rect };
        r.right = r.left + (r.right - r.left).max(100);
        r.bottom = r.top + (r.bottom - r.top).max(80);
        kResultOk
    }
}

impl IPlugViewContentScaleSupportTrait for View {
    unsafe fn setContentScaleFactor(&self, factor: f32) -> tresult {
        self.shared.or(probe::GUI_CALLS, 32);
        self.shared
            .set(probe::GUI_SCALE, f64::from(factor).to_bits());
        kResultOk
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.shared.view.store(0, Ordering::Relaxed);
    }
}

// ----- commands -----------------------------------------------------------------------------

/// `atomik_test_command`: makes the plugin call the host from the calling thread.
pub fn command_run(handle: usize, cmd: u32, a: u64, b: u64) -> u64 {
    let Some(shared) = find_by_ptr(handle) else {
        return u64::MAX;
    };
    let handler = lock(&shared.handler).clone();
    let restart = |flags: i32| -> u64 {
        handler.as_ref().map_or(u64::MAX, |h| {
            // SAFETY: a live handler the host gave us.
            unsafe { h.restartComponent(flags) as u64 }
        })
    };
    match cmd {
        command::GUI_REQUEST_RESIZE => {
            let frame = lock(&shared.frame).clone();
            let view = shared.view.load(Ordering::Relaxed) as *mut IPlugView;
            match frame {
                Some(f) if !view.is_null() => {
                    let mut rect = ViewRect {
                        left: 0,
                        top: 0,
                        right: a as i32,
                        bottom: b as i32,
                    };
                    // SAFETY: a live frame and the view's own pointer.
                    unsafe { f.resizeView(view, &mut rect) as u64 }
                }
                _ => u64::MAX,
            }
        }
        command::LATENCY_CHANGED => restart(RestartFlags_::kLatencyChanged),
        command::REQUEST_RESTART => restart(RestartFlags_::kReloadComponent),
        command::MIDI_MAPPING_CHANGED => restart(RestartFlags_::kMidiCCAssignmentChanged),
        command::PARAM_VALUES_CHANGED => restart(RestartFlags_::kParamValuesChanged),
        command::IO_CHANGED => restart(RestartFlags_::kIoChanged),
        command::RESCAN_PARAMS => {
            shared.set_grown();
            restart(RestartFlags_::kParamTitlesChanged)
        }
        command::MARK_DIRTY => handler
            .as_ref()
            .and_then(|h| h.cast::<IComponentHandler2>())
            // SAFETY: a live handler.
            .map_or(u64::MAX, |h| unsafe { h.setDirty(1) as u64 }),
        command::GUI_EDIT => handler.as_ref().map_or(u64::MAX, |h| {
            // SAFETY: a live handler.
            unsafe {
                h.beginEdit(a as u32);
                h.performEdit(a as u32, f64::from_bits(b));
                h.endEdit(a as u32);
            }
            0
        }),
        _ => u64::MAX,
    }
}
