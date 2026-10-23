//! Flush-to-zero and denormals-are-zero around every plugin `process`.
//!
//! Denormal floats make DSP code slow by orders of magnitude, and plugins expect the host to
//! turn them off in the audio thread. The control bits live in the floating-point control
//! register (MXCSR on x86-64, FPCR on aarch64), which has no safe API: this module and `ole`
//! are the only places in the host crate that use `unsafe` (pinned by name in
//! `tests/unsafe_policy.rs`). Each block has a `SAFETY:` comment; nothing here allocates or blocks.
#![allow(unsafe_code)] // Justification: reading and writing the FP control register.
#![deny(clippy::undocumented_unsafe_blocks)]

use std::marker::PhantomData;

#[cfg(target_arch = "x86_64")]
mod imp {
    /// MXCSR: flush-to-zero (bit 15) and denormals-are-zero (bit 6).
    pub const BITS: u64 = (1 << 15) | (1 << 6);

    pub fn read() -> u64 {
        let mut v: u32 = 0;
        // SAFETY: `stmxcsr` stores the 4 bytes of MXCSR at the given address, which is a live
        // local; SSE is part of the x86-64 baseline.
        unsafe {
            std::arch::asm!(
                "stmxcsr [{}]",
                in(reg) &raw mut v,
                options(nostack, preserves_flags)
            );
        }
        u64::from(v)
    }

    pub fn write(value: u64) {
        let v = value as u32;
        // SAFETY: `ldmxcsr` loads 4 bytes from the given address, a live local; the value is one
        // read from the register with only the flush bits changed, so it holds no reserved bits
        // and unmasks no exception.
        unsafe {
            std::arch::asm!(
                "ldmxcsr [{}]",
                in(reg) &raw const v,
                options(nostack, readonly)
            );
        }
    }
}

#[cfg(target_arch = "aarch64")]
mod imp {
    /// FPCR.FZ (bit 24): flush denormal inputs and results to zero.
    pub const BITS: u64 = 1 << 24;

    pub fn read() -> u64 {
        let v: u64;
        // SAFETY: `mrs` of FPCR reads a system register at EL0 and touches no memory.
        unsafe {
            std::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack, preserves_flags));
        }
        v
    }

    pub fn write(value: u64) {
        // SAFETY: the value is one read from FPCR with only FZ changed; `msr fpcr` at EL0 is
        // allowed and touches no memory.
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) value, options(nomem, nostack));
        }
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod imp {
    pub const BITS: u64 = 0;

    pub fn read() -> u64 {
        0
    }

    pub fn write(_value: u64) {}
}

/// Whether this target can flush denormals (x86-64 and aarch64); elsewhere the guard does nothing.
pub const FLUSH_SUPPORTED: bool = cfg!(any(target_arch = "x86_64", target_arch = "aarch64"));

/// Whether flush-to-zero and denormals-are-zero are currently on for the calling thread.
pub fn denormals_flushed() -> bool {
    imp::BITS != 0 && imp::read() & imp::BITS == imp::BITS
}

/// Turns flush-to-zero and denormals-are-zero on for the calling thread until dropped, then
/// restores the previous register value (so guards nest and the host never leaks the setting).
#[derive(Debug)]
pub struct FtzGuard {
    saved: u64,
    _not_send: PhantomData<*const ()>,
}

impl FtzGuard {
    /// Sets the flush bits on the calling thread.
    pub fn enter() -> Self {
        let saved = imp::read();
        imp::write(saved | imp::BITS);
        Self {
            saved,
            _not_send: PhantomData,
        }
    }
}

impl Drop for FtzGuard {
    fn drop(&mut self) {
        imp::write(self.saved);
    }
}
