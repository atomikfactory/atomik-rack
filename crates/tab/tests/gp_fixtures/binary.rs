//! An encoder for the binary Guitar Pro layout (3.00, 4.00, 5.00, 5.10) for a handful of bars.

use super::W;

/// How the stored repeat count of a bar header reads in the file (see the reader's notes).
#[derive(Clone, Default)]
pub struct BarHdr {
    pub num: Option<u8>,
    pub den: Option<u8>,
    pub repeat_open: bool,
    /// The total number of plays of the section that ends here.
    pub repeat_close: Option<u8>,
    /// The passes (bit 0 = the first) of an alternative ending.
    pub alt: u8,
    pub marker: Option<String>,
    pub key: Option<(i8, bool)>,
}

#[derive(Clone)]
pub struct TrackDef {
    pub name: String,
    /// Open pitches, highest string first (at most 7).
    pub strings: Vec<i32>,
    pub drums: bool,
    pub banjo: bool,
    pub twelve: bool,
    /// 1-based channel in port 1.
    pub channel: i32,
    pub frets: i32,
    pub capo: i32,
}

impl TrackDef {
    pub fn guitar(name: &str) -> Self {
        Self {
            name: name.into(),
            strings: vec![64, 59, 55, 50, 45, 40],
            drums: false,
            banjo: false,
            twelve: false,
            channel: 1,
            frets: 24,
            capo: 0,
        }
    }
    pub fn with_strings(mut self, s: &[i32]) -> Self {
        self.strings = s.to_vec();
        self
    }
}

#[derive(Clone, Default)]
pub struct ChordDef {
    pub name: String,
    /// Fret per string, -1 = not played.
    pub frets: Vec<i32>,
    pub new_format: bool,
}

#[derive(Clone, Default)]
pub struct BeatFx {
    pub vibrato: bool,
    pub wide_vibrato: bool,
    pub fade_in: bool,
    /// 1 tapping, 2 slapping, 3 popping.
    pub slap: i8,
    /// Version 3 only: a tremolo bar value.
    pub tremolo_v3: Option<i32>,
    /// Versions 4 and 5: a tremolo bar as points `(position 0..60, value)`.
    pub tremolo_points: Option<Vec<(i32, i32)>>,
    /// `Some(true)` = up stroke.
    pub stroke_up: Option<bool>,
    /// Version 3: 1 natural, 2 artificial harmonic on the beat.
    pub v3_harmonic: u8,
    /// Versions 4 and 5: the picking stroke, 1 up, 2 down.
    pub pick: Option<i8>,
}

#[derive(Clone, Default)]
pub struct NoteFx {
    pub hammer: bool,
    pub let_ring: bool,
    pub bend: Option<Vec<(i32, i32)>>,
    /// `(fret, duration, flags)`; flags are version 5 only.
    pub grace: Option<(u8, u8, u8)>,
    pub staccato: bool,
    pub palm_mute: bool,
    /// 1 eighth, 2 sixteenth, 3 thirty-second.
    pub tremolo_picking: Option<i8>,
    /// The raw slide value: version 3 a flag, version 4 a number, version 5 a bit mask.
    pub slide: Option<i8>,
    /// Version 4: the type byte; version 5: type, then its data.
    pub harmonic: Option<(i8, Vec<u8>)>,
    pub trill: Option<(i8, i8)>,
    pub vibrato: bool,
}

#[derive(Clone, Default)]
pub struct NoteDef {
    /// 0 = the top string.
    pub string: usize,
    pub fret: i8,
    /// 1 normal, 2 tied, 3 dead.
    pub kind: u8,
    pub dynamic: Option<i8>,
    pub ghost: bool,
    pub heavy: bool,
    pub accent: bool,
    pub fingering: bool,
    pub own_duration: bool,
    pub fx: Option<NoteFx>,
}

impl NoteDef {
    pub fn new(string: usize, fret: i8) -> Self {
        Self {
            string,
            fret,
            kind: 1,
            ..Self::default()
        }
    }
    pub fn fx(mut self, f: NoteFx) -> Self {
        self.fx = Some(f);
        self
    }
}

