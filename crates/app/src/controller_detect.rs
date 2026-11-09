//! Working out what a control is from the messages it sends.
//!
//! A [`Capture`] listens to the messages of one input port while the user moves one control (or
//! presses one key) and says what it found: a 7 bit controller, a 14 bit controller pair, an NRPN,
//! a note or pitch bend; for a knob or fader whether it reports its position (**absolute**) or
//! its turns (a **relative** encoder in one of the three usual codings). It is pure: it takes
//! parsed messages and a [`Verdict`] comes out, so every case is tested from fake MIDI streams.
//!
//! How an encoder is told from a knob: a relative encoder sends small numbers of detents, a few
//! different values in all, in **both** directions (clockwise and counter-clockwise are two
//! clusters of values: next to 0 and 127, next to 64, or next to 0 and 64, depending on the
//! coding); a knob walks through a range of values. Until the stream shows one or the other the
//! verdict says what to do next (turn it the other way, move it further).

use crate::controller_profile::ControlMode;
use atomik_core::{ChannelMessage, MessageKind};
use atomik_model::{
    ParamChange, ParamEvent, ParamParser, ParamSpace, RelativeKind, SourceKind, is_param_controller,
};

/// The widest swing of detents one message of an encoder may carry for the stream to count as an
/// encoder.
const MAX_DETENTS: i32 = 16;
/// The most different values an encoder stream has.
const MAX_ENCODER_VALUES: usize = 8;
/// How far an absolute 7 bit control must have moved to be told from a nudge.
const MIN_SPREAD: i32 = 16;
/// How far an absolute 14 bit control must have moved.
const MIN_SPREAD_14: i32 = 2048;
/// The most streams one capture follows (a noisy port cannot grow it without bound).
const MAX_STREAMS: usize = 64;
/// The most values kept of one stream.
const MAX_VALUES: usize = 256;

/// What the user is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Move a knob or a fader.
    Continuous,
    /// Press a key.
    Button,
}

/// A control as found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    /// The kind of message and its number.
    pub source: SourceKind,
    /// The MIDI channel, 0..=15.
    pub channel: u8,
    /// How its messages are read.
    pub mode: ControlMode,
}

impl Detection {
    /// A short text for the wizard (`CC 21 on channel 1, absolute`).
    pub fn describe(&self) -> String {
        let message = match self.source {
            SourceKind::Cc(n) => format!("CC {n}"),
            SourceKind::Cc14(n) => format!("CC {n} + {} (14 bit)", u16::from(n) + 32),
            SourceKind::Nrpn(n) => format!("NRPN {n}"),
            SourceKind::Rpn(n) => format!("RPN {n}"),
            SourceKind::Note(n) => format!("note {}", atomik_model::note_name(n)),
            SourceKind::PitchBend => "pitch bend".to_owned(),
        };
        format!(
            "{message} on channel {}, {}",
            u16::from(self.channel) + 1,
            self.mode.describe()
        )
    }
}

/// What a capture knows so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing usable has been heard.
    Nothing,
    /// Something was heard but not enough to tell: what to do next.
    NeedsMore(&'static str),
    /// The control is known.
    Found(Detection),
}

/// One stream of messages: the same kind, number and channel.
#[derive(Debug, Clone)]
struct Stream {
    source: SourceKind,
    channel: u8,
    /// The raw values in the order they arrived (7 bit, or 14 bit for pitch bend and NRPN).
    values: Vec<i32>,
    /// Data increment and decrement steps of an NRPN, as signed detents.
    steps: Vec<i32>,
}

/// Listens to one control.
#[derive(Debug, Clone)]
pub struct Capture {
    want: Want,
    streams: Vec<Stream>,
    parsers: [ParamParser; 16],
    /// Messages of these controls are not heard (they are set up already).
    ignore: Vec<(SourceKind, u8)>,
    /// The first key press heard (for [`Want::Button`]).
    pressed: Option<Detection>,
}

