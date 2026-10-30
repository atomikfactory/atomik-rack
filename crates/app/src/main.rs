//! `atomik`: the Atomik Rack app (rack UI with live audio and MIDI).

use clap::Parser;

fn main() -> std::process::ExitCode {
    // Hidden: `atomik --plugin-scan <clap|vst3> <path>` is the out-of-process plugin scanner
    // (the same code as the `atomik-plugin-scan` program, which the app prefers when it sits
    // beside it). Handled before anything else starts: no window, no audio, no settings.
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new(atomik_plugin_scan::SELF_SCAN_FLAG)) {
        let rest: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
        return atomik_plugin_scan::run(&rest);
    }
    // Hidden: `atomik --plugin-host <args...>` is a sandbox child (the same code as the
    // `atomik-plugin-host` program, used when it is not beside the app).
    if first.as_deref()
        == Some(std::ffi::OsStr::new(
            atomik_app::plugin_setup::PLUGIN_HOST_FLAG,
        ))
    {
        return atomik_plugin_hostd::run(args.collect());
    }
    run_app();
    std::process::ExitCode::SUCCESS
}

fn run_app() {
    let cli = atomik_app::cli::Cli::parse();
    if cli.print_keymap {
        match atomik_app::cli::cheat_sheet(&cli) {
            Ok(text) => print!("{text}"),
            Err(e) => {
                atomik_app::logging::fatal(&e.to_string());
                std::process::exit(1);
            }
        }
        return;
    }
    // The log and the crash reports come first, so that everything after them (settings,
    // plugins, audio) can report. Both live in `<config folder>/logs`.
    let settings_path = cli
        .settings
        .clone()
        .or_else(atomik_app::settings::default_settings_path);
    let config = atomik_app::settings::config_folder(settings_path.as_deref());
    let logs = atomik_app::logging::log_dir(&config.dir);
    atomik_app::logging::init(&logs);
    // Kept for the whole run; dropping it would put the previous panic hook back.
    let _crash_hook = atomik_app::crash::install(&logs);
    if let Err(e) = atomik_app::platform::run(cli) {
        atomik_app::logging::fatal(&e);
        std::process::exit(1);
    }
}
