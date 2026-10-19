//! A VST3 library whose module entry never returns: the scanner must kill it at its timeout
//! (`atomik-test-vst3-hang`).

use atomik_test_vst3_shell::{Info, LibraryDef, Passthrough, PluginDef, vst3_library};

static INFO: Info = Info {
    name: "Atomik Test Hang",
    cid: [0xA70F_1C04, 0x4A5B_4C01, 0x8D2E_1F00, 1],
    controller_cid: None,
    sub_categories: "Fx",
    gui: false,
    midi: false,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || Box::new(Passthrough),
}];

fn hang() -> bool {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

vst3_library!(LibraryDef::new(&PLUGINS, hang));
