//! Tests of the host objects of `host.rs`: what a plugin may call, called here directly.
// Justification: exact floats copied through attributes and values are compared.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_plugin_host::{InstanceId, ManagerEnd, plugin_channel};
use std::sync::atomic::AtomicUsize;

fn hub() -> (Arc<Hub>, ManagerEnd) {
    let (_device, manager) = plugin_channel(InstanceId(7));
    (Hub::new(manager.messenger()), manager)
}

fn drain(m: &ManagerEnd) -> Vec<PluginMessage> {
    std::iter::from_fn(|| m.try_recv()).collect()
}

#[test]
fn the_host_application_names_itself_and_makes_messages_and_attribute_lists() {
    let app = ComWrapper::new(HostApplication);
    let r = app.as_com_ref::<IHostApplication>().unwrap();
    let mut name: String128 = [0; 128];
    // SAFETY: valid pointers for every call of this test.
    unsafe {
        assert_eq!(r.getName(&mut name), kResultOk);
        assert_eq!(crate::util::utf16_text(&name), "Atomik Rack");
        let (mut cid, mut iid) = (IMessage_iid, IMessage_iid);
        let mut obj = std::ptr::null_mut();
        assert_eq!(r.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
        let msg = ComPtr::<IMessage>::from_raw(obj.cast()).unwrap();
        msg.setMessageID(c"hello".as_ptr());
        assert_eq!(CStr::from_ptr(msg.getMessageID()), c"hello");
        let attrs = ComRef::from_raw(msg.getAttributes()).unwrap();
        assert_eq!(attrs.setInt(c"n".as_ptr(), 42), kResultOk);
        let mut n = 0i64;
        assert_eq!(attrs.getInt(c"n".as_ptr(), &mut n), kResultOk);
        assert_eq!(n, 42);
        let (mut cid, mut iid) = (IAttributeList_iid, IAttributeList_iid);
        assert_eq!(r.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
        drop(ComPtr::<IAttributeList>::from_raw(obj.cast()));
        let (mut cid, mut iid) = (IPlugFrame_iid, IPlugFrame_iid);
        assert_eq!(r.createInstance(&mut cid, &mut iid, &mut obj), kNoInterface);
        assert_eq!(
            r.createInstance(std::ptr::null_mut(), &mut iid, &mut obj),
            kInvalidArgument
        );
    }
}

#[test]
fn the_host_says_which_interfaces_it_supports() {
    let app = ComWrapper::new(HostApplication);
    let s = app.as_com_ref::<IPlugInterfaceSupport>().unwrap();
    // SAFETY: valid pointers.
    unsafe {
        assert_eq!(
            s.isPlugInterfaceSupported(&IComponentHandler_iid),
            kResultTrue
        );
        assert_eq!(s.isPlugInterfaceSupported(&IPlugFrame_iid), kResultTrue);
        assert_eq!(s.isPlugInterfaceSupported(&IMessage_iid), kResultFalse);
        assert_eq!(
            s.isPlugInterfaceSupported(std::ptr::null()),
            kInvalidArgument
        );
    }
}

#[test]
fn attributes_keep_their_type_and_strings_are_cut_to_the_buffer() {
    let a = ComWrapper::new(HostAttributes::default());
    let r = a.as_com_ref::<IAttributeList>().unwrap();
    let text: Vec<u16> = "gr\u{fc}\u{df}e\0".encode_utf16().collect();
    let blob = [1u8, 2, 3];
    // SAFETY: valid pointers and buffers.
    unsafe {
        assert_eq!(r.setFloat(c"f".as_ptr(), 0.5), kResultOk);
        assert_eq!(r.setString(c"s".as_ptr(), text.as_ptr()), kResultOk);
        assert_eq!(
            r.setBinary(c"b".as_ptr(), blob.as_ptr().cast(), 3),
            kResultOk
        );
        let mut f = 0.0;
        assert_eq!(r.getFloat(c"f".as_ptr(), &mut f), kResultOk);
        assert_eq!(f, 0.5);
        let mut i = 0i64;
        assert_eq!(
            r.getInt(c"f".as_ptr(), &mut i),
            kResultFalse,
            "a float is not an int"
        );
        assert_eq!(r.getInt(c"nope".as_ptr(), &mut i), kResultFalse);
        let mut buf = [0u16; 4];
        assert_eq!(r.getString(c"s".as_ptr(), buf.as_mut_ptr(), 8), kResultOk);
        assert_eq!(
            crate::util::utf16_text(&buf),
            "gr\u{fc}",
            "three units and the terminator fit"
        );
        let (mut p, mut n) = (std::ptr::null(), 0u32);
        assert_eq!(r.getBinary(c"b".as_ptr(), &mut p, &mut n), kResultOk);
        assert_eq!(std::slice::from_raw_parts(p.cast::<u8>(), n as usize), blob);
        // Setting a key again replaces it, whatever its old type.
        assert_eq!(r.setInt(c"f".as_ptr(), 9), kResultOk);
        assert_eq!(r.getInt(c"f".as_ptr(), &mut i), kResultOk);
        assert_eq!(i, 9);
        assert_eq!(r.setInt(std::ptr::null(), 1), kInvalidArgument);
    }
}

#[test]
fn editor_edits_become_gestures_with_the_rack_values_and_reach_the_processor() {
    let (hub, m) = hub();
    hub.set_steps(vec![(3, 512)]);
    let (producer, mut consumer) = rtrb::RingBuffer::new(8);
    hub.set_edit_sink(Some(producer));
    let h = ComWrapper::new(ComponentHandler {
        hub: Arc::clone(&hub),
    });
    let r = h.as_com_ref::<IComponentHandler>().unwrap();
    // SAFETY: plain calls.
    unsafe {
        r.beginEdit(3);
        r.performEdit(3, 37.0 / 512.0);
        r.performEdit(9, 0.25);
        r.endEdit(3);
    }
    let got = drain(&m);
    assert_eq!(got.len(), 4);
    assert!(
        matches!(got[0], PluginMessage::ParamEdit { id: 3, phase: ParamPhase::Begin, value } if value.is_nan())
    );
    assert_eq!(
        got[1],
        PluginMessage::ParamEdit {
            id: 3,
            value: 37.0,
            phase: ParamPhase::Change
        },
        "a stepped param is reported as its step index"
    );
    assert_eq!(
        got[2],
        PluginMessage::ParamEdit {
            id: 9,
            value: 0.25,
            phase: ParamPhase::Change
        }
    );
    assert!(
        matches!(got[3], PluginMessage::ParamEdit { id: 3, phase: ParamPhase::End, value } if value.is_nan())
    );
    assert_eq!(
        consumer.pop(),
        Ok((3, 37.0 / 512.0)),
        "the processor gets the normalised value"
    );
    assert_eq!(consumer.pop(), Ok((9, 0.25)));
    assert!(consumer.pop().is_err());
}

#[test]
fn a_full_edit_ring_drops_and_counts_but_the_app_still_hears_the_edit() {
    let (hub, m) = hub();
    let (producer, _consumer) = rtrb::RingBuffer::new(1);
    hub.set_edit_sink(Some(producer));
    for _ in 0..3 {
        hub.edit(1, 0.5);
    }
    assert_eq!(hub.edits_dropped.load(Ordering::Relaxed), 2);
    assert_eq!(drain(&m).len(), 3);
}

#[test]
fn restart_requests_dirty_flags_and_open_requests_become_messages() {
    let (hub, m) = hub();
    let h = ComWrapper::new(ComponentHandler {
        hub: Arc::clone(&hub),
    });
    let r = h.as_com_ref::<IComponentHandler>().unwrap();
    let r2 = h.as_com_ref::<IComponentHandler2>().unwrap();
    // SAFETY: plain calls.
    unsafe {
        r.restartComponent(8);
        assert_eq!(drain(&m), vec![PluginMessage::LatencyChanged]);
        r.restartComponent(16);
        assert_eq!(drain(&m), vec![PluginMessage::RescanParams]);
        assert!(hub.layout_dirty.swap(false, Ordering::Acquire));
        r.restartComponent(1);
        let got = drain(&m);
        assert!(got.contains(&PluginMessage::RescanParams));
        assert!(got.contains(&PluginMessage::RequestRestart));
        r.restartComponent(32);
        assert_eq!(drain(&m), vec![PluginMessage::RequestRestart]);
        assert!(hub.midi_dirty.swap(false, Ordering::Acquire));
        r.restartComponent(4);
        assert_eq!(drain(&m), vec![PluginMessage::StateDirty]);
        r.restartComponent(0);
        assert!(drain(&m).is_empty());
        r2.setDirty(1);
        assert_eq!(drain(&m), vec![PluginMessage::StateDirty]);
        r2.setDirty(0);
        assert!(drain(&m).is_empty(), "a clean flag is not an edit");
        r2.requestOpenEditor(std::ptr::null());
        assert_eq!(drain(&m), vec![PluginMessage::GuiRequestShow]);
        assert_eq!(r2.startGroupEdit(), kResultOk);
        assert_eq!(r2.finishGroupEdit(), kResultOk);
    }
}

struct Ticker(Arc<AtomicUsize>);

impl Class for Ticker {
    type Interfaces = (ITimerHandler,);
}

impl ITimerHandlerTrait for Ticker {
    unsafe fn onTimer(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn plugin_timers_run_when_due_and_stop_when_unregistered() {
    let (hub, m) = hub();
    let frame = PlugFrame::new(hub);
    let count = Arc::new(AtomicUsize::new(0));
    let t = ComWrapper::new(Ticker(Arc::clone(&count)));
    let handler = t.as_com_ref::<ITimerHandler>().unwrap();
    let r = frame.as_com_ref::<IRunLoop>().unwrap();
    // SAFETY: valid handler pointers.
    unsafe {
        assert_eq!(r.registerTimer(handler.as_ptr(), 1), kResultOk);
        assert_eq!(r.registerTimer(std::ptr::null_mut(), 1), kInvalidArgument);
    }
    assert_eq!(
        frame.run_timers(),
        0,
        "not due yet: the shortest period is 10 ms"
    );
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(frame.run_timers(), 1);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(frame.run_timers(), 0, "advanced to the next tick");
    // SAFETY: valid handler pointers.
    unsafe {
        assert_eq!(r.unregisterTimer(handler.as_ptr()), kResultOk);
        assert_eq!(r.unregisterTimer(handler.as_ptr()), kResultFalse);
    }
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(frame.run_timers(), 0);
    // A resize request goes to the app.
    let f = frame.as_com_ref::<IPlugFrame>().unwrap();
    let mut rect = ViewRect {
        left: 10,
        top: 20,
        right: 510,
        bottom: 320,
    };
    // SAFETY: a valid rect.
    unsafe {
        assert_eq!(f.resizeView(std::ptr::null_mut(), &mut rect), kResultOk);
        assert_eq!(
            f.resizeView(std::ptr::null_mut(), std::ptr::null_mut()),
            kInvalidArgument
        );
    }
    assert_eq!(drain(&m), vec![PluginMessage::GuiResize { w: 500, h: 300 }]);
    frame.clear();
}

struct Sink(Arc<AtomicUsize>);

impl Class for Sink {
    type Interfaces = (IConnectionPoint,);
}

impl IConnectionPointTrait for Sink {
    unsafe fn connect(&self, _o: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn disconnect(&self, _o: *mut IConnectionPoint) -> tresult {
        kResultOk
    }
    unsafe fn notify(&self, _m: *mut IMessage) -> tresult {
        self.0.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
}

#[test]
fn the_connection_proxy_forwards_to_its_destination_and_stops_when_cut() {
    let seen = Arc::new(AtomicUsize::new(0));
    let sink = ComWrapper::new(Sink(Arc::clone(&seen)));
    let proxy = ConnectionProxy::new();
    let p = proxy.as_com_ref::<IConnectionPoint>().unwrap();
    // SAFETY: valid pointers.
    unsafe {
        assert_eq!(
            p.notify(std::ptr::null_mut()),
            kResultFalse,
            "nowhere to go yet"
        );
        assert_eq!(
            p.connect(sink.as_com_ref::<IConnectionPoint>().unwrap().as_ptr()),
            kResultOk
        );
        assert_eq!(p.notify(std::ptr::null_mut()), kResultOk);
        assert_eq!(seen.load(Ordering::Relaxed), 1);
        assert_eq!(p.connect(std::ptr::null_mut()), kInvalidArgument);
        assert_eq!(p.disconnect(std::ptr::null_mut()), kResultOk);
        assert_eq!(p.notify(std::ptr::null_mut()), kResultFalse);
    }
    proxy.set_destination(sink.to_com_ptr::<IConnectionPoint>());
    // SAFETY: a valid proxy pointer.
    assert_eq!(unsafe { p.notify(std::ptr::null_mut()) }, kResultOk);
    assert_eq!(seen.load(Ordering::Relaxed), 2);
}

#[cfg(target_os = "linux")]
struct FdWatcher(Arc<AtomicUsize>);

#[cfg(target_os = "linux")]
impl Class for FdWatcher {
    type Interfaces = (IEventHandler,);
}

#[cfg(target_os = "linux")]
impl vst3::Steinberg::Linux::IEventHandlerTrait for FdWatcher {
    unsafe fn onFDIsSet(&self, _fd: i32) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn plugin_event_handlers_run_when_their_fd_is_readable_and_stop_when_unregistered() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    let (hub, _m) = hub();
    let frame = PlugFrame::new(hub);
    let count = Arc::new(AtomicUsize::new(0));
    let w = ComWrapper::new(FdWatcher(Arc::clone(&count)));
    let handler = w.as_com_ref::<IEventHandler>().unwrap();
    let r = frame.as_com_ref::<IRunLoop>().unwrap();
    let (mut ours, mut theirs) = UnixStream::pair().unwrap();
    // SAFETY: valid handler pointers.
    unsafe {
        assert_eq!(
            r.registerEventHandler(handler.as_ptr(), theirs.as_raw_fd()),
            kResultOk
        );
        assert_eq!(
            r.registerEventHandler(std::ptr::null_mut(), 0),
            kInvalidArgument
        );
    }
    assert_eq!(frame.run_event_handlers(), 0, "nothing to read yet");
    ours.write_all(&[1]).unwrap();
    assert_eq!(frame.run_event_handlers(), 1);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    let mut byte = [0u8];
    theirs.read_exact(&mut byte).unwrap();
    assert_eq!(frame.run_event_handlers(), 0, "drained");
    // SAFETY: valid handler pointers.
    unsafe {
        assert_eq!(r.unregisterEventHandler(handler.as_ptr()), kResultOk);
    }
    ours.write_all(&[1]).unwrap();
    assert_eq!(frame.run_event_handlers(), 0, "unregistered");
    assert_eq!(count.load(Ordering::Relaxed), 1);
    frame.clear();
}
