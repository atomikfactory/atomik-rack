//! The control channel of the plugin sandbox: a frame is a length, JSON and an optional blob.
//! Any byte stream, read as either direction of the channel, must give a message or an error
//! without allocating beyond what the stream supplies.
#![no_main]

use std::io::Cursor;

use atomik_plugin_ipc::{FromChild, ToChild, read_frame};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut stream = Cursor::new(data);
    // A stream holds several frames: read until the first error.
    while read_frame::<FromChild>(&mut stream).is_ok() {}
    let mut stream = Cursor::new(data);
    while read_frame::<ToChild>(&mut stream).is_ok() {}
});
