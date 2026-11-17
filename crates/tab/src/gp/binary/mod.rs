//! The binary Guitar Pro formats: `.gp3` (versions 3.xx), `.gp4` (4.xx) and `.gp5` (5.00 and 5.10).
//!
//! A file is a version string, the song information, (4.x and 5.x) lyrics, the tempo and key, a
//! table of 64 MIDI channels, the number of bars and tracks, one header per bar (meter, repeats,
//! alternative endings, marker, key), one header per track (tuning highest string first, capo,
//! channel) and then, bar by bar and track by track, the beats with their notes. Version 5 adds a
//! page layout, the repeat directions, a second voice per bar and some mixing data the reader skips.
//!
//! Numbers are little endian. This reader was written from the layout of the format as understood
//! by its author; the byte layout of each version is checked by the test writer in
//! `tests/gp_fixtures`, which encodes the same layout, and not against files made by the original
//! program (the decisions are in docs/ARCHITECTURE.md).

mod beat;
mod cursor;

use cursor::Cursor;

use super::dom::{Bar, Doc, Info, Lyrics, MasterBar, Track};
use super::{GpError, Warnings, limits};
use crate::score::KeySignature;

const MAGIC: &[u8] = b"FICHIER GUITAR PRO";

/// Whether the bytes start with the version string of a binary Guitar Pro file.
pub(super) fn has_header(bytes: &[u8]) -> bool {
    bytes.len() > MAGIC.len() + 1 && bytes[0] <= 30 && bytes[1..].starts_with(MAGIC)
}

/// The state of a track while its beats are read.
struct TrackState {
    pub strings: usize,
    pub percussion: bool,
    pub velocity: u8,
}

/// The reading state shared by the sections.
struct Reader<'a, 'w> {
    pub c: Cursor<'a>,
    /// The version as `major * 100 + minor`: 300, 400, 406, 500, 510.
    pub ver: u16,
    pub w: &'w mut Warnings,
    pub beats: usize,
    pub notes: usize,
}

/// One channel of the table: the General MIDI program.
#[derive(Clone, Copy, Default)]
struct Channel {
    program: i32,
}

/// Reads a `.gp3`, `.gp4` or `.gp5` file.
pub(super) fn read(bytes: &[u8], w: &mut Warnings) -> Result<Doc, GpError> {
    let mut r = Reader {
        c: Cursor::new(bytes),
        ver: 0,
        w,
        beats: 0,
        notes: 0,
    };
    r.song()
}

impl Reader<'_, '_> {
    fn version(&mut self) -> Result<(), GpError> {
        let text = self.c.byte_size_string(30)?;
        let v = text
            .rsplit_once(" v")
            .map(|(_, v)| v.trim().to_owned())
            .ok_or(GpError::UnknownFormat)?;
        let mut parts = v.split('.');
        let major: u16 = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or(GpError::UnknownFormat)?;
        let minor: u16 = parts
            .next()
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        if !(3..=5).contains(&major) {
            return Err(GpError::UnsupportedVersion(v));
        }
        self.ver = major * 100 + minor.min(99);
        Ok(())
    }

