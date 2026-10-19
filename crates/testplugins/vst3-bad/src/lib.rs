//! A VST3 library whose module entry fails: the scanner and the backend must report it as
//! failed and carry on (`atomik-test-vst3-bad`).

use atomik_test_vst3_shell::{Info, LibraryDef, Passthrough, PluginDef, vst3_library};

static INFO: Info = Info {
    name: "Atomik Test Bad",
    cid: [0xA70F_1C03, 0x4A5B_4C01, 0x8D2E_1F00, 1],
    controller_cid: None,
    sub_categories: "Fx",
    gui: false,
    midi: false,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || Box::new(Passthrough),
}];

vst3_library!(LibraryDef::new(&PLUGINS, || false));
