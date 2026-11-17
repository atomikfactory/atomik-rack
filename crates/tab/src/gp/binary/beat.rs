//! Beats, notes and their effects in the binary Guitar Pro formats (3, 4 and 5).

use super::{Reader, TrackState};
use crate::gp::GpError;
use crate::gp::dom::{Beat, Diagram, Note, WHOLE, bend_from_units};
use crate::gp::limits;
use crate::score::{
    Bend, Grace, Harmonic, HarmonicKind, PPQ, SlideKind, StrumDirection, TechFlags, Techniques,
    VibratoKind,
};

/// What a beat's effect block says; it applies to every note of the beat.
#[derive(Default)]
struct BeatFx {
    vibrato: Option<VibratoKind>,
    fade_in: bool,
    /// 1 tapping, 2 slapping, 3 popping.
    slap: u8,
    stroke: Option<StrumDirection>,
    whammy: Option<Bend>,
    harmonic: Option<Harmonic>,
}

/// The sound of a tuplet written as its number (3 = triplet): `(notes, in the time of)`.
fn tuplet_ratio(n: i32) -> Option<(i64, i64)> {
    match n {
        3 => Some((3, 2)),
        5 => Some((5, 4)),
        6 => Some((6, 4)),
        7 => Some((7, 4)),
        9 => Some((9, 8)),
        10 => Some((10, 8)),
        11 => Some((11, 8)),
        12 => Some((12, 8)),
        13 => Some((13, 8)),
        _ => None,
    }
}

/// The velocity of a dynamic mark 1 (ppp) to 8 (fff): 95 is forte, the default.
fn velocity_of(dynamic: i8) -> u8 {
    u8::try_from((15 + 16 * (i32::from(dynamic) - 1)).clamp(1, 127)).unwrap_or(95)
}

