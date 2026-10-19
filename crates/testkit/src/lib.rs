//! Atomik Rack test kit (dev-dependency only): allocation guard, single-device harness,
//! signal analysis and fake devices.
//!
//! `no_alloc`, `harness` and `signal` are shared contracts; the fakes live in the `fakes` module.

pub mod fakes;
pub mod harness;
pub mod no_alloc;
pub mod signal;

#[cfg(test)]
install_alloc_guard!();

#[cfg(test)]
mod tests {
    use crate::no_alloc::assert_no_alloc;

    #[test]
    fn assert_no_alloc_passes_without_allocation() {
        let x = assert_no_alloc(|| 2 + 2);
        assert_eq!(x, 4);
    }

    #[test]
    #[should_panic(expected = "HI-1 violated")]
    fn assert_no_alloc_catches_allocation() {
        assert_no_alloc(|| std::hint::black_box(vec![1u8; 16]).len());
    }

    #[test]
    #[should_panic(expected = "HI-1 violated")]
    fn assert_no_alloc_catches_deallocation() {
        let v = vec![1u8; 16];
        assert_no_alloc(move || drop(v));
    }
}
