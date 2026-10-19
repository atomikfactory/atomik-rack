//! `atomik-plugin-scan <format> <path>`: the out-of-process plugin scanner. The work is
//! in the library (`atomik_plugin_scan::run`), which `atomik --plugin-scan` shares.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    atomik_plugin_scan::run(&args)
}
