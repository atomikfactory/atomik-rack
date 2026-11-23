//! The COM classes of a test plugin: a combined single-component class, and the split
//! component and controller classes, over the cores of `component` and `controller`, plus the
//! factory that creates them.

use crate::api::{LibraryDef, PluginDef};
use crate::component::ComponentCore;
use crate::controller::ControllerCore;
use crate::hooks::{self, Shared, bump, counter, set_counter, stamp};
use crate::util::{copy_cstr, copy_wstr};
use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vst3::{Class, ComPtr, ComWrapper, Steinberg::Vst::*, Steinberg::*, uid};

static NEXT_NUMBER: AtomicU64 = AtomicU64::new(1);

fn b(state: TBool) -> bool {
    state != 0
}

// ----- trait impls shared by the three classes ----------------------------------------------

macro_rules! impl_component {
    ($t:ty) => {
        impl_icomponent!($t);
        impl_processor!($t);
    };
}

macro_rules! impl_icomponent {
    ($t:ty) => {
        impl IComponentTrait for $t {
            unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
                match self.comp.info.controller_cid {
                    Some([a, b, c, d]) => {
                        // SAFETY: the host passes a valid out pointer.
                        unsafe { *class_id = uid(a, b, c, d) };
                        kResultOk
                    }
                    None => kResultFalse,
                }
            }
            unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
                kResultOk
            }
            unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
                self.comp.bus_count(media, dir)
            }
            unsafe fn getBusInfo(
                &self,
                media: MediaType,
                dir: BusDirection,
                index: i32,
                bus: *mut BusInfo,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.bus_info(media, dir, index, bus) }
            }
            unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
                kNotImplemented
            }
            unsafe fn activateBus(
                &self,
                media: MediaType,
                dir: BusDirection,
                index: i32,
                state: TBool,
            ) -> tresult {
                self.comp.activate_bus(media, dir, index, b(state))
            }
            unsafe fn setActive(&self, state: TBool) -> tresult {
                self.comp.set_active(b(state))
            }
            unsafe fn setState(&self, state: *mut IBStream) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.set_state(state) }
            }
            unsafe fn getState(&self, state: *mut IBStream) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.get_state(state) }
            }
        }
    };
}

macro_rules! impl_processor {
    ($t:ty) => {
        impl IAudioProcessorTrait for $t {
            unsafe fn setBusArrangements(
                &self,
                inputs: *mut SpeakerArrangement,
                num_ins: i32,
                outputs: *mut SpeakerArrangement,
                num_outs: i32,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe {
                    self.comp
                        .set_bus_arrangements(inputs, num_ins, outputs, num_outs)
                }
            }
            unsafe fn getBusArrangement(
                &self,
                dir: BusDirection,
                index: i32,
                arr: *mut SpeakerArrangement,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.bus_arrangement(dir, index, arr) }
            }
            unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
                if size == SymbolicSampleSizes_::kSample32 as i32 {
                    kResultOk
                } else {
                    kNotImplemented
                }
            }
            unsafe fn getLatencySamples(&self) -> u32 {
                self.comp.latency()
            }
            unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.setup_processing(setup) }
            }
            unsafe fn setProcessing(&self, state: TBool) -> tresult {
                self.comp.set_processing(b(state))
            }
            unsafe fn process(&self, data: *mut ProcessData) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.comp.process(data) }
            }
            unsafe fn getTailSamples(&self) -> u32 {
                self.comp.tail()
            }
        }

        impl IProcessContextRequirementsTrait for $t {
            unsafe fn getProcessContextRequirements(&self) -> u32 {
                (IProcessContextRequirements_::Flags_::kNeedTempo
                    | IProcessContextRequirements_::Flags_::kNeedTimeSignature
                    | IProcessContextRequirements_::Flags_::kNeedProjectTimeMusic
                    | IProcessContextRequirements_::Flags_::kNeedBarPositionMusic
                    | IProcessContextRequirements_::Flags_::kNeedCycleMusic
                    | IProcessContextRequirements_::Flags_::kNeedContinousTimeSamples)
                    as u32
            }
        }
    };
}

