//! The first-run audio and MIDI setup: what the panel knows and decides.
//!
//! On the very first start (and from Options > Audio and MIDI Setup...) a short panel gets sound
//! working in three pages: the sound output (device, buffer, a test tone, the latency), an
//! optional input with a live meter, and the MIDI keyboard. The panel edits a copy of the audio
//! settings through the rules of the Settings dialog ([`SettingsDialog`]: the same lists, the
//! same "not found" markers, the same check boxes), and the shell applies each change through
//! the one path Audio Settings uses, so the choices land in the same `settings.json`.
//!
//! This module holds the state: the page, the draft, the meters, the "note received" lamp and
//! the texts that depend on the stream. The screen is [`crate::audio_setup_view`]; the shell
//! opens it, applies what it picks and plays the tone.
//!
//! # First start
//!
//! There is no marker file of its own: the start is the first one when no `settings.json`
//! exists yet, and Skip or Done writes that file, so the panel never shows by itself again.
//!
//! # The tone
//!
//! A soft sine chord, one second, peaking at -18 dBFS, played by the preview voice of the engine
//! (the one the file browser uses). That voice is mixed after the master meters, so the panel
//! reads the output peak the engine itself measures ([`atomik_engine::IoPeaks`]) to show that
//! sound reached the device.

use crate::settings::AudioSettings;
use crate::settings_dialog::{Choice, DeviceLists, SettingsDialog};
use crate::status::{AudioStatus, input_note};
use atomik_ui::kurbo::Point;

/// The sample rate the tones are made at (the preview voice resamples to the rate of the stream).
pub const TONE_RATE: u32 = 48_000;
/// The level of the tones: their loudest sample, in dBFS.
pub const TONE_LEVEL_DB: f32 = -18.0;
/// How long the chord sounds, in seconds.
pub const CHORD_SECONDS: f32 = 1.0;
/// How long the tone of a key sounds, in seconds.
pub const KEY_SECONDS: f32 = 0.5;
/// How many frames the "note received" lamp stays lit after a note.
pub const LAMP_FRAMES: u32 = 40;

/// The three pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The audio device, the buffer, the test tone and the latency.
    Output,
    /// The optional audio input and its meter.
    Input,
    /// The MIDI keyboard.
    Midi,
}

impl Page {
    /// Every page, in order.
    pub const ALL: [Page; 3] = [Page::Output, Page::Input, Page::Midi];

    /// The place of the page, from 0.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// The title of the page.
    pub fn title(self) -> &'static str {
        match self {
            Page::Output => "Sound output",
            Page::Input => "Input",
            Page::Midi => "MIDI keyboard",
        }
    }
}

/// The notes of the test chord: C4, E4 and G4.
const CHORD_HZ: [f32; 3] = [261.63, 329.63, 392.0];

/// A soft sine tone of `freqs` lasting `seconds`, peaking at [`TONE_LEVEL_DB`]: a quick rise, a
/// slow fall to nothing, no clicks at either end.
fn shaped(freqs: &[f32], seconds: f32) -> Vec<f32> {
    let n = (seconds * TONE_RATE as f32) as usize;
    let attack = (0.03 * TONE_RATE as f32) as usize;
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / TONE_RATE as f32;
            let sum: f32 = freqs
                .iter()
                .map(|f| (std::f32::consts::TAU * f * t).sin())
                .sum();
            let rise = if i < attack {
                i as f32 / attack as f32
            } else {
                1.0
            };
            // A raised cosine from full to nothing over the whole tone.
            let fall = 0.5 + 0.5 * (std::f32::consts::PI * i as f32 / n as f32).cos();
            sum * rise * fall
        })
        .collect();
    let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let gain = 10.0_f32.powf(TONE_LEVEL_DB / 20.0) / peak;
        out.iter_mut().for_each(|s| *s *= gain);
    }
    out
}

/// The test tone: a soft sine chord, one second, at -18 dBFS (mono at [`TONE_RATE`]).
pub fn chord_tone() -> Vec<f32> {
    shaped(&CHORD_HZ, CHORD_SECONDS)
}

/// The tone a key of the MIDI keyboard plays: a soft sine at its pitch, half a second.
pub fn key_tone(key: u8) -> Vec<f32> {
    let hz = 440.0 * 2.0_f32.powf((f32::from(key) - 69.0) / 12.0);
    shaped(&[hz.min(TONE_RATE as f32 / 2.2)], KEY_SECONDS)
}

