//! Program changes and SysEx messages carried by a note clip.
//!
//! A clip imported from a MIDI file keeps the messages the engine does not play but a MIDI file
//! export must write back: program changes (the instrument a channel switches to; the bank
//! select that goes with one is the `CC 0` and `CC 32` controller lane, which a clip already
//! holds) and system exclusive messages (device specific data). They are plain data in content
//! ticks, like the notes and controller lanes: they follow the clip's offset, loop and audible
//! span when a file is exported. Nothing in the model or the engine acts on them.
//!
//! A clip without any of it (the usual clip) stores nothing, so projects written before this
//! data existed load unchanged and an older reader may drop it (the saved fields are
//! optional).

/// The most program changes one clip keeps.
pub const MAX_PROGRAM_CHANGES: usize = 4096;
/// The most SysEx messages one clip keeps.
pub const MAX_SYSEX_EVENTS: usize = 1024;
/// The longest SysEx message kept, in data bytes (without the framing `F0` and `F7`).
pub const MAX_SYSEX_BYTES: usize = 4096;

/// A program change at a content tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramChange {
    /// Content tick, `0..=MAX_TICK`.
    pub tick: i64,
    /// The program, `0..=127`.
    pub program: u8,
}

/// A SysEx message at a content tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysexEvent {
    /// Content tick, `0..=MAX_TICK`.
    pub tick: i64,
    /// The data bytes between `F0` and `F7`, each `0..=127`, at most [`MAX_SYSEX_BYTES`].
    pub data: Vec<u8>,
}

/// The program changes and SysEx messages of a note clip (module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiData {
    /// Sorted by tick, at most one per tick.
    pub programs: Vec<ProgramChange>,
    /// Sorted by tick (messages at one tick keep their order).
    pub sysex: Vec<SysexEvent>,
}

impl MidiData {
    /// True when the clip carries no program change and no SysEx message.
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty() && self.sysex.is_empty()
    }

    /// `programs` and `sysex` in canonical form: sorted by tick, the last program change of a
    /// tick wins, empty SysEx messages dropped.
    pub fn canonical(mut self) -> Self {
        self.programs.sort_by_key(|p| p.tick);
        let mut programs: Vec<ProgramChange> = Vec::with_capacity(self.programs.len());
        for p in self.programs {
            match programs.last_mut() {
                Some(last) if last.tick == p.tick => *last = p,
                _ => programs.push(p),
            }
        }
        self.programs = programs;
        self.sysex.retain(|s| !s.data.is_empty());
        self.sysex.sort_by_key(|s| s.tick);
        self
    }

    /// Checks the data: counts and sizes within the limits, ticks in `0..=max_tick`, programs
    /// `0..=127` sorted without duplicate ticks, SysEx bytes `0..=127` and sorted by tick. `Err`
    /// names the first fault.
    pub fn check(&self, max_tick: i64) -> Result<(), String> {
        if self.programs.len() > MAX_PROGRAM_CHANGES {
            return Err(format!(
                "a clip keeps at most {MAX_PROGRAM_CHANGES} program changes"
            ));
        }
        if self.sysex.len() > MAX_SYSEX_EVENTS {
            return Err(format!(
                "a clip keeps at most {MAX_SYSEX_EVENTS} SysEx messages"
            ));
        }
        for (i, p) in self.programs.iter().enumerate() {
            if !(0..=max_tick).contains(&p.tick) {
                return Err(format!(
                    "program change {i}: tick {} is outside 0..={max_tick}",
                    p.tick
                ));
            }
            if p.program > 127 {
                return Err(format!("program change {i}: program beyond 127"));
            }
            if i > 0 && self.programs[i - 1].tick >= p.tick {
                return Err(format!(
                    "program changes must be sorted by tick without duplicates (change {i})"
                ));
            }
        }
        for (i, s) in self.sysex.iter().enumerate() {
            if !(0..=max_tick).contains(&s.tick) {
                return Err(format!(
                    "SysEx message {i}: tick {} is outside 0..={max_tick}",
                    s.tick
                ));
            }
            if s.data.is_empty() || s.data.len() > MAX_SYSEX_BYTES {
                return Err(format!(
                    "SysEx message {i}: 1 to {MAX_SYSEX_BYTES} data bytes expected"
                ));
            }
            if s.data.iter().any(|b| *b > 127) {
                return Err(format!("SysEx message {i}: a data byte above 127"));
            }
            if i > 0 && self.sysex[i - 1].tick > s.tick {
                return Err(format!(
                    "SysEx messages must be sorted by tick (message {i})"
                ));
            }
        }
        Ok(())
    }

    /// The part inside the content window `[from, to)`, relative to `from` and moved by
    /// `shift`: the messages inside, and a program change at the window start carrying the
    /// program in force there when an earlier change gave the clip one (so a cut window plays
    /// like the window of the whole clip).
    pub fn window(&self, from: i64, to: i64, shift: i64) -> MidiData {
        let mut programs = Vec::new();
        let before = self.programs.partition_point(|p| p.tick <= from);
        if let Some(p) = self.programs[..before].last() {
            programs.push(ProgramChange {
                tick: from,
                program: p.program,
            });
        }
        programs.extend(
            self.programs[before..]
                .iter()
                .take_while(|p| p.tick < to)
                .copied(),
        );
        let sysex = self
            .sysex
            .iter()
            .filter(|s| (from..to).contains(&s.tick))
            .cloned()
            .collect();
        let mut out = MidiData { programs, sysex };
        out.shift_by(shift - from);
        out
    }

    /// Every tick mapped through `f` (the order stays, so a map that is not monotonic is the
    /// caller's problem; `canonical` repairs ties).
    pub fn map_ticks(&self, f: impl Fn(i64) -> i64) -> MidiData {
        MidiData {
            programs: self
                .programs
                .iter()
                .map(|p| ProgramChange {
                    tick: f(p.tick),
                    ..*p
                })
                .collect(),
            sysex: self
                .sysex
                .iter()
                .map(|s| SysexEvent {
                    tick: f(s.tick),
                    data: s.data.clone(),
                })
                .collect(),
        }
        .canonical()
    }

    /// Appends `other` (already in this clip's ticks) and restores the canonical form.
    pub fn append(&mut self, other: MidiData) {
        self.programs.extend(other.programs);
        self.sysex.extend(other.sysex);
        *self = std::mem::take(self).canonical();
    }

    fn shift_by(&mut self, delta: i64) {
        for p in &mut self.programs {
            p.tick += delta;
        }
        for s in &mut self.sysex {
            s.tick += delta;
        }
    }
}

