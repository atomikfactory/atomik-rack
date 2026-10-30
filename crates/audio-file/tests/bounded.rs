//! Hostile and corrupt audio never allocates more than the file itself can account for: a header
//! that claims gigabytes is refused (or read as the few bytes it has), with a small peak.
#![allow(missing_docs, unsafe_code)] // the counting allocator below is `unsafe` by nature

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Write;

use atomik_audio_file::{
    AudioData, AudioFileError, Bundle, WavStreamReader, decode_bytes, decode_bytes_with,
    decode_file, read_wav_f32, read_wav_f32_limited, write_wav_f32,
};

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

/// Counts the live heap bytes of each thread and remembers the highest value; a test measures
/// its own thread, so the other tests of this binary do not disturb it.
struct Counting;

fn grow(by: usize) {
    let _ = LIVE.try_with(|live| {
        live.set(live.get() + by as isize);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}

fn shrink(by: usize) {
    let _ = LIVE.try_with(|live| live.set(live.get() - by as isize));
}

// SAFETY: every method forwards unchanged to `System`, which upholds the `GlobalAlloc`
// contract; the counting only touches const-initialised thread-locals that never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's layout.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded with the caller's layout.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        shrink(layout.size());
        // SAFETY: `ptr` was allocated by `System` through this allocator with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` and `layout` come from this allocator; `new_size` is the caller's.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            shrink(layout.size());
            grow(new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and returns its result with the most heap bytes it held at once (this thread).
fn peak_of<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(base));
    let result = f();
    let peak = PEAK.with(Cell::get) - base;
    (result, usize::try_from(peak).unwrap_or(0))
}

const FEW_MIB: usize = 4 << 20;

/// A 44-byte WAV whose data chunk claims `claimed_bytes` and holds none.
fn lying_wav(tag: u16, bits: u16, channels: u16, claimed_bytes: u32) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&36u32.wrapping_add(claimed_bytes).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&tag.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&(48_000 * u32::from(block)).to_le_bytes());
    b.extend_from_slice(&block.to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&claimed_bytes.to_le_bytes());
    assert_eq!(b.len(), 44);
    b
}

fn tmp(name: &str) -> std::path::PathBuf {
    common::tmp_dir(name).join(name)
}

#[test]
fn a_header_claiming_two_billion_samples_is_an_error_with_a_small_peak() {
    // 16-bit mono: 0xFFFF_FFFE bytes is 2^31 - 1 samples; float: 2^30 - 1 samples.
    for (tag, bits, claimed) in [(1, 16, 0xFFFF_FFFEu32), (3, 32, 0xFFFF_FFFC)] {
        let bytes = lying_wav(tag, bits, 1, claimed);
        let path = tmp(&format!("lying-{bits}.wav"));
        std::fs::write(&path, &bytes).unwrap();

        let (r, peak) = peak_of(|| read_wav_f32(&path));
        assert!(r.is_err(), "{bits}-bit: {r:?}");
        assert!(
            peak < FEW_MIB,
            "read_wav_f32 {bits}-bit peaked at {peak} bytes"
        );

        let (r, peak) = peak_of(|| decode_file(&path));
        assert!(r.is_err(), "{bits}-bit: {r:?}");
        assert!(
            peak < FEW_MIB,
            "decode_file {bits}-bit peaked at {peak} bytes"
        );

        let (r, peak) = peak_of(|| decode_bytes(&bytes, Some("wav")));
        assert!(r.is_err(), "{bits}-bit: {r:?}");
        assert!(
            peak < FEW_MIB,
            "decode_bytes {bits}-bit peaked at {peak} bytes"
        );

        // The streaming reader (float only) opens it and holds no frames.
        let (r, peak) = peak_of(|| WavStreamReader::open(&path));
        match r {
            Ok(stream) => assert_eq!((tag, stream.frames()), (3, 0)),
            Err(e) => assert_eq!((tag, e.to_string().contains("32-bit float")), (1, true)),
        }
        assert!(
            peak < FEW_MIB,
            "stream open {bits}-bit peaked at {peak} bytes"
        );
    }
}