macro_rules! impl_controller {
    ($t:ty) => {
        impl IEditControllerTrait for $t {
            unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.set_component_state(state) }
            }
            unsafe fn setState(&self, state: *mut IBStream) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.set_state(state) }
            }
            unsafe fn getState(&self, state: *mut IBStream) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.get_state(state) }
            }
            unsafe fn getParameterCount(&self) -> i32 {
                self.ctrl.param_count()
            }
            unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.param_info(index, info) }
            }
            unsafe fn getParamStringByValue(
                &self,
                id: ParamID,
                value: ParamValue,
                string: *mut String128,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.string_by_value(id, value, string) }
            }
            unsafe fn getParamValueByString(
                &self,
                id: ParamID,
                string: *mut TChar,
                value: *mut ParamValue,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.value_by_string(id, string, value) }
            }
            unsafe fn normalizedParamToPlain(&self, id: ParamID, value: ParamValue) -> ParamValue {
                self.ctrl.normalized_to_plain(id, value)
            }
            unsafe fn plainParamToNormalized(&self, id: ParamID, plain: ParamValue) -> ParamValue {
                self.ctrl.plain_to_normalized(id, plain)
            }
            unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
                self.ctrl.param_normalized(id)
            }
            unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
                self.ctrl.set_param_normalized(id, value)
            }
            unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.set_handler(handler) }
            }
            unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
                self.ctrl.create_view(name)
            }
        }

        impl IMidiMappingTrait for $t {
            unsafe fn getMidiControllerAssignment(
                &self,
                bus: i32,
                channel: i16,
                number: CtrlNumber,
                id: *mut ParamID,
            ) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.midi_assignment(bus, channel, number, id) }
            }
        }

        impl IUnitInfoTrait for $t {
            unsafe fn getUnitCount(&self) -> i32 {
                self.ctrl.unit_count()
            }
            unsafe fn getUnitInfo(&self, index: i32, info: *mut UnitInfo) -> tresult {
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.unit_info(index, info) }
            }
            unsafe fn getProgramListCount(&self) -> i32 {
                0
            }
            unsafe fn getProgramListInfo(&self, _i: i32, _info: *mut ProgramListInfo) -> tresult {
                kInvalidArgument
            }
            unsafe fn getProgramName(&self, _l: i32, _i: i32, _n: *mut String128) -> tresult {
                kInvalidArgument
            }
            unsafe fn getProgramInfo(
                &self,
                _l: i32,
                _i: i32,
                _a: *const c_char,
                _v: *mut String128,
            ) -> tresult {
                kInvalidArgument
            }
            unsafe fn hasProgramPitchNames(&self, _l: i32, _i: i32) -> tresult {
                kResultFalse
            }
            unsafe fn getProgramPitchName(
                &self,
                _l: i32,
                _i: i32,
                _p: i16,
                _n: *mut String128,
            ) -> tresult {
                kInvalidArgument
            }
            unsafe fn getSelectedUnit(&self) -> UnitID {
                kRootUnitId
            }
            unsafe fn selectUnit(&self, _id: UnitID) -> tresult {
                kResultFalse
            }
            unsafe fn getUnitByBus(
                &self,
                _t: MediaType,
                _d: BusDirection,
                _b: i32,
                _c: i32,
                _u: *mut UnitID,
            ) -> tresult {
                kResultFalse
            }
            unsafe fn setUnitProgramData(&self, _l: i32, _i: i32, _d: *mut IBStream) -> tresult {
                kResultFalse
            }
        }

        impl INoteExpressionControllerTrait for $t {
            unsafe fn getNoteExpressionCount(&self, bus: i32, channel: i16) -> i32 {
                if bus == 0 && channel == 0 {
                    self.ctrl.expression_count()
                } else {
                    0
                }
            }
            unsafe fn getNoteExpressionInfo(
                &self,
                bus: i32,
                channel: i16,
                index: i32,
                info: *mut NoteExpressionTypeInfo,
            ) -> tresult {
                if bus != 0 || channel != 0 {
                    return kInvalidArgument;
                }
                // SAFETY: forwarded from the host.
                unsafe { self.ctrl.expression_info(index, info) }
            }
            unsafe fn getNoteExpressionStringByValue(
                &self,
                _bus: i32,
                _channel: i16,
                _id: NoteExpressionTypeID,
                value: NoteExpressionValue,
                string: *mut String128,
            ) -> tresult {
                // SAFETY: the host passes a valid String128.
                copy_wstr(&format!("{value:.3}"), unsafe { &mut *string });
                kResultOk
            }
            unsafe fn getNoteExpressionValueByString(
                &self,
                _bus: i32,
                _channel: i16,
                _id: NoteExpressionTypeID,
                _string: *const TChar,
                _value: *mut NoteExpressionValue,
            ) -> tresult {
                kNotImplemented
            }
        }
    };
}

