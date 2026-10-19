//! The `atomik-play` command line. `main.rs` only installs the Ctrl-C handler and forwards to
//! [`main_with`].

use crate::args::{ASIO_COMPILED, Args, validate};
use crate::backend::Backend;
use crate::listing::format_listing;
use crate::session::play;
use crate::stats::{EXIT_FAILURE, EXIT_OK, exit_code};
use clap::Parser;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Parses `argv` (including the program name), runs, and returns the process exit code:
/// 0 on success, 1 when a `--duration` run had xruns, 2 on any error (`error: ...` on stderr).
pub fn main_with<B, I, T>(
    argv: I,
    backend: &B,
    stop: &AtomicBool,
    sleep: &mut dyn FnMut(Duration),
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32
where
    B: Backend,
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(e) => {
            // --help and --version are "errors" for clap but succeed for the user.
            return if e.use_stderr() {
                let _ = write!(stderr, "{}", e.render());
                EXIT_FAILURE
            } else {
                let _ = write!(stdout, "{}", e.render());
                EXIT_OK
            };
        }
    };
    if let Err(message) = validate(&args, ASIO_COMPILED) {
        let _ = writeln!(stderr, "error: {message}");
        return EXIT_FAILURE;
    }
    if args.list {
        let (text, warnings) = format_listing(&backend.list_audio(), &backend.list_midi());
        for w in warnings {
            let _ = writeln!(stderr, "{w}");
        }
        let _ = write!(stdout, "{text}");
        return EXIT_OK;
    }
    match play(&args, backend, stop, sleep, stdout, stderr) {
        Ok(totals) => exit_code(totals.xruns, args.duration),
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            EXIT_FAILURE
        }
    }
}
