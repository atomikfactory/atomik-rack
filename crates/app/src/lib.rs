//! Atomik Rack app shell.
//!
//! The crate is split into a window-free, unit-tested core and a thin platform layer:
//!
//! - [`AppCore`] owns the [`atomik_model::Document`], the device [`atomik_device_sdk::Registry`],
//!   the [`atomik_ui::RackView`], the device key ↔ engine id map and the engine
//!   [`atomik_engine::Topology`]. It applies UI actions and file actions and translates every
//!   [`atomik_model::SyncOp`] into calls on an [`EngineSink`].
//! - [`EngineSink`] is implemented by the live session and by [`sink::RecordingSink`] in tests.

pub mod about_box;
pub mod app_core;
pub mod app_icon;
pub mod archive_worker;
pub mod audio_import;
pub mod audio_record;
pub mod audio_relink;
pub mod audio_writer;
pub mod bass_read_worker;
pub mod blind_ab;
pub mod browser_workers;
pub mod cli;
pub mod clicks;
pub mod clipboard;
pub mod clipboard_os;
pub mod commands;
pub mod context_menus;
pub mod controller_detect;
pub mod controller_driver;
pub mod controller_profile;
pub mod controller_wizard;
pub mod controller_wizard_view;
pub mod crash;
pub mod cursor_art;
pub mod delivery_worker;
pub mod device_info;
pub mod diagnostics;
pub mod drag_cursor;
pub mod energy_worker;
pub mod export_prefs;
pub mod export_worker;
pub mod factory;
pub mod freeze_worker;
pub mod guide;
pub mod import_warp;
pub(crate) mod instrument_import;
pub mod interface_tab;
pub mod keyboard_tab;
pub mod keymap_store;
pub mod kick_bass;
pub mod library_store;
pub mod logging;
pub mod masking_worker;
pub mod master_keyboard;
pub use atomik_ui::menu_layout;
pub mod menubar;
pub mod midi;
pub mod midi_control;
pub mod midi_file;
pub mod mixcheck_worker;
#[cfg(test)]
pub(crate) mod offline_sink;
pub mod platform;
pub mod plugin_setup;
#[cfg(test)]
pub(crate) mod plugin_test_support;
pub mod recent;
pub mod record;
pub mod record_over;
pub mod recovery;
pub mod session;
pub mod settings;
pub mod settings_dialog;
pub mod shell;
pub mod sink;
pub mod sound_worker;
pub mod spectrum_reference;
pub mod stage_timer;
pub mod start_screen;
pub mod status;
pub mod stream_source;
pub mod stress;
pub mod surface;
pub mod tap_tempo;
pub mod target;
pub mod text_clipboard;
pub mod transition_choice;
pub mod translate;
pub mod ui_state;
pub mod video_tab;
pub mod video_worker;
pub mod windows;
pub mod workflow_tab;
pub mod workspace;

pub use crate::app_core::{AppCore, AppError};
pub use crate::sink::EngineSink;
