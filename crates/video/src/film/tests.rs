use super::*;
use crate::fake::FakeTool;
use crate::probe::parse_probe;
use crate::samples::{MKV_HEVC, MOV_PRORES, MP4_H264};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const S: u64 = 1_000_000;
const FRAME: u64 = 40_000;

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn has(args: &[String], pair: [&str; 2]) -> bool {
    args.windows(2).any(|w| w == pair)
}

fn after(args: &[String], flag: &str) -> String {
    let at = args.iter().position(|a| a == flag).unwrap();
    args[at + 1].clone()
}

fn ffmpeg() -> Ffmpeg {
    Ffmpeg {
        ffmpeg: PathBuf::from("/bin/ffmpeg"),
        ffprobe: PathBuf::from("/bin/ffprobe"),
        version: crate::parse_version("ffmpeg version 7.1 x").unwrap(),
    }
}

fn info() -> ProbeInfo {
    parse_probe(MP4_H264).unwrap()
}

fn spec(picture: FilmPicture, sound: FilmSound) -> FilmSpec {
    FilmSpec {
        picture,
        sound,
        own_audio: false,
        rate: 48_000,
        channels: 2,
    }
}

/// The cut of a film that fills its span: 10 s of film, trimmed from 0, at the song's start.
fn plain_cut() -> FilmCut {
    FilmCut::layout((0, 0, 10 * S), (0, 10 * S), false, FRAME)
}

fn args_of(spec: &FilmSpec, cut: &FilmCut, tags: &Tags) -> Vec<String> {
    strings(&film_args(
        (
            Path::new("My Films/a film.mp4"),
            Path::new("tmp dir/mix.wav"),
            Path::new("out dir/new film.mp4.partial"),
        ),
        &info(),
        spec,
        cut,
        tags,
    ))
}