impl Capture {
    /// A capture for `want` that does not hear the controls in `ignore`.
    pub fn new(want: Want, ignore: Vec<(SourceKind, u8)>) -> Self {
        Self {
            want,
            streams: Vec::new(),
            parsers: [ParamParser::new(); 16],
            ignore,
            pressed: None,
        }
    }

    /// What the capture is for.
    pub fn want(&self) -> Want {
        self.want
    }

    /// The messages that count are taken in; everything else is dropped.
    pub fn feed(&mut self, msg: &ChannelMessage) {
        let channel = msg.channel & 0x0F;
        match msg.kind {
            MessageKind::Cc { number, value } => {
                if is_param_controller(number) {
                    let parser = &mut self.parsers[usize::from(channel)];
                    if let Some(event) = parser.feed(msg) {
                        self.param(channel, event);
                    }
                } else {
                    self.cc(channel, number, i32::from(value & 0x7F));
                }
            }
            MessageKind::PitchBend { value } => {
                if self.want == Want::Continuous {
                    self.push(SourceKind::PitchBend, channel, i32::from(value & 0x3FFF));
                }
            }
            MessageKind::NoteOn { key, .. } => {
                if self.want == Want::Button && !self.ignored(SourceKind::Note(key), channel) {
                    self.press(SourceKind::Note(key), channel);
                }
            }
            MessageKind::NoteOff { .. } => {}
        }
    }

    fn ignored(&self, source: SourceKind, channel: u8) -> bool {
        self.ignore.iter().any(|(s, c)| {
            *c == channel
                && (*s == source
                    || matches!(
                        (s, source),
                        (SourceKind::Cc14(n), SourceKind::Cc(m)) if m == *n || m == *n + 32
                    ))
        })
    }

    fn press(&mut self, source: SourceKind, channel: u8) {
        if self.pressed.is_none() {
            self.pressed = Some(Detection {
                source,
                channel,
                mode: ControlMode::Button,
            });
        }
    }

    fn cc(&mut self, channel: u8, number: u8, value: i32) {
        match self.want {
            Want::Button => {
                if value > 0 && !self.ignored(SourceKind::Cc(number), channel) {
                    self.press(SourceKind::Cc(number), channel);
                }
            }
            Want::Continuous => self.push(SourceKind::Cc(number), channel, value),
        }
    }

    fn param(&mut self, channel: u8, event: ParamEvent) {
        if event.space != ParamSpace::Nrpn {
            return;
        }
        let source = SourceKind::Nrpn(event.number);
        if self.ignored(source, channel) {
            return;
        }
        match (self.want, event.change) {
            (Want::Button, ParamChange::Value(v)) => {
                if v >= 8192 {
                    self.press(source, channel);
                }
            }
            (Want::Button, ParamChange::Step(_)) => {}
            (Want::Continuous, ParamChange::Value(v)) => self.push(source, channel, i32::from(v)),
            (Want::Continuous, ParamChange::Step(d)) => {
                if let Some(s) = self.stream(source, channel)
                    && s.steps.len() < MAX_VALUES
                {
                    s.steps.push(i32::from(d));
                }
            }
        }
    }

    fn stream(&mut self, source: SourceKind, channel: u8) -> Option<&mut Stream> {
        if let Some(i) = self
            .streams
            .iter()
            .position(|s| s.source == source && s.channel == channel)
        {
            return self.streams.get_mut(i);
        }
        if self.streams.len() >= MAX_STREAMS {
            return None;
        }
        self.streams.push(Stream {
            source,
            channel,
            values: Vec::new(),
            steps: Vec::new(),
        });
        self.streams.last_mut()
    }

    fn push(&mut self, source: SourceKind, channel: u8, value: i32) {
        // A 14 bit pair's low half is not a control of its own: the pair is told at the end.
        if self.ignored(source, channel) {
            return;
        }
        if let Some(s) = self.stream(source, channel)
            && s.values.len() < MAX_VALUES
        {
            s.values.push(value);
        }
    }

    /// What the capture knows so far.
    pub fn verdict(&self) -> Verdict {
        match self.want {
            Want::Button => self.pressed.map_or(Verdict::Nothing, Verdict::Found),
            Want::Continuous => self.continuous_verdict(),
        }
    }

