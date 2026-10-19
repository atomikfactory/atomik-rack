//! `atomik-play`: plays an Atomik Rack patch live on an audio device with MIDI input.
//!
//! All logic lives in [`atomik_play::cli`]; this binary only installs the Ctrl-C handler and
//! wires the process streams and the exit code.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the Ctrl-C handler; polled by the main loop.
static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    if let Err(e) = ctrlc::set_handler(|| STOP.store(true, Ordering::SeqCst)) {
        eprintln!("warning: cannot install the Ctrl-C handler ({e}); use --duration to stop");
    }
    let code = atomik_play::cli::main_with(
        std::env::args_os(),
        &atomik_play::SystemBackend,
        &STOP,
        &mut std::thread::sleep,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
