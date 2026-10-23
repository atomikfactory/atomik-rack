//! `atomik-plugin-host`: the plugin sandbox child. The work is [`atomik_plugin_hostd::run`].

fn main() -> std::process::ExitCode {
    atomik_plugin_hostd::run(std::env::args_os().skip(1).collect())
}