/// A level meter: it jumps up to a new peak and falls back slowly.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    level: f32,
}

/// What a meter keeps of its level at each frame.
const FALL: f32 = 0.9;
/// The quietest level a meter shows, in dB.
const FLOOR_DB: f32 = -60.0;

impl Meter {
    /// Takes the loudest sample since the last frame.
    pub fn feed(&mut self, peak: f32) {
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        self.level = if peak >= self.level {
            peak
        } else {
            self.level * FALL
        };
        if self.level < 1e-4 {
            self.level = 0.0;
        }
    }

    /// The level now, linear.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// How far the bar fills, from 0 (-60 dB and below) to 1 (0 dB).
    pub fn fraction(&self) -> f32 {
        if self.level <= 0.0 {
            return 0.0;
        }
        ((20.0 * self.level.log10() - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
    }
}

/// The measured output latency as text: the figure the stream reports when the host gives one,
/// else the buffer divided by the rate, said to be an estimate.
pub fn latency_text(status: &AudioStatus) -> String {
    if !status.running || status.sample_rate == 0 {
        return "Output latency: not measured (no sound output is open)".to_owned();
    }
    let ms = |frames: u32| f64::from(frames) / f64::from(status.sample_rate) * 1000.0;
    if status.output_latency_frames > 0 {
        return format!("Output latency: {:.1} ms", ms(status.output_latency_frames));
    }
    match status.buffer_frames {
        Some(b) if b > 0 => format!(
            "Output latency: about {:.1} ms (an estimate from the buffer)",
            ms(b)
        ),
        _ => "Output latency: not measured yet".to_owned(),
    }
}

/// Why there is no sound, in plain words with the way out; `None` while the output works.
pub fn output_problem(status: &AudioStatus, lists: &DeviceLists) -> Option<String> {
    let has_output = lists
        .hosts
        .iter()
        .any(|h| h.devices.iter().any(|d| d.max_outputs > 0));
    if !has_output {
        return Some(
            "No sound output was found. Connect one, then open Audio and MIDI Setup again."
                .to_owned(),
        );
    }
    if status.running {
        return None;
    }
    Some(match &status.error {
        Some(why) => format!("The sound output did not open: {why}. Pick another device."),
        None => "Sound is off. Pick a device to turn it on.".to_owned(),
    })
}

/// What one frame of the panel found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Update {
    /// Something on the screen changed: repaint.
    pub changed: bool,
    /// A key was played since the last frame: the shell plays its tone.
    pub new_key: Option<u8>,
}

/// What the screen shows that depends on the stream (compared to see whether to repaint).
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    out: Meter,
    input: Meter,
    lamp: u32,
    latency: String,
    problem: Option<String>,
    input_line: String,
    input_open: bool,
}

/// The panel.
#[derive(Debug, Clone)]
pub struct AudioSetup {
    pub(crate) page: Page,
    /// The settings being edited, through the rules of the Settings dialog.
    pub(crate) dialog: SettingsDialog,
    pub(crate) out_meter: Meter,
    pub(crate) in_meter: Meter,
    /// Frames the "note received" lamp has left.
    pub(crate) lamp: u32,
    pub(crate) last_key: Option<u8>,
    /// The note-ons counted so far (only later ones light the lamp).
    notes_seen: u64,
    /// The tone has been asked for at least once.
    pub(crate) tested: bool,
    /// The latency line.
    pub(crate) latency: String,
    /// What is wrong with the output, when something is.
    pub(crate) problem: Option<String>,
    /// What the input is doing (a line under the input list).
    pub(crate) input_line: String,
    /// Whether the stream has an input open (the input meter can move).
    pub(crate) input_open: bool,
    /// Whether the project asks for no input channels at all (a button turns them on).
    pub(crate) input_unwanted: bool,
    /// The first row of the device list shown, on each page.
    pub(crate) first: [usize; 3],
    /// Where the pointer rests (for the hover colour).
    pub(crate) hover: Option<Point>,
    /// The settings as they were when the panel opened.
    opened_with: AudioSettings,
}

impl AudioSetup {
    /// The panel on its first page, editing `current` with the devices of `lists`.
    pub fn new(current: AudioSettings, lists: DeviceLists) -> Self {
        Self {
            page: Page::Output,
            opened_with: current.clone(),
            dialog: SettingsDialog::new(current, lists),
            out_meter: Meter::default(),
            in_meter: Meter::default(),
            lamp: 0,
            last_key: None,
            notes_seen: 0,
            tested: false,
            latency: String::new(),
            problem: None,
            input_line: String::new(),
            input_open: false,
            input_unwanted: false,
            first: [0; 3],
            hover: None,
        }
    }

