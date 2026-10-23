//! The host side of the VST3 interfaces: what a plugin may call and where each
//! call goes. Everything here is safe to run on any thread: it touches atomics, the instance's
//! wait-free message ring and short mutexes (VST3 asks for the main thread, but plugins do not
//! always comply, and the host must not break when they do not).
//!
//! * `HostApplication`: `IHostApplication` (name, messages, attribute lists) and
//!   `IPlugInterfaceSupport`.
//! * `ComponentHandler`: `IComponentHandler` and `IComponentHandler2` (edits, restarts, dirty).
//! * `PlugFrame`: `IPlugFrame` (resize requests) and `IRunLoop` (timers for Linux editors).
//! * `ConnectionProxy`: the proxy that keeps component and controller from holding each other.
//! * `HostMessage` and `HostAttributes`: `IMessage` and `IAttributeList`.

use crate::util::write_utf16;
use crate::values::{self, Steps};
use atomik_plugin_host::{HostMessenger, ParamPhase, PluginMessage, warmed};
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use vst3::Steinberg::Linux::{
    IEventHandler, IRunLoop, IRunLoopTrait, ITimerHandler, ITimerHandlerTrait,
};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The shortest timer period the host grants.
const MIN_TIMER_MS: u64 = 10;

/// State shared by the host objects of one plugin instance and its instance and processor.
pub(crate) struct Hub {
    pub messenger: HostMessenger,
    steps: Mutex<Arc<Steps>>,
    edits: Mutex<Option<rtrb::Producer<(u32, f64)>>>,
    /// A restart asked for a new param table or new buses.
    pub layout_dirty: AtomicBool,
    /// A restart asked for new MIDI or expression maps.
    pub midi_dirty: AtomicBool,
    /// Edits from the plugin's own editor that did not fit the ring to the processor.
    pub edits_dropped: AtomicU32,
}

impl Hub {
    pub(crate) fn new(messenger: HostMessenger) -> Arc<Self> {
        Arc::new(Self {
            messenger,
            // `performEdit` may arrive on any thread, the audio thread included: warm both locks.
            steps: warmed(Mutex::new(Arc::new(Vec::new()))),
            edits: warmed(Mutex::new(None)),
            layout_dirty: AtomicBool::new(false),
            midi_dirty: AtomicBool::new(false),
            edits_dropped: AtomicU32::new(0),
        })
    }

    /// The step counts of the stepped params (sorted by id).
    pub(crate) fn set_steps(&self, steps: Steps) {
        *lock(&self.steps) = Arc::new(steps);
    }

    pub(crate) fn steps(&self) -> Arc<Steps> {
        Arc::clone(&lock(&self.steps))
    }

    /// Where `performEdit` values go so the processor hears them (the next activation's ring).
    pub(crate) fn set_edit_sink(&self, sink: Option<rtrb::Producer<(u32, f64)>>) {
        *lock(&self.edits) = sink;
    }

    fn edit(&self, id: u32, normalized: f64) {
        if let Some(sink) = lock(&self.edits).as_mut()
            && sink.push((id, normalized)).is_err()
        {
            self.edits_dropped.fetch_add(1, Ordering::Relaxed);
        }
        let value = values::host_value(&self.steps(), id, normalized);
        self.messenger.post(PluginMessage::ParamEdit {
            id,
            value,
            phase: ParamPhase::Change,
        });
    }
}

// ----- IHostApplication ----------------------------------------------------------------------

/// `IHostApplication` and `IPlugInterfaceSupport`: the context both halves initialise with.
pub(crate) struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication, IPlugInterfaceSupport);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null String128 from the caller.
        write_utf16("Atomik Rack", unsafe { &mut *name });
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: non-null pointers from the caller.
        let (cid, iid) = unsafe { (*cid, *iid) };
        let made: Option<ComPtr<FUnknown>> = if cid == IMessage_iid {
            HostMessage::new().to_com_ptr::<FUnknown>()
        } else if cid == IAttributeList_iid {
            ComWrapper::new(HostAttributes::default()).to_com_ptr::<FUnknown>()
        } else {
            return kNoInterface;
        };
        let Some(made) = made else {
            return kInternalError;
        };
        let ptr = made.as_ptr();
        let mut iid = iid;
        // SAFETY: a live object; the query adds the reference the caller receives.
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, &mut iid, obj) }
    }
}

