//! Standard MIDI Files (SMF format 0 and 1): a small reader and writer, pure bytes in and out.
//!
//! [`read`] turns a file into an [`SmfFile`]: the header (format and pulses per quarter note)
//! and each `MTrk` chunk as a list of [`SmfEvent`]s at absolute ticks. Only what a sequencer
//! uses is kept: channel messages (notes, polyphonic and channel pressure, controllers, program
//! change, pitch bend) and the meta events for tempo, time signature, markers and track names.
//! Everything else (SysEx, sequencer-specific and unknown meta events, unknown chunks) is
//! skipped, and a type 2 file or an SMPTE time division is refused with a message.
//!
//! The reader is lenient where real files are sloppy and strict where a guess would be wrong:
//! running status carries across meta events and SysEx, a note-on with velocity 0 stays a
//! [`SmfMessage::NoteOn`] (the caller decides it is an off), a track chunk that claims more bytes
//! than the file has is read up to the end of the file, and an event cut short by the end of its
//! chunk ends the track. A data byte with no running status, a data byte above 127, a delta
//! time of more than four bytes, a bad header or a file with no track at all are errors. It never
//! panics, for any input.
//!
//! [`SmfFile::to_bytes`] writes the file back with running status and an end-of-track event per
//! track. Events must be in tick order; a delta above 28 bits is clamped (about 4.5 hours at
//! 120 BPM and 15 360 ticks per quarter note).

/// The most `MTrk` chunks [`read`] takes (a larger count is more likely garbage than music).
pub const MAX_TRACKS: usize = 4096;

/// Why a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmfError {
    /// The bytes are not a Standard MIDI File.
    #[error("not a MIDI file: {0}")]
    NotMidi(String),
    /// A valid file this reader does not handle (type 2, SMPTE time division, too many tracks).
    #[error("unsupported MIDI file: {0}")]
    Unsupported(String),
    /// An event inside a track is malformed.
    #[error("bad MIDI file: track {track}, byte {offset}: {reason}")]
    BadEvent {
        /// Zero-based index of the `MTrk` chunk.
        track: usize,
        /// Offset of the offending byte in the file.
        offset: usize,
        /// What is wrong.
        reason: &'static str,
    },
}

/// One message of a track, with the channel (0..=15) of channel messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmfMessage {
    /// Note off.
    NoteOff {
        /// Channel, 0..=15.
        channel: u8,
        /// Key, 0..=127.
        key: u8,
        /// Release velocity.
        velocity: u8,
    },
    /// Note on; velocity 0 means note off.
    NoteOn {
        /// Channel, 0..=15.
        channel: u8,
        /// Key, 0..=127.
        key: u8,
        /// Velocity, 0..=127.
        velocity: u8,
    },
    /// Polyphonic key pressure.
    PolyPressure {
        /// Channel, 0..=15.
        channel: u8,
        /// Key, 0..=127.
        key: u8,
        /// Pressure, 0..=127.
        pressure: u8,
    },
    /// Control change.
    Controller {
        /// Channel, 0..=15.
        channel: u8,
        /// Controller number, 0..=127.
        controller: u8,
        /// Value, 0..=127.
        value: u8,
    },
    /// Program change.
    Program {
        /// Channel, 0..=15.
        channel: u8,
        /// Program, 0..=127.
        program: u8,
    },
    /// Channel pressure.
    ChannelPressure {
        /// Channel, 0..=15.
        channel: u8,
        /// Pressure, 0..=127.
        pressure: u8,
    },
    /// Pitch bend, 14 bit: 0..=16383 with 8192 at the centre.
    PitchBend {
        /// Channel, 0..=15.
        channel: u8,
        /// Raw value.
        value: u16,
    },
    /// Tempo in microseconds per quarter note (1..=16 777 215).
    Tempo(u32),
    /// Time signature: `numerator` over `2^denominator_power`.
    TimeSignature {
        /// Beats per bar.
        numerator: u8,
        /// The beat unit as a power of two (2 is a quarter note).
        denominator_power: u8,
    },
    /// A marker (meta event 6).
    Marker(String),
    /// A track or sequence name (meta event 3).
    TrackName(String),
}

/// A message at an absolute tick of its track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmfEvent {
    /// Ticks from the start of the track.
    pub tick: u64,
    /// The message.
    pub message: SmfMessage,
}