fn folder(tag: &str) -> PathBuf {
    use std::sync::atomic::AtomicU32;
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-video-film-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------ the cut

#[test]
fn a_film_at_the_songs_start_has_no_delay_and_no_black() {
    let cut = plain_cut();
    assert_eq!(
        cut,
        FilmCut {
            in_us: 0,
            length_us: 10 * S,
            black_before_us: 0,
            black_after_us: 0,
            mix_delay_us: 0,
            total_us: 10 * S,
        }
    );
    assert!(!cut.needs_black());
}

#[test]
fn a_trimmed_film_plays_its_trim_and_the_mix_covers_the_same_span() {
    // The trim 3..8 s of the source plays at 5 s of the song: the mix is song time 5..10 s.
    let cut = FilmCut::layout(
        (5 * S as i64, 3 * S, 8 * S),
        (5 * S as i64, 5 * S),
        false,
        FRAME,
    );
    assert_eq!(cut.in_us, 3 * S);
    assert_eq!(cut.length_us, 5 * S);
    assert_eq!(cut.total_us, 5 * S);
    assert_eq!(cut.mix_delay_us, 0);
    assert!(!cut.needs_black());
}

#[test]
fn a_film_that_begins_before_the_song_gets_silence_before_the_mix() {
    // The film starts 2 s before the song: the first 2 s of the film have no mix under them.
    let cut = FilmCut::layout((-2 * S as i64, 0, 10 * S), (0, 8 * S), false, FRAME);
    assert_eq!(cut.mix_delay_us, 2 * S);
    assert_eq!(cut.total_us, 10 * S);
    assert_eq!(cut.black_before_us, 0);
}

#[test]
fn a_mix_that_starts_a_few_microseconds_late_is_delayed_by_exactly_that() {
    let cut = FilmCut::layout((1_000_000, 0, 4 * S), (1_000_021, 4 * S), false, FRAME);
    assert_eq!(cut.mix_delay_us, 21);
    // A mix that starts a hair early cannot be pulled forward: no delay.
    let cut = FilmCut::layout((1_000_000, 0, 4 * S), (999_990, 4 * S), false, FRAME);
    assert_eq!(cut.mix_delay_us, 0);
}

#[test]
fn the_whole_song_adds_black_where_only_the_mix_runs() {
    // A 3 s film at 4 s of a 20 s song: 4 s of black, the film, 13 s of black.
    let cut = FilmCut::layout((4 * S as i64, 0, 3 * S), (0, 20 * S), true, FRAME);
    assert_eq!(cut.black_before_us, 4 * S);
    assert_eq!(cut.black_after_us, 13 * S);
    assert_eq!(cut.mix_delay_us, 0);
    assert_eq!(cut.total_us, 20 * S);
    assert!(cut.needs_black());
}

#[test]
fn the_whole_song_with_a_film_that_begins_before_it_starts_with_the_film() {
    // The film plays -2..8 s, the song is 0..20 s: the result runs -2..20 s, the mix waits 2 s.
    let cut = FilmCut::layout((-2 * S as i64, 0, 10 * S), (0, 20 * S), true, FRAME);
    assert_eq!(cut.black_before_us, 0);
    assert_eq!(cut.mix_delay_us, 2 * S);
    assert_eq!(cut.black_after_us, 12 * S);
    assert_eq!(cut.total_us, 22 * S);
}

#[test]
fn a_film_longer_than_the_song_ends_the_whole_song_export_with_the_film() {
    let cut = FilmCut::layout((0, 0, 30 * S), (0, 20 * S), true, FRAME);
    assert_eq!((cut.black_before_us, cut.black_after_us), (0, 0));
    assert_eq!(cut.total_us, 30 * S);
}

#[test]
fn black_shorter_than_half_a_frame_is_dropped() {
    let cut = FilmCut::layout((19_000, 0, 10 * S), (0, 10 * S), true, FRAME);
    assert_eq!(
        cut.black_before_us, 0,
        "19 ms is less than half of a 40 ms frame"
    );
    let cut = FilmCut::layout((21_000, 0, 10 * S), (0, 10 * S), true, FRAME);
    assert_eq!(cut.black_before_us, 21_000);
}

#[test]
fn a_frame_is_as_long_as_the_rate_says() {
    assert_eq!(frame_us(Rational { num: 25, den: 1 }), 40_000);
    assert_eq!(
        frame_us(Rational {
            num: 30_000,
            den: 1_001
        }),
        33_367
    );
}

// ------------------------------------------------------------------ copy or re-encode

#[test]
fn auto_copies_a_cut_at_the_start_and_encodes_the_rest() {
    let i = info();
    let at_start = plain_cut();
    assert_eq!(
        plan_picture(
            PictureChoice::Auto(FilmQuality::Standard),
            &i,
            FilmContainer::Mp4,
            &at_start
        )
        .unwrap(),
        FilmPicture::Copy
    );
    let trimmed = FilmCut::layout((0, 3 * S, 8 * S), (0, 5 * S), false, FRAME);
    assert!(matches!(
        plan_picture(
            PictureChoice::Auto(FilmQuality::Standard),
            &i,
            FilmContainer::Mp4,
            &trimmed
        )
        .unwrap(),
        FilmPicture::Encode(FilmQuality::Standard)
    ));
    let black = FilmCut::layout((4 * S as i64, 0, 3 * S), (0, 20 * S), true, FRAME);
    assert!(matches!(
        plan_picture(
            PictureChoice::Auto(FilmQuality::Standard),
            &i,
            FilmContainer::Mp4,
            &black
        )
        .unwrap(),
        FilmPicture::Encode(_)
    ));
}

#[test]
fn auto_encodes_a_picture_the_container_cannot_hold() {
    // ProRes goes into MOV as it is, not into MP4.
    let prores = parse_probe(MOV_PRORES).unwrap();
    let cut = plain_cut();
    assert_eq!(
        plan_picture(
            PictureChoice::Auto(FilmQuality::Standard),
            &prores,
            FilmContainer::Mov,
            &cut
        )
        .unwrap(),
        FilmPicture::Copy
    );
    assert!(matches!(
        plan_picture(
            PictureChoice::Auto(FilmQuality::Standard),
            &prores,
            FilmContainer::Mp4,
            &cut
        )
        .unwrap(),
        FilmPicture::Encode(_)
    ));
}

#[test]
fn a_forced_copy_is_refused_when_it_cannot_be_done() {
    let cut = plain_cut();
    let prores = parse_probe(MOV_PRORES).unwrap();
    let err = plan_picture(PictureChoice::Copy, &prores, FilmContainer::Mp4, &cut).unwrap_err();
    assert!(err.to_string().contains("RE-ENCODE"), "{err}");
    assert!(err.to_string().contains("prores"), "{err}");
    let black = FilmCut::layout((4 * S as i64, 0, 3 * S), (0, 20 * S), true, FRAME);
    let err = plan_picture(PictureChoice::Copy, &info(), FilmContainer::Mp4, &black).unwrap_err();
    assert!(err.to_string().contains("black"), "{err}");
    // A copy of a trimmed start is allowed: it snaps to a keyframe.
    let trimmed = FilmCut::layout((0, 3 * S, 8 * S), (0, 5 * S), false, FRAME);
    assert_eq!(
        plan_picture(PictureChoice::Copy, &info(), FilmContainer::Mp4, &trimmed).unwrap(),
        FilmPicture::Copy
    );
}

#[test]
fn a_forced_encode_is_always_possible() {
    let hevc = parse_probe(MKV_HEVC).unwrap();
    assert_eq!(
        plan_picture(
            PictureChoice::Encode(FilmQuality::High),
            &hevc,
            FilmContainer::Mp4,
            &plain_cut()
        )
        .unwrap(),
        FilmPicture::Encode(FilmQuality::High)
    );
}

// ------------------------------------------------------------------ the arguments

#[test]
fn a_copy_takes_the_picture_as_it_is_and_the_mix_as_aac_in_mp4() {
    let a = args_of(
        &spec(FilmPicture::Copy, FilmSound::Aac(256)),
        &plain_cut(),
        &Tags::default(),
    );
    assert_eq!(a[0], "-nostdin");
    assert!(has(&a, ["-c:v", "copy"]), "{a:?}");
    assert!(!a.contains(&"libx264".to_owned()));
    assert!(has(&a, ["-map", "0:0"]));
    assert!(has(&a, ["-map", "[mix]"]));
    assert!(has(&a, ["-c:a", "aac"]));
    assert!(has(&a, ["-b:a", "256k"]));
    assert!(has(&a, ["-ar", "48000"]));
    assert!(has(&a, ["-f", "mp4"]));
    assert!(has(&a, ["-movflags", "+faststart"]));
    assert!(has(&a, ["-progress", "pipe:1"]));
    assert!(has(&a, ["-map_metadata", "-1"]));
    // The film's sound is not taken.
    assert!(!a.contains(&"0:1".to_owned()));
    // No trim of the input when the cut is at the start.
    assert!(!a.contains(&"-ss".to_owned()));
    assert_eq!(after(&a, "-t"), "10.000000");
    // Both inputs are files, and the output is the last argument.
    let inputs: Vec<&String> = a
        .iter()
        .enumerate()
        .filter(|(_, x)| *x == "-i")
        .map(|(i, _)| &a[i + 1])
        .collect();
    assert_eq!(inputs, ["file:My Films/a film.mp4", "file:tmp dir/mix.wav"]);
    assert_eq!(a.iter().filter(|x| *x == "-protocol_whitelist").count(), 2);
    assert_eq!(a.last().unwrap(), "file:out dir/new film.mp4.partial");
    // The mix is only padded: nothing waits.
    assert_eq!(after(&a, "-filter_complex"), "[1:a:0]apad[mix]");
}

#[test]
fn a_reencode_is_h264_with_the_quality_asked_for() {
    for (quality, crf) in [
        (FilmQuality::High, "18"),
        (FilmQuality::Standard, "23"),
        (FilmQuality::Small, "28"),
    ] {
        let cut = FilmCut::layout((0, 3 * S, 8 * S), (0, 5 * S), false, FRAME);
        let a = args_of(
            &spec(FilmPicture::Encode(quality), FilmSound::Aac(192)),
            &cut,
            &Tags::default(),
        );
        assert!(has(&a, ["-c:v", "libx264"]), "{a:?}");
        assert!(has(&a, ["-crf", crf]), "{a:?}");
        assert!(has(&a, ["-pix_fmt", "yuv420p"]));
        // The trim is the input's, to the microsecond, and frame accurate on a re-encode.
        assert_eq!(after(&a, "-ss"), "3.000000");
        let first_t = a.iter().position(|x| x == "-t").unwrap();
        assert_eq!(a[first_t + 1], "5.000000");
        assert!(first_t < a.iter().position(|x| x == "-i").unwrap(), "{a:?}");
    }
}

#[test]
fn pcm_goes_into_a_mov_file() {
    let a = args_of(
        &spec(FilmPicture::Copy, FilmSound::Pcm),
        &plain_cut(),
        &Tags::default(),
    );
    assert!(has(&a, ["-c:a", "pcm_s24le"]), "{a:?}");
    assert!(has(&a, ["-f", "mov"]));
    assert!(!a.contains(&"-b:a".to_owned()));
    assert!(!a.contains(&"+faststart".to_owned()));
    assert_eq!(FilmSound::Pcm.container().extension(), "mov");
    assert_eq!(FilmSound::Aac(192).container().extension(), "mp4");
}

#[test]
fn a_mix_that_starts_late_is_delayed_in_samples() {
    // 2 s before the film: 96 000 samples at 48 kHz, on each of the two channels.
    let cut = FilmCut::layout((-2 * S as i64, 0, 10 * S), (0, 8 * S), false, FRAME);
    let a = args_of(
        &spec(FilmPicture::Copy, FilmSound::Aac(192)),
        &cut,
        &Tags::default(),
    );
    assert_eq!(
        after(&a, "-filter_complex"),
        "[1:a:0]adelay=96000S|96000S,apad[mix]"
    );
}

#[test]
fn black_pictures_come_from_a_generated_source_and_join_the_film() {
    let cut = FilmCut::layout((4 * S as i64, 0, 3 * S), (0, 20 * S), true, FRAME);
    let a = args_of(
        &spec(
            FilmPicture::Encode(FilmQuality::Standard),
            FilmSound::Aac(192),
        ),
        &cut,
        &Tags::default(),
    );
    let graph = after(&a, "-filter_complex");
    assert!(
        graph.contains("color=c=black:s=1920x1080:r=25/1:d=4.000000"),
        "{graph}"
    );
    assert!(graph.contains("d=13.000000"), "{graph}");
    assert!(
        graph.contains("[bb][fv][ba]concat=n=3:v=1:a=0[v]"),
        "{graph}"
    );
    assert!(has(&a, ["-map", "[v]"]));
    assert_eq!(
        after(&a, "-t").as_str(),
        "3.000000",
        "the film's own length"
    );
    let last_t = a.iter().rposition(|x| x == "-t").unwrap();
    assert_eq!(a[last_t + 1], "20.000000");
    // No input is a generated one: the source filter lives in the graph.
    assert!(!a.contains(&"lavfi".to_owned()));
}

#[test]
fn only_the_black_that_is_needed_is_made() {
    let cut = FilmCut::layout((0, 0, 3 * S), (0, 20 * S), true, FRAME);
    let a = args_of(
        &spec(FilmPicture::Encode(FilmQuality::High), FilmSound::Pcm),
        &cut,
        &Tags::default(),
    );
    let graph = after(&a, "-filter_complex");
    assert!(!graph.contains("[bb]"), "{graph}");
    assert!(graph.contains("[fv][ba]concat=n=2:v=1:a=0[v]"), "{graph}");
}

#[test]
fn the_films_own_sound_is_a_second_track_lined_up_with_its_picture() {
    let mut s = spec(FilmPicture::Copy, FilmSound::Aac(192));
    s.own_audio = true;
    let a = args_of(&s, &plain_cut(), &Tags::default());
    let maps: Vec<String> = a
        .windows(2)
        .filter(|w| w[0] == "-map")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(maps, ["0:0", "[mix]", "0:1"]);
    assert!(has(&a, ["-metadata:s:a:0", "title=Mix"]));
    assert!(has(&a, ["-metadata:s:a:1", "title=Film audio"]));
    // Behind black pictures it waits for the film: 4 s at the film's 48 kHz, in each channel.
    let cut = FilmCut::layout((4 * S as i64, 0, 3 * S), (0, 20 * S), true, FRAME);
    s.picture = FilmPicture::Encode(FilmQuality::Standard);
    let a = args_of(&s, &cut, &Tags::default());
    let graph = after(&a, "-filter_complex");
    assert!(
        graph.contains("[0:1]adelay=192000S|192000S[own]"),
        "{graph}"
    );
    assert!(has(&a, ["-map", "[own]"]));
}

#[test]
fn a_film_without_sound_of_its_own_adds_no_second_track() {
    let silent = parse_probe(crate::samples::NO_AUDIO).unwrap();
    let mut s = spec(FilmPicture::Copy, FilmSound::Aac(192));
    s.own_audio = true;
    let a = strings(&film_args(
        (Path::new("f.mp4"), Path::new("m.wav"), Path::new("o.mp4")),
        &silent,
        &s,
        &plain_cut(),
        &Tags::default(),
    ));
    assert_eq!(a.iter().filter(|x| *x == "-map").count(), 2);
    assert!(!a.contains(&"-metadata:s:a:1".to_owned()));
}

#[test]
fn more_than_two_channels_are_mixed_down_and_the_delay_covers_each_one() {
    let mut s = spec(FilmPicture::Copy, FilmSound::Aac(192));
    s.channels = 6;
    let cut = FilmCut::layout((-(S as i64), 0, 4 * S), (0, 3 * S), false, FRAME);
    let a = args_of(&s, &cut, &Tags::default());
    assert!(has(&a, ["-ac", "2"]));
    assert_eq!(
        after(&a, "-filter_complex"),
        "[1:a:0]adelay=48000S|48000S|48000S|48000S|48000S|48000S,apad[mix]"
    );
}

#[test]
fn the_tags_are_separate_arguments_and_names_are_plain_files() {
    let tags = Tags {
        title: "Say \"hi\"; rm -rf $HOME".to_owned(),
        author: "-f lavfi\nsecond".to_owned(),
    };
    let a = args_of(
        &spec(FilmPicture::Copy, FilmSound::Aac(192)),
        &plain_cut(),
        &tags,
    );
    assert!(a.contains(&"title=Say \"hi\"; rm -rf $HOME".to_owned()));
    assert!(a.contains(&"artist=-f lavfi second".to_owned()));
    for name in ["-f lavfi", "http://example.invalid/x.mp4", "concat:a|b"] {
        let a = strings(&film_args(
            (Path::new(name), Path::new(name), Path::new(name)),
            &info(),
            &spec(FilmPicture::Copy, FilmSound::Aac(192)),
            &plain_cut(),
            &Tags::default(),
        ));
        assert_eq!(
            a.iter().filter(|x| **x == format!("file:{name}")).count(),
            3
        );
    }
}

// ------------------------------------------------------------------ the job

fn job<'a>(
    i: &'a ProbeInfo,
    film: &'a Path,
    mix: &'a Path,
    out: &'a Path,
    tags: &'a Tags,
) -> FilmJob<'a> {
    FilmJob {
        film,
        mix,
        out,
        info: i,
        spec: spec(FilmPicture::Copy, FilmSound::Aac(192)),
        cut: FilmCut {
            total_us: 12_021_333,
            ..plain_cut()
        },
        tags,
    }
}