/// The data bytes of a SysEx message as upper-case hex pairs separated by spaces, the form the
/// project file stores.
pub fn sysex_to_hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 3);
    for (i, b) in data.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// The bytes of a hex text written by [`sysex_to_hex`] (pairs separated by spaces; case and
/// extra white space do not matter), `None` when it is not hex.
pub fn sysex_from_hex(text: &str) -> Option<Vec<u8>> {
    text.split_whitespace()
        .map(|pair| {
            (pair.len() == 2)
                .then(|| u8::from_str_radix(pair, 16).ok())
                .flatten()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(tick: i64, program: u8) -> ProgramChange {
        ProgramChange { tick, program }
    }

    fn sx(tick: i64, data: &[u8]) -> SysexEvent {
        SysexEvent {
            tick,
            data: data.to_vec(),
        }
    }

    #[test]
    fn canonical_sorts_and_keeps_the_last_program_of_a_tick() {
        let d = MidiData {
            programs: vec![prog(50, 3), prog(10, 1), prog(50, 9)],
            sysex: vec![sx(20, &[1]), sx(5, &[]), sx(10, &[2])],
        }
        .canonical();
        assert_eq!(d.programs, [prog(10, 1), prog(50, 9)]);
        assert_eq!(d.sysex, [sx(10, &[2]), sx(20, &[1])]);
    }

    #[test]
    fn check_names_the_first_fault() {
        let ok = MidiData {
            programs: vec![prog(0, 1), prog(10, 2)],
            sysex: vec![sx(0, &[0x7E, 0x7F]), sx(0, &[1])],
        };
        assert!(ok.check(100).is_ok());
        let bad = |d: MidiData| d.check(100).unwrap_err();
        assert!(
            bad(MidiData {
                programs: vec![prog(101, 1)],
                ..Default::default()
            })
            .contains("outside")
        );
        assert!(
            bad(MidiData {
                programs: vec![prog(1, 128)],
                ..Default::default()
            })
            .contains("beyond 127")
        );
        assert!(
            bad(MidiData {
                programs: vec![prog(1, 1), prog(1, 2)],
                ..Default::default()
            })
            .contains("sorted")
        );
        assert!(
            bad(MidiData {
                sysex: vec![sx(1, &[0x80])],
                ..Default::default()
            })
            .contains("above 127")
        );
        assert!(
            bad(MidiData {
                sysex: vec![sx(1, &[])],
                ..Default::default()
            })
            .contains("data bytes")
        );
        assert!(
            bad(MidiData {
                sysex: vec![sx(5, &[1]), sx(2, &[1])],
                ..Default::default()
            })
            .contains("sorted")
        );
    }

    #[test]
    fn a_window_carries_the_program_in_force_and_shifts() {
        let d = MidiData {
            programs: vec![prog(0, 4), prog(100, 5), prog(300, 6)],
            sysex: vec![sx(50, &[1]), sx(120, &[2]), sx(400, &[3])],
        };
        let w = d.window(80, 200, 1000);
        assert_eq!(w.programs, [prog(1000, 4), prog(1020, 5)]);
        assert_eq!(w.sysex, [sx(1040, &[2])]);
        // No earlier program: nothing at the start.
        let w = d.window(0, 50, 0);
        assert_eq!(w.programs, [prog(0, 4)]);
        let empty = MidiData::default().window(0, 100, 7);
        assert!(empty.is_empty());
    }

    #[test]
    fn map_and_append_stay_canonical() {
        let d = MidiData {
            programs: vec![prog(10, 1), prog(11, 2)],
            sysex: vec![sx(10, &[1])],
        };
        let half = d.map_ticks(|t| t / 4);
        assert_eq!(half.programs, [prog(2, 2)], "ties keep the later change");
        let mut a = MidiData {
            programs: vec![prog(0, 9)],
            sysex: Vec::new(),
        };
        a.append(d);
        assert_eq!(a.programs, [prog(0, 9), prog(10, 1), prog(11, 2)]);
        assert_eq!(a.sysex.len(), 1);
    }

    #[test]
    fn hex_round_trips_and_refuses_junk() {
        let bytes = [0x7E, 0x00, 0x09, 0x01];
        let text = sysex_to_hex(&bytes);
        assert_eq!(text, "7E 00 09 01");
        assert_eq!(sysex_from_hex(&text), Some(bytes.to_vec()));
        assert_eq!(sysex_from_hex(" 7e   0A "), Some(vec![0x7E, 0x0A]));
        assert_eq!(sysex_from_hex("7E 0"), None);
        assert_eq!(sysex_from_hex("7E ZZ"), None);
        assert_eq!(sysex_from_hex(""), Some(Vec::new()));
    }
}