// ----- a single-component plugin --------------------------------------------------------------

pub(crate) struct Both {
    comp: ComponentCore,
    ctrl: ControllerCore,
}

impl Class for Both {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IProcessContextRequirements,
        IEditController,
        IMidiMapping,
        IUnitInfo,
        INoteExpressionController,
    );
}

impl IPluginBaseTrait for Both {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        bump(counter::COMPONENT_INITS);
        // SAFETY: forwarded from the host.
        let (a, b) = unsafe { (self.comp.initialize(context), self.ctrl.initialize(context)) };
        if a == kResultOk { b } else { a }
    }

    unsafe fn terminate(&self) -> tresult {
        bump(counter::COMPONENT_TERMS);
        stamp(counter::SEQ_TERMINATE_COMPONENT);
        self.ctrl.terminate();
        self.comp.terminate()
    }
}

impl_component!(Both);
impl_controller!(Both);

// ----- a component that is not an audio processor ---------------------------------------------

pub(crate) struct NotProc {
    comp: ComponentCore,
}

impl Class for NotProc {
    type Interfaces = (IComponent,);
}

impl IPluginBaseTrait for NotProc {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        bump(counter::COMPONENT_INITS);
        // SAFETY: forwarded from the host.
        unsafe { self.comp.initialize(context) }
    }

    unsafe fn terminate(&self) -> tresult {
        bump(counter::COMPONENT_TERMS);
        stamp(counter::SEQ_TERMINATE_COMPONENT);
        self.comp.terminate()
    }
}

impl_icomponent!(NotProc);

// ----- a split plugin -------------------------------------------------------------------------

pub(crate) struct Comp {
    comp: ComponentCore,
}

impl Class for Comp {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IProcessContextRequirements,
        IConnectionPoint,
    );
}

impl IPluginBaseTrait for Comp {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        bump(counter::COMPONENT_INITS);
        // SAFETY: forwarded from the host.
        unsafe { self.comp.initialize(context) }
    }

    unsafe fn terminate(&self) -> tresult {
        bump(counter::COMPONENT_TERMS);
        stamp(counter::SEQ_TERMINATE_COMPONENT);
        self.comp.terminate()
    }
}

impl_component!(Comp);

impl IConnectionPointTrait for Comp {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.comp.connect(other) }
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.comp.disconnect(other) }
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.comp.notify(message) }
    }
}

pub(crate) struct Ctrl {
    ctrl: ControllerCore,
}

impl Class for Ctrl {
    type Interfaces = (
        IEditController,
        IMidiMapping,
        IUnitInfo,
        INoteExpressionController,
        IConnectionPoint,
    );
}

impl IPluginBaseTrait for Ctrl {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        bump(counter::CONTROLLER_INITS);
        if self.ctrl.quirks.controller_refuses {
            return kInternalError;
        }
        // SAFETY: forwarded from the host.
        unsafe { self.ctrl.initialize(context) }
    }

    unsafe fn terminate(&self) -> tresult {
        bump(counter::CONTROLLER_TERMS);
        stamp(counter::SEQ_TERMINATE_CONTROLLER);
        self.ctrl.terminate()
    }
}

impl Drop for Ctrl {
    fn drop(&mut self) {
        bump(counter::CONTROLLERS_DESTROYED);
    }
}

impl_controller!(Ctrl);

impl IConnectionPointTrait for Ctrl {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.ctrl.connect(other) }
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.ctrl.disconnect(other) }
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        // SAFETY: forwarded from the host.
        unsafe { self.ctrl.notify(message) }
    }
}

// ----- the factory ------------------------------------------------------------------------------

