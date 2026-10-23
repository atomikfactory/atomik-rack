//! The child's frame stream is private: after `take_frame_stdout`, anything the process or a
//! plugin prints to stdout lands on stderr. The child is this test binary run again.

use atomik_plugin_ipc::take_frame_stdout;
use std::io::Write;
use std::process::{Command, Stdio};

const ROLE: &str = "ATOMIK_IPC_STDIO_CHILD";
const TEST: &str = "plugin_output_never_reaches_the_frame_stream";

/// What a C or C++ plugin does with `printf`, at the C runtime's fd 1 (Windows only).
#[cfg(windows)]
fn c_runtime_write(text: &[u8]) {
    // SAFETY: `_write` is exported by the C runtime; the signature is that of `io.h`.
    unsafe extern "C" {
        fn _write(fd: i32, buf: *const std::ffi::c_void, count: u32) -> i32;
    }
    // SAFETY: `text` is valid for `count` bytes for the call; fd 1 is the CRT's stdout.
    unsafe { _write(1, text.as_ptr().cast(), text.len() as u32) };
}

#[test]
fn plugin_output_never_reaches_the_frame_stream() {
    if std::env::var(ROLE).is_ok() {
        let mut frames = take_frame_stdout().expect("the frame handle");
        frames.write_all(b"FRAME").unwrap();
        frames.flush().unwrap();
        println!("NOISE-RUST");
        #[cfg(windows)]
        c_runtime_write(b"NOISE-C\n");
        #[cfg(unix)]
        {
            // SAFETY: fd 1 is open; the buffer is valid for its length.
            unsafe { libc::write(1, b"NOISE-C\n".as_ptr().cast(), 8) };
        }
        std::process::exit(0);
    }
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // libtest prints its own header to stdout before the test body runs, so the stream is
    // checked for the frame bytes and the absence of any noise.
    assert!(stdout.contains("FRAME"), "stdout: {stdout:?}");
    assert!(!stdout.contains("NOISE"), "stdout: {stdout:?}");
    assert!(stderr.contains("NOISE-RUST"), "stderr: {stderr:?}");
    assert!(stderr.contains("NOISE-C"), "stderr: {stderr:?}");
    assert!(!stderr.contains("FRAME"), "stderr: {stderr:?}");
}