    fn song(&mut self) -> Result<Doc, GpError> {
        self.version()?;
        let mut doc = Doc {
            info: self.info()?,
            ..Doc::default()
        };
        if self.ver < 500 {
            self.c.skip(1)?; // triplet feel
        }
        let mut lyrics = None;
        if self.ver >= 400 {
            lyrics = Some(self.lyrics()?);
        }
        if self.ver > 500 {
            self.c.skip(4 + 4 + 11)?; // the master volume, reverb and equaliser
        }
        if self.ver >= 500 {
            self.c.skip(7 * 4 + 2)?; // page size, margins, score size and flags
            for _ in 0..10 {
                self.c.int_byte_size_string()?; // the header and footer texts
            }
            self.c.int_byte_size_string()?; // the tempo's name
        }
        doc.tempo = f64::from(self.c.i32()?);
        let key = if self.ver >= 500 {
            if self.ver > 500 {
                self.c.skip(1)?; // hide the tempo
            }
            let k = self.c.i8()?;
            self.c.skip(4)?; // octave
            i32::from(k)
        } else {
            let k = self.c.i32()?;
            if self.ver >= 400 {
                self.c.skip(1)?; // octave
            }
            k
        };
        let mut channels = [Channel::default(); 64];
        for ch in &mut channels {
            ch.program = self.c.i32()?;
            self.c.skip(6 + 2)?; // volume, balance, chorus, reverb, phaser, tremolo, two spare
        }
        if self.ver >= 500 {
            self.c.skip(19 * 2 + 4)?; // the jump marks (coda, segno, D.C.) and the master reverb
        }
        let bar_count = self.c.count()?;
        let track_count = self.c.count()?;
        if bar_count == 0 || track_count == 0 {
            return Err(GpError::Corrupt(
                "the score has no bars or no tracks".into(),
            ));
        }
        if bar_count > limits::MAX_BARS {
            return Err(GpError::TooMany("bars"));
        }
        if track_count > limits::MAX_TRACKS {
            return Err(GpError::TooMany("tracks"));
        }
        let mut running_key = KeySignature {
            fifths: i8::try_from(key.clamp(-7, 7)).unwrap_or(0),
            minor: false,
        };
        doc.master_bars = self.bar_headers(bar_count, &mut running_key)?;
        let mut heads = Vec::with_capacity(track_count);
        for n in 0..track_count {
            heads.push(self.track_header(n, &channels)?);
        }
        if self.ver >= 500 {
            self.c.skip(if self.ver == 500 { 2 } else { 1 })?;
        }
        let mut states: Vec<TrackState> = heads
            .iter()
            .map(|h| TrackState {
                strings: h.opens.len(),
                percussion: h.percussion,
                velocity: 95,
            })
            .collect();
        for _ in 0..bar_count {
            for (head, st) in heads.iter_mut().zip(&mut states) {
                let voices = if self.ver >= 500 { 2 } else { 1 };
                let mut bar = Bar::default();
                for _ in 0..voices {
                    bar.voices.push(self.voice(st)?);
                }
                if self.ver >= 500 {
                    self.c.skip(1)?; // the line break
                }
                bar.voices.retain(|v| !v.is_empty());
                head.bars.push(bar);
            }
        }
        doc.tracks = heads;
        doc.lyrics = lyrics;
        Ok(doc)
    }

    fn info(&mut self) -> Result<Info, GpError> {
        let mut i = Info::default();
        i.title = self.c.int_byte_size_string()?;
        i.subtitle = self.c.int_byte_size_string()?;
        i.artist = self.c.int_byte_size_string()?;
        i.album = self.c.int_byte_size_string()?;
        i.words = self.c.int_byte_size_string()?;
        i.music = if self.ver >= 500 {
            self.c.int_byte_size_string()?
        } else {
            i.words.clone()
        };
        i.copyright = self.c.int_byte_size_string()?;
        i.tab_by = self.c.int_byte_size_string()?;
        let _instructions = self.c.int_byte_size_string()?;
        let notices = self.c.count()?;
        if notices > self.c.remaining() {
            return Err(GpError::Truncated);
        }
        let mut lines = Vec::new();
        for _ in 0..notices {
            let line = self.c.int_byte_size_string()?;
            if lines.len() < 64 {
                lines.push(line);
            }
        }
        i.notices = lines.join("\n");
        Ok(i)
    }

    fn lyrics(&mut self) -> Result<Lyrics, GpError> {
        let track = self.c.i32()?;
        let mut out = Lyrics {
            track: usize::try_from(track - 1).unwrap_or(0),
            lines: Vec::new(),
        };
        for _ in 0..5 {
            let start = self.c.i32()?;
            let text = self.c.int_size_string()?;
            if !text.trim().is_empty() {
                out.lines
                    .push((usize::try_from(start - 1).unwrap_or(0), text));
            }
        }
        Ok(out)
    }