/// The plugin factory of a library.
pub(crate) struct Factory {
    pub lib: &'static LibraryDef,
    /// The host context a factory keeps after `setHostContext`, as real plugins do.
    context: Mutex<Option<ComPtr<FUnknown>>>,
    /// `setHostContext` has been called.
    given: AtomicBool,
    /// The last single-object plugin made (a component the factory also answers its controller
    /// class with): its class id and the object, handed out once more for the controller class.
    single: Mutex<Option<(TUID, ComPtr<FUnknown>)>>,
}

impl Factory {
    pub(crate) fn new(lib: &'static LibraryDef) -> Self {
        Self {
            lib,
            context: Mutex::new(None),
            given: AtomicBool::new(false),
            single: Mutex::new(None),
        }
    }
}

impl Class for Factory {
    type Interfaces = (IPluginFactory3,);
}

impl Drop for Factory {
    fn drop(&mut self) {
        let context = self
            .context
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(context) = context {
            let raw = context.into_raw();
            // SAFETY: the one reference this factory kept; `release` answers how many remain.
            let remaining = unsafe { ((*(*raw).vtbl).release)(raw) };
            set_counter(counter::CONTEXT_REFS_AT_FACTORY_DROP, u64::from(remaining));
        }
    }
}

/// One factory class: which plugin and whether it is the controller class.
fn class_at(lib: &LibraryDef, index: i32) -> Option<(&'static PluginDef, bool)> {
    let mut n = 0;
    for def in lib.plugins {
        if n == index {
            return Some((def, false));
        }
        n += 1;
        if def.info.controller_cid.is_some() {
            if n == index {
                return Some((def, true));
            }
            n += 1;
        }
    }
    None
}