#[test]
fn a_lying_header_over_a_short_body_reads_only_the_body() {
    // Claims 2^30 float samples, holds 100: the read fails on the missing data, small.
    let mut bytes = lying_wav(3, 32, 2, 0xFFFF_FFF8);
    bytes.extend(std::iter::repeat_n(0u8, 400));
    let path = tmp("lying-short.wav");
    std::fs::write(&path, &bytes).unwrap();
    let (r, peak) = peak_of(|| read_wav_f32(&path));
    assert!(r.is_err());
    assert!(peak < FEW_MIB, "peaked at {peak} bytes");
    // The streaming reader serves the 50 frames that are there.
    let mut stream = WavStreamReader::open(&path).unwrap();
    assert_eq!(stream.frames(), 50);
    let mut out = vec![vec![1.0f32; 64]; 2];
    assert_eq!(stream.read_frames(0, &mut out).unwrap(), 50);
}

#[test]
fn the_sample_limit_is_a_parameter() {
    let data = AudioData::new(48_000, vec![vec![0.25; 1000], vec![0.5; 1000]]).unwrap();
    let path = tmp("limit.wav");
    write_wav_f32(&path, &data).unwrap();
    assert!(matches!(
        read_wav_f32_limited(&path, 1999),
        Err(AudioFileError::Unsupported(_))
    ));
    assert_eq!(read_wav_f32_limited(&path, 2000).unwrap().audio, data);
    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(
        decode_bytes_with(&bytes, Some("wav"), 1999),
        Err(AudioFileError::Unsupported(_))
    ));
    assert_eq!(
        decode_bytes_with(&bytes, Some("wav"), 2000).unwrap().audio,
        data
    );
}

#[test]
fn truncations_of_a_real_file_stay_small() {
    let bytes = std::fs::read(common::fixture("stereo_wavf32.wav")).unwrap();
    for cut in (0..bytes.len()).step_by(997) {
        let (_, peak) = peak_of(|| decode_bytes(&bytes[..cut], Some("wav")));
        assert!(peak < FEW_MIB, "cut at {cut}: peak {peak}");
    }
}

/// A float WAV with a NaN and infinities in it.
fn non_finite_wav() -> Vec<u8> {
    let samples = [
        0.5f32,
        f32::NAN,
        f32::INFINITY,
        -0.25,
        f32::NEG_INFINITY,
        0.75,
    ];
    let mut bytes = lying_wav(3, 32, 2, 4 * samples.len() as u32);
    for s in samples {
        bytes.write_all(&s.to_le_bytes()).unwrap();
    }
    bytes
}

#[test]
fn non_finite_samples_become_silence_everywhere_a_wav_is_read() {
    let bytes = non_finite_wav();
    let path = tmp("nan.wav");
    std::fs::write(&path, &bytes).unwrap();
    let want = [vec![0.5, 0.0, 0.0], vec![0.0, -0.25, 0.75]];
    // Symphonia (imports).
    let d = decode_bytes_with(&bytes, Some("wav"), 100).unwrap();
    assert_eq!((d.replaced, &d.audio.channels[..]), (3, &want[..]));
    // hound (whole-asset loads).
    let d = read_wav_f32_limited(&path, 100).unwrap();
    assert_eq!((d.replaced, &d.audio.channels[..]), (3, &want[..]));
    // The streaming reader.
    let mut stream = WavStreamReader::open(&path).unwrap();
    let mut out = vec![vec![9.0f32; 3]; 2];
    assert_eq!(stream.read_frames(0, &mut out).unwrap(), 3);
    assert_eq!(out, want);
}

#[test]
fn an_import_reports_the_samples_it_replaced_and_stores_a_finite_asset() {
    let dir = common::tmp_dir("nan-import");
    let src = dir.join("nan.wav");
    std::fs::write(&src, non_finite_wav()).unwrap();
    let mut bundle = Bundle::create(&dir.join("project")).unwrap();
    let info = bundle.import(&src, 48_000).unwrap();
    assert_eq!(info.replaced_samples, 3);
    let stored = bundle.load(&info.hash).unwrap();
    assert!(stored.channels.iter().flatten().all(|s| s.is_finite()));
    // A clean file reports none.
    let clean = AudioData::new(48_000, vec![vec![0.1; 64]]).unwrap();
    assert_eq!(
        bundle
            .add_recording(&clean, "take")
            .unwrap()
            .replaced_samples,
        0
    );
}
