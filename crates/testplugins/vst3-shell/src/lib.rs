//! A minimal VST3 plugin shell over the `vst3` crate for the Atomik Rack test plugins.
//! The plugin libraries (`atomik-test-vst3` and its isolation siblings)
//! implement [`Dsp`] and declare a [`LibraryDef`]; this crate turns that into a `.vst3` module:
//! factory, `IComponent`, `IAudioProcessor`, `IEditController`, `IMidiMapping`, `IUnitInfo`,
//! note expressions, a headless `IPlugView`, a split variant that talks through
//! `IConnectionPoint`, state through `IBStream`, and the module entry points.
//!
//! **Test hooks.** Every library also exports `atomik_test_probe(handle, index)` (what the
//! instance saw, see [`probe`]), `atomik_test_command(handle, command, a, b)` (make it call the
//! host from the calling thread, see [`command`]) and `atomik_test_counter(index)` (process-wide
//! counters, see [`counter`]). The handle is the instance's `IComponent` pointer.
//!
//! A VST3 parameter is normalised; the shell maps it to the DSP's plain value linearly
//! (`atomik_test_plugin_core::plain_from_normalized`), a stepped parameter to a whole step index
//! from `min` (`stepCount = max - min`).

// Justification: the generated VST3 integer types differ per platform (`int32` enums are `c_int`
// on Windows and `c_uint` elsewhere), so a cast that is a no-op on one host is needed on another.
#![allow(clippy::unnecessary_cast)]

mod api;
mod classes;
mod component;
mod controller;
mod hooks;
mod util;

pub use api::{
    Arrangement, Dsp, Info, Io, LibraryDef, ParamSpec, Passthrough, PluginDef, Ports, Status,
};
pub use classes::make_factory;
pub use component::{MIDI_ALL_OFF, MIDI_BEND, MIDI_MOD, MIDI_PRESSURE};
pub use controller::command_run;
pub use hooks::{command, counter, counter_read, probe, probe_read};
pub use vst3;

/// Module entry: counts the call and runs the library's own entry.
pub fn lib_init(lib: &'static LibraryDef) -> bool {
    hooks::bump(counter::ENTRY_INIT);
    (lib.init)()
}

/// Module exit: counts the call.
pub fn lib_exit(_lib: &'static LibraryDef) -> bool {
    hooks::bump(counter::ENTRY_DEINIT);
    true
}

/// Exports the module entry points, `GetPluginFactory` and the test hooks for a library:
/// `vst3_library!(LibraryDef::new(&PLUGINS, || true));`.
#[macro_export]
macro_rules! vst3_library {
    ($lib:expr) => {
        static __ATOMIK_LIBRARY: $crate::LibraryDef = $lib;

        /// Module entry (Windows).
        #[cfg(windows)]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn InitDll() -> bool {
            $crate::lib_init(&__ATOMIK_LIBRARY)
        }

        /// Module exit (Windows).
        #[cfg(windows)]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn ExitDll() -> bool {
            $crate::lib_exit(&__ATOMIK_LIBRARY)
        }

        /// Module entry (macOS).
        #[cfg(target_os = "macos")]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleEntry(_bundle: *mut ::std::ffi::c_void) -> bool {
            $crate::lib_init(&__ATOMIK_LIBRARY)
        }

        /// Module exit (macOS).
        #[cfg(target_os = "macos")]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleExit() -> bool {
            $crate::lib_exit(&__ATOMIK_LIBRARY)
        }

        /// Module entry (Linux).
        #[cfg(all(unix, not(target_os = "macos")))]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn ModuleEntry(_handle: *mut ::std::ffi::c_void) -> bool {
            $crate::lib_init(&__ATOMIK_LIBRARY)
        }

        /// Module exit (Linux).
        #[cfg(all(unix, not(target_os = "macos")))]
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn ModuleExit() -> bool {
            $crate::lib_exit(&__ATOMIK_LIBRARY)
        }

        /// The plugin factory.
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub extern "system" fn GetPluginFactory() -> *mut $crate::vst3::Steinberg::IPluginFactory {
            $crate::make_factory(&__ATOMIK_LIBRARY)
        }

        /// Test hook: what the plugin instance saw (see `atomik_test_vst3_shell::probe`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_probe(handle: usize, index: u32) -> u64 {
            $crate::probe_read(handle, index)
        }

        /// Test hook: make the plugin call into the host (see `atomik_test_vst3_shell::command`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_command(handle: usize, cmd: u32, a: u64, b: u64) -> u64 {
            $crate::command_run(handle, cmd, a, b)
        }

        /// Test hook: process-wide instance counters (see `atomik_test_vst3_shell::counter`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_counter(index: u32) -> u64 {
            $crate::counter_read(index)
        }
    };
}
