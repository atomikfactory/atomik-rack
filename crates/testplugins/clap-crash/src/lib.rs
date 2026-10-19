//! A CLAP library whose plugin segfaults in `process` when the environment variable
//! `ATOMIK_TEST_CRASH` is set at create time; scanning (which never processes) is safe
//! unless `ATOMIK_TEST_CRASH_ON_CREATE` is set, which crashes the process while the plugin is
//! created (the scanner isolation test) (`atomik-test-clap-crash`).

use atomik_test_clap_shell::{
    Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Status, clap_library,
    sys::plugin_features as feat,
};

struct Crasher {
    crash: bool,
}

impl Dsp for Crasher {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        if self.crash {
            // SAFETY: this is deliberately undefined behaviour: a write through a null pointer
            // crashes the process, which is what this test plugin exists to do.
            unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        }
        io.output = io.input;
        Status::Continue
    }
}

static INFO: Info = Info {
    id: c"atomik.test.crash",
    name: c"Atomik Test Crash",
    features: &[feat::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT],
    gui: false,
    remote_page: None,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || {
        if std::env::var_os("ATOMIK_TEST_CRASH_ON_CREATE").is_some() {
            // SAFETY: deliberately undefined behaviour, as in `process`: the scanner's
            // isolation test needs a plugin that crashes while it is being created.
            unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        }
        Box::new(Crasher {
            crash: std::env::var_os("ATOMIK_TEST_CRASH").is_some(),
        })
    },
}];

clap_library!(LibraryDef::new(&PLUGINS, || true));
