//! Hardware (gated): play on the virtual audio cable for 5 s with 0 xruns.
//!
//! Returns early (passes) unless `ATOMIK_HW_TESTS=1`. Safety:
//! - the ONLY device is the virtual audio cable's playback endpoint ("CABLE Input") on WASAPI; the
//!   device argument is asserted to contain "CABLE" before the player starts, so neither the
//!   default device nor the main audio interface is ever opened;
//! - no MIDI port is opened and no MIDI is sent;
//! - level: the patch's notes are ignored in live mode and no MIDI is connected, so the synth
//!   outputs silence (far below −20 dBFS).
//!
//! Run: `ATOMIK_HW_TESTS=1 cargo test -p atomik-play --test hw_cable -- --nocapture`

use std::process::Command;

const PATCH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/patches/minimal.json"
);
/// Substring of the cable's playback endpoint.
const DEVICE: &str = "CABLE Input";

fn hw_enabled() -> bool {
    std::env::var("ATOMIK_HW_TESTS").as_deref() == Ok("1")
}

#[test]
fn hw_cable_plays_5s_without_xruns() {
    if !hw_enabled() {
        return;
    }
    let args = [
        PATCH,
        "--host",
        "wasapi",
        "--device",
        DEVICE,
        "--duration",
        "5",
        "--stats",
    ];
    let device = args[args.iter().position(|a| *a == "--device").unwrap() + 1];
    assert!(
        device.contains("CABLE"),
        "refusing to play on a non-virtual cable device: {device:?}"
    );
    assert!(!args.contains(&"--midi"), "no MIDI in automated tests");
    let out = Command::new(env!("CARGO_BIN_EXE_atomik-play"))
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!("{stderr}{stdout}");
    assert!(stderr.contains("CABLE"), "the cable was opened: {stderr}");
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let summary = stdout.lines().last().unwrap_or_default();
    assert!(summary.starts_with("summary: t=5s "), "{summary}");
    assert!(summary.contains(" xruns=0 "), "{summary}");
    assert!(summary.ends_with("result=PASS"), "{summary}");
    assert_eq!(stdout.lines().filter(|l| l.starts_with("t=")).count(), 5);
}
