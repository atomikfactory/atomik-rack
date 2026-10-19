//! The `atomik-play` binary itself: `--list` works on any machine (including CI without audio
//! or MIDI devices), and argument errors exit 2. Nothing here opens an audio stream.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_atomik-play"))
}

#[test]
fn list_exits_0_on_any_machine() {
    let out = bin().arg("--list").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    assert!(stdout.starts_with("audio hosts:\n"), "{stdout}");
    assert!(stdout.contains("\nmidi inputs:\n"), "{stdout}");
    // Any enumeration failure is reported as a warning, never an error.
    // (Warning texts may quote backend messages such as "MIDI backend error: ...", so only a
    // line that starts with `error:` counts.)
    assert!(!stderr.lines().any(|l| l.starts_with("error:")), "{stderr}");
}

#[test]
fn bad_arguments_exit_2() {
    let out = bin().args(["p.json", "--host", "beos"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin().output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "PATCH is required without --list"
    );
}

#[test]
fn missing_patch_exits_2_before_opening_any_device() {
    let out = bin().arg("does-not-exist.json").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: does-not-exist.json: cannot read the patch"),
        "{stderr}"
    );
}
