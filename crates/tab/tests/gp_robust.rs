//! Hostile and damaged Guitar Pro input: truncation at every byte, flipped bytes, garbage,
//! decompression bombs and size caps. Nothing here may panic, run for long or allocate without
//! bound; the readers return an error or a score.

mod gp_fixtures;

use std::io::Write;

use atomik_tab::gp::{GpError, limits, read_gp};
use flate2::Compression;
use flate2::write::DeflateEncoder;
use gp_fixtures::binary::*;
use gp_fixtures::containers::{bcfz, gp, gpx, zip};
use gp_fixtures::gpif::{GUITAR, Gpif};
use gp_fixtures::{Rng, W};
use proptest::prelude::*;

fn small_song(ver: u16) -> Song {
    let mut s = Song::new(ver, 3);
    s.add_track(TrackDef::guitar("Bass").with_strings(&[43, 38, 33, 28]));
    s.bars[1].repeat_open = true;
    s.bars[2].repeat_close = Some(2);
    s.bars[0].marker = Some("A".into());
    s.lyrics = Some((1, vec![(1, "la la".into())]));
    let fx = NoteFx {
        bend: Some(vec![(0, 0), (30, 100)]),
        grace: Some((3, 1, 0)),
        harmonic: Some((1, vec![])),
        slide: Some(1),
        ..NoteFx::default()
    };
    for b in 0..3 {
        let mut beat =
            BeatDef::quarter(vec![NoteDef::new(0, 3).fx(fx.clone()), NoteDef::new(2, 4)]);
        beat.chord = Some(ChordDef {
            name: "C".into(),
            frets: vec![0, 1, 0, 2, 3, -1],
            new_format: b % 2 == 0,
        });
        beat.fx = Some(BeatFx {
            stroke_up: Some(true),
            ..BeatFx::default()
        });
        beat.tempo = Some(100 + b);
        s.beats[b as usize][0] = vec![
            beat,
            BeatDef::rest(0),
            BeatDef::of(-1, vec![NoteDef::new(1, 2)]),
        ];
        s.beats[b as usize][1] = vec![BeatDef::of(-2, vec![NoteDef::new(3, 0)])];
    }
    s
}