impl Reader<'_, '_> {
    /// One voice of one bar: a beat count, then the beats.
    pub(super) fn voice(&mut self, ts: &mut TrackState) -> Result<Vec<Beat>, GpError> {
        let n = self.c.count()?;
        if n > limits::MAX_BEATS_PER_BAR {
            return Err(GpError::Corrupt("a bar has too many beats".into()));
        }
        if n > self.c.remaining() {
            return Err(GpError::Truncated);
        }
        self.beats += n;
        if self.beats > limits::MAX_BEATS {
            return Err(GpError::TooMany("beats"));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.beat(ts)?);
        }
        crate::gp::dom::place(&mut out);
        Ok(out)
    }

    fn beat(&mut self, ts: &mut TrackState) -> Result<Beat, GpError> {
        let flags = self.c.u8()?;
        let mut rest = false;
        if flags & 0x40 != 0 {
            let status = self.c.u8()?;
            rest = status == 0 || status == 2;
        }
        let exp = self.c.i8()?;
        if !(-2..=4).contains(&exp) {
            return Err(GpError::Corrupt("a note has an impossible duration".into()));
        }
        let mut units = WHOLE / (1i64 << (exp + 2));
        if flags & 0x01 != 0 {
            units = units * 3 / 2;
        }
        if flags & 0x20 != 0 {
            let t = self.c.i32()?;
            match tuplet_ratio(t) {
                Some((enters, times)) => units = units * times / enters,
                None => self
                    .w
                    .warn("a tuplet of an unusual size was read as plain notes"),
            }
        }
        let mut beat = Beat {
            units,
            ..Beat::default()
        };
        if flags & 0x02 != 0 {
            beat.diagram = Some(self.chord(ts.strings)?);
        }
        if flags & 0x04 != 0 {
            self.c.int_byte_size_string()?;
            self.w.warn("beat text is not imported");
        }
        let mut fx = BeatFx::default();
        if flags & 0x08 != 0 {
            fx = self.beat_effects()?;
        }
        if flags & 0x10 != 0 {
            beat.tempo = self.mix_table()?;
        }
        let strings = self.c.u8()?;
        for i in 0..7usize {
            if strings & (0x40 >> i) == 0 {
                continue;
            }
            let note = self.note(ts)?;
            if i >= ts.strings {
                self.w
                    .warn("a note on a string the track does not have was dropped");
                continue;
            }
            self.notes += 1;
            if self.notes > limits::MAX_NOTES {
                return Err(GpError::TooMany("notes"));
            }
            let mut note = note;
            note.string = i as u8;
            apply_fx(&mut note, &fx);
            beat.notes.push(note);
        }
        if self.ver >= 500 {
            let flags2 = self.c.i16()?;
            if flags2 & 0x0800 != 0 {
                self.c.skip(1)?;
            }
        }
        if rest {
            beat.notes.clear();
        }
        Ok(beat)
    }

    /// A chord diagram (the old three-field layout, or the fixed 4.x/5.x one).
    fn chord(&mut self, strings: usize) -> Result<Diagram, GpError> {
        let new_format = self.c.u8()? != 0;
        let mut frets: Vec<Option<u8>> = vec![None; strings];
        let name;
        let mut put = |i: usize, f: i32| {
            if let Some(slot) = frets.get_mut(i) {
                *slot = u8::try_from(f).ok().map(|f| f.min(40));
            }
        };
        if new_format {
            self.c.skip(16)?;
            name = self.c.byte_size_string(21)?;
            self.c.skip(4)?;
            let _first_fret = self.c.i32()?;
            for i in 0..7 {
                let f = self.c.i32()?;
                put(i, f);
            }
            // Barres (count, frets, starts, ends), omissions, a spare byte, fingers, "shown".
            self.c.skip(1 + 15 + 7 + 1 + 7 + 1)?;
        } else {
            name = self.c.int_byte_size_string()?;
            let first = self.c.i32()?;
            if first != 0 {
                for i in 0..6 {
                    let f = self.c.i32()?;
                    put(i, f);
                }
            }
        }
        Ok(Diagram { name, frets })
    }

    fn beat_effects(&mut self) -> Result<BeatFx, GpError> {
        let mut fx = BeatFx::default();
        let flags1 = self.c.u8()?;
        if self.ver < 400 {
            if flags1 & 0x01 != 0 {
                fx.vibrato = Some(VibratoKind::Normal);
            }
            if flags1 & 0x02 != 0 {
                fx.vibrato = Some(VibratoKind::Wide);
            }
            fx.fade_in = flags1 & 0x10 != 0;
            if flags1 & 0x20 != 0 {
                let kind = self.c.i8()?;
                if kind == 0 {
                    // The tremolo bar of a Guitar Pro 3 file: one number, a dip held to the middle.
                    let v = i64::from(self.c.i32()?);
                    fx.whammy = Some(bend_from_units(&[(0.0, 0), (0.5, -v), (1.0, 0)]));
                } else {
                    fx.slap = u8::try_from(kind).unwrap_or(0).min(3);
                }
            }
            if flags1 & 0x40 != 0 {
                fx.stroke = self.stroke()?;
            }
            if flags1 & 0x04 != 0 {
                fx.harmonic = Some(Harmonic {
                    kind: HarmonicKind::Natural,
                    fret: None,
                });
            }
            if flags1 & 0x08 != 0 {
                fx.harmonic = Some(Harmonic {
                    kind: HarmonicKind::Artificial,
                    fret: None,
                });
            }
            return Ok(fx);
        }
        let flags2 = self.c.u8()?;
        if flags1 & 0x01 != 0 {
            fx.vibrato = Some(VibratoKind::Normal);
        }
        if flags1 & 0x02 != 0 {
            fx.vibrato = Some(VibratoKind::Wide);
        }
        fx.fade_in = flags1 & 0x10 != 0;
        if flags1 & 0x20 != 0 {
            fx.slap = u8::try_from(self.c.i8()?).unwrap_or(0).min(3);
        }
        if flags2 & 0x04 != 0 {
            fx.whammy = Some(self.bend()?);
        }
        if flags1 & 0x40 != 0 {
            fx.stroke = self.stroke()?;
        }
        if flags2 & 0x02 != 0 {
            let pick = self.c.i8()?;
            if fx.stroke.is_none() {
                fx.stroke = match pick {
                    1 => Some(StrumDirection::Up),
                    2 => Some(StrumDirection::Down),
                    _ => None,
                };
            }
        }
        Ok(fx)
    }

    /// A stroke: the down speed, then the up speed; the one that is not zero gives the direction.
    fn stroke(&mut self) -> Result<Option<StrumDirection>, GpError> {
        let down = self.c.i8()?;
        let up = self.c.i8()?;
        Ok(if up > 0 {
            Some(StrumDirection::Up)
        } else if down > 0 {
            Some(StrumDirection::Down)
        } else {
            None
        })
    }

    /// A bend: a type, a height, then points `(position 0..60, value, vibrato)`.
    fn bend(&mut self) -> Result<Bend, GpError> {
        let _kind = self.c.i8()?;
        let _height = self.c.i32()?;
        let n = self.c.count()?;
        if n > 1000 {
            return Err(GpError::Corrupt("a bend has too many points".into()));
        }
        if n.saturating_mul(9) > self.c.remaining() {
            return Err(GpError::Truncated);
        }
        let mut pts = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let pos = self.c.i32()?;
            let value = self.c.i32()?;
            let _vibrato = self.c.u8()?;
            if pts.len() < 64 {
                pts.push((pos as f32 / 60.0, i64::from(value)));
            }
        }
        Ok(bend_from_units(&pts))
    }

    /// A change of the mixing table: only a tempo change is kept.
    fn mix_table(&mut self) -> Result<Option<f64>, GpError> {
        let v500 = self.ver >= 500;
        let _instrument = self.c.i8()?;
        if v500 {
            // The sound bank of the instrument: three numbers and the effect number.
            self.c.skip(12)?;
            if self.ver == 500 {
                self.c.skip(3)?;
            } else {
                self.c.skip(4)?;
            }
        }
        let mut set = [false; 6];
        for s in &mut set {
            *s = self.c.i8()? >= 0;
        }
        if v500 {
            self.c.int_byte_size_string()?; // the tempo's name
        }
        let tempo = self.c.i32()?;
        for s in set {
            if s {
                self.c.skip(1)?;
            }
        }
        if tempo >= 0 {
            self.c.skip(1)?;
            if v500 && self.ver > 500 {
                self.c.skip(1)?; // hide the tempo
            }
        }
        if self.ver >= 400 {
            self.c.skip(1)?; // which tracks the change applies to
        }
        if v500 {
            self.c.skip(1)?; // the wah pedal
            if self.ver > 500 {
                self.c.int_byte_size_string()?;
                self.c.int_byte_size_string()?;
            }
        }
        Ok((tempo > 0).then_some(f64::from(tempo)))
    }

    fn note(&mut self, ts: &mut TrackState) -> Result<Note, GpError> {
        let flags = self.c.u8()?;
        let mut tech = Techniques::default();
        if flags & 0x02 != 0 {
            tech.flags.insert(TechFlags::HEAVY_ACCENT);
        }
        if flags & 0x04 != 0 {
            tech.flags.insert(TechFlags::GHOST);
        }
        if flags & 0x40 != 0 {
            tech.flags.insert(TechFlags::ACCENT);
        }
        let mut kind = 1u8;
        if flags & 0x20 != 0 {
            kind = self.c.u8()?;
        }
        if flags & 0x01 != 0 && self.ver < 500 {
            self.c.skip(2)?; // a duration and tuplet of its own: not used
        }
        if flags & 0x10 != 0 {
            ts.velocity = velocity_of(self.c.i8()?);
        }
        let mut fret = 0i32;
        if flags & 0x20 != 0 {
            fret = i32::from(self.c.i8()?);
        }
        if flags & 0x80 != 0 {
            self.c.skip(2)?; // left and right hand fingers
        }
        if self.ver >= 500 {
            if flags & 0x01 != 0 {
                self.c.skip(8)?; // the sounding length as a fraction of the note
            }
            self.c.skip(1)?; // flags: swap accidentals
        }
        let mut hopo_origin = false;
        if flags & 0x08 != 0 {
            hopo_origin = self.note_effects(&mut tech)?;
        }
        if kind == 3 {
            tech.flags.insert(TechFlags::DEAD);
        }
        let max = if ts.percussion { 127 } else { 99 };
        let fret = u8::try_from(fret).ok().filter(|f| *f <= max).unwrap_or(0);
        Ok(Note {
            string: 0,
            fret,
            velocity: ts.velocity,
            tie: kind == 2,
            hopo_origin,
            hopo_dest: false,
            tech,
        })
    }

    /// The effects of a note. Returns whether the note is the origin of a hammer-on or pull-off.
    fn note_effects(&mut self, tech: &mut Techniques) -> Result<bool, GpError> {
        let flags1 = self.c.u8()?;
        let flags2 = if self.ver >= 400 { self.c.u8()? } else { 0 };
        let hammer = flags1 & 0x02 != 0;
        if flags1 & 0x08 != 0 {
            tech.flags.insert(TechFlags::LET_RING);
        }
        if flags1 & 0x01 != 0 {
            tech.bend = Some(self.bend()?);
        }
        if flags1 & 0x10 != 0 {
            tech.grace = self.grace()?;
        }
        if self.ver < 400 {
            if flags1 & 0x04 != 0 {
                tech.slide = Some(SlideKind::Shift);
            }
            return Ok(hammer);
        }
        if flags2 & 0x01 != 0 {
            tech.flags.insert(TechFlags::STACCATO);
        }
        if flags2 & 0x02 != 0 {
            tech.flags.insert(TechFlags::PALM_MUTE);
        }
        if flags2 & 0x04 != 0 {
            tech.tremolo_division = match self.c.i8()? {
                1 => 8,
                2 => 16,
                3 => 32,
                _ => 0,
            };
        }
        if flags2 & 0x08 != 0 {
            tech.slide = self.slide()?;
        }
        if flags2 & 0x10 != 0 {
            tech.harmonic = self.harmonic()?;
        }
        if flags2 & 0x20 != 0 {
            self.c.skip(2)?;
            self.w.warn("trills are not imported");
        }
        if flags2 & 0x40 != 0 {
            tech.vibrato = Some(VibratoKind::Normal);
        }
        Ok(hammer)
    }

    fn grace(&mut self) -> Result<Option<Grace>, GpError> {
        let fret = self.c.u8()?;
        let _dynamic = self.c.u8()?;
        let _transition = self.c.u8()?;
        let duration = self.c.u8()?;
        let mut on_beat = false;
        let mut dead = fret == 255;
        if self.ver >= 500 {
            let f = self.c.u8()?;
            dead |= f & 0x01 != 0;
            on_beat = f & 0x02 != 0;
        }
        if dead {
            self.w.warn("dead grace notes are not imported");
            return Ok(None);
        }
        Ok(Some(Grace {
            fret: fret.min(60),
            ticks: (PPQ / 16 * i64::from(duration.max(1))).min(PPQ),
            on_beat,
        }))
    }

    fn slide(&mut self) -> Result<Option<SlideKind>, GpError> {
        let v = self.c.i8()?;
        if self.ver >= 500 {
            // A bit mask; the first kind in this order is kept.
            const ORDER: [(i8, SlideKind); 6] = [
                (0x01, SlideKind::Shift),
                (0x02, SlideKind::Legato),
                (0x04, SlideKind::OutDown),
                (0x08, SlideKind::OutUp),
                (0x10, SlideKind::InFromBelow),
                (0x20, SlideKind::InFromAbove),
            ];
            return Ok(ORDER.iter().find(|(m, _)| v & m != 0).map(|(_, k)| *k));
        }
        Ok(match v {
            1 => Some(SlideKind::Shift),
            2 => Some(SlideKind::Legato),
            3 => Some(SlideKind::OutDown),
            4 => Some(SlideKind::OutUp),
            5 | -1 => Some(SlideKind::InFromBelow),
            6 | -2 => Some(SlideKind::InFromAbove),
            _ => None,
        })
    }

    fn harmonic(&mut self) -> Result<Option<Harmonic>, GpError> {
        let kind = self.c.i8()?;
        let h = |kind, fret| Some(Harmonic { kind, fret });
        if self.ver >= 500 {
            return Ok(match kind {
                1 => h(HarmonicKind::Natural, None),
                2 => {
                    self.c.skip(3)?; // the note, its accidental and the octave
                    h(HarmonicKind::Artificial, None)
                }
                3 => {
                    let fret = self.c.u8()?;
                    h(HarmonicKind::Tapped, Some(f32::from(fret)))
                }
                4 => h(HarmonicKind::Pinch, None),
                5 => h(HarmonicKind::Semi, None),
                _ => {
                    self.w.warn("an unknown harmonic was read as none");
                    None
                }
            });
        }
        Ok(match kind {
            1 => h(HarmonicKind::Natural, None),
            3 => h(HarmonicKind::Tapped, None),
            4 => h(HarmonicKind::Pinch, None),
            5 => h(HarmonicKind::Semi, None),
            15 | 17 | 22 => h(HarmonicKind::Artificial, None),
            _ => {
                self.w.warn("an unknown harmonic was read as none");
                None
            }
        })
    }
}

