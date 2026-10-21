//! Atomik Rack app shell.
//!
//! The crate is split into a window-free, unit-tested core and a thin platform layer:
//!
//! - [`AppCore`] owns the [`atomik_model::Document`], the device [`atomik_device_sdk::Registry`],
//!   the [`atomik_ui::RackView`], the device key ↔ engine id map and the engine
//!   [`atomik_engine::Topology`]. It applies UI actions and file actions and translates every
//!   [`atomik_model::SyncOp`] into calls on an [`EngineSink`].
//! - [`EngineSink`] is implemented by the live session and by [`sink::RecordingSink`] in tests.

pub mod app_core;
pub mod audio_import;
pub mod audio_record;
pub mod audio_writer;
pub mod browser_workers;
pub mod cli;
pub mod clicks;
pub mod clipboard;
pub mod commands;
pub mod context_menus;
pub mod keyboard_tab;
pub mod keymap_store;
pub mod library_store;
pub mod menubar;
pub mod midi;
#[cfg(test)]
pub(crate) mod offline_sink;
pub mod platform;
pub mod plugin_setup;
#[cfg(test)]
pub(crate) mod plugin_test_support;
pub mod record;
pub mod session;
pub mod settings;
pub mod settings_dialog;
pub mod shell;
pub mod sink;
pub mod status;
pub mod stream_source;
pub mod stress;
pub mod target;
pub mod translate;
pub mod ui_state;
pub mod workflow_tab;

pub use crate::app_core::{AppCore, AppError};
pub use crate::sink::EngineSink;