    /// The page showing.
    pub fn page(&self) -> Page {
        self.page
    }

    /// Whether this is the last page (its button is Done, not Next).
    pub fn is_last(&self) -> bool {
        self.page == Page::Midi
    }

    /// Goes to the next page (stays on the last).
    pub fn next(&mut self) {
        let at = (self.page.index() + 1).min(Page::ALL.len() - 1);
        self.page = Page::ALL[at];
    }

    /// Goes back a page (stays on the first).
    pub fn back(&mut self) {
        self.page = Page::ALL[self.page.index().saturating_sub(1)];
    }

    /// The audio settings as they were when the panel opened.
    pub fn opened_with(&self) -> &AudioSettings {
        &self.opened_with
    }

    /// The audio settings as edited so far.
    pub fn draft(&self) -> &AudioSettings {
        self.dialog.draft()
    }

    /// Makes `choice` the value of the draft.
    pub fn choose(&mut self, choice: Choice) {
        self.dialog.pick(choice);
    }

    /// Whether `choice` is the value of the draft.
    pub fn is_selected(&self, choice: &Choice) -> bool {
        self.dialog.is_selected(choice)
    }

    /// The device lists the panel was given.
    pub fn lists(&self) -> &DeviceLists {
        self.dialog.lists()
    }

    /// The options of the current page that are picked from a list or a row of chips: hosts,
    /// output devices and buffers on the first, input devices on the second, MIDI inputs on the
    /// third. The same options, in the same order, as the Settings dialog lists them.
    pub fn options(&self) -> Vec<(String, Choice)> {
        let wanted = |c: &Choice| match self.page {
            Page::Output => {
                matches!(c, Choice::Host(_) | Choice::Device(_) | Choice::Buffer(_))
            }
            Page::Input => matches!(c, Choice::Input(_)),
            Page::Midi => matches!(c, Choice::MidiInput(_)),
        };
        self.dialog
            .sections()
            .into_iter()
            .flat_map(|s| s.options)
            .filter(|(_, c)| wanted(c))
            .collect()
    }

    /// Takes the note count as it stands now as the start: notes played before do not light the
    /// lamp.
    pub fn start_listening(&mut self, notes: (u64, Option<u8>)) {
        self.notes_seen = notes.0;
    }

    /// Records the note activity the MIDI inputs report: a new note lights the lamp and gives
    /// its key back so the shell can play the tone.
    fn hear(&mut self, (count, key): (u64, Option<u8>)) -> Option<u8> {
        let fresh = count > self.notes_seen;
        self.notes_seen = count;
        if !fresh {
            return None;
        }
        self.lamp = LAMP_FRAMES;
        self.last_key = key;
        key
    }

    fn shown(&self) -> Shown {
        Shown {
            out: self.out_meter,
            input: self.in_meter,
            lamp: self.lamp,
            latency: self.latency.clone(),
            problem: self.problem.clone(),
            input_line: self.input_line.clone(),
            input_open: self.input_open,
        }
    }

    /// One frame: the status of the stream, its peaks `(input, output)` (`None` while no engine
    /// runs) and the note activity of the MIDI inputs. Returns what changed.
    pub fn update(
        &mut self,
        status: &AudioStatus,
        peaks: Option<(f32, f32)>,
        notes: (u64, Option<u8>),
    ) -> Update {
        let before = self.shown();
        let (input, output) = peaks.unwrap_or((0.0, 0.0));
        self.out_meter.feed(output);
        self.in_meter.feed(input);
        self.lamp = self.lamp.saturating_sub(1);
        self.latency = latency_text(status);
        self.problem = output_problem(status, self.dialog.lists());
        self.input_open = status.running && status.input_channels_open > 0;
        self.input_unwanted = status.running && status.input_channels_wanted == 0;
        self.input_line = input_note(status)
            .unwrap_or_else(|| "Input is off while there is no sound output.".to_owned());
        let new_key = self.hear(notes);
        Update {
            changed: before != self.shown() || new_key.is_some(),
            new_key,
        }
    }

    /// Whether the "note received" lamp is lit.
    pub fn lamp_lit(&self) -> bool {
        self.lamp > 0
    }
}

#[cfg(test)]
#[path = "audio_setup_tests.rs"]
mod tests;
