//! The seam between the app and stem separation: one trait, [`StemBackend`], for the separation
//! itself.
//!
//! * [`DspStems`] is the one the app uses: the signal-processing split of `atomik-stems`. It needs
//!   no model and no download, so nothing is fetched and nothing is stored but the stems.
//! * The trait stays so that a learned model can be plugged in later (none is: no model is known
//!   whose weights and training data both allow commercial use, see DECISIONS).
//! * `FakeStems` (tests only) separates a signal into scaled copies of itself and can hold until
//!   cancelled, so a test can cancel a run.

use atomik_stems::{Stem, StemError, StemSink};
use std::sync::atomic::AtomicBool;

/// Everything the app needs from the separation, behind one seam.
pub trait StemBackend: Send + Sync {
    /// The sample rate the audio must have when it is given to [`StemBackend::separate`].
    fn rate(&self) -> u32;
    /// Separates a stereo song (at [`StemBackend::rate`]) and gives the wanted stems to `sink`.
    fn separate(
        &self,
        left: &[f32],
        right: &[f32],
        wanted: &[Stem],
        sink: &mut dyn StemSink,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<(), StemError>;
}

/// The signal-processing separation of `atomik-stems`: no model, no download.
#[derive(Debug, Clone, Copy, Default)]
pub struct DspStems;

impl StemBackend for DspStems {
    fn rate(&self) -> u32 {
        atomik_stems::RATE
    }

    fn separate(
        &self,
        left: &[f32],
        right: &[f32],
        wanted: &[Stem],
        sink: &mut dyn StemSink,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<(), StemError> {
        atomik_stems::separate(left, right, wanted, sink, cancel, progress)
    }
}

#[cfg(test)]
pub(crate) use fake::FakeStems;

#[cfg(test)]
mod fake {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A backend for tests. A separation gives each stem as a copy of the input times
    /// `0.1 * (index + 1)`, in a few pieces; it can hold (until cancelled or released) so a test
    /// can cancel a run.
    #[derive(Clone)]
    pub(crate) struct FakeStems {
        inner: Arc<Inner>,
    }

    struct Inner {
        hold: AtomicBool,
        separations: AtomicUsize,
        rate: u32,
    }

    impl FakeStems {
        /// A fake running at `rate`.
        pub(crate) fn new(rate: u32) -> Self {
            Self {
                inner: Arc::new(Inner {
                    hold: AtomicBool::new(false),
                    separations: AtomicUsize::new(0),
                    rate,
                }),
            }
        }
        /// Makes a separation wait until it is cancelled or [`Self::release`] is called.
        pub(crate) fn hold(&self) {
            self.inner.hold.store(true, Ordering::SeqCst);
        }
        /// Lets a held separation go on.
        pub(crate) fn release(&self) {
            self.inner.hold.store(false, Ordering::SeqCst);
        }
        /// How many separations started.
        pub(crate) fn separations(&self) -> usize {
            self.inner.separations.load(Ordering::SeqCst)
        }
    }

    impl StemBackend for FakeStems {
        fn rate(&self) -> u32 {
            self.inner.rate
        }
        fn separate(
            &self,
            left: &[f32],
            right: &[f32],
            wanted: &[Stem],
            sink: &mut dyn StemSink,
            cancel: &AtomicBool,
            progress: &mut dyn FnMut(f32),
        ) -> Result<(), StemError> {
            self.inner.separations.fetch_add(1, Ordering::SeqCst);
            while self.inner.hold.load(Ordering::SeqCst) {
                if cancel.load(Ordering::SeqCst) {
                    return Err(StemError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let len = left.len().min(right.len());
            let pieces = 4;
            for p in 0..pieces {
                if cancel.load(Ordering::SeqCst) {
                    return Err(StemError::Cancelled);
                }
                let (a, b) = (len * p / pieces, len * (p + 1) / pieces);
                for stem in wanted {
                    let g = 0.1 * (stem.index() as f32 + 1.0);
                    let l: Vec<f32> = left[a..b].iter().map(|v| v * g).collect();
                    let r: Vec<f32> = right[a..b].iter().map(|v| v * g).collect();
                    sink.write(*stem, &l, &r)?;
                }
                progress((p + 1) as f32 / pieces as f32);
            }
            Ok(())
        }
    }
}