impl IPlugInterfaceSupportTrait for HostApplication {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        if iid.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null iid from the caller.
        let iid = unsafe { *iid };
        let known = [
            IComponentHandler_iid,
            IComponentHandler2_iid,
            IPlugFrame_iid,
            vst3::Steinberg::Linux::IRunLoop_iid,
            IMidiMapping_iid,
            IUnitInfo_iid,
            INoteExpressionController_iid,
            IConnectionPoint_iid,
        ];
        if known.contains(&iid) {
            kResultTrue
        } else {
            kResultFalse
        }
    }
}

// ----- IAttributeList and IMessage -------------------------------------------------------------

enum Attr {
    Int(i64),
    Float(f64),
    Text(Vec<u16>),
    Binary(Vec<u8>),
}

/// `IAttributeList`: a small key-value store.
#[derive(Default)]
pub(crate) struct HostAttributes {
    items: Mutex<Vec<(String, Attr)>>,
}

impl Class for HostAttributes {
    type Interfaces = (IAttributeList,);
}

impl HostAttributes {
    fn key(id: *const c_char) -> Option<String> {
        if id.is_null() {
            return None;
        }
        // SAFETY: a NUL-terminated attribute id from the caller.
        Some(unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned())
    }

    fn set(&self, id: *const c_char, value: Attr) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        let mut items = lock(&self.items);
        match items.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => items.push((key, value)),
        }
        kResultOk
    }
}

impl IAttributeListTrait for HostAttributes {
    unsafe fn setInt(&self, id: *const c_char, value: i64) -> tresult {
        self.set(id, Attr::Int(value))
    }

