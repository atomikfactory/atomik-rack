//! The platform layer: winit window and events, vello/wgpu rendering, file dialogs (rfd) and,
//! on macOS, native menus (muda). Not unit-tested (window and GPU); everything it calls is.

mod app;
mod cursor;
mod dock_frames;
pub mod gpu;
pub mod gpu_recovery;
mod learn_strips;
mod native_child;
#[cfg(target_os = "macos")]
mod native_menu;
mod plugin_windows;
mod window_logic;

pub use app::run;
