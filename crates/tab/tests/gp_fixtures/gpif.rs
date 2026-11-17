//! A builder for the GPIF XML score: ids are handed out as the parts are added.

use std::fmt::Write;

#[derive(Default)]
pub struct Gpif {
    pub title: String,
    pub artist: String,
    tempo: Vec<String>,
    tracks: Vec<String>,
    track_ids: Vec<usize>,
    masters: Vec<String>,
    bars: Vec<String>,
    voices: Vec<String>,
    beats: Vec<String>,
    notes: Vec<String>,
    rhythms: Vec<String>,
}

/// One XML property of a note or a beat.
pub fn property(name: &str, inner: &str) -> String {
    format!("<Property name=\"{name}\">{inner}</Property>")
}

pub fn enabled(name: &str) -> String {
    property(name, "<Enable/>")
}

impl Gpif {
    pub fn new() -> Self {
        Self {
            title: "Test Piece".into(),
            artist: "Nobody".into(),
            ..Self::default()
        }
    }

    pub fn tempo(&mut self, bar: usize, pos: f64, value: &str) {
        self.tempo.push(format!(
            "<Automation><Type>Tempo</Type><Linear>false</Linear><Bar>{bar}</Bar><Position>{pos}</Position><Visible>true</Visible><Value>{value}</Value></Automation>"
        ));
    }

    /// A track; `pitches` in the file's order (lowest string first). Returns the track index.
    pub fn track(&mut self, name: &str, pitches: &[u8], capo: u8, extra: &str) -> usize {
        let id = self.tracks.len();
        let p: Vec<String> = pitches.iter().map(u8::to_string).collect();
        self.tracks.push(format!(
            "<Track id=\"{id}\"><Name>{name}</Name><GeneralMidi><Program>29</Program></GeneralMidi>\
             <Staves><Staff id=\"0\"><Properties>\
             <Property name=\"Tuning\"><Pitches>{}</Pitches></Property>\
             <Property name=\"CapoFret\"><Fret>{capo}</Fret></Property>\
             <Property name=\"FretCount\"><Fret>24</Fret></Property>{extra}</Properties></Staff></Staves></Track>",
            p.join(" ")
        ));
        self.track_ids.push(id);
        id
    }

    /// A track from raw XML (a drum kit, say).
    pub fn raw_track(&mut self, xml: &str) -> usize {
        let id = self.tracks.len();
        self.tracks.push(xml.replace("{id}", &id.to_string()));
        self.track_ids.push(id);
        id
    }

    pub fn rhythm(&mut self, value: &str, dots: u32, tuplet: Option<(u32, u32)>) -> usize {
        let mut s = format!("<NoteValue>{value}</NoteValue>");
        if dots > 0 {
            write!(s, "<AugmentationDot count=\"{dots}\"/>").unwrap();
        }
        if let Some((n, d)) = tuplet {
            write!(s, "<PrimaryTuplet num=\"{n}\" den=\"{d}\"/>").unwrap();
        }
        let id = self.rhythms.len();
        self.rhythms
            .push(format!("<Rhythm id=\"{id}\">{s}</Rhythm>"));
        id
    }

    /// A note; `string` counts from the lowest string.
    pub fn note(&mut self, string: usize, fret: i32, props: &[String], children: &str) -> usize {
        let id = self.notes.len();
        self.notes.push(format!(
            "<Note id=\"{id}\"><Properties>{}{}{}</Properties>{children}</Note>",
            property("String", &format!("<String>{string}</String>")),
            property("Fret", &format!("<Fret>{fret}</Fret>")),
            props.concat()
        ));
        id
    }

    /// A note with only properties of its own (a drum hit).
    pub fn raw_note(&mut self, xml: &str) -> usize {
        let id = self.notes.len();
        self.notes.push(xml.replace("{id}", &id.to_string()));
        id
    }

    pub fn beat(
        &mut self,
        rhythm: usize,
        notes: &[usize],
        props: &[String],
        children: &str,
    ) -> usize {
        let id = self.beats.len();
        let n: Vec<String> = notes.iter().map(usize::to_string).collect();
        let notes_xml = if notes.is_empty() {
            String::new()
        } else {
            format!("<Notes>{}</Notes>", n.join(" "))
        };
        let props_xml = if props.is_empty() {
            String::new()
        } else {
            format!("<Properties>{}</Properties>", props.concat())
        };
        self.beats.push(format!(
            "<Beat id=\"{id}\"><Rhythm ref=\"{rhythm}\"/>{notes_xml}{props_xml}{children}</Beat>"
        ));
        id
    }

    pub fn voice(&mut self, beats: &[usize]) -> usize {
        let id = self.voices.len();
        let b: Vec<String> = beats.iter().map(usize::to_string).collect();
        self.voices.push(format!(
            "<Voice id=\"{id}\"><Beats>{}</Beats></Voice>",
            b.join(" ")
        ));
        id
    }

    pub fn bar(&mut self, voices: &[usize]) -> usize {
        let id = self.bars.len();
        let mut v: Vec<String> = voices.iter().map(usize::to_string).collect();
        while v.len() < 4 {
            v.push("-1".into());
        }
        self.bars.push(format!(
            "<Bar id=\"{id}\"><Clef>G2</Clef><Voices>{}</Voices></Bar>",
            v.join(" ")
        ));
        id
    }

    /// A master bar with one bar id per track.
    pub fn master(&mut self, time: &str, bars: &[usize], extra: &str) {
        let b: Vec<String> = bars.iter().map(usize::to_string).collect();
        self.masters.push(format!(
            "<MasterBar><Time>{time}</Time><Bars>{}</Bars>{extra}</MasterBar>",
            b.join(" ")
        ));
    }

    pub fn render(&self) -> String {
        let ids: Vec<String> = self.track_ids.iter().map(usize::to_string).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><GPIF><GPVersion>7.5.0</GPVersion>\
             <Score><Title>{}</Title><SubTitle></SubTitle><Artist>{}</Artist><Album></Album>\
             <Words>W</Words><Music>M</Music><Tabber>T</Tabber><Copyright>C</Copyright></Score>\
             <MasterTrack><Tracks>{}</Tracks><Automations>{}</Automations></MasterTrack>\
             <Tracks>{}</Tracks><MasterBars>{}</MasterBars><Bars>{}</Bars><Voices>{}</Voices>\
             <Beats>{}</Beats><Notes>{}</Notes><Rhythms>{}</Rhythms></GPIF>",
            self.title,
            self.artist,
            ids.join(" "),
            self.tempo.concat(),
            self.tracks.concat(),
            self.masters.concat(),
            self.bars.concat(),
            self.voices.concat(),
            self.beats.concat(),
            self.notes.concat(),
            self.rhythms.concat(),
        )
    }
}

/// A standard guitar, lowest string first.
pub const GUITAR: [u8; 6] = [40, 45, 50, 55, 59, 64];
