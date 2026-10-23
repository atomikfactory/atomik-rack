//! `atomik-mcp`: the Atomik Rack MCP server (JSON-RPC over stdin and stdout, one message per
//! line). All logic lives in the library; this only reads lines and writes replies.
//!
//! ```text
//! atomik-mcp [--root DIR]...
//! ```
//!
//! The working directory is always an allowed folder; each `--root` adds one.

use atomik_mcp::{Paths, Server};
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let mut roots = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(dir) => roots.push(PathBuf::from(dir)),
                None => return usage("--root needs a folder"),
            },
            "--help" | "-h" => {
                println!(
                    "atomik-mcp {}: MCP server (stdio) for Atomik Rack QA.\n\nusage: atomik-mcp [--root DIR]...\n\nThe working directory is an allowed folder; --root adds more. See docs/MCP.md.",
                    env!("CARGO_PKG_VERSION")
                );
                return std::process::ExitCode::SUCCESS;
            }
            "--version" | "-V" => {
                println!("atomik-mcp {}", env!("CARGO_PKG_VERSION"));
                return std::process::ExitCode::SUCCESS;
            }
            other => return usage(&format!("unknown argument `{other}`")),
        }
    }
    let Ok(cwd) = std::env::current_dir() else {
        return usage("cannot read the working directory");
    };
    let mut server = Server::new(Paths::new(cwd, &roots));
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line)
            && (writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err())
        {
            break;
        }
    }
    std::process::ExitCode::SUCCESS
}

fn usage(message: &str) -> std::process::ExitCode {
    eprintln!("atomik-mcp: {message}\nusage: atomik-mcp [--root DIR]...");
    std::process::ExitCode::from(2)
}