fn tuid_of(words: [u32; 4]) -> TUID {
    uid(words[0], words[1], words[2], words[3])
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        // SAFETY: the host passes a valid struct.
        let info = unsafe { &mut *info };
        copy_cstr("Atomik", &mut info.vendor);
        copy_cstr("https://example.invalid", &mut info.url);
        copy_cstr("test@example.invalid", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        self.lib
            .plugins
            .iter()
            .map(|d| 1 + i32::from(d.info.controller_cid.is_some()))
            .sum()
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some((def, controller)) = class_at(self.lib, index) else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid struct.
        let info = unsafe { &mut *info };
        info.cid = tuid_of(if controller {
            def.info.controller_cid.unwrap_or(def.info.cid)
        } else {
            def.info.cid
        });
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_cstr(
            if controller {
                "Component Controller Class"
            } else {
                "Audio Module Class"
            },
            &mut info.category,
        );
        copy_cstr(def.info.name, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if !self.given.load(Ordering::Acquire) {
            bump(counter::CREATES_BEFORE_CONTEXT);
        }
        // SAFETY: the host passes a 16 byte class id.
        let want: TUID = unsafe { *cid.cast::<TUID>() };
        for def in self.lib.plugins {
            let make: Option<vst3::ComPtr<FUnknown>> = if tuid_of(def.info.cid) == want {
                let number = NEXT_NUMBER.fetch_add(1, Ordering::Relaxed);
                let shared = Shared::new(number);
                let comp = ComponentCore::new(def, shared.clone());
                let quirks = comp.quirks;
                if quirks.not_a_processor {
                    let w = ComWrapper::new(NotProc { comp });
                    register_not_proc(&w, shared);
                    w.to_com_ptr::<FUnknown>()
                } else if def.info.controller_cid.is_some() && !quirks.controller_is_component {
                    let w = ComWrapper::new(Comp { comp });
                    register(&w, shared);
                    w.to_com_ptr::<FUnknown>()
                } else {
                    let w = ComWrapper::new(Both {
                        comp,
                        ctrl: ControllerCore::new(def, shared.clone()),
                    });
                    register_both(&w, shared);
                    let unknown = w.to_com_ptr::<FUnknown>();
                    if let (true, Some(u)) = (quirks.controller_is_component, unknown.as_ref()) {
                        *lock(&self.single) = Some((tuid_of(def.info.cid), u.clone()));
                    }
                    unknown
                }
            } else if def.info.controller_cid.map(tuid_of) == Some(want) {
                // A plugin that is its own controller answers its controller class with the
                // object it made for the component class.
                let single = {
                    let mut single = lock(&self.single);
                    match single.take() {
                        Some((class, object)) if class == tuid_of(def.info.cid) => Some(object),
                        other => {
                            *single = other;
                            None
                        }
                    }
                };
                if let Some(object) = single {
                    Some(object)
                } else {
                    let number = NEXT_NUMBER.fetch_add(1, Ordering::Relaxed);
                    bump(counter::CONTROLLERS_CREATED);
                    let w = ComWrapper::new(Ctrl {
                        ctrl: ControllerCore::new(def, Shared::new(number)),
                    });
                    w.to_com_ptr::<FUnknown>()
                }
            } else {
                continue;
            };
            let Some(unknown) = make else {
                return kInternalError;
            };
            let ptr = unknown.as_ptr();
            // SAFETY: a valid object; the query adds the reference the host receives.
            return unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid.cast::<TUID>(), obj) };
        }
        kInvalidArgument
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn register(w: &ComWrapper<Comp>, shared: std::sync::Arc<Shared>) {
    if let Some(p) = w.as_com_ref::<IComponent>() {
        hooks::register(p.as_ptr() as usize, shared);
    }
}

fn register_both(w: &ComWrapper<Both>, shared: std::sync::Arc<Shared>) {
    if let Some(p) = w.as_com_ref::<IComponent>() {
        hooks::register(p.as_ptr() as usize, shared);
    }
}

fn register_not_proc(w: &ComWrapper<NotProc>, shared: std::sync::Arc<Shared>) {
    if let Some(p) = w.as_com_ref::<IComponent>() {
        hooks::register(p.as_ptr() as usize, shared);
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some((def, controller)) = class_at(self.lib, index) else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid struct.
        let info = unsafe { &mut *info };
        info.cid = tuid_of(if controller {
            def.info.controller_cid.unwrap_or(def.info.cid)
        } else {
            def.info.cid
        });
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_cstr(
            if controller {
                "Component Controller Class"
            } else {
                "Audio Module Class"
            },
            &mut info.category,
        );
        copy_cstr(def.info.name, &mut info.name);
        info.classFlags = 0;
        copy_cstr(
            if controller {
                ""
            } else {
                def.info.sub_categories
            },
            &mut info.subCategories,
        );
        copy_cstr("Atomik", &mut info.vendor);
        copy_cstr("1.0.0", &mut info.version);
        copy_cstr("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, index: i32, info: *mut PClassInfoW) -> tresult {
        let Some((def, controller)) = class_at(self.lib, index) else {
            return kInvalidArgument;
        };
        // SAFETY: the host passes a valid struct.
        let info = unsafe { &mut *info };
        info.cid = tuid_of(if controller {
            def.info.controller_cid.unwrap_or(def.info.cid)
        } else {
            def.info.cid
        });
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_cstr(
            if controller {
                "Component Controller Class"
            } else {
                "Audio Module Class"
            },
            &mut info.category,
        );
        copy_wstr(def.info.name, &mut info.name);
        info.classFlags = 0;
        copy_cstr(
            if controller {
                ""
            } else {
                def.info.sub_categories
            },
            &mut info.subCategories,
        );
        copy_wstr("Atomik", &mut info.vendor);
        copy_wstr("1.0.0", &mut info.version);
        copy_wstr("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }

    unsafe fn setHostContext(&self, context: *mut FUnknown) -> tresult {
        bump(counter::HOST_CONTEXT_CALLS);
        self.given.store(true, Ordering::Release);
        if self.lib.refuse_host_context {
            return kInternalError;
        }
        // SAFETY: the host passes null or a valid FUnknown.
        if let Some(c) = unsafe { vst3::ComRef::from_raw(context) } {
            *lock(&self.context) = Some(c.to_com_ptr());
        }
        kResultOk
    }
}

/// Makes the factory object for `GetPluginFactory` (one reference, owned by the caller).
pub fn make_factory(lib: &'static LibraryDef) -> *mut IPluginFactory {
    ComWrapper::new(Factory::new(lib))
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), vst3::ComPtr::into_raw)
}

#[allow(dead_code)]
fn _unused(_: *const c_char) {}
