//! Whether a plugin has an editor is asked of the plugin once, when it is first wanted: creating
//! an instance and scanning a library build no view. Alone in its test binary because the
//! counters of the test library are process-wide (the tests still take turns).

mod support;

use atomik_plugin_host::NativeParent;
use support::*;

fn parent() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xABCD)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xABCD)
    } else {
        NativeParent::X11(0xABCD)
    }
}

#[test]
fn a_scan_of_a_library_builds_no_editor_view() {
    let _turn = serial();
    let k = kit(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    let (asks, created) = (
        lib.counter(counter::VIEW_ASKS),
        lib.counter(counter::CREATED),
    );
    let found = k.backend.scan_file(&k.path).unwrap();
    assert_eq!(found.len(), 4);
    assert!(
        lib.counter(counter::CREATED) >= created + 4,
        "the scan created the plugins"
    );
    assert_eq!(
        lib.counter(counter::VIEW_ASKS),
        asks,
        "no plugin was asked for a view"
    );
}

#[test]
fn has_gui_asks_the_plugin_once_however_often_it_is_called() {
    let _turn = serial();
    let (k, lib) = {
        let k = kit(Format::Vst3);
        let lib = TestLib::open_path(&k.path);
        (k, lib)
    };
    let (inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    assert_eq!(
        lib.probe(&*inst, probe::VIEW_ASKS),
        0,
        "creating asks nothing"
    );
    assert!(inst.has_gui());
    assert!(inst.has_gui());
    assert!(inst.has_gui());
    assert_eq!(lib.probe(&*inst, probe::VIEW_ASKS), 1);
    assert_eq!(
        lib.probe(&*inst, probe::VIEWS_CREATED),
        1,
        "the probe's view"
    );
    let (inst, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
    assert!(!inst.has_gui());
    assert!(!inst.has_gui());
    assert_eq!(
        lib.probe(&*inst, probe::VIEW_ASKS),
        1,
        "a plugin without an editor is asked once, too"
    );
}

#[test]
fn a_plugin_whose_editor_was_opened_is_not_probed() {
    let _turn = serial();
    let k = kit(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    let (mut inst, _m) = bare(&*k.backend, &k.plugin(Plug::Gain));
    inst.open_gui(parent(), 1.0).unwrap();
    assert!(inst.has_gui());
    inst.close_gui();
    assert!(inst.has_gui(), "the answer outlives the editor");
    assert_eq!(
        lib.probe(&*inst, probe::VIEW_ASKS),
        1,
        "the view that was opened is the only one asked for"
    );
    assert_eq!(lib.probe(&*inst, probe::VIEWS_CREATED), 1);
}