    fn continuous_verdict(&self) -> Verdict {
        let mut streams: Vec<&Stream> = self.streams.iter().collect();
        // A controller 0..=31 and the one 32 above it, both moving: one 14 bit control.
        let pair = streams.iter().find_map(|hi| {
            let SourceKind::Cc(n) = hi.source else {
                return None;
            };
            if n >= 32 {
                return None;
            }
            let lo = streams
                .iter()
                .find(|s| s.channel == hi.channel && s.source == SourceKind::Cc(n + 32))?;
            (hi.values.len() >= 2 && lo.values.len() >= 2)
                .then(|| (n, hi.channel, hi.values.clone()))
        });
        if let Some((n, channel, _)) = &pair {
            streams.retain(|s| {
                !(s.channel == *channel
                    && (s.source == SourceKind::Cc(*n) || s.source == SourceKind::Cc(*n + 32)))
            });
        }
        // The stream with the most messages is the control the user is moving.
        let busiest = streams
            .iter()
            .max_by_key(|s| s.values.len() + s.steps.len())
            .copied();
        let pair_len = pair.as_ref().map_or(0, |(_, _, v)| v.len());
        if let Some((n, channel, values)) = pair
            && busiest.is_none_or(|b| b.values.len() + b.steps.len() <= pair_len)
        {
            return if spread(&values) >= MIN_SPREAD {
                Verdict::Found(Detection {
                    source: SourceKind::Cc14(n),
                    channel,
                    mode: ControlMode::Absolute,
                })
            } else {
                Verdict::NeedsMore("Move it through its whole range")
            };
        }
        let Some(stream) = busiest else {
            return Verdict::Nothing;
        };
        stream_verdict(stream)
    }
}

fn spread(values: &[i32]) -> i32 {
    let max = values.iter().copied().max().unwrap_or(0);
    let min = values.iter().copied().min().unwrap_or(0);
    max - min
}

fn found(stream: &Stream, mode: ControlMode) -> Verdict {
    Verdict::Found(Detection {
        source: stream.source,
        channel: stream.channel,
        mode,
    })
}

fn stream_verdict(s: &Stream) -> Verdict {
    match s.source {
        SourceKind::PitchBend => {
            if spread(&s.values) >= MIN_SPREAD_14 {
                found(s, ControlMode::Absolute)
            } else {
                Verdict::NeedsMore("Move it through its whole range")
            }
        }
        SourceKind::Nrpn(_) => {
            if !s.steps.is_empty() && s.values.is_empty() {
                found(s, ControlMode::Relative(RelativeKind::TwosComplement))
            } else if spread(&s.values) >= MIN_SPREAD_14 {
                found(s, ControlMode::Absolute)
            } else {
                Verdict::NeedsMore("Move it through its whole range")
            }
        }
        _ => cc_verdict(s),
    }
}

/// The verdict for a stream of 7 bit controller values.
fn cc_verdict(s: &Stream) -> Verdict {
    let values = &s.values;
    if values.len() < 2 {
        return Verdict::NeedsMore("Keep moving it");
    }
    let mut distinct: Vec<i32> = values.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() <= MAX_ENCODER_VALUES {
        for kind in [
            RelativeKind::TwosComplement,
            RelativeKind::BinaryOffset,
            RelativeKind::SignMagnitude,
        ] {
            let detents: Vec<i32> = values.iter().map(|v| kind.detents(*v as u8)).collect();
            let small = detents.iter().all(|d| *d != 0 && d.abs() <= MAX_DETENTS);
            let both = detents.iter().any(|d| *d > 0) && detents.iter().any(|d| *d < 0);
            if small && both {
                return found(s, ControlMode::Relative(kind));
            }
        }
    }
    if spread(values) >= MIN_SPREAD {
        return found(s, ControlMode::Absolute);
    }
    Verdict::NeedsMore("Move it further, or turn it both ways")
}

#[cfg(test)]
#[path = "controller_detect_tests.rs"]
mod tests;
