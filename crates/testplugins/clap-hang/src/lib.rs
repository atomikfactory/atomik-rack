//! A CLAP library whose entry `init` never returns: the scanner must kill it at its timeout
//! (`atomik-test-clap-hang`).

use atomik_test_clap_shell::{
    Info, LibraryDef, Passthrough, PluginDef, clap_library, sys::plugin_features as feat,
};

static INFO: Info = Info {
    id: c"atomik.test.hang",
    name: c"Atomik Test Hang",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
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

clap_library!(LibraryDef::new(&PLUGINS, hang));
