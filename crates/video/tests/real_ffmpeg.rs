//! The real FFmpeg, when it is installed. Ignored by default: CI and most machines have none.
//!
//! Run it with `cargo test -p atomik-video --test real_ffmpeg -- --ignored --nocapture`. It looks
//! for FFmpeg like the app does (the `ATOMIK_FFMPEG_DIR` folder when set, else the system path),
//! makes a two-second test film with FFmpeg's own generators in a temporary folder, probes it,
//! builds its proxy and reads frames back. Without FFmpeg it says so and passes.

use atomik_video::runner::capture;
use atomik_video::{
    Control, FfmpegStatus, FrameDecoder, FrameEvent, Platform, ProxyCache, ProxyReader, ProxySpec,
    Search, SystemTool, decode_jpeg, discover, prepare,
};
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-video-real-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "needs a real FFmpeg"]
fn a_generated_film_is_probed_and_gets_its_proxy() {
    let folder = std::env::var_os("ATOMIK_FFMPEG_DIR").map(PathBuf::from);
    let path_var = std::env::var_os("PATH");
    let status = discover(
        &Search {
            folder: folder.as_deref(),
            path_var: path_var.as_deref(),
            platform: Platform::current(),
        },
        &SystemTool,
        &Path::is_file,
    );
    let FfmpegStatus::Found(ffmpeg) = status else {
        println!("no usable FFmpeg here: {}", status.message());
        return;
    };
    println!("{}", FfmpegStatus::Found(ffmpeg.clone()).message());
    let dir = temp("film");
    let film = dir.join("test film.mp4");
    let never = AtomicBool::new(false);
    let args: Vec<OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=25:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
        "-pix_fmt",
        "yuv420p",
        "-shortest",
        "-y",
    ]
    .map(OsString::from)
    .into_iter()
    .chain([film.clone().into_os_string()])
    .collect();
    capture(&SystemTool, &ffmpeg.ffmpeg, &args, 1 << 20, &never).unwrap();

    let cache = ProxyCache::new(dir.join("cache"));
    let done = AtomicU64::new(0);
    let total = AtomicU64::new(0);
    let ctl = Control {
        cancel: &never,
        done: &done,
        total: &total,
    };
    let got = prepare(
        &SystemTool,
        &ffmpeg,
        &film,
        &cache,
        &ProxySpec::default(),
        &ctl,
    )
    .unwrap();
    println!("{:?}", got.probe);
    assert_eq!((got.probe.video.width, got.probe.video.height), (640, 360));
    assert_eq!(got.probe.video.frame_rate.num, 25);
    assert_eq!(got.probe.audio.len(), 1);
    assert!((got.probe.duration_s - 2.0).abs() < 0.1);
    assert!((49..=51).contains(&got.info.frames), "{}", got.info.frames);
    assert_eq!(got.info.height, 360);
    assert_eq!(done.load(Ordering::Relaxed), u64::from(got.info.frames));

    let mut reader = ProxyReader::open(File::open(&got.proxy).unwrap()).unwrap();
    let mut frame = Vec::new();
    for n in [0, got.info.frames / 2, got.info.frames - 1] {
        reader.read_frame(n, &mut frame).unwrap();
        assert_eq!(&frame[..2], &[0xFF, 0xD8]);
        assert_eq!(&frame[frame.len() - 2..], &[0xFF, 0xD9]);
    }
    // The pure-Rust decoder reads what FFmpeg wrote, frame by frame, and the frames differ (the
    // test picture runs a counter), through the worker the viewer uses as well.
    let first = {
        reader.read_frame(0, &mut frame).unwrap();
        decode_jpeg(&frame).unwrap()
    };
    assert_eq!(
        (first.width, first.height),
        (got.info.width, got.info.height)
    );
    assert!(
        first
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] != [0, 0, 0])
    );
    let worker = FrameDecoder::spawn_file(got.proxy.clone()).unwrap();
    let last = got.info.frames - 1;
    worker.want(vec![last, 0]);
    let mut decoded = Vec::new();
    for _ in 0..2_000 {
        for e in worker.poll() {
            match e {
                FrameEvent::Frame(n, p) => decoded.push((n, p)),
                FrameEvent::Failed(n, why) => panic!("frame {n:?}: {why}"),
                FrameEvent::Opened(_) => {}
            }
        }
        if decoded.len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(decoded.len(), 2, "the worker decoded both frames");
    assert_eq!(decoded[0].0, last);
    assert_eq!(decoded[1].1, first);
    assert_ne!(
        decoded[0].1.rgba, first.rgba,
        "the last frame is not the first"
    );
    println!("decoded {}x{}", first.width, first.height);
    // The thumbnail worker of the arrangement: the same frames, scaled down on its thread.
    let thumbs = FrameDecoder::spawn_file_scaled(got.proxy.clone(), 40).unwrap();
    thumbs.want(vec![last]);
    let mut small = None;
    for _ in 0..2_000 {
        for e in thumbs.poll() {
            if let FrameEvent::Frame(n, p) = e {
                assert_eq!(n, last);
                small = Some(p);
            }
        }
        if small.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let small = small.expect("the thumbnail worker decoded the last frame");
    assert_eq!(small.height, 40);
    assert_eq!(small, decoded[0].1.scaled_to_height(40));
    // The second call is a cache hit.
    let again = prepare(
        &SystemTool,
        &ffmpeg,
        &film,
        &cache,
        &ProxySpec::default(),
        &ctl,
    )
    .unwrap();
    assert!(again.reused);

    // A cancelled build leaves nothing: a flag that is already set stops before the start.
    let stop = AtomicBool::new(true);
    let other = dir.join("other.mp4");
    fs::copy(&film, &other).unwrap();
    let ctl = Control {
        cancel: &stop,
        done: &done,
        total: &total,
    };
    assert!(
        prepare(
            &SystemTool,
            &ffmpeg,
            &other,
            &cache,
            &ProxySpec::default(),
            &ctl
        )
        .is_err()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The frames of the `data` chunk of a 24 bit WAV file, its channels and its rate.
fn wav_shape(bytes: &[u8]) -> (u64, u16, u32) {
    assert_eq!(&bytes[..4], b"RIFF");
    let (mut at, mut channels, mut rate) = (12usize, 0u16, 0u32);
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        match &bytes[at..at + 4] {
            b"fmt " => {
                channels = u16::from_le_bytes(bytes[at + 10..at + 12].try_into().unwrap());
                rate = u32::from_le_bytes(bytes[at + 12..at + 16].try_into().unwrap());
                let bits = u16::from_le_bytes(bytes[at + 22..at + 24].try_into().unwrap());
                assert_eq!(bits, 24);
            }
            b"data" => return ((size / (3 * usize::from(channels))) as u64, channels, rate),
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

#[test]
#[ignore = "needs a real FFmpeg"]
fn the_audio_of_a_generated_film_is_extracted_with_its_start_offset() {
    let folder = std::env::var_os("ATOMIK_FFMPEG_DIR").map(PathBuf::from);
    let path_var = std::env::var_os("PATH");
    let status = discover(
        &Search {
            folder: folder.as_deref(),
            path_var: path_var.as_deref(),
            platform: Platform::current(),
        },
        &SystemTool,
        &Path::is_file,
    );
    let FfmpegStatus::Found(ffmpeg) = status else {
        println!("no usable FFmpeg here: {}", status.message());
        return;
    };
    let dir = temp("audio");
    let film = dir.join("late sound.mkv");
    let never = AtomicBool::new(false);
    // The sound is made to start half a second after the picture.
    let mut args: Vec<OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x180:rate=25:duration=2",
        "-itsoffset",
        "0.5",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2:sample_rate=44100",
        "-pix_fmt",
        "yuv420p",
        "-y",
    ]
    .map(OsString::from)
    .into();
    args.push(film.clone().into_os_string());
    capture(&SystemTool, &ffmpeg.ffmpeg, &args, 1 << 20, &never).unwrap();

    let info = atomik_video::probe_file(&SystemTool, &ffmpeg, &film, &never).unwrap();
    assert_eq!(info.audio.len(), 1);
    let Ok(atomik_video::Pick::One(stream)) = atomik_video::pick_stream(&info, None) else {
        panic!("one audio track expected");
    };
    let lead = atomik_video::lead_us(&info, stream);
    println!("lead {lead} us");
    assert!((lead - 500_000).abs() <= 5_000, "{lead}");

    let out = dir.join("sound.wav");
    let (done, total) = (AtomicU64::new(0), AtomicU64::new(0));
    atomik_video::extract_audio(
        &SystemTool,
        &ffmpeg,
        &film,
        (&info, stream),
        &out,
        &Control {
            cancel: &never,
            done: &done,
            total: &total,
        },
    )
    .unwrap();
    let (frames, channels, rate) = wav_shape(&fs::read(&out).unwrap());
    println!("{frames} frames, {channels} channels, {rate} Hz");
    // The file holds the sound only: no silence was written for the late start (the lead
    // places it), and the track keeps its own rate.
    assert_eq!((channels, rate), (1, 44_100));
    let seconds = frames as f64 / f64::from(rate);
    assert!((seconds - 2.0).abs() < 0.05, "{seconds} s");
    assert_eq!(done.load(Ordering::Relaxed), total.load(Ordering::Relaxed));

    // A stopped extraction leaves no file.
    let stop = AtomicBool::new(true);
    let other = dir.join("other.wav");
    assert!(
        atomik_video::extract_audio(
            &SystemTool,
            &ffmpeg,
            &film,
            (&info, stream),
            &other,
            &Control {
                cancel: &stop,
                done: &done,
                total: &total,
            },
        )
        .is_err()
    );
    assert!(!other.exists());
    let _ = fs::remove_dir_all(&dir);
}