#[derive(Clone, Default)]
pub struct BeatDef {
    /// -2 whole, -1 half, 0 quarter, 1 eighth, 2 sixteenth, 3 thirty-second, 4 sixty-fourth.
    pub dur: i8,
    pub dotted: bool,
    pub tuplet: Option<i32>,
    pub rest: bool,
    pub chord: Option<ChordDef>,
    pub text: Option<String>,
    pub fx: Option<BeatFx>,
    pub tempo: Option<i32>,
    /// A mixer change that sets the volume (and a tempo, if given).
    pub mix_volume: Option<i8>,
    pub notes: Vec<NoteDef>,
    /// Version 5: the secondary beam byte.
    pub break_secondary: bool,
}

impl BeatDef {
    pub fn quarter(notes: Vec<NoteDef>) -> Self {
        Self {
            dur: 0,
            notes,
            ..Self::default()
        }
    }
    pub fn of(dur: i8, notes: Vec<NoteDef>) -> Self {
        Self {
            dur,
            notes,
            ..Self::default()
        }
    }
    pub fn rest(dur: i8) -> Self {
        Self {
            dur,
            rest: true,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct Song {
    /// 300, 400, 500 or 510.
    pub ver: u16,
    pub title: String,
    pub artist: String,
    pub words: String,
    pub music: String,
    pub notices: Vec<String>,
    pub tempo: i32,
    pub key: i8,
    pub lyrics: Option<(i32, Vec<(i32, String)>)>,
    pub bars: Vec<BarHdr>,
    pub tracks: Vec<TrackDef>,
    /// `[bar][track]` the beats of voice 1.
    pub beats: Vec<Vec<Vec<BeatDef>>>,
    /// `[bar][track]` the beats of voice 2 (version 5 only).
    pub voice2: Vec<Vec<Vec<BeatDef>>>,
    /// The program of each (1-based) channel of port 1.
    pub programs: Vec<(usize, i32)>,
}

impl Song {
    /// A song with one guitar track and `bars` empty 4/4 bars.
    pub fn new(ver: u16, bars: usize) -> Self {
        Self {
            ver,
            title: "Test Piece".into(),
            artist: "Nobody".into(),
            words: "W".into(),
            music: "M".into(),
            notices: vec![],
            tempo: 120,
            key: 0,
            lyrics: None,
            bars: vec![BarHdr::default(); bars],
            tracks: vec![TrackDef::guitar("Guitar")],
            beats: vec![vec![vec![]]; bars],
            voice2: vec![vec![vec![]]; bars],
            programs: vec![],
        }
    }

    pub fn add_track(&mut self, t: TrackDef) {
        self.tracks.push(t);
        for b in &mut self.beats {
            b.push(vec![]);
        }
        for b in &mut self.voice2 {
            b.push(vec![]);
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let v = self.ver;
        let mut w = W::default();
        w.byte_size_string(
            &format!("FICHIER GUITAR PRO v{}.{:02}", v / 100, v % 100),
            30,
        );
        // Information.
        w.int_byte_size_string(&self.title);
        w.int_byte_size_string("");
        w.int_byte_size_string(&self.artist);
        w.int_byte_size_string("");
        w.int_byte_size_string(&self.words);
        if v >= 500 {
            w.int_byte_size_string(&self.music);
        }
        w.int_byte_size_string("(c) nobody");
        w.int_byte_size_string("tabber");
        w.int_byte_size_string("");
        w.i32(self.notices.len() as i32);
        for n in &self.notices {
            w.int_byte_size_string(n);
        }
        if v < 500 {
            w.u8(0); // triplet feel
        }
        if v >= 400 {
            let (track, lines) = self.lyrics.clone().unwrap_or((0, vec![]));
            w.i32(track);
            for i in 0..5 {
                let (start, text) = lines.get(i).cloned().unwrap_or((1, String::new()));
                w.i32(start);
                w.int_size_string(&text);
            }
        }
        if v > 500 {
            w.zeros(19);
        }
        if v >= 500 {
            w.zeros(30);
            for _ in 0..10 {
                w.int_byte_size_string("");
            }
            w.int_byte_size_string("Moderate");
        }
        w.i32(self.tempo);
        if v >= 500 {
            if v > 500 {
                w.u8(0);
            }
            w.i8(self.key);
            w.i32(0);
        } else {
            w.i32(i32::from(self.key));
            if v >= 400 {
                w.u8(0);
            }
        }
        // The 64 channels.
        for i in 0..64usize {
            let program = self
                .programs
                .iter()
                .find(|p| p.0 == i + 1)
                .map_or(25, |p| p.1);
            w.i32(program);
            w.zeros(8);
        }
        if v >= 500 {
            w.zeros(19 * 2 + 4);
        }
        w.i32(self.bars.len() as i32);
        w.i32(self.tracks.len() as i32);
        for (n, h) in self.bars.iter().enumerate() {
            self.bar_header(&mut w, n, h);
        }
        for (n, t) in self.tracks.iter().enumerate() {
            self.track_header(&mut w, n, t);
        }
        if v >= 500 {
            w.zeros(if v == 500 { 2 } else { 1 });
        }
        for b in 0..self.bars.len() {
            for t in 0..self.tracks.len() {
                let strings = self.tracks[t].strings.len();
                self.voice(&mut w, &self.beats[b][t], strings);
                if v >= 500 {
                    self.voice(&mut w, &self.voice2[b][t], strings);
                    w.u8(0);
                }
            }
        }
        w.0
    }

    fn bar_header(&self, w: &mut W, n: usize, h: &BarHdr) {
        let v = self.ver;
        if v >= 500 && n > 0 {
            w.u8(0);
        }
        let mut flags = 0u8;
        if h.num.is_some() {
            flags |= 0x01;
        }
        if h.den.is_some() {
            flags |= 0x02;
        }
        if h.repeat_open {
            flags |= 0x04;
        }
        if h.repeat_close.is_some() {
            flags |= 0x08;
        }
        if h.alt != 0 {
            flags |= 0x10;
        }
        if h.marker.is_some() {
            flags |= 0x20;
        }
        if h.key.is_some() {
            flags |= 0x40;
        }
        w.u8(flags);
        if let Some(n) = h.num {
            w.u8(n);
        }
        if let Some(d) = h.den {
            w.u8(d);
        }
        if let Some(plays) = h.repeat_close {
            // Before version 5 the byte counts the repeats, from 5 on all the plays.
            w.u8(if v >= 500 { plays } else { plays - 1 });
        }
        if v < 500 && h.alt != 0 {
            w.u8((8 - h.alt.leading_zeros()) as u8);
        }
        if let Some(m) = &h.marker {
            w.int_byte_size_string(m);
            w.zeros(4);
        }
        if let Some((k, minor)) = h.key {
            w.i8(k);
            w.u8(u8::from(minor));
        }
        if v >= 500 {
            if h.alt != 0 {
                w.u8(h.alt);
            }
            if flags & 0x03 != 0 {
                w.zeros(4);
            }
            if h.alt == 0 {
                w.u8(0);
            }
            w.u8(0); // triplet feel
        }
    }

    fn track_header(&self, w: &mut W, n: usize, t: &TrackDef) {
        let v = self.ver;
        if v >= 500 && (n == 0 || v == 500) {
            w.u8(0);
        }
        let flags = u8::from(t.drums) | (u8::from(t.twelve) << 1) | (u8::from(t.banjo) << 2);
        w.u8(flags);
        w.byte_size_string(&t.name, 40);
        w.i32(t.strings.len() as i32);
        for i in 0..7 {
            w.i32(t.strings.get(i).copied().unwrap_or(0));
        }
        w.i32(1); // port
        w.i32(t.channel);
        w.i32(t.channel + 1);
        w.i32(t.frets);
        w.i32(t.capo);
        w.zeros(4);
        if v >= 500 {
            w.zeros(2 + 1 + 1);
            w.zeros(1 + 12 + 12 + 12);
            if v == 500 {
                w.zeros(3);
            } else {
                w.zeros(4);
                w.zeros(4);
                w.int_byte_size_string("");
                w.int_byte_size_string("");
            }
        }
    }

    fn voice(&self, w: &mut W, beats: &[BeatDef], strings: usize) {
        w.i32(beats.len() as i32);
        for b in beats {
            self.beat(w, b, strings);
        }
    }

    fn beat(&self, w: &mut W, b: &BeatDef, strings: usize) {
        let v = self.ver;
        let mut flags = 0u8;
        if b.dotted {
            flags |= 0x01;
        }
        if b.chord.is_some() {
            flags |= 0x02;
        }
        if b.text.is_some() {
            flags |= 0x04;
        }
        if b.fx.is_some() {
            flags |= 0x08;
        }
        if b.tempo.is_some() || b.mix_volume.is_some() {
            flags |= 0x10;
        }
        if b.tuplet.is_some() {
            flags |= 0x20;
        }
        if b.rest {
            flags |= 0x40;
        }
        w.u8(flags);
        if b.rest {
            w.u8(2);
        }
        w.i8(b.dur);
        if let Some(t) = b.tuplet {
            w.i32(t);
        }
        if let Some(c) = &b.chord {
            self.chord(w, c);
        }
        if let Some(t) = &b.text {
            w.int_byte_size_string(t);
        }
        if let Some(fx) = &b.fx {
            self.beat_fx(w, fx);
        }
        if flags & 0x10 != 0 {
            self.mix_table(w, b.tempo, b.mix_volume);
        }
        let mut mask = 0u8;
        if !b.rest {
            for n in &b.notes {
                assert!(n.string < strings.max(1) + 3);
                mask |= 0x40 >> n.string;
            }
        }
        w.u8(mask);
        if !b.rest {
            let mut notes = b.notes.clone();
            notes.sort_by_key(|n| n.string);
            for n in &notes {
                self.note(w, n);
            }
        }
        if v >= 500 {
            w.i16(if b.break_secondary { 0x0800 } else { 0 });
            if b.break_secondary {
                w.u8(1);
            }
        }
    }

    fn chord(&self, w: &mut W, c: &ChordDef) {
        if c.new_format {
            w.u8(1);
            w.zeros(16);
            w.byte_size_string(&c.name, 21);
            w.zeros(4);
            w.i32(1);
            for i in 0..7 {
                w.i32(c.frets.get(i).copied().unwrap_or(-1));
            }
            w.zeros(1 + 15 + 7 + 1 + 7 + 1);
        } else {
            w.u8(0);
            w.int_byte_size_string(&c.name);
            w.i32(1);
            for i in 0..6 {
                w.i32(c.frets.get(i).copied().unwrap_or(-1));
            }
        }
    }

    fn beat_fx(&self, w: &mut W, fx: &BeatFx) {
        let v = self.ver;
        let mut f1 = 0u8;
        if fx.vibrato {
            f1 |= 0x01;
        }
        if fx.wide_vibrato {
            f1 |= 0x02;
        }
        if fx.fade_in {
            f1 |= 0x10;
        }
        if fx.slap != 0 || fx.tremolo_v3.is_some() {
            f1 |= 0x20;
        }
        if fx.stroke_up.is_some() {
            f1 |= 0x40;
        }
        if v < 400 {
            if fx.v3_harmonic == 1 {
                f1 |= 0x04;
            }
            if fx.v3_harmonic == 2 {
                f1 |= 0x08;
            }
            w.u8(f1);
            if f1 & 0x20 != 0 {
                if let Some(t) = fx.tremolo_v3 {
                    w.i8(0);
                    w.i32(t);
                } else {
                    w.i8(fx.slap);
                }
            }
            if f1 & 0x40 != 0 {
                self.stroke(w, fx.stroke_up == Some(true));
            }
            return;
        }
        let mut f2 = 0u8;
        if fx.tremolo_points.is_some() {
            f2 |= 0x04;
        }
        if fx.pick.is_some() {
            f2 |= 0x02;
        }
        w.u8(f1);
        w.u8(f2);
        if f1 & 0x20 != 0 {
            w.i8(fx.slap);
        }
        if let Some(p) = &fx.tremolo_points {
            self.bend(w, p);
        }
        if f1 & 0x40 != 0 {
            self.stroke(w, fx.stroke_up == Some(true));
        }
        if let Some(p) = fx.pick {
            w.i8(p);
        }
    }

    fn stroke(&self, w: &mut W, up: bool) {
        // The down speed, then the up speed.
        if up {
            w.i8(0);
            w.i8(4);
        } else {
            w.i8(4);
            w.i8(0);
        }
    }

    fn bend(&self, w: &mut W, points: &[(i32, i32)]) {
        w.i8(1);
        w.i32(100);
        w.i32(points.len() as i32);
        for (pos, value) in points {
            w.i32(*pos);
            w.i32(*value);
            w.u8(0);
        }
    }

    fn mix_table(&self, w: &mut W, tempo: Option<i32>, volume: Option<i8>) {
        let v = self.ver;
        w.i8(-1); // instrument
        if v >= 500 {
            w.zeros(12);
            w.zeros(if v == 500 { 3 } else { 4 });
        }
        w.i8(volume.unwrap_or(-1));
        for _ in 0..5 {
            w.i8(-1);
        }
        if v >= 500 {
            w.int_byte_size_string("");
        }
        w.i32(tempo.unwrap_or(-1));
        if volume.is_some() {
            w.u8(0); // the volume's change duration
        }
        if tempo.is_some() {
            w.u8(0);
            if v > 500 {
                w.u8(0);
            }
        }
        if v >= 400 {
            w.u8(0);
        }
        if v >= 500 {
            w.u8(0);
            if v > 500 {
                w.int_byte_size_string("");
                w.int_byte_size_string("");
            }
        }
    }

    fn note(&self, w: &mut W, n: &NoteDef) {
        let v = self.ver;
        let mut flags = 0u8;
        if n.own_duration {
            flags |= 0x01;
        }
        if n.heavy {
            flags |= 0x02;
        }
        if n.ghost {
            flags |= 0x04;
        }
        if n.fx.is_some() {
            flags |= 0x08;
        }
        if n.dynamic.is_some() {
            flags |= 0x10;
        }
        flags |= 0x20; // type and fret
        if n.accent {
            flags |= 0x40;
        }
        if n.fingering {
            flags |= 0x80;
        }
        w.u8(flags);
        w.u8(n.kind.max(1));
        if n.own_duration && v < 500 {
            w.zeros(2);
        }
        if let Some(d) = n.dynamic {
            w.i8(d);
        }
        w.i8(n.fret);
        if n.fingering {
            w.zeros(2);
        }
        if v >= 500 {
            if n.own_duration {
                w.zeros(8);
            }
            w.u8(0);
        }
        if let Some(fx) = &n.fx {
            self.note_fx(w, fx);
        }
    }

    fn note_fx(&self, w: &mut W, fx: &NoteFx) {
        let v = self.ver;
        let mut f1 = 0u8;
        if fx.bend.is_some() {
            f1 |= 0x01;
        }
        if fx.hammer {
            f1 |= 0x02;
        }
        if fx.let_ring {
            f1 |= 0x08;
        }
        if fx.grace.is_some() {
            f1 |= 0x10;
        }
        if v < 400 {
            if fx.slide.is_some() {
                f1 |= 0x04;
            }
            w.u8(f1);
        } else {
            let mut f2 = 0u8;
            if fx.staccato {
                f2 |= 0x01;
            }
            if fx.palm_mute {
                f2 |= 0x02;
            }
            if fx.tremolo_picking.is_some() {
                f2 |= 0x04;
            }
            if fx.slide.is_some() {
                f2 |= 0x08;
            }
            if fx.harmonic.is_some() {
                f2 |= 0x10;
            }
            if fx.trill.is_some() {
                f2 |= 0x20;
            }
            if fx.vibrato {
                f2 |= 0x40;
            }
            w.u8(f1);
            w.u8(f2);
        }
        if let Some(b) = &fx.bend {
            self.bend(w, b);
        }
        if let Some((fret, dur, flags)) = fx.grace {
            w.u8(fret);
            w.u8(6);
            w.u8(0);
            w.u8(dur);
            if v >= 500 {
                w.u8(flags);
            }
        }
        if v < 400 {
            return;
        }
        if let Some(t) = fx.tremolo_picking {
            w.i8(t);
        }
        if let Some(s) = fx.slide {
            w.i8(s);
        }
        if let Some((kind, data)) = &fx.harmonic {
            w.i8(*kind);
            if v >= 500 {
                w.0.extend_from_slice(data);
            }
        }
        if let Some((fret, period)) = fx.trill {
            w.i8(fret);
            w.i8(period);
        }
    }
}
