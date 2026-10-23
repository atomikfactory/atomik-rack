//! The plugin sandbox child: `atomik-plugin-host <format> <plugin-file> <plugin-id> --block
//! <path> --doorbell <name> --parent-pid <pid>` hosts one plugin instance. The main thread is
//! the plugin's main thread (control channel, timers, the editor window); an audio thread
//! serves the shared block. The process ends on stdin EOF, on `Shutdown`, or when the parent is
//! gone.
//!
//! [`run`] is the whole program; the `atomik-plugin-host` binary and `atomik --plugin-host`
//! both call it. The contract is `not-for-git/plan/sandbox/03-interfaces.md`, section D.

mod args;
mod audio;
mod builtin;
mod control;
mod driver;
mod editor;
mod notify;
mod winit_driver;

use atomik_plugin_host::mark_main_thread;
use atomik_plugin_ipc::{ChildPort, Doorbell, DoorbellName, SharedBlock};
use std::ffi::OsString;
use std::process::ExitCode;

/// Usage line.
pub const USAGE: &str = "usage: atomik-plugin-host clap|vst3|null <plugin-file> <plugin-id> --block <path> --doorbell <name> --parent-pid <pid>";

/// Runs the child with its arguments (without the program name).
pub fn run(args: Vec<OsString>) -> ExitCode {
    let args = match args::parse(&args) {
        Ok(a) => {
            args::with_format_override(a, std::env::var(args::FORMAT_OVERRIDE_ENV).ok().as_deref())
        }
        Err(e) => {
            eprintln!("atomik-plugin-host: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Before any plugin code runs: frames get a private handle, and whatever a plugin prints to
    // stdout goes to stderr.
    let frames = match atomik_plugin_ipc::take_frame_stdout() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("atomik-plugin-host: cannot take the control channel's stdout: {e}");
            return ExitCode::from(control::EXIT_MISMATCH);
        }
    };
    // Every backend call asserts that it runs on the plugin main thread: this one.
    mark_main_thread();
    let block = match SharedBlock::open(&args.block) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "atomik-plugin-host: cannot open the plugin block {}: {e}",
                args.block.display()
            );
            return ExitCode::from(control::EXIT_MISMATCH);
        }
    };
    let bell = match Doorbell::open(&DoorbellName(args.doorbell.clone()), &block) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "atomik-plugin-host: cannot open the doorbell {}: {e}",
                args.doorbell
            );
            return ExitCode::from(control::EXIT_MISMATCH);
        }
    };
    let audio = match audio::AudioThread::spawn(ChildPort::new(block, bell)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("atomik-plugin-host: cannot start the audio thread: {e}");
            return ExitCode::FAILURE;
        }
    };
    let backend = control::make_backend(&args);
    let mut control = control::Control::new(&args, backend, audio, Box::new(frames));
    if !control.greet() {
        return ExitCode::SUCCESS;
    }
    let code = match winit_driver::run(control) {
        Ok(code) => code,
        Err(control) => driver::run_plain(*control),
    };
    ExitCode::from(code)
}
