//! Atomik Rack audio files: decoding, sample-rate conversion, WAV I/O, content
//! hashing, waveform peaks and the project audio bundle.
//!
//! Nothing in this crate runs on the audio thread. Every function may allocate and block on
//! disk. Results are deterministic: the same input bytes always give the same samples.

mod bundle;
mod data;
mod decode;
mod error;
mod hash;
mod peaks;
mod resample;
mod wav;

pub use bundle::{AssetInfo, Bundle, is_valid_hash, save_text_atomic};
pub use data::AudioData;
pub use decode::{decode_bytes, decode_file};
pub use error::{AudioFileError, Result};
pub use hash::content_hash;
pub use peaks::{PEAK_LEVELS, PeakPyramid, Peaks, compute_peaks};
pub use resample::resample;
pub use wav::{WavStreamReader, read_wav_f32, write_wav_f32};
