//! `atomik-render`: renders an Atomik Rack patch (JSON) offline to a WAV file.
//!
//! All logic lives in [`atomik_render::cli`]; this binary only wires the process streams and
//! the exit code.

fn main() {
    let code = atomik_render::cli::main_with(
        std::env::args_os(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
