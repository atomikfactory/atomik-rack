//! A VST3 library whose plugin segfaults in `process` when the environment variable
//! `ATOMIK_TEST_CRASH` is set at create time; scanning (which never processes) is safe unless
//! `ATOMIK_TEST_CRASH_ON_CREATE` is set, which crashes the process while the plugin is created
//! (the scanner isolation test) (`atomik-test-vst3-crash`).

use atomik_test_vst3_shell::{
    Dsp, Info, Io, LibraryDef, ParamSpec, PluginDef, Ports, Status, vst3_library,
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
    name: "Atomik Test Crash",
    cid: [0xA70F_1C06, 0x4A5B_4C01, 0x8D2E_1F00, 1],
    controller_cid: None,
    sub_categories: "Fx",
    gui: false,
    midi: false,
};

static PLUGINS: [PluginDef; 1] = [PluginDef {
    info: &INFO,
    create: || {
        if std::env::var_os("ATOMIK_TEST_CRASH_ON_CREATE").is_some() {
            // SAFETY: deliberately undefined behaviour, as in `process`: the scanner isolation
            // test needs a plugin that crashes while it is being created.
            unsafe { std::ptr::null_mut::<u8>().write_volatile(1) };
        }
        Box::new(Crasher {
            crash: std::env::var_os("ATOMIK_TEST_CRASH").is_some(),
        })
    },
}];

vst3_library!(LibraryDef::new(&PLUGINS, || true));