/// Puts the beat's effects on one of its notes.
fn apply_fx(note: &mut Note, fx: &BeatFx) {
    let t = &mut note.tech;
    if t.vibrato.is_none() {
        t.vibrato = fx.vibrato;
    }
    if fx.fade_in {
        t.flags.insert(TechFlags::FADE_IN);
    }
    match fx.slap {
        1 => t.flags.insert(TechFlags::TAP),
        2 => t.flags.insert(TechFlags::SLAP),
        3 => t.flags.insert(TechFlags::POP),
        _ => {}
    }
    if t.strum.is_none() {
        t.strum = fx.stroke;
    }
    if t.whammy.is_none() {
        t.whammy = fx.whammy.clone();
    }
    if t.harmonic.is_none() {
        t.harmonic = fx.harmonic;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamics_map_to_velocities() {
        assert_eq!(velocity_of(1), 15);
        assert_eq!(velocity_of(6), 95);
        assert_eq!(velocity_of(8), 127);
        assert_eq!(velocity_of(100), 127);
        assert_eq!(velocity_of(-5), 1);
    }

    #[test]
    fn tuplets_are_the_usual_ratios() {
        assert_eq!(tuplet_ratio(3), Some((3, 2)));
        assert_eq!(tuplet_ratio(5), Some((5, 4)));
        assert_eq!(tuplet_ratio(2), None);
    }
}
