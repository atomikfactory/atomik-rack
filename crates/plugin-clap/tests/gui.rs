//! GUI plumbing of the CLAP backend (host half): create, set scale, get size,
//! set parent, show, resize both ways, the plugin's timers, close, and the plugins that have no
//! editor. The test plugin's editor is headless: it records what the host asked for.

mod support;

use atomik_plugin_host::{
    BackendError, InstanceId, ManagerError, ManagerEvent, NativeParent, PluginManager,
};
use std::time::Duration;
use support::*;

const PKG: &str = "atomik-test-clap";

const CREATE: u64 = 1;
const SET_SCALE: u64 = 2;
const GET_SIZE: u64 = 4;
const SET_PARENT: u64 = 8;
const SHOW: u64 = 16;
const HIDE: u64 = 32;
const DESTROY: u64 = 64;
const SET_SIZE: u64 = 128;

fn parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0x4242)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0x4242)
    } else {
        NativeParent::X11(0x4242)
    }
}

fn other_window_system() -> NativeParent {
    if cfg!(windows) {
        NativeParent::X11(7)
    } else {
        NativeParent::Win32(7)
    }
}

fn open(m: &mut PluginManager, id: &str) -> (InstanceId, usize) {
    let handle = m.instantiate(&request(id)).expect("instantiates");
    let native = m.native_handle(handle.id).unwrap();
    // The device is not needed: keep it alive inside the manager's reaper-free life by leaking
    // it into the test's scope.
    std::mem::forget(handle.device);
    (handle.id, native)
}

#[test]
fn opening_the_editor_creates_scales_sizes_parents_and_shows_it() {
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (id, native) = open(&mut m, "atomik.test.gain");
    assert!(m.has_gui(id));
    let size = m.open_gui(id, parent(), 1.5).expect("opens");
    assert_eq!(size, (320, 200));
    let calls = lib.probe_raw(native, probe::GUI_CALLS);
    let want = CREATE
        | GET_SIZE
        | SET_PARENT
        | SHOW
        | if cfg!(target_os = "macos") {
            0
        } else {
            SET_SCALE
        };
    assert_eq!(calls & want, want, "{calls:#b}");
    assert_eq!(lib.probe_raw(native, probe::GUI_PARENT), 0x4242);
    if !cfg!(target_os = "macos") {
        assert_eq!(lib.probe_f64_raw(native, probe::GUI_SCALE), 1.5);
    } else {
        assert_eq!(
            calls & SET_SCALE,
            0,
            "Cocoa uses logical sizes: no set_scale"
        );
    }
    // Opening twice is refused.
    let err = m.open_gui(id, parent(), 1.0).unwrap_err();
    assert!(
        matches!(err, ManagerError::Backend(BackendError::Failed(_))),
        "{err:?}"
    );
}

#[test]
fn the_window_and_the_plugin_resize_each_other() {
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (id, native) = open(&mut m, "atomik.test.gain");
    m.open_gui(id, parent(), 1.0).unwrap();
    // Window to plugin: the plugin adjusts the size (even, at least 100 x 80).
    let size = m.resize_gui(id, 501, 377).unwrap();
    assert_eq!(size, (500, 376));
    assert_eq!(lib.probe_raw(native, probe::GUI_WIDTH), 500);
    assert_eq!(lib.probe_raw(native, probe::GUI_HEIGHT), 376);
    assert_ne!(lib.probe_raw(native, probe::GUI_CALLS) & SET_SIZE, 0);
    let small = m.resize_gui(id, 10, 10).unwrap();
    assert_eq!(small, (100, 80), "the plugin's minimum");
    // Plugin to window: request_resize from another thread arrives as an event on poll.
    let path = lib.path.clone();
    std::thread::spawn(move || {
        // SAFETY: the test library is loaded by the test; the handle is a live instance.
        let l = unsafe { libloading::Library::new(&path) }.unwrap();
        // SAFETY: the symbol is exported by every test plugin library with this signature.
        let f: libloading::Symbol<'_, unsafe extern "C" fn(usize, u32, u64, u64) -> u64> =
            unsafe { l.get(b"atomik_test_command\0") }.unwrap();
        // SAFETY: the handle is a live instance of this library.
        unsafe { f(native, command::GUI_REQUEST_RESIZE, 640, 480) };
    })
    .join()
    .unwrap();
    let events = m.poll();
    assert_eq!(
        events,
        vec![ManagerEvent::GuiResize {
            instance: id,
            w: 640,
            h: 480
        }]
    );
    // The app answers by resizing the plugin's view to the window.
    assert_eq!(m.resize_gui(id, 640, 480).unwrap(), (640, 480));
}

#[test]
fn closing_hides_and_destroys_the_view_and_the_editor_can_be_reopened() {
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (id, native) = open(&mut m, "atomik.test.gain");
    m.close_gui(id).unwrap(); // not open: harmless
    assert_eq!(lib.probe_raw(native, probe::GUI_CALLS) & DESTROY, 0);
    m.open_gui(id, parent(), 1.0).unwrap();
    m.close_gui(id).unwrap();
    let calls = lib.probe_raw(native, probe::GUI_CALLS);
    assert_ne!(calls & HIDE, 0);
    assert_ne!(calls & DESTROY, 0);
    assert!(
        m.resize_gui(id, 400, 300).is_err(),
        "closed: nothing to resize"
    );
    m.open_gui(id, parent(), 1.0).expect("opens again");
    m.close_gui(id).unwrap();
}

#[test]
fn plugin_timers_run_from_poll_while_the_editor_is_open() {
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let (id, native) = open(&mut m, "atomik.test.gain");
    m.open_gui(id, parent(), 1.0).unwrap();
    // Plugin timers are wall-clock by nature: poll for up to 3 s (250 polls of 12 ms).
    for _ in 0..250 {
        if lib.probe_raw(native, probe::TIMER_TICKS) >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(12));
        m.poll();
    }
    let ticks = lib.probe_raw(native, probe::TIMER_TICKS);
    assert!(ticks >= 3, "the 10 ms timer fired {ticks} times");
    m.close_gui(id).unwrap();
    let after_close = lib.probe_raw(native, probe::TIMER_TICKS);
    std::thread::sleep(Duration::from_millis(40));
    m.poll();
    assert_eq!(
        lib.probe_raw(native, probe::TIMER_TICKS),
        after_close,
        "the timer was unregistered with the editor"
    );
}

#[test]
fn a_plugin_without_an_editor_has_no_gui_and_a_foreign_window_system_is_refused() {
    let mut m = manager_for(&[PKG]);
    let (synth, _) = open(&mut m, "atomik.test.synth");
    assert!(!m.has_gui(synth));
    let err = m.open_gui(synth, parent(), 1.0).unwrap_err();
    assert!(
        matches!(err, ManagerError::Backend(BackendError::Unsupported(_))),
        "{err:?}"
    );
    let (gain, _) = open(&mut m, "atomik.test.gain");
    let err = m.open_gui(gain, other_window_system(), 1.0).unwrap_err();
    assert!(
        matches!(err, ManagerError::Backend(BackendError::Unsupported(_))),
        "{err:?}"
    );
}

#[test]
fn the_default_pins_are_the_first_remote_controls_page() {
    let mut m = manager_for(&[PKG]);
    let (synth, _) = open(&mut m, "atomik.test.synth");
    assert_eq!(m.default_pins(synth), vec![2, 1]);
    let (gain, _) = open(&mut m, "atomik.test.gain");
    assert!(m.default_pins(gain).is_empty());
}
