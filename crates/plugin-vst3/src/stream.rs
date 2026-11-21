//! The host's `IBStream` over a byte buffer: what a plugin saves its state into
//! and restores it from.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, PoisonError};
use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

/// The most a stream may hold: the largest state the rest of the system can carry (the frame
/// limit of the plugin protocol). A plugin that seeks or writes past it gets an error result
/// instead of an allocation that could abort the process inside a callback.
pub(crate) const MAX_STREAM_BYTES: usize = 64 << 20;

/// A growable in-memory stream with a read and write position, capped at [`MAX_STREAM_BYTES`].
pub(crate) struct MemStream {
    inner: Mutex<Inner>,
}

struct Inner {
    data: Vec<u8>,
    pos: usize,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl MemStream {
    /// A stream holding `bytes`, positioned at the start.
    pub(crate) fn new(bytes: Vec<u8>) -> ComWrapper<Self> {
        ComWrapper::new(Self {
            inner: Mutex::new(Inner {
                data: bytes,
                pos: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The bytes written so far (the whole buffer), taken out of the stream: it is empty after.
    pub(crate) fn take_bytes(&self) -> Vec<u8> {
        let mut s = self.lock();
        s.pos = 0;
        std::mem::take(&mut s.data)
    }

    /// Moves the read and write position back to the start.
    pub(crate) fn rewind(&self) {
        self.lock().pos = 0;
    }
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: i32, num_read: *mut i32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let mut s = self.lock();
        let left = s.data.len().saturating_sub(s.pos);
        let n = left.min(num_bytes as usize);
        // SAFETY: the caller passes a buffer of at least `num_bytes` bytes; `n <= num_bytes` and
        // the source range is inside the vector.
        unsafe {
            std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buffer.cast::<u8>(), n);
        }
        s.pos += n;
        if !num_read.is_null() {
            // SAFETY: a non-null out pointer from the caller.
            unsafe { *num_read = n as i32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: i32, num_written: *mut i32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let mut s = self.lock();
        let n = num_bytes as usize;
        let end = match s.pos.checked_add(n) {
            Some(end) if end <= MAX_STREAM_BYTES => end,
            _ => return kOutOfMemory,
        };
        if s.data.len() < end {
            // `try_reserve` so that an allocation the system refuses is an error result, not an
            // abort inside this callback.
            let extra = end - s.data.len();
            if s.data.try_reserve(extra).is_err() {
                return kOutOfMemory;
            }
            s.data.resize(end, 0);
        }
        let pos = s.pos;
        // SAFETY: the caller passes `num_bytes` readable bytes; the destination range was just
        // made to fit.
        unsafe {
            std::ptr::copy_nonoverlapping(buffer.cast::<u8>(), s.data.as_mut_ptr().add(pos), n);
        }
        s.pos = end;
        if !num_written.is_null() {
            // SAFETY: a non-null out pointer from the caller.
            unsafe { *num_written = n as i32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let mut s = self.lock();
        let base = match mode as i64 {
            m if m == kIBSeekSet as i64 => 0i64,
            m if m == kIBSeekCur as i64 => s.pos as i64,
            m if m == kIBSeekEnd as i64 => s.data.len() as i64,
            _ => return kInvalidArgument,
        };
        let target = match base.checked_add(pos) {
            Some(t) if t >= 0 && t as u64 <= MAX_STREAM_BYTES as u64 => t,
            _ => return kInvalidArgument,
        };
        s.pos = target as usize;
        if !result.is_null() {
            // SAFETY: a non-null out pointer from the caller.
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: a non-null out pointer from the caller.
        unsafe { *pos = self.lock().pos as i64 };
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::ComRef;

    fn ptr(s: &ComWrapper<MemStream>) -> ComRef<'_, IBStream> {
        s.as_com_ref::<IBStream>().unwrap()
    }

    #[test]
    fn write_then_rewind_then_read_gives_the_bytes_back() {
        let s = MemStream::new(Vec::new());
        let p = ptr(&s);
        let mut data = *b"hello world";
        let mut n = 0i32;
        // SAFETY: valid buffers and out pointers for the whole test.
        unsafe {
            assert_eq!(p.write(data.as_mut_ptr().cast(), 11, &mut n), kResultOk);
        }
        assert_eq!(n, 11);
        assert_eq!(s.lock().data, b"hello world");
        s.rewind();
        let mut out = [0u8; 20];
        // SAFETY: as above.
        unsafe {
            assert_eq!(p.read(out.as_mut_ptr().cast(), 20, &mut n), kResultOk);
        }
        assert_eq!(&out[..n as usize], b"hello world");
        // At the end a read gives 0 bytes, still kResultOk.
        // SAFETY: as above.
        unsafe {
            assert_eq!(p.read(out.as_mut_ptr().cast(), 20, &mut n), kResultOk);
        }
        assert_eq!(n, 0);
    }

    #[test]
    fn seek_and_tell_follow_the_three_modes_and_refuse_negative_positions() {
        let s = MemStream::new(b"0123456789".to_vec());
        let p = ptr(&s);
        let mut at = 0i64;
        // SAFETY: valid out pointers.
        unsafe {
            assert_eq!(p.seek(4, kIBSeekSet as i32, &mut at), kResultOk);
            assert_eq!(at, 4);
            assert_eq!(p.seek(2, kIBSeekCur as i32, &mut at), kResultOk);
            assert_eq!(at, 6);
            assert_eq!(p.seek(-3, kIBSeekEnd as i32, &mut at), kResultOk);
            assert_eq!(at, 7);
            assert_eq!(p.tell(&mut at), kResultOk);
            assert_eq!(at, 7);
            assert_eq!(p.seek(-1, kIBSeekSet as i32, &mut at), kInvalidArgument);
            assert_eq!(p.seek(0, 9, &mut at), kInvalidArgument);
        }
    }

    #[test]
    fn a_write_past_the_end_zero_fills_the_gap() {
        let s = MemStream::new(b"ab".to_vec());
        let p = ptr(&s);
        let mut one = *b"z";
        let mut n = 0;
        // SAFETY: valid buffers and out pointers.
        unsafe {
            p.seek(4, kIBSeekSet as i32, std::ptr::null_mut());
            p.write(one.as_mut_ptr().cast(), 1, &mut n);
        }
        assert_eq!(s.take_bytes(), [b'a', b'b', 0, 0, b'z']);
    }

    #[test]
    fn null_buffers_and_negative_sizes_are_refused() {
        let s = MemStream::new(Vec::new());
        let p = ptr(&s);
        // SAFETY: the calls are made with deliberately invalid arguments the stream must refuse
        // without touching them.
        unsafe {
            assert_eq!(
                p.read(std::ptr::null_mut(), 1, std::ptr::null_mut()),
                kInvalidArgument
            );
            let mut b = [0u8; 1];
            assert_eq!(
                p.write(b.as_mut_ptr().cast(), -1, std::ptr::null_mut()),
                kInvalidArgument
            );
            assert_eq!(p.tell(std::ptr::null_mut()), kInvalidArgument);
        }
    }

    #[test]
    fn a_hostile_plugin_cannot_grow_the_stream_past_the_cap() {
        let s = MemStream::new(Vec::new());
        let p = ptr(&s);
        let mut b = [7u8; 4];
        let mut at = 0i64;
        let mut n = 0i32;
        // SAFETY: valid buffers and out pointers; the offsets are deliberately hostile.
        unsafe {
            // A seek far beyond the cap, then a write: both refused, nothing allocated.
            assert_eq!(
                p.seek(i64::MAX, kIBSeekSet as i32, &mut at),
                kInvalidArgument
            );
            assert_eq!(
                p.seek(i64::MAX, kIBSeekCur as i32, &mut at),
                kInvalidArgument
            );
            assert_eq!(
                p.seek(i64::MIN, kIBSeekEnd as i32, &mut at),
                kInvalidArgument
            );
            assert_eq!(p.seek(-1, kIBSeekCur as i32, &mut at), kInvalidArgument);
            let over = MAX_STREAM_BYTES as i64 + 1;
            assert_eq!(p.seek(over, kIBSeekSet as i32, &mut at), kInvalidArgument);
            assert_eq!(p.write(b.as_mut_ptr().cast(), 4, &mut n), kResultOk);
            // A seek to exactly the cap is allowed, but a write there would pass it.
            assert_eq!(
                p.seek(MAX_STREAM_BYTES as i64, kIBSeekSet as i32, &mut at),
                kResultOk
            );
            assert_eq!(at, MAX_STREAM_BYTES as i64);
            assert_eq!(p.write(b.as_mut_ptr().cast(), 1, &mut n), kOutOfMemory);
            // A position plus a size that overflows `usize` arithmetic is refused as well.
            assert_eq!(
                p.write(b.as_mut_ptr().cast(), i32::MAX, &mut n),
                kOutOfMemory
            );
        }
        let s = s.take_bytes();
        assert_eq!(s, [7, 7, 7, 7], "nothing grew past the first write");
    }

    #[test]
    fn a_write_that_ends_at_the_cap_is_refused_beyond_it_without_growth() {
        let s = MemStream::new(Vec::new());
        let p = ptr(&s);
        let mut b = [1u8; 8];
        let mut n = 0i32;
        // SAFETY: valid buffers and out pointers.
        unsafe {
            p.seek(
                MAX_STREAM_BYTES as i64 - 4,
                kIBSeekSet as i32,
                std::ptr::null_mut(),
            );
            assert_eq!(p.write(b.as_mut_ptr().cast(), 8, &mut n), kOutOfMemory);
        }
        assert!(s.lock().data.is_empty());
    }
}
