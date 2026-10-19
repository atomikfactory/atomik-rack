//! The host's `IBStream` over a byte buffer: what a plugin saves its state into
//! and restores it from.

use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, PoisonError};
use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper};

/// A growable in-memory stream with a read and write position.
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

    /// The bytes written so far (the whole buffer).
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.lock().data.clone()
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
        let end = s.pos + n;
        if s.data.len() < end {
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
        let target = base.saturating_add(pos);
        if target < 0 {
            return kInvalidArgument;
        }
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
        assert_eq!(s.bytes(), b"hello world");
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
        assert_eq!(s.bytes(), [b'a', b'b', 0, 0, b'z']);
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
}
