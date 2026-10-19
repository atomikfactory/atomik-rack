//! A CLAP library whose entry `init` fails: the scanner and the backend must report it as
//! failed and carry on (`atomik-test-clap-bad`).

use atomik_test_clap_shell::{
    Info, LibraryDef, Passthrough, PluginDef, clap_library, sys::plugin_features as feat,
};

static INFO: Info = Info {
    id: c"atomik.test.bad",
    name: c"Atomik Test Bad",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || Box::new(Passthrough),
}];

clap_library!(LibraryDef::new(&PLUGINS, || false));