    fn bar_headers(
        &mut self,
        count: usize,
        key: &mut KeySignature,
    ) -> Result<Vec<MasterBar>, GpError> {
        let mut out: Vec<MasterBar> = Vec::with_capacity(count);
        let (mut num, mut den) = (4u8, 4u8);
        for n in 0..count {
            if self.ver >= 500 && n > 0 {
                self.c.skip(1)?; // a spare byte between the headers
            }
            let flags = self.c.u8()?;
            if flags & 0x01 != 0 {
                num = u8::try_from(self.c.i8()?).unwrap_or(4).max(1);
            }
            if flags & 0x02 != 0 {
                let d = u8::try_from(self.c.i8()?).unwrap_or(4);
                den = if d.is_power_of_two() && d <= 64 { d } else { 4 };
            }
            let mut mb = MasterBar::new(num, den);
            mb.repeat_open = flags & 0x04 != 0;
            let mut close = None;
            if flags & 0x08 != 0 {
                let c = self.c.i8()?;
                // Before version 5 the byte counts the repeats, from 5 on all the plays.
                let plays = if self.ver >= 500 {
                    i32::from(c).max(2)
                } else {
                    i32::from(c) + 1
                };
                close = Some(u8::try_from(plays.clamp(2, 255)).unwrap_or(2));
            }
            mb.repeat_close = close;
            let mut alt = None;
            if self.ver < 500 && flags & 0x10 != 0 {
                alt = Some(self.c.u8()?);
            }
            if flags & 0x20 != 0 {
                let name = self.c.int_byte_size_string()?;
                self.c.skip(4)?; // colour
                mb.marker = Some(name);
            }
            if flags & 0x40 != 0 {
                let fifths = self.c.i8()?;
                let minor = self.c.i8()? != 0;
                *key = KeySignature {
                    fifths: fifths.clamp(-7, 7),
                    minor,
                };
            }
            if self.ver >= 500 {
                if flags & 0x10 != 0 {
                    let mask = self.c.u8()?;
                    mb.alt = mask;
                }
                if flags & 0x03 != 0 {
                    self.c.skip(4)?; // how the beats are beamed
                }
                if flags & 0x10 == 0 {
                    self.c.skip(1)?;
                }
                self.c.skip(1)?; // triplet feel
            } else if let Some(value) = alt {
                // The byte says "the nth ending": earlier endings of this repeat are left out.
                let mut existing = 0u16;
                for h in out.iter().rev() {
                    if h.repeat_open {
                        break;
                    }
                    existing |= u16::from(h.alt);
                }
                let v = u32::from(value.min(8));
                mb.alt = u8::try_from(((1u16 << v) - 1) & !existing & 0xFF).unwrap_or(0);
            }
            mb.key = Some(*key);
            out.push(mb);
        }
        Ok(out)
    }

    fn track_header(&mut self, number: usize, channels: &[Channel; 64]) -> Result<Track, GpError> {
        if self.ver >= 500 && (number == 0 || self.ver == 500) {
            self.c.skip(1)?;
        }
        let flags = self.c.u8()?;
        let name = self.c.byte_size_string(40)?;
        let strings = self.c.count()?;
        if strings == 0 || strings > 7 {
            return Err(GpError::Corrupt(
                "a track has no strings or more than seven".into(),
            ));
        }
        let mut opens = Vec::with_capacity(strings);
        for i in 0..7 {
            let p = self.c.i32()?;
            if i < strings {
                opens.push(u8::try_from(p.clamp(0, 127)).unwrap_or(0));
            }
        }
        let port = self.c.i32()?;
        let channel = self.c.i32()?;
        let _effect_channel = self.c.i32()?;
        let frets = self.c.i32()?;
        let capo = self.c.i32()?;
        self.c.skip(4)?; // colour
        if self.ver >= 500 {
            self.c.skip(2 + 1 + 1)?; // display flags, automatic accentuation, sound bank
            // The track's sound settings: a humanising byte, spare numbers, the instrument and, from
            // 5.10, an equaliser and two texts.
            self.c.skip(1 + 12 + 12 + 12)?;
            if self.ver == 500 {
                self.c.skip(3)?;
            } else {
                self.c.skip(4)?;
                self.c.skip(4)?;
                self.c.int_byte_size_string()?;
                self.c.int_byte_size_string()?;
            }
        }
        let idx = (i64::from(port.max(1)) - 1) * 16 + i64::from(channel.max(1)) - 1;
        let program = usize::try_from(idx)
            .ok()
            .and_then(|i| channels.get(i))
            .map_or(25, |c| c.program.clamp(0, 127));
        let track = Track {
            name,
            opens,
            frets: u8::try_from(frets.clamp(0, 99)).unwrap_or(24),
            capo: u8::try_from(capo.clamp(0, 99)).unwrap_or(0),
            program: u8::try_from(program).unwrap_or(25),
            percussion: flags & 0x01 != 0 || (port <= 1 && channel == 10),
            banjo: flags & 0x04 != 0,
            twelve_string: flags & 0x02 != 0,
            bars: Vec::new(),
        };
        Ok(track)
    }
}