/// One `MTrk` chunk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmfTrack {
    /// The kept events, in tick order.
    pub events: Vec<SmfEvent>,
    /// The tick of the end-of-track event (at least the last event's tick). The reader sets it
    /// to the tick where the track data ended; the writer never writes it earlier than the last
    /// event.
    pub end_tick: u64,
}

/// A Standard MIDI File.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmfFile {
    /// 0 (one track) or 1 (simultaneous tracks; the first holds the tempo map by convention).
    pub format: u16,
    /// Ticks per quarter note, 1..=32767.
    pub ppq: u16,
    /// The tracks.
    pub tracks: Vec<SmfTrack>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn left(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn u8(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.bytes.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
}

fn be_u16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Where the `MThd` chunk starts: at 0, or inside a RIFF `RMID` wrapper.
fn header_start(bytes: &[u8]) -> Result<usize, SmfError> {
    if bytes.starts_with(b"MThd") {
        return Ok(0);
    }
    if bytes.len() >= 12
        && &bytes[..4] == b"RIFF"
        && &bytes[8..12] == b"RMID"
        && let Some(i) = bytes[12..].windows(4).position(|w| w == b"MThd")
    {
        return Ok(12 + i);
    }
    Err(SmfError::NotMidi("no MThd header".to_owned()))
}

/// Reads a Standard MIDI File (see the module docs for what is kept and what is refused).
pub fn read(bytes: &[u8]) -> Result<SmfFile, SmfError> {
    let start = header_start(bytes)?;
    let mut c = Cursor { bytes, pos: start };
    c.take(4);
    let header_len = c
        .take(4)
        .map(be_u32)
        .ok_or_else(|| SmfError::NotMidi("the header is cut short".to_owned()))?;
    if header_len < 6 {
        return Err(SmfError::NotMidi(
            "the header is shorter than 6 bytes".to_owned(),
        ));
    }
    let body = c
        .take(6)
        .ok_or_else(|| SmfError::NotMidi("the header is cut short".to_owned()))?;
    // The track count at body[2..4] is only a hint: files in the wild get it wrong.
    let (format, division) = (be_u16(body), be_u16(&body[4..]));
    // A header longer than 6 bytes carries extensions: skip them.
    let extra = usize::try_from(header_len - 6).unwrap_or(usize::MAX);
    c.take(extra.min(c.left()));
    match format {
        0 | 1 => {}
        2 => {
            return Err(SmfError::Unsupported(
                "type 2 files (independent sequences) are not supported".to_owned(),
            ));
        }
        n => return Err(SmfError::NotMidi(format!("unknown format {n}"))),
    }
    if division & 0x8000 != 0 {
        return Err(SmfError::Unsupported(
            "SMPTE time division is not supported (only ticks per quarter note)".to_owned(),
        ));
    }
    if division == 0 {
        return Err(SmfError::NotMidi("zero ticks per quarter note".to_owned()));
    }
    let mut tracks = Vec::new();
    while c.left() >= 8 {
        let id = c.take(4).map(<[u8]>::to_vec).unwrap_or_default();
        let len = c.take(4).map(be_u32).unwrap_or(0);
        // A chunk that claims more than is left (a truncated file) reads to the end.
        let len = usize::try_from(len).unwrap_or(usize::MAX).min(c.left());
        let data_start = c.pos;
        let Some(data) = c.take(len) else { break };
        if id != b"MTrk" {
            continue;
        }
        if tracks.len() >= MAX_TRACKS {
            return Err(SmfError::Unsupported(format!(
                "more than {MAX_TRACKS} tracks"
            )));
        }
        tracks.push(read_track(data, tracks.len(), data_start)?);
    }
    if tracks.is_empty() {
        return Err(SmfError::NotMidi("the file has no track".to_owned()));
    }
    Ok(SmfFile {
        format,
        ppq: division,
        tracks,
    })
}

fn read_vlq(c: &mut Cursor<'_>) -> Result<Option<u32>, &'static str> {
    let mut value = 0u32;
    for _ in 0..4 {
        let Some(b) = c.u8() else { return Ok(None) };
        value = (value << 7) | u32::from(b & 0x7F);
        if b & 0x80 == 0 {
            return Ok(Some(value));
        }
    }
    Err("a variable-length number is longer than 4 bytes")
}

fn read_track(data: &[u8], index: usize, base: usize) -> Result<SmfTrack, SmfError> {
    let mut c = Cursor {
        bytes: data,
        pos: 0,
    };
    let bad = |c: &Cursor<'_>, reason| SmfError::BadEvent {
        track: index,
        offset: base + c.pos.saturating_sub(1),
        reason,
    };
    let mut track = SmfTrack::default();
    let mut tick = 0u64;
    let mut running = 0u8;
    loop {
        let delta = match read_vlq(&mut c) {
            Ok(Some(d)) => d,
            Ok(None) => break,
            Err(reason) => return Err(bad(&c, reason)),
        };
        tick = tick.saturating_add(u64::from(delta));
        let Some(first) = c.u8() else { break };
        let status = if first >= 0x80 {
            first
        } else if running != 0 {
            c.pos -= 1;
            running
        } else {
            return Err(bad(&c, "a data byte with no running status"));
        };
        match status {
            0xFF => {
                let Some(kind) = c.u8() else { break };
                let len = match read_vlq(&mut c) {
                    Ok(Some(l)) => l as usize,
                    Ok(None) => break,
                    Err(reason) => return Err(bad(&c, reason)),
                };
                let Some(payload) = c.take(len) else { break };
                match (kind, payload) {
                    (0x2F, _) => break,
                    (0x03, p) => track.events.push(SmfEvent {
                        tick,
                        message: SmfMessage::TrackName(String::from_utf8_lossy(p).into_owned()),
                    }),
                    (0x06, p) => track.events.push(SmfEvent {
                        tick,
                        message: SmfMessage::Marker(String::from_utf8_lossy(p).into_owned()),
                    }),
                    (0x51, [a, b, d]) => {
                        let micros = u32::from_be_bytes([0, *a, *b, *d]);
                        if micros > 0 {
                            track.events.push(SmfEvent {
                                tick,
                                message: SmfMessage::Tempo(micros),
                            });
                        }
                    }
                    (0x58, [num, den, ..]) => track.events.push(SmfEvent {
                        tick,
                        message: SmfMessage::TimeSignature {
                            numerator: *num,
                            denominator_power: *den,
                        },
                    }),
                    _ => {}
                }
            }
            0xF0 | 0xF7 => {
                let len = match read_vlq(&mut c) {
                    Ok(Some(l)) => l as usize,
                    Ok(None) => break,
                    Err(reason) => return Err(bad(&c, reason)),
                };
                if c.take(len).is_none() {
                    break;
                }
            }
            0xF1..=0xF6 | 0xF8..=0xFE => {
                // System messages have no place in a file; skip their data bytes.
                let n = match status {
                    0xF2 => 2,
                    0xF1 | 0xF3 => 1,
                    _ => 0,
                };
                if c.take(n).is_none() {
                    break;
                }
            }
            _ => {
                running = status;
                let channel = status & 0x0F;
                let wants = if matches!(status >> 4, 0xC | 0xD) {
                    1
                } else {
                    2
                };
                let mut d = [0u8; 2];
                let mut cut = false;
                for slot in d.iter_mut().take(wants) {
                    let Some(b) = c.u8() else {
                        cut = true;
                        break;
                    };
                    if b >= 0x80 {
                        return Err(bad(&c, "a data byte above 127"));
                    }
                    *slot = b;
                }
                if cut {
                    break;
                }
                let message = match status >> 4 {
                    0x8 => SmfMessage::NoteOff {
                        channel,
                        key: d[0],
                        velocity: d[1],
                    },
                    0x9 => SmfMessage::NoteOn {
                        channel,
                        key: d[0],
                        velocity: d[1],
                    },
                    0xA => SmfMessage::PolyPressure {
                        channel,
                        key: d[0],
                        pressure: d[1],
                    },
                    0xB => SmfMessage::Controller {
                        channel,
                        controller: d[0],
                        value: d[1],
                    },
                    0xC => SmfMessage::Program {
                        channel,
                        program: d[0],
                    },
                    0xD => SmfMessage::ChannelPressure {
                        channel,
                        pressure: d[0],
                    },
                    _ => SmfMessage::PitchBend {
                        channel,
                        value: u16::from(d[0]) | (u16::from(d[1]) << 7),
                    },
                };
                track.events.push(SmfEvent { tick, message });
            }
        }
    }
    track.end_tick = tick;
    Ok(track)
}

fn write_vlq(out: &mut Vec<u8>, value: u32) {
    let value = value.min(0x0FFF_FFFF);
    let mut groups = [0u8; 4];
    let mut n = 0;
    let mut v = value;
    loop {
        groups[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push(groups[i] | if i > 0 { 0x80 } else { 0 });
    }
}

fn write_meta(out: &mut Vec<u8>, kind: u8, payload: &[u8]) {
    out.extend([0xFF, kind]);
    write_vlq(out, u32::try_from(payload.len()).unwrap_or(u32::MAX));
    out.extend(payload);
}

impl SmfFile {
    /// The file's bytes: `MThd`, then one `MTrk` per track with running status and an
    /// end-of-track event. A `format` of 0 with several tracks is written as 1.
    pub fn to_bytes(&self) -> Vec<u8> {
        let format = if self.tracks.len() > 1 {
            1
        } else {
            self.format.min(1)
        };
        let mut out = Vec::new();
        out.extend(b"MThd");
        out.extend(6u32.to_be_bytes());
        out.extend(format.to_be_bytes());
        out.extend(
            u16::try_from(self.tracks.len())
                .unwrap_or(u16::MAX)
                .to_be_bytes(),
        );
        out.extend(self.ppq.clamp(1, 0x7FFF).to_be_bytes());
        for track in &self.tracks {
            let body = track_bytes(track);
            out.extend(b"MTrk");
            out.extend(u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
            out.extend(body);
        }
        out
    }
}

fn track_bytes(track: &SmfTrack) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0u64;
    let mut running = 0u8;
    let delta = |at: &mut u64, tick: u64| {
        let d = tick.saturating_sub(*at);
        *at = (*at).max(tick);
        u32::try_from(d).unwrap_or(u32::MAX)
    };
    for event in &track.events {
        write_vlq(&mut out, delta(&mut at, event.tick));
        let channel_message = |m: &SmfMessage| -> Option<(u8, [u8; 2], usize)> {
            let ch = |status: u8, channel: u8| status | (channel & 0x0F);
            let seven = |v: u8| v & 0x7F;
            Some(match *m {
                SmfMessage::NoteOff {
                    channel,
                    key,
                    velocity,
                } => (ch(0x80, channel), [seven(key), seven(velocity)], 2),
                SmfMessage::NoteOn {
                    channel,
                    key,
                    velocity,
                } => (ch(0x90, channel), [seven(key), seven(velocity)], 2),
                SmfMessage::PolyPressure {
                    channel,
                    key,
                    pressure,
                } => (ch(0xA0, channel), [seven(key), seven(pressure)], 2),
                SmfMessage::Controller {
                    channel,
                    controller,
                    value,
                } => (ch(0xB0, channel), [seven(controller), seven(value)], 2),
                SmfMessage::Program { channel, program } => {
                    (ch(0xC0, channel), [seven(program), 0], 1)
                }
                SmfMessage::ChannelPressure { channel, pressure } => {
                    (ch(0xD0, channel), [seven(pressure), 0], 1)
                }
                SmfMessage::PitchBend { channel, value } => {
                    let v = value.min(0x3FFF);
                    (ch(0xE0, channel), [(v & 0x7F) as u8, (v >> 7) as u8], 2)
                }
                _ => return None,
            })
        };
        if let Some((status, data, n)) = channel_message(&event.message) {
            if status != running {
                out.push(status);
                running = status;
            }
            out.extend(&data[..n]);
            continue;
        }
        // A meta event cancels running status for a strict reader.
        running = 0;
        match &event.message {
            SmfMessage::Tempo(micros) => {
                let b = (*micros).clamp(1, 0x00FF_FFFF).to_be_bytes();
                write_meta(&mut out, 0x51, &b[1..]);
            }
            SmfMessage::TimeSignature {
                numerator,
                denominator_power,
            } => write_meta(&mut out, 0x58, &[*numerator, *denominator_power, 24, 8]),
            SmfMessage::Marker(text) => write_meta(&mut out, 0x06, text.as_bytes()),
            SmfMessage::TrackName(text) => write_meta(&mut out, 0x03, text.as_bytes()),
            _ => {}
        }
    }
    write_vlq(&mut out, delta(&mut at, track.end_tick));
    write_meta(&mut out, 0x2F, &[]);
    out
}

#[cfg(test)]
#[path = "smf_tests.rs"]
mod tests;
