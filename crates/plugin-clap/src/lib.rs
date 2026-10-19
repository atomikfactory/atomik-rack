//! The CLAP backend of the plugin host: loads `.clap` files,
//! scans them and hosts their plugins through `clack-host` 0.2.0.
//!
//! Nothing outside this crate names a clack or CLAP type: the app and the render tool see only
//! [`ClapBackend`] as a `PluginBackend`, and the instances and processors it creates through the
//! traits of `atomik-plugin-host`.
//!
//! * `hub`: the host callbacks a plugin may make and where each goes (atomics, the instance's
//!   wait-free message ring, plugin timers).
//! * `layout`: ports, notes and params to a `PluginLayout`.
//! * `instance`: the main-thread half (activation, state, param text, editor, polling).
//! * `processor`: the audio half (buffers, events, transport, tail).
//! * `backend`: file loading, scanning, creating instances.
//!
//! `unsafe` is allowed here and kept to the boundary: loading a
//! library, raw audio buffer descriptors, and the editor's parent window handle.

mod backend;
mod hub;
mod instance;
mod layout;
mod processor;

pub use backend::ClapBackend;
