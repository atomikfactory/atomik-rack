//! Atomik Rack audio files: decoding, sample-rate conversion, WAV I/O, content
//! hashing, waveform peaks and the project audio bundle.
//!
//! Nothing in this crate runs on the audio thread. Every function may allocate and block on
//! disk. Results are deterministic: the same input bytes always give the same samples.

mod archive;
mod bundle;
mod data;
mod decode;
mod edit;
mod error;
mod hash;
mod instrument;
mod meta;
mod overview;
mod peaks;
mod resample;
mod sf2;
mod sfz;
mod stream_decode;
mod wav;

pub use archive::{
    ARCHIVE_EXTENSION, ArchiveError, ArchiveLimits, ArchiveReport, Extracted, archive_size,
    archive_work, extract_archive, is_safe_component, read_archive_project, write_archive,
};
pub use bundle::{AssetInfo, Bundle, is_valid_hash, save_text_atomic};
pub use data::AudioData;
pub use decode::{
    DEFAULT_MAX_SAMPLES, Decoded, decode_bytes, decode_bytes_with, decode_file, decode_file_with,
    decode_vec_with,
};
pub use edit::{FadeShape, crop, fade_in, fade_out, normalise, range_peak, reverse};
pub use error::{AudioFileError, Result};
pub use hash::content_hash;
pub use instrument::{
    Instrument, InstrumentKind, InstrumentLimits, InstrumentRegion, InstrumentSample,
    MAX_SFZ_BYTES, RegionEnv, RegionFilter, SampleSource, instrument_kind, is_instrument_path,
    read_instrument,
};
pub use meta::{FileMeta, parse_meta, read_file_meta};
pub use overview::overview_levels;
pub use peaks::{FINEST_PEAK_FRAMES, PEAK_LEVELS, PeakPyramid, Peaks, compute_peaks, raw_min_max};
pub use resample::{resample, resample_owned};
pub use sf2::{parse_sf2, read_sf2, sf2_presets};
pub use sfz::{parse_sfz, read_sfz};
pub use stream_decode::{StreamDecoder, probe_file};
pub use wav::{WavStreamReader, read_wav_f32, read_wav_f32_limited, write_wav_f32};