    unsafe fn getInt(&self, id: *const c_char, value: *mut i64) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        match lock(&self.items).iter().find(|(k, _)| *k == key) {
            Some((_, Attr::Int(v))) if !value.is_null() => {
                // SAFETY: a non-null out pointer from the caller.
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: *const c_char, value: f64) -> tresult {
        self.set(id, Attr::Float(value))
    }

    unsafe fn getFloat(&self, id: *const c_char, value: *mut f64) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        match lock(&self.items).iter().find(|(k, _)| *k == key) {
            Some((_, Attr::Float(v))) if !value.is_null() => {
                // SAFETY: a non-null out pointer from the caller.
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: *const c_char, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut units = Vec::new();
        let mut p = string;
        // SAFETY: a NUL-terminated UTF-16 string from the caller.
        unsafe {
            while *p != 0 {
                units.push(*p);
                p = p.add(1);
            }
        }
        self.set(id, Attr::Text(units))
    }

    unsafe fn getString(
        &self,
        id: *const c_char,
        string: *mut TChar,
        size_in_bytes: u32,
    ) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        match lock(&self.items).iter().find(|(k, _)| *k == key) {
            Some((_, Attr::Text(units))) if !string.is_null() && size_in_bytes >= 2 => {
                let room = (size_in_bytes as usize / 2).saturating_sub(1);
                let n = units.len().min(room);
                // SAFETY: the caller's buffer holds `size_in_bytes / 2` units; `n + 1` fits.
                unsafe {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), string, n);
                    *string.add(n) = 0;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setBinary(&self, id: *const c_char, data: *const c_void, size: u32) -> tresult {
        if data.is_null() && size > 0 {
            return kInvalidArgument;
        }
        let bytes = if size == 0 {
            Vec::new()
        } else {
            // SAFETY: `size` readable bytes at `data` from the caller.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) }.to_vec()
        };
        self.set(id, Attr::Binary(bytes))
    }

    unsafe fn getBinary(
        &self,
        id: *const c_char,
        data: *mut *const c_void,
        size: *mut u32,
    ) -> tresult {
        let Some(key) = Self::key(id) else {
            return kInvalidArgument;
        };
        match lock(&self.items).iter().find(|(k, _)| *k == key) {
            Some((_, Attr::Binary(b))) if !data.is_null() && !size.is_null() => {
                // SAFETY: non-null out pointers; the bytes live as long as the attribute does,
                // which is what the interface promises.
                unsafe {
                    *data = b.as_ptr().cast();
                    *size = b.len() as u32;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

/// `IMessage`: an id and an attribute list.
pub(crate) struct HostMessage {
    id: Mutex<CString>,
    attributes: ComWrapper<HostAttributes>,
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl HostMessage {
    fn new() -> ComWrapper<Self> {
        ComWrapper::new(Self {
            id: Mutex::new(CString::default()),
            attributes: ComWrapper::new(HostAttributes::default()),
        })
    }
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> *const c_char {
        // The pointer stays valid until the next `setMessageID`, as the interface requires.
        lock(&self.id).as_ptr()
    }

    unsafe fn setMessageID(&self, id: *const c_char) {
        if id.is_null() {
            return;
        }
        // SAFETY: a NUL-terminated id from the caller.
        *lock(&self.id) = unsafe { CStr::from_ptr(id) }.to_owned();
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

// ----- IComponentHandler -------------------------------------------------------------------------

/// `IComponentHandler` and `IComponentHandler2`: edits made in the plugin's own editor, restart
/// requests and the dirty flag.
pub(crate) struct ComponentHandler {
    pub hub: Arc<Hub>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler, IComponentHandler2);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, id: u32) -> tresult {
        self.hub.messenger.post(PluginMessage::ParamEdit {
            id,
            value: f64::NAN,
            phase: ParamPhase::Begin,
        });
        kResultOk
    }

    unsafe fn performEdit(&self, id: u32, value_normalized: f64) -> tresult {
        self.hub.edit(id, value_normalized);
        kResultOk
    }

    unsafe fn endEdit(&self, id: u32) -> tresult {
        self.hub.messenger.post(PluginMessage::ParamEdit {
            id,
            value: f64::NAN,
            phase: ParamPhase::End,
        });
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> tresult {
        let a = values::restart_actions(flags);
        if a.latency {
            self.hub.messenger.post(PluginMessage::LatencyChanged);
        }
        if a.params {
            self.hub.layout_dirty.store(true, Ordering::Release);
            self.hub.messenger.post(PluginMessage::RescanParams);
        }
        if a.reload {
            self.hub.messenger.post(PluginMessage::RequestRestart);
        }
        if a.midi {
            self.hub.midi_dirty.store(true, Ordering::Release);
            self.hub.messenger.post(PluginMessage::RequestRestart);
        }
        if a.values {
            self.hub.messenger.post(PluginMessage::StateDirty);
        }
        kResultOk
    }
}

impl IComponentHandler2Trait for ComponentHandler {
    unsafe fn setDirty(&self, state: TBool) -> tresult {
        if state != 0 {
            self.hub.messenger.post(PluginMessage::StateDirty);
        }
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, _name: *const c_char) -> tresult {
        self.hub.messenger.post(PluginMessage::GuiRequestShow);
        kResultOk
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        kResultOk
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        kResultOk
    }
}

// ----- IPlugFrame and IRunLoop ---------------------------------------------------------------------

/// `poll(2)`, declared here: `IRunLoop` is a Linux interface and no crate is needed for one call.
#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::c_ulong;

    pub const POLLIN: i16 = 0x001;
    pub const POLLERR: i16 = 0x008;
    pub const POLLHUP: i16 = 0x010;

    /// `struct pollfd`.
    #[repr(C)]
    pub struct PollFd {
        pub fd: i32,
        pub events: i16,
        pub revents: i16,
    }

    // SAFETY: the signature is that of `poll(2)` in libc, which every Linux target links.
    unsafe extern "C" {
        pub fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: i32) -> i32;
    }
}

struct TimerReg {
    handler: ComPtr<ITimerHandler>,
    period: Duration,
    next: Instant,
}

/// `IPlugFrame` and `IRunLoop`: resize requests go to the message ring; timers registered by
/// Linux editors are run by [`PlugFrame::run_timers`] from the instance's `poll`. Event
/// handlers (file descriptors, such as an editor's own X connection) are recorded, and
/// [`PlugFrame::run_event_handlers`] calls `onFDIsSet` for those whose descriptor is readable,
/// also from `poll`; that is how an editor gets its mouse and keyboard input.
pub(crate) struct PlugFrame {
    hub: Arc<Hub>,
    timers: Mutex<Vec<TimerReg>>,
    handlers: Mutex<Vec<(ComPtr<IEventHandler>, i32)>>,
}

impl Class for PlugFrame {
    type Interfaces = (IPlugFrame, IRunLoop);
}

// Justification: plugin timers are wall-clock by nature (a GUI refresh) and never reach the
// audio path, so HI-2's ban on clocks does not apply to this one function.
#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

impl PlugFrame {
    pub(crate) fn new(hub: Arc<Hub>) -> ComWrapper<Self> {
        ComWrapper::new(Self {
            hub,
            timers: Mutex::new(Vec::new()),
            handlers: Mutex::new(Vec::new()),
        })
    }

    /// Calls the timers that are due (on the main thread). Returns how many ran.
    pub(crate) fn run_timers(&self) -> usize {
        let at = now();
        let due: Vec<ComPtr<ITimerHandler>> = {
            let mut timers = lock(&self.timers);
            let mut due = Vec::new();
            for t in timers.iter_mut() {
                if t.next <= at {
                    due.push(t.handler.clone());
                    t.next = at + t.period;
                }
            }
            due
        };
        for h in &due {
            // SAFETY: a live timer handler the plugin registered.
            unsafe { h.onTimer() };
        }
        due.len()
    }

    /// Calls the event handlers whose file descriptor is readable, in error or hung up (on the
    /// main thread). Never waits. Returns how many ran.
    #[cfg(target_os = "linux")]
    pub(crate) fn run_event_handlers(&self) -> usize {
        use vst3::Steinberg::Linux::IEventHandlerTrait;
        // Cloned out, so the plugin may register or unregister from inside `onFDIsSet`.
        let handlers: Vec<(ComPtr<IEventHandler>, i32)> = lock(&self.handlers).clone();
        if handlers.is_empty() {
            return 0;
        }
        let mut fds: Vec<sys::PollFd> = handlers
            .iter()
            .map(|(_, fd)| sys::PollFd {
                fd: *fd,
                events: sys::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is a live, writable array of exactly `len` entries; a zero timeout
        // makes the call return at once.
        let ready = unsafe { sys::poll(fds.as_mut_ptr(), fds.len() as std::ffi::c_ulong, 0) };
        if ready <= 0 {
            return 0;
        }
        let mut ran = 0;
        for ((handler, fd), polled) in handlers.iter().zip(&fds) {
            if polled.revents & (sys::POLLIN | sys::POLLERR | sys::POLLHUP) != 0 {
                // SAFETY: a live event handler the plugin registered.
                unsafe { handler.onFDIsSet(*fd) };
                ran += 1;
            }
        }
        ran
    }

    /// Event handlers are a Linux interface (`IRunLoop`): nothing to run elsewhere.
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn run_event_handlers(&self) -> usize {
        0
    }

    /// Drops every registration (the editor is gone).
    pub(crate) fn clear(&self) {
        lock(&self.timers).clear();
        lock(&self.handlers).clear();
    }
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, _view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        if new_size.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null rect from the caller.
        let r = unsafe { *new_size };
        self.hub.messenger.post(PluginMessage::GuiResize {
            w: (r.right - r.left).max(0) as u32,
            h: (r.bottom - r.top).max(0) as u32,
        });
        kResultOk
    }
}

impl IRunLoopTrait for PlugFrame {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: i32) -> tresult {
        // SAFETY: null or a valid handler from the caller.
        let Some(h) = (unsafe { ComRef::from_raw(handler) }) else {
            return kInvalidArgument;
        };
        lock(&self.handlers).push((h.to_com_ptr(), fd));
        kResultOk
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> tresult {
        let mut handlers = lock(&self.handlers);
        let before = handlers.len();
        handlers.retain(|(h, _)| h.as_ptr() != handler);
        if handlers.len() < before {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: u64) -> tresult {
        // SAFETY: null or a valid handler from the caller.
        let Some(h) = (unsafe { ComRef::from_raw(handler) }) else {
            return kInvalidArgument;
        };
        let period = Duration::from_millis(milliseconds.max(MIN_TIMER_MS));
        lock(&self.timers).push(TimerReg {
            handler: h.to_com_ptr(),
            period,
            next: now() + period,
        });
        kResultOk
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        let mut timers = lock(&self.timers);
        let before = timers.len();
        timers.retain(|t| t.handler.as_ptr() != handler);
        if timers.len() < before {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

// ----- the connection proxy -----------------------------------------------------------------------

/// Sits between component and controller: each half gets a proxy that forwards to the other, so
/// neither holds the other directly (the VST3 hosting rule that lets either side be torn down).
pub(crate) struct ConnectionProxy {
    dst: Mutex<Option<ComPtr<IConnectionPoint>>>,
}

impl Class for ConnectionProxy {
    type Interfaces = (IConnectionPoint,);
}

impl ConnectionProxy {
    pub(crate) fn new() -> ComWrapper<Self> {
        ComWrapper::new(Self {
            dst: Mutex::new(None),
        })
    }

    /// Sets the destination without going through the interface.
    pub(crate) fn set_destination(&self, dst: Option<ComPtr<IConnectionPoint>>) {
        *lock(&self.dst) = dst;
    }
}

impl IConnectionPointTrait for ConnectionProxy {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        // SAFETY: null or a valid connection point from the caller.
        let Some(o) = (unsafe { ComRef::from_raw(other) }) else {
            return kInvalidArgument;
        };
        *lock(&self.dst) = Some(o.to_com_ptr());
        kResultOk
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        *lock(&self.dst) = None;
        kResultOk
    }

    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let dst = lock(&self.dst).clone();
        match dst {
            // SAFETY: forwarding the caller's message to the peer's live connection point.
            Some(d) => unsafe { d.notify(message) },
            None => kResultFalse,
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
