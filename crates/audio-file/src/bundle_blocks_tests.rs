//! The block-wise store gives the asset the whole-buffer store gives.

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join("atomik-audio-file-blocks-unit")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Samples with the awkward values in them, as raw bits so that a NaN pattern survives.
fn audio(channels: usize, frames: usize) -> AudioData {
    let chans = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| match (i + c) % 11 {
                    0 => 0.0,
                    1 => -0.0,
                    2 => f32::MIN_POSITIVE / 4.0,
                    3 => 1.0,
                    4 => -1.0,
                    5 => f32::from_bits(0x7fc0_1234),
                    _ => ((i * 31 + c * 17) as f32).sin() * 1.5,
                })
                .collect()
        })
        .collect();
    AudioData {
        sample_rate: 48_000,
        channels: chans,
    }
}

fn files(b: &Bundle) -> Vec<String> {
    let mut names: Vec<String> = [b.audio_dir(), b.peaks_dir()]
        .iter()
        .flat_map(|d| fs::read_dir(d).unwrap().flatten())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn copy_from(data: &AudioData, from: u64, out: &mut [Vec<f32>]) {
    for (o, ch) in out.iter_mut().zip(&data.channels) {
        let from = from as usize;
        let n = o.len();
        o.copy_from_slice(&ch[from..from + n]);
    }
}

fn store_blocks_of(b: &mut Bundle, data: &AudioData, block: usize) -> Result<AssetInfo> {
    b.store_blocks(
        data.sample_rate,
        data.channels.len() as u16,
        data.frames() as u64,
        "take",
        block,
        |from, out| {
            copy_from(data, from, out);
            Ok(())
        },
    )
}

#[test]
fn blocks_give_the_same_hash_file_and_peaks_as_the_whole_buffer() {
    let real = STORE_BLOCK_FRAMES;
    let lengths = [1, 2, 256, 257, real - 1, real, real + 1, 3 * real + 1234];
    for (n, channels) in [1usize, 2, 6].into_iter().enumerate() {
        for &frames in &lengths {
            let data = audio(channels, frames);
            let mut whole = Bundle::create(&dir(&format!("whole-{n}-{frames}"))).unwrap();
            let expected = whole.add_recording(&data, "take").unwrap();
            for block in [1usize, 7, real] {
                if block == 1 && frames > 3000 {
                    continue; // the same code path, only slow
                }
                let mut b = Bundle::create(&dir(&format!("blocks-{n}-{frames}-{block}"))).unwrap();
                let info = store_blocks_of(&mut b, &data, block).unwrap();
                let label = format!("{channels} channels, {frames} frames, block {block}");
                assert_eq!(info, expected, "{label}");
                assert_eq!(
                    fs::read(b.asset_path(&info.hash)).unwrap(),
                    fs::read(whole.asset_path(&info.hash)).unwrap(),
                    "file bytes, {label}"
                );
                assert_eq!(
                    fs::read(b.peaks_path(&info.hash)).unwrap(),
                    fs::read(whole.peaks_path(&info.hash)).unwrap(),
                    "peak bytes, {label}"
                );
                assert_eq!(files(&b), files(&whole), "no temporary file, {label}");
            }
        }
    }
}

#[test]
fn the_public_block_store_gives_the_same_asset() {
    let data = audio(2, 2 * STORE_BLOCK_FRAMES + 5);
    let mut whole = Bundle::create(&dir("pub-whole")).unwrap();
    let expected = whole.add_recording(&data, "take").unwrap();
    let mut b = Bundle::create(&dir("pub-blocks")).unwrap();
    let info = b
        .add_recording_from(48_000, 2, data.frames() as u64, "take", |from, out| {
            copy_from(&data, from, out);
            Ok(())
        })
        .unwrap();
    assert_eq!(info, expected);
}

#[test]
fn no_frames_and_bad_shapes_are_refused_like_the_whole_buffer_store() {
    let mut b = Bundle::create(&dir("refused")).unwrap();
    let nothing = |_: u64, _: &mut [Vec<f32>]| Ok(());
    assert!(matches!(
        b.add_recording_from(48_000, 2, 0, "x", nothing),
        Err(AudioFileError::Empty)
    ));
    assert!(matches!(
        b.add_recording_from(0, 2, 10, "x", nothing),
        Err(AudioFileError::InvalidData(_))
    ));
    assert!(matches!(
        b.add_recording_from(48_000, 0, 10, "x", nothing),
        Err(AudioFileError::InvalidData(_))
    ));
    assert!(files(&b).is_empty());
}

#[test]
fn a_second_store_finds_the_asset_and_does_not_write_it_again() {
    let data = audio(2, 3000);
    let mut b = Bundle::create(&dir("twice")).unwrap();
    let first = store_blocks_of(&mut b, &data, 512).unwrap();
    let before = files(&b);
    let path = b.asset_path(&first.hash);
    let stamp = fs::metadata(&path).unwrap().modified().unwrap();
    let mut passes = 0u32;
    let again = b
        .store_blocks(48_000, 2, 3000, "again", 512, |from, out| {
            passes += 1;
            copy_from(&data, from, out);
            Ok(())
        })
        .unwrap();
    assert_eq!(again.hash, first.hash);
    assert_eq!(again.original_name, "again");
    assert_eq!(files(&b), before, "no temporary file, no second copy");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    // Only the hashing passes ran: one read per channel and block, none for the file.
    assert_eq!(passes, 2 * 3000u32.div_ceil(512));
    // The same audio stored from memory finds the same asset.
    assert_eq!(b.add_recording(&data, "mem").unwrap().hash, first.hash);
}

#[test]
fn a_failure_in_either_pass_leaves_nothing_behind() {
    let data = audio(2, 3000);
    let per_pass = 3000usize.div_ceil(500);
    let reads = 3 * per_pass;
    for fail_at in [0, 3, per_pass, 2 * per_pass, reads - 1] {
        let mut b = Bundle::create(&dir(&format!("fail-{fail_at}"))).unwrap();
        let mut n = 0usize;
        let result = b.store_blocks(48_000, 2, 3000, "x", 500, |from, out| {
            n += 1;
            if n == fail_at + 1 {
                return Err(AudioFileError::Io(io::Error::other("the reader failed")));
            }
            copy_from(&data, from, out);
            Ok(())
        });
        assert!(result.is_err(), "failing at read {fail_at}");
        assert_eq!(files(&b), Vec::<String>::new(), "failing at read {fail_at}");
        // The bundle works afterwards.
        let info = store_blocks_of(&mut b, &data, 500).unwrap();
        assert!(b.exists(&info.hash));
    }
}

#[test]
fn a_temporary_file_a_crash_left_is_cleaned_by_the_orphan_pass() {
    let data = audio(1, 100);
    let mut b = Bundle::create(&dir("crash")).unwrap();
    let info = store_blocks_of(&mut b, &data, 16).unwrap();
    // A process killed between the write and the rename leaves `<hash>.wav.tmp~`: it is not
    // an asset, and the orphan pass removes it.
    let stale = b.audio_dir().join(format!("{}.wav.tmp~", "ab".repeat(32)));
    fs::write(&stale, b"half a write").unwrap();
    let referenced: BTreeSet<String> = [info.hash.clone()].into();
    assert!(b.orphans(&referenced).is_empty());
    b.remove_orphans(&referenced).unwrap();
    assert!(!stale.exists());
    assert!(b.exists(&info.hash));
}
