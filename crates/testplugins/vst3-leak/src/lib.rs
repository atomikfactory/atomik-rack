//! A VST3 library for the leak tests: the shell counts instances created and destroyed,
//! activations and deactivations in process-wide counters that every library exports as
//! `atomik_test_counter(index)` (`atomik-test-vst3-leak`).

use atomik_test_vst3_shell::{Info, LibraryDef, Passthrough, PluginDef, vst3_library};

static INFO: Info = Info {
    name: "Atomik Test Leak",
    cid: [0xA70F_1C05, 0x4A5B_4C01, 0x8D2E_1F00, 1],
    controller_cid: None,
    sub_categories: "Fx",
    gui: false,
    midi: false,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || Box::new(Passthrough),
}];

vst3_library!(LibraryDef::new(&PLUGINS, || true));