#[test]
fn the_film_appears_complete_and_progress_runs_to_one() {
    let dir = folder("ok");
    let (film, mix, out) = (dir.join("a.mp4"), dir.join("mix.wav"), dir.join("new.mp4"));
    let tool = FakeTool::new().audio(b"MOVIE".to_vec());
    let seen = Mutex::new(Vec::new());
    let (i, tags) = (info(), Tags::default());
    export_film(
        &tool,
        &ffmpeg(),
        &job(&i, &film, &mix, &out, &tags),
        &AtomicBool::new(false),
        &|p| seen.lock().unwrap().push(p),
    )
    .unwrap();
    assert_eq!(fs::read(&out).unwrap(), b"MOVIE");
    assert!(!partial_path(&out).exists(), "the partial file was renamed");
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.first(), Some(&0.0));
    assert_eq!(seen.last(), Some(&1.0));
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    let calls = tool.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].exe, PathBuf::from("/bin/ffmpeg"));
    // FFmpeg wrote to the temporary name, not to the film.
    assert_eq!(
        calls[0].args.last().unwrap().to_string_lossy(),
        format!("file:{}", partial_path(&out).display())
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_existing_film_is_replaced_only_by_a_complete_one() {
    let dir = folder("replace");
    let (film, mix, out) = (dir.join("a.mp4"), dir.join("mix.wav"), dir.join("new.mp4"));
    fs::write(&out, b"old").unwrap();
    let tool = FakeTool::new()
        .audio(b"half".to_vec())
        .stream_fails("Unknown encoder 'libx264'");
    let (i, tags) = (info(), Tags::default());
    let err = export_film(
        &tool,
        &ffmpeg(),
        &job(&i, &film, &mix, &out, &tags),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("libx264"), "{err}");
    assert_eq!(fs::read(&out).unwrap(), b"old", "the old film is untouched");
    assert!(!partial_path(&out).exists(), "no partial file is left");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_stopped_job_leaves_no_file() {
    let dir = folder("cancel");
    let (film, mix, out) = (dir.join("a.mp4"), dir.join("mix.wav"), dir.join("new.mp4"));
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let tool = FakeTool::new().audio(b"half".to_vec()).on_frame(move |n| {
        if n == 2 {
            flag.store(true, Ordering::Relaxed);
        }
    });
    let (i, tags) = (info(), Tags::default());
    let result = export_film(
        &tool,
        &ffmpeg(),
        &job(&i, &film, &mix, &out, &tags),
        &cancel,
        &|_| {},
    );
    assert!(matches!(result, Err(VideoError::Cancelled)), "{result:?}");
    assert!(!out.exists());
    assert!(
        !partial_path(&out).exists(),
        "the half-written film is removed"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_empty_output_is_an_error_and_leaves_nothing() {
    let dir = folder("empty");
    let (film, mix, out) = (dir.join("a.mp4"), dir.join("mix.wav"), dir.join("new.mp4"));
    let (i, tags) = (info(), Tags::default());
    let err = export_film(
        &FakeTool::new().audio(Vec::new()),
        &ffmpeg(),
        &job(&i, &film, &mix, &out, &tags),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("no film"), "{err}");
    assert!(!out.exists() && !partial_path(&out).exists());
    let _ = fs::remove_dir_all(dir);
}
