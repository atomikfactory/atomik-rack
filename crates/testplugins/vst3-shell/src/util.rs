//! Small helpers: fixed-size C and UTF-16 strings, a stream reader and writer over `IBStream`.

use crate::api::ParamSpec;
use atomik_test_plugin_core::{normalized_from_plain, plain_from_normalized};
use std::ffi::{c_char, c_void};
use vst3::{ComRef, Steinberg::*};

/// Copies `src` into a fixed C string buffer, always terminated.
pub(crate) fn copy_cstr(src: &str, dst: &mut [c_char]) {
    let n = src.len().min(dst.len().saturating_sub(1));
    for (d, s) in dst.iter_mut().zip(src.bytes().take(n)) {
        *d = s as c_char;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

/// Copies `src` into a fixed UTF-16 buffer, always terminated.
pub(crate) fn copy_wstr(src: &str, dst: &mut [u16]) {
    let mut n = 0;
    let room = dst.len().saturating_sub(1);
    for (d, s) in dst.iter_mut().take(room).zip(src.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

/// The text of a NUL-terminated C string pointer (lossy).
///
/// # Safety
/// `p` must be null or point to a NUL-terminated string.
pub(crate) unsafe fn cstr_text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees a valid NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// Reads everything left in `stream`.
///
/// # Safety
/// `stream` must be null or a valid `IBStream`.
pub(crate) unsafe fn read_all(stream: *mut IBStream) -> Option<Vec<u8>> {
    // SAFETY: the caller guarantees the pointer is null or valid.
    let stream = unsafe { ComRef::from_raw(stream) }?;
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut n: i32 = 0;
        // SAFETY: `chunk` is a live buffer of the size passed; `n` is a valid out pointer.
        let r = unsafe { stream.read(chunk.as_mut_ptr().cast::<c_void>(), 4096, &mut n) };
        if r != kResultOk || n <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    Some(out)
}

/// Writes all of `bytes` to `stream`.
///
/// # Safety
/// `stream` must be null or a valid `IBStream`.
pub(crate) unsafe fn write_all(stream: *mut IBStream, bytes: &[u8]) -> bool {
    // SAFETY: the caller guarantees the pointer is null or valid.
    let Some(stream) = (unsafe { ComRef::from_raw(stream) }) else {
        return false;
    };
    let mut done = 0usize;
    while done < bytes.len() {
        let mut n: i32 = 0;
        let left = (bytes.len() - done).min(i32::MAX as usize) as i32;
        // SAFETY: the pointer addresses `left` live bytes; `n` is a valid out pointer.
        let r = unsafe {
            stream.write(
                bytes[done..].as_ptr().cast_mut().cast::<c_void>(),
                left,
                &mut n,
            )
        };
        if r != kResultOk || n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}

/// The plain value of a normalised one: a stepped param is a whole step index from `min`.
pub(crate) fn plain_of(spec: &ParamSpec, normalized: f64) -> f64 {
    if spec.stepped {
        spec.min + (normalized.clamp(0.0, 1.0) * (spec.max - spec.min)).round()
    } else {
        plain_from_normalized(spec.min, spec.max, normalized)
    }
}

/// The normalised value of a plain one.
pub(crate) fn norm_of(spec: &ParamSpec, plain: f64) -> f64 {
    normalized_from_plain(spec.min, spec.max, plain)
}