fn small_gpif() -> String {
    let mut g = Gpif::new();
    g.track("Guitar", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(
        3,
        5,
        &[gp_fixtures::gpif::enabled("PalmMuted")],
        "<Vibrato>Slight</Vibrato>",
    );
    let b = g.beat(q, &[n], &[], "<Dynamic>F</Dynamic>");
    let v = g.voice(&[b, b]);
    let bar = g.bar(&[v]);
    g.master(
        "4/4",
        &[bar],
        "<Repeat start=\"true\" end=\"true\" count=\"2\"/>",
    );
    g.render()
}

#[test]
fn a_truncated_binary_file_is_an_error_at_every_length() {
    for ver in [300, 400, 500, 510] {
        let bytes = small_song(ver).encode();
        assert!(read_gp(&bytes).is_ok(), "{ver}");
        for n in 0..bytes.len() {
            assert!(
                read_gp(&bytes[..n]).is_err(),
                "version {ver} cut at {n} of {}",
                bytes.len()
            );
        }
    }
}

#[test]
fn truncated_containers_never_panic() {
    let xml = small_gpif();
    for bytes in [
        gp(&xml, true),
        gp(&xml, false),
        gpx(&xml, true, true),
        gpx(&xml, true, false),
        gpx(&xml, false, false),
        xml.clone().into_bytes(),
    ] {
        assert!(read_gp(&bytes).is_ok());
        for n in 0..bytes.len() {
            let _ = read_gp(&bytes[..n]);
        }
    }
    // A zip is only complete with its directory.
    let z = gp(&xml, true);
    for n in 0..z.len() {
        assert!(read_gp(&z[..n]).is_err(), "cut at {n}");
    }
}

#[test]
fn flipped_bytes_never_panic_or_hang() {
    let xml = small_gpif();
    let mut inputs: Vec<Vec<u8>> = [300, 400, 500, 510]
        .iter()
        .map(|v| small_song(*v).encode())
        .collect();
    inputs.push(gp(&xml, true));
    inputs.push(gpx(&xml, true, true));
    inputs.push(gpx(&xml, false, false));
    inputs.push(xml.into_bytes());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for base in &inputs {
        for _ in 0..1500 {
            let mut b = base.clone();
            for _ in 0..1 + rng.below(4) {
                let i = rng.below(b.len());
                match rng.below(3) {
                    0 => b[i] ^= 1 << rng.below(8),
                    1 => b[i] = rng.next() as u8,
                    _ => b[i] = if rng.below(2) == 0 { 0xFF } else { 0x7F },
                }
            }
            if rng.below(8) == 0 {
                b.truncate(rng.below(b.len()));
            }
            let _ = read_gp(&b);
        }
    }
}

#[test]
fn garbage_is_refused() {
    let mut rng = Rng(42);
    for len in [0usize, 1, 2, 3, 5, 8, 30, 31, 100, 1000, 5000] {
        for _ in 0..40 {
            let b: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let _ = read_gp(&b);
        }
    }
    // Valid magic, random rest.
    for magic in [
        &b"BCFZ"[..],
        b"BCFS",
        b"PK\x03\x04",
        b"PK\x05\x06",
        b"<?xml",
        b"<GPIF>",
    ] {
        for len in [0usize, 4, 64, 4096] {
            for _ in 0..30 {
                let mut b = magic.to_vec();
                b.extend((0..len).map(|_| rng.next() as u8));
                let _ = read_gp(&b);
            }
        }
    }
    // The binary header, then random.
    for ver in [300, 400, 500, 510] {
        let head = Song::new(ver, 1).encode();
        for _ in 0..200 {
            let cut = 31 + rng.below(head.len() - 31);
            let mut b = head[..cut].to_vec();
            b.extend((0..rng.below(2000)).map(|_| rng.next() as u8));
            let _ = read_gp(&b);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_bytes_after_a_binary_header_never_panic(tail in proptest::collection::vec(any::<u8>(), 0..3000), ver in 0usize..4) {
        let v = [300u16, 400, 500, 510][ver];
        let mut b = Song::new(v, 1).encode();
        b.truncate(31);
        b.extend(tail);
        let _ = read_gp(&b);
    }

    #[test]
    fn any_bytes_never_panic(b in proptest::collection::vec(any::<u8>(), 0..4000)) {
        let _ = read_gp(&b);
    }
}

#[test]
fn a_file_over_the_cap_is_refused_unread() {
    let big = vec![0u8; limits::MAX_FILE_BYTES + 1];
    assert!(matches!(read_gp(&big), Err(GpError::TooLarge(_, _))));
}

#[test]
fn a_bcfz_that_claims_gigabytes_is_refused() {
    let mut d = b"BCFZ".to_vec();
    d.extend_from_slice(&u32::MAX.to_le_bytes());
    d.extend_from_slice(&[0; 64]);
    assert!(matches!(read_gp(&d), Err(GpError::TooBig(_))));
}

#[test]
fn a_bcfz_whose_copies_blow_up_is_cut() {
    // One literal byte, then copies of 4095 bytes over and over: 32 bits each unpack to 4 KiB, so a
    // small file reaches the cap quickly. Declared size = the cap, which is allowed, so the loop
    // must stop at it.
    let mut bits: Vec<u8> = Vec::new();
    let mut n = 0usize;
    let mut put = |v: u32, count: u32, msb: bool, bits: &mut Vec<u8>| {
        for i in 0..count {
            let bit = if msb {
                (v >> (count - 1 - i)) & 1
            } else {
                (v >> i) & 1
            };
            if n.is_multiple_of(8) {
                bits.push(0);
            }
            if bit != 0 {
                let last = bits.len() - 1;
                bits[last] |= 0x80 >> (n % 8);
            }
            n += 1;
        }
    };
    // 4095 literal bytes (three per token), then copies of all of them: 4 KiB out for 29 bits in.
    for chunk in 0..1365 {
        put(0, 1, true, &mut bits);
        put(3, 2, false, &mut bits);
        for k in 0..3u32 {
            let idx = chunk * 3 + k as usize;
            put(
                if idx < 4 { u32::from(b"BCFS"[idx]) } else { 0 },
                8,
                true,
                &mut bits,
            );
        }
    }
    for _ in 0..40_000 {
        put(1, 1, true, &mut bits);
        put(12, 4, true, &mut bits);
        put(4095, 12, false, &mut bits);
        put(4095, 12, false, &mut bits);
    }
    let mut d = b"BCFZ".to_vec();
    d.extend_from_slice(&(limits::MAX_DECOMPRESSED as u32).to_le_bytes());
    d.extend_from_slice(&bits);
    // The output reaches the cap and stops there: an error, quickly.
    assert!(read_gp(&d).is_err());
}

#[test]
fn a_zip_bomb_is_cut_at_the_cap() {
    // About 130 MiB of zeros, which deflates to a few hundred kilobytes.
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..(limits::MAX_DECOMPRESSED / (1 << 20) + 2) {
        enc.write_all(&chunk).unwrap();
    }
    let packed = enc.finish().unwrap();
    assert!(packed.len() < 1_000_000, "{}", packed.len());
    // Build the archive by hand around the already deflated bytes.
    let name = b"Content/score.gpif";
    let mut w = W::default();
    w.0.extend_from_slice(b"PK\x03\x04");
    w.i16(20);
    w.i16(0);
    w.i16(8);
    w.i16(0);
    w.i16(0);
    w.i32(0);
    w.i32(packed.len() as i32);
    w.i32(10); // lies about the size
    w.i16(name.len() as i16);
    w.i16(0);
    w.0.extend_from_slice(name);
    w.0.extend_from_slice(&packed);
    let cd = w.0.len();
    w.0.extend_from_slice(b"PK\x01\x02");
    w.i16(20);
    w.i16(20);
    w.i16(0);
    w.i16(8);
    w.i16(0);
    w.i16(0);
    w.i32(0);
    w.i32(packed.len() as i32);
    w.i32(10);
    w.i16(name.len() as i16);
    w.zeros(2 + 2 + 2 + 2 + 4);
    w.i32(0);
    w.0.extend_from_slice(name);
    let cd_size = w.0.len() - cd;
    w.0.extend_from_slice(b"PK\x05\x06");
    w.zeros(4);
    w.i16(1);
    w.i16(1);
    w.i32(cd_size as i32);
    w.i32(cd as i32);
    w.i16(0);
    assert!(matches!(read_gp(&w.0), Err(GpError::TooBig(_))));
}

#[test]
fn zip_directories_that_lie_are_errors() {
    // A directory entry pointing past the end, and one claiming thousands of entries.
    let mut z = zip(&[("Content/score.gpif", b"<GPIF/>")], false);
    let at = z.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
    z[at + 42..at + 46].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(read_gp(&z).is_err());
    let mut z = zip(&[("Content/score.gpif", b"<GPIF/>")], false);
    let at = z.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    z[at + 10..at + 12].copy_from_slice(&60_000u16.to_le_bytes());
    assert!(read_gp(&z).is_err());
}

#[test]
fn xml_bombs_are_refused() {
    // Nesting.
    let mut deep = String::from("<?xml version=\"1.0\"?><GPIF>");
    deep.push_str(&"<a>".repeat(20_000));
    assert!(matches!(read_gp(deep.as_bytes()), Err(GpError::Xml(_))));
    // Too many elements for the cap, in a file within the size cap.
    let mut wide = String::from("<?xml version=\"1.0\"?><GPIF>");
    wide.push_str(&"<a/>".repeat(limits::MAX_XML_NODES + 10));
    wide.push_str("</GPIF>");
    assert!(matches!(read_gp(wide.as_bytes()), Err(GpError::TooMany(_))));
    // A reference loop of ids is not followed: ids only name lists.
    let mut g = Gpif::new();
    g.track("G", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(0, 0, &[], "");
    let b = g.beat(q, &[n], &[], "");
    // One voice naming the same beat thousands of times is a bar that is too long, refused.
    let v = g.voice(&vec![b; 5000]);
    let bar = g.bar(&[v]);
    g.master("4/4", &[bar], "");
    assert!(read_gp(g.render().as_bytes()).is_err());
}

#[test]
fn many_bars_naming_one_busy_voice_hit_the_beat_cap() {
    let mut g = Gpif::new();
    g.track("G", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(0, 0, &[], "");
    let b = g.beat(q, &[n], &[], "");
    let v = g.voice(&vec![b; 4000]);
    let bar = g.bar(&[v]);
    for _ in 0..1000 {
        g.master("4/4", &[bar], "");
    }
    assert!(matches!(
        read_gp(g.render().as_bytes()),
        Err(GpError::TooMany(_))
    ));
}

#[test]
fn endless_repeats_stay_bounded() {
    let mut g = Gpif::new();
    g.track("G", &GUITAR, 0, "");
    let q = g.rhythm("Quarter", 0, None);
    let n = g.note(0, 0, &[], "");
    let b = g.beat(q, &[n], &[], "");
    let v = g.voice(&[b]);
    let bar = g.bar(&[v]);
    for i in 0..400 {
        g.master(
            "4/4",
            &[bar],
            if i % 2 == 0 {
                "<Repeat start=\"true\" end=\"false\" count=\"0\"/>"
            } else {
                "<Repeat start=\"false\" end=\"true\" count=\"255\"/>"
            },
        );
    }
    let (s, w) = read_gp(g.render().as_bytes()).unwrap();
    assert!(s.bars.len() == 400);
    assert!(s.tracks[0].notes.len() <= atomik_tab::score::limits::MAX_BARS);
    assert!(w.iter().any(|w| w.message.contains("repeats")), "{w:?}");
}

#[test]
fn binary_counts_that_lie_are_refused() {
    // Patch the bar count of a version 3 file (right after the 64 channels): billions of bars.
    for ver in [300, 400, 500, 510] {
        let mut s = Song::new(ver, 1);
        let bytes = s.encode();
        s.bars = vec![BarHdr::default(); 3];
        s.beats = vec![vec![vec![]]; 3];
        s.voice2 = vec![vec![vec![]]; 3];
        let three = s.encode();
        // The first place where the two files differ is the bar count.
        let at = bytes.iter().zip(&three).position(|(a, b)| a != b).unwrap();
        for bad in [i32::MAX, 20_001, -1, 0] {
            let mut b = three.clone();
            b[at..at + 4].copy_from_slice(&bad.to_le_bytes());
            assert!(read_gp(&b).is_err(), "{ver} {bad}");
        }
    }
}

#[test]
fn a_compressed_container_of_zeros_is_not_a_score() {
    let image = vec![0u8; 3 * 4096];
    assert!(read_gp(&bcfz(&image, true)).is_err());
}

/// Writes one fixture of every format to `$ATOMIK_FUZZ_SEED_DIR`, as seeds for the fuzz target
/// (`scripts/fuzz-seed.sh` runs this with `--ignored`; it does nothing without the variable).
#[test]
#[ignore = "writes the fuzz seed corpus"]
fn write_fuzz_seeds() {
    let Some(dir) = std::env::var_os("ATOMIK_FUZZ_SEED_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let xml = small_gpif();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for ver in [300u16, 400, 500, 510] {
        files.push((format!("gp-{ver}.bin"), small_song(ver).encode()));
    }
    files.push(("score.gpif".into(), xml.clone().into_bytes()));
    files.push(("score.gp".into(), gp(&xml, true)));
    files.push(("score-stored.gp".into(), gp(&xml, false)));
    files.push(("score.gpx".into(), gpx(&xml, true, true)));
    files.push(("score-plain.gpx".into(), gpx(&xml, false, false)));
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
}
