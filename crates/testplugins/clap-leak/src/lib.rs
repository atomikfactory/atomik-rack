//! A CLAP library for the leak tests: the shell counts instances created and destroyed,
//! activations and deactivations in process-wide counters that every library exports as
//! `atomik_test_counter(index)` (`atomik-test-clap-leak`).

use atomik_test_clap_shell::{
    Info, LibraryDef, Passthrough, PluginDef, clap_library, sys::plugin_features as feat,
};

static INFO: Info = Info {
    id: c"atomik.test.leak",
    name: c"Atomik Test Leak",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || Box::new(Passthrough),
}];

clap_library!(LibraryDef::new(&PLUGINS, || true));
