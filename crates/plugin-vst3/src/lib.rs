//! The VST3 backend of the plugin host: loads `.vst3`
//! modules, scans them and hosts their plugins over the `vst3` crate (bindings generated from the
//! MIT-licensed VST3 SDK 3.8.0 headers).
//!
//! Nothing outside this crate names a VST3 type: the app and the render tool see only
//! [`Vst3Backend`] as a `PluginBackend`, and the instances and processors it creates through the
//! traits of `atomik-plugin-host`.
//!
//! * `module`: file and bundle layout per operating system, module entry and exit, factory.
//! * `layout`: factory classes, bus negotiation, parameters, units, MIDI mapping, kinds.
//! * `host`: the host objects a plugin calls (`IHostApplication`, `IComponentHandler`,
//!   `IPlugFrame`, `IRunLoop`, connection proxies, messages).
//! * `params`, `events`, `stream`: the fixed-capacity parameter and event lists and the state
//!   stream.
//! * `instance`: the main-thread half (activation, state, param text, editor, polling).
//! * `processor`: the audio half (buffers, events, transport, tail).
//! * `backend`: file loading, scanning, creating instances.
//!
//! `unsafe` is allowed here and kept to the boundary: loading a module,
//! calling and implementing COM interfaces, raw audio buffers.
//!
//! VST3 is a trademark of Steinberg Media Technologies GmbH. Atomik Rack uses the name only to
//! say which plugin format it hosts, and uses no VST logo.

// Justification: the generated VST3 integer types differ per platform (`int32` enums are `c_int`
// on Windows and `c_uint` elsewhere), so a cast that is a no-op on one host is needed on another.
#![allow(clippy::unnecessary_cast)]

mod backend;
mod events;
mod host;
mod ids;
mod instance;
mod layout;
mod module;
mod params;
mod presets;
mod processor;
mod stream;
mod util;
mod values;

pub use backend::Vst3Backend;

/// A `.vstpreset` file for the plugin with class id `class_id` (32 hex digits) holding the
/// state a plugin saved: the way to write a preset a host or another tool can open again.
///
/// # Errors
/// The blob is not a VST3 state of this host.
pub fn vstpreset_from_state(
    class_id: &str,
    state: &atomik_device_sdk::PluginStateBlob,
) -> Result<Vec<u8>, String> {
    let (component, controller) =
        instance::unpack_state(&state.0).ok_or_else(|| "not a VST3 state".to_string())?;
    Ok(presets::write(class_id, component, controller))
}
